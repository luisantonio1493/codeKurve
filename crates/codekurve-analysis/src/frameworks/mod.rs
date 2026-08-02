//! Framework recognition pass (design "Architecture Decisions" D1-D6,
//! D8-D10; "Module Layout"; "Data Flow"). Runs *inside* `extract::analyze`,
//! immediately after the per-language analyzer returns (D3), and does its
//! own marker-gated tree-sitter re-parse of the same source text (D1) —
//! it never consumes `languages/`'s already-extracted edges, because
//! `cs_simple_type_name`/`cs_callee_name` drop type arguments and call
//! receivers, and TypeScript extracts no parameter symbols at all. Every
//! edge this pass appends carries `Provenance::Heuristic` and is always
//! `EdgeTarget::Unresolved(<name as written>)` (D4) — `resolve.rs` binds it
//! through the existing cross-file resolver, never a second resolver here.
//!
//! PR3 scaffolds the pass end to end (marker prefilter, own parse,
//! `kind_matches`, the pattern-matcher/array-literal helpers PR4-6 will
//! consume) with **empty catalogues** — `angular.rs`/`dotnet.rs` don't exist
//! yet, so `recognize` always appends zero edges and zero roles. The
//! load-bearing part of this PR is the D5 provenance floor in `resolve.rs`,
//! proven with a synthetic edge before either catalogue exists.

// ponytail: `MetaKey`/`MetaEntry`/`object_literal_entries`/`AttrPattern`/
// `CallPattern` are scaffolded here for PR4 (`angular.rs`)/PR5-6
// (`dotnet.rs`) to consume — this PR's own tests exercise them directly,
// but a plain `cargo build` (no `--tests`) still sees zero production
// call sites until a catalogue lands, hence the blanket allow.
#![allow(dead_code)]

use codekurve_core::{LanguageId, RelationshipKind, SourceSpan, SymbolKind};
use tree_sitter::{Node, Parser, Tree};

use crate::extract::span_of;
use crate::ir::FileAnalysis;

mod angular;

mod dotnet;

// ponytail: thread-local, not a global `AtomicUsize` — `cargo test` runs
// each test function on its own thread, so this stays race-free without any
// reset/lock ceremony. Only compiled into test builds.
#[cfg(test)]
thread_local! {
    static PARSE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// D2 marker prefilter, split by resolution domain (TS/JS share one list,
/// C# has its own) — a repo with no framework usage pays one substring scan
/// per file and produces zero framework edges.
const TS_MARKERS: &[&str] = &[
    "@Component",
    "@Injectable",
    "@Directive",
    "@Pipe",
    "@NgModule",
    "@Inject",
    "inject(",
    "Routes",
];
const CS_MARKERS: &[&str] = &[
    "[ApiController]",
    "Http",
    ".Map",
    ".Add",
    "DbSet<",
    "[Function",
    // PR6: `.Use` middleware calls (`app.UseCors()`, `app.UseMiddleware<T>()`)
    // carry none of the markers above — a Use*-only `Program.cs` would
    // silently fail the prefilter and never get re-parsed without this.
    // Found via a PR6 test failure, same as PR4's TS_MARKERS gap.
    ".Use",
];

fn markers_for(language: LanguageId) -> &'static [&'static str] {
    match language {
        LanguageId::TypeScript | LanguageId::JavaScript => TS_MARKERS,
        LanguageId::CSharp => CS_MARKERS,
    }
}

/// True if `source` contains at least one of `language`'s markers — the
/// gate that keeps a framework-free file from ever being re-parsed (D2).
pub(crate) fn has_marker(source: &str, language: LanguageId) -> bool {
    markers_for(language).iter().any(|m| source.contains(m))
}

/// Marker-gated re-parse (D1). Returns `None` on a grammar setup failure;
/// callers already gate this behind `has_marker`, so this is only ever
/// invoked for a file that has at least one framework marker.
fn parse(source: &str, language: LanguageId) -> Option<Tree> {
    #[cfg(test)]
    PARSE_CALLS.with(|c| c.set(c.get() + 1));

    let grammar = match language {
        LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        LanguageId::JavaScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
        LanguageId::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(source, None)
}

/// The recognition pass entry point (design "Data Flow"). Appends
/// framework-level edges to `analysis.relationships` and sets `roles` on
/// matching `analysis.symbols` entries; never removes or mutates an
/// existing edge or symbol. PR3's catalogues are empty — `angular.rs` and
/// `dotnet.rs` land in PR4-6 — so this always leaves `analysis` untouched
/// once the marker prefilter and own re-parse have run.
pub fn recognize(source: &str, language: LanguageId, analysis: &mut FileAnalysis) {
    if !has_marker(source, language) {
        return;
    }
    let Some(tree) = parse(source, language) else {
        return;
    };
    match language {
        LanguageId::TypeScript | LanguageId::JavaScript => {
            let relative_path = analysis.file.clone();
            angular::recognize(&tree, source, &relative_path, analysis);
        }
        LanguageId::CSharp => dotnet::recognize(tree.root_node(), source.as_bytes(), analysis),
    }
}

/// D6: framework-level kinds are answered here before falling through to a
/// language analyzer's own table (`languages::kind_matches` consults this
/// first). `None` means "not a framework kind" — the caller falls through
/// to the per-analyzer table; neither analyzer's existing table changes.
pub fn kind_matches(kind: RelationshipKind, sym: SymbolKind) -> Option<bool> {
    match kind {
        RelationshipKind::Injects => Some(matches!(sym, SymbolKind::Class | SymbolKind::Interface)),
        RelationshipKind::RegisteredAs => Some(matches!(
            sym,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Variable
        )),
        RelationshipKind::HandlesRoute => Some(matches!(
            sym,
            SymbolKind::Class | SymbolKind::Method | SymbolKind::Function | SymbolKind::Variable
        )),
        RelationshipKind::Triggers => Some(matches!(
            sym,
            SymbolKind::Class | SymbolKind::Method | SymbolKind::Function
        )),
        RelationshipKind::PersistsTo => Some(matches!(
            sym,
            SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface
        )),
        _ => None,
    }
}

/// One resolved name inside a `MetaKey` (D14) — an array element, or the
/// single value of a non-array key.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetaEntry {
    pub(crate) name: String,
    pub(crate) span: SourceSpan,
}

/// One key of an object literal walked by `object_literal_entries`, plus
/// every name `entry_name` could resolve from its value (D14).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetaKey {
    pub(crate) key: String,
    pub(crate) entries: Vec<MetaEntry>,
}

/// D14: walks *any* TypeScript/JavaScript object literal `node` and returns
/// one `MetaKey` per `pair`. An array-valued pair yields one entry per
/// resolvable array element; a non-array-valued pair yields a single
/// one-entry `MetaKey` (so a route object's `path`/`component` come back in
/// the same shape as `@Component({ providers: [...] })`'s `providers`).
/// Reused as-is by Angular's `@Component`/`@NgModule` metadata and route
/// config arrays (PR4) — the only difference between them is what encloses
/// the object literal, not the object literal's own shape.
pub(crate) fn object_literal_entries(node: Node, source: &[u8]) -> Vec<MetaKey> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() == "pair")
        .filter_map(|pair| {
            let key_node = pair.child_by_field_name("key")?;
            let key = pair_key_text(key_node, source)?;
            let value = pair.child_by_field_name("value")?;
            let entries = if value.kind() == "array" {
                let mut vcursor = value.walk();
                value
                    .named_children(&mut vcursor)
                    .filter_map(|el| entry_name(el, source))
                    .collect()
            } else {
                entry_name(value, source).into_iter().collect()
            };
            Some(MetaKey { key, entries })
        })
        .collect()
}

/// A `pair`'s `key` field text: a bare/property identifier as written, or a
/// string literal with its quotes stripped.
fn pair_key_text(key_node: Node, source: &[u8]) -> Option<String> {
    let text = key_node.utf8_text(source).ok()?;
    match key_node.kind() {
        "string" => Some(
            text.trim_matches(|c| c == '\'' || c == '"' || c == '`')
                .to_string(),
        ),
        _ => Some(text.to_string()),
    }
}

/// D14's entry-name resolution, applied to one array element or one
/// non-array value: bare identifier as-is; a member expression's last
/// segment; `new X()` -> `X`; an object literal with a `useClass`/
/// `useExisting`/`useFactory` key -> that value's resolved name, else its
/// `provide` value's resolved name; a string literal (route `path`) with
/// its quotes stripped; one level of `(...)` unwrapped. Anything else (an
/// arrow function, a spread, ...) resolves to `None` rather than a guess —
/// `loadComponent`'s arrow-function shape is Angular-specific (PR4), not
/// this generic walker's job.
fn entry_name(node: Node, source: &[u8]) -> Option<MetaEntry> {
    let span = span_of(node);
    let name = match node.kind() {
        "identifier" | "property_identifier" | "shorthand_property_identifier" => {
            node.utf8_text(source).ok()?.to_string()
        }
        "member_expression" => {
            let prop = node.child_by_field_name("property")?;
            prop.utf8_text(source).ok()?.to_string()
        }
        "new_expression" => {
            let ctor = node.child_by_field_name("constructor")?;
            entry_name(ctor, source)?.name
        }
        "string" => node
            .utf8_text(source)
            .ok()?
            .trim_matches(|c| c == '\'' || c == '"' || c == '`')
            .to_string(),
        "parenthesized_expression" => {
            let inner = node.named_child(0)?;
            entry_name(inner, source)?.name
        }
        "object" => return object_provider_name(node, source, span),
        _ => return None,
    };
    Some(MetaEntry { name, span })
}

/// The `{ provide, useClass }`/`{ provide, useExisting }`/`{ provide,
/// useFactory }` shape (D14): the resolved name of whichever of those three
/// keys is present, falling back to `provide`'s own resolved name.
fn object_provider_name(node: Node, source: &[u8], span: SourceSpan) -> Option<MetaEntry> {
    let mut cursor = node.walk();
    let pairs: Vec<Node> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "pair")
        .collect();

    for wanted in ["useClass", "useExisting", "useFactory"] {
        if let Some(name) = find_pair_value_name(&pairs, wanted, source) {
            return Some(MetaEntry { name, span });
        }
    }
    find_pair_value_name(&pairs, "provide", source).map(|name| MetaEntry { name, span })
}

fn find_pair_value_name(pairs: &[Node], key: &str, source: &[u8]) -> Option<String> {
    pairs.iter().find_map(|pair| {
        let key_node = pair.child_by_field_name("key")?;
        if pair_key_text(key_node, source)?.as_str() != key {
            return None;
        }
        let value = pair.child_by_field_name("value")?;
        entry_name(value, source).map(|e| e.name)
    })
}

/// D8: a decorator/attribute name matcher. PR3 scaffolds the type with the
/// one piece every future catalogue entry needs (a literal name); PR4-6 add
/// argument-slot predicates as they're actually needed by a real pattern —
/// speculative slots now would be unused width no catalogue has asked for.
#[allow(dead_code)]
pub(crate) struct AttrPattern {
    pub(crate) name: &'static str,
}

impl AttrPattern {
    pub(crate) fn matches(&self, decorator_name: &str) -> bool {
        self.name == decorator_name
    }
}

/// D8/D9: a call-shape matcher — method name plus a minimum type-argument
/// count. D9: the receiver expression is never part of the match (an
/// untyped local like `app`/`services` isn't a type the indexer can check);
/// shape (here: type-argument count) is the discriminator instead.
#[allow(dead_code)]
pub(crate) struct CallPattern {
    pub(crate) name: &'static str,
    pub(crate) min_type_args: usize,
}

impl CallPattern {
    pub(crate) fn matches(&self, method_name: &str, type_arg_count: usize) -> bool {
        self.name == method_name && type_arg_count >= self.min_type_args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use codekurve_core::LanguageId;
    use tree_sitter::Parser;

    fn find_first<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_first(child, kind) {
                return Some(found);
            }
        }
        None
    }

    fn parse_ts(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    // --- kind_matches (task 3.10) -----------------------------------------

    #[test]
    fn kind_matches_table_sweep_for_all_five_framework_kinds() {
        const ALL_SYMBOL_KINDS: [SymbolKind; 16] = [
            SymbolKind::Module,
            SymbolKind::Namespace,
            SymbolKind::Class,
            SymbolKind::Interface,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Constructor,
            SymbolKind::Property,
            SymbolKind::Field,
            SymbolKind::Variable,
            SymbolKind::Parameter,
            SymbolKind::TypeAlias,
            SymbolKind::Import,
            SymbolKind::Export,
        ];

        for sym in ALL_SYMBOL_KINDS {
            assert_eq!(
                kind_matches(RelationshipKind::Injects, sym),
                Some(matches!(sym, SymbolKind::Class | SymbolKind::Interface))
            );
            assert_eq!(
                kind_matches(RelationshipKind::RegisteredAs, sym),
                Some(matches!(
                    sym,
                    SymbolKind::Class | SymbolKind::Interface | SymbolKind::Variable
                ))
            );
            assert_eq!(
                kind_matches(RelationshipKind::HandlesRoute, sym),
                Some(matches!(
                    sym,
                    SymbolKind::Class
                        | SymbolKind::Method
                        | SymbolKind::Function
                        | SymbolKind::Variable
                ))
            );
            assert_eq!(
                kind_matches(RelationshipKind::Triggers, sym),
                Some(matches!(
                    sym,
                    SymbolKind::Class | SymbolKind::Method | SymbolKind::Function
                ))
            );
            assert_eq!(
                kind_matches(RelationshipKind::PersistsTo, sym),
                Some(matches!(
                    sym,
                    SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface
                ))
            );
        }

        // Every non-framework kind must fall through (`None`) so the
        // per-analyzer table still owns the answer.
        for rel in [
            RelationshipKind::Defines,
            RelationshipKind::Contains,
            RelationshipKind::Imports,
            RelationshipKind::Exports,
            RelationshipKind::References,
            RelationshipKind::Calls,
            RelationshipKind::Constructs,
            RelationshipKind::Inherits,
            RelationshipKind::Implements,
            RelationshipKind::Overrides,
            RelationshipKind::UsesType,
            RelationshipKind::Reads,
            RelationshipKind::Writes,
            RelationshipKind::Decorates,
        ] {
            for sym in ALL_SYMBOL_KINDS {
                assert_eq!(
                    kind_matches(rel, sym),
                    None,
                    "{rel:?}/{sym:?} must fall through"
                );
            }
        }
    }

    /// Both analyzers' PR2 `kind_matches` sweeps still pass unchanged —
    /// enforced by `typescript.rs`'s and `csharp.rs`'s own existing tests;
    /// re-run here through `cargo test -p codekurve-analysis frameworks`'s
    /// sibling `typescript`/`csharp` filters isn't needed, this test only
    /// asserts the *new* free function's own contract above.
    #[test]
    fn kind_matches_never_answers_for_decorates() {
        for sym in [SymbolKind::Class, SymbolKind::Method, SymbolKind::Property] {
            assert_eq!(kind_matches(RelationshipKind::Decorates, sym), None);
        }
    }

    // --- marker prefilter (task 3.11) --------------------------------------

    #[test]
    fn marker_prefilter_never_parses_a_file_with_no_marker() {
        let before = PARSE_CALLS.with(|c| c.get());
        let analysis = extract::analyze(
            "export class Plain {}",
            LanguageId::TypeScript,
            "src/plain.ts",
        )
        .unwrap();
        let after = PARSE_CALLS.with(|c| c.get());

        assert_eq!(before, after, "no marker present, parse must never run");
        assert!(analysis.symbols.iter().all(|s| s.roles.is_empty()));
    }

    #[test]
    fn marker_prefilter_parses_when_a_marker_is_present() {
        let before = PARSE_CALLS.with(|c| c.get());
        extract::analyze(
            "@Injectable()\nexport class Svc {}",
            LanguageId::TypeScript,
            "src/svc.ts",
        )
        .unwrap();
        let after = PARSE_CALLS.with(|c| c.get());

        assert_eq!(
            after,
            before + 1,
            "a marker is present, parse must run exactly once"
        );
    }

    // --- object_literal_entries (task 3.12) --------------------------------

    #[test]
    fn object_literal_entries_resolves_every_shape() {
        let source = r#"
const x = {
  providers: [A, b.C, new D(), { provide: TOKEN, useClass: E }, { provide: TOKEN2, useExisting: F }],
  path: 'home',
};
"#;
        let tree = parse_ts(source);
        let object = find_first(tree.root_node(), "object").unwrap();
        let keys = object_literal_entries(object, source.as_bytes());

        let providers = keys.iter().find(|k| k.key == "providers").unwrap();
        let names: Vec<&str> = providers.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["A", "C", "D", "E", "F"]);

        let path = keys.iter().find(|k| k.key == "path").unwrap();
        assert_eq!(path.entries.len(), 1);
        assert_eq!(path.entries[0].name, "home");
    }

    #[test]
    fn object_literal_entries_nested_array_yields_one_entry_per_element() {
        let source = "const x = { imports: [ModA, ModB, ModC] };";
        let tree = parse_ts(source);
        let object = find_first(tree.root_node(), "object").unwrap();
        let keys = object_literal_entries(object, source.as_bytes());

        let imports = keys.iter().find(|k| k.key == "imports").unwrap();
        let names: Vec<&str> = imports.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["ModA", "ModB", "ModC"]);
    }

    // --- pattern matchers (D8/D9) -------------------------------------------

    #[test]
    fn attr_pattern_matches_by_literal_name_only() {
        let pattern = AttrPattern { name: "Component" };
        assert!(pattern.matches("Component"));
        assert!(!pattern.matches("Injectable"));
    }

    #[test]
    fn call_pattern_matches_by_name_and_minimum_type_arg_count() {
        let pattern = CallPattern {
            name: "AddScoped",
            min_type_args: 2,
        };
        assert!(pattern.matches("AddScoped", 2));
        assert!(pattern.matches("AddScoped", 3));
        assert!(!pattern.matches("AddScoped", 1));
        assert!(!pattern.matches("AddSingleton", 2));
    }

    // --- empty-catalogue regression guard (task 3.13) -----------------------

    /// Every phase-2/5 fixture that predates the Angular/.NET catalogues,
    /// run through the real `extract::analyze` entry point (which now calls
    /// `recognize` internally, task 3.5) — zero framework edges and zero
    /// roles on every one of them, since none of them use a recognized
    /// pattern. `fixtures/angular/` and `fixtures/dotnet/` (PR7, task 7.1-7.3)
    /// are deliberately excluded: those fixtures exist specifically to
    /// exercise the now-real catalogues and are asserted non-empty by
    /// `angular_graph.rs`/`dotnet_graph.rs` instead — a plain "zero
    /// catalogues" test would no longer be true of them by the time PR7
    /// landed, so this guard's scope narrows to what it can still honestly
    /// claim rather than special-casing every future fixture forever.
    #[test]
    fn empty_catalogues_produce_zero_framework_edges_on_every_fixture() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut roots = vec![
            manifest.join("tests/fixtures"),
            manifest.join("../../fixtures/mixed"),
        ];
        roots.retain(|p| p.exists());
        assert!(
            !roots.is_empty(),
            "expected at least one fixture root to exist"
        );

        let mut checked = 0usize;
        for root in roots {
            for entry in ignore::WalkBuilder::new(&root).build().flatten() {
                let path = entry.path();
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                let language = match ext {
                    "ts" | "tsx" => LanguageId::TypeScript,
                    "js" | "jsx" => LanguageId::JavaScript,
                    "cs" => LanguageId::CSharp,
                    _ => continue,
                };
                let source = std::fs::read_to_string(path).unwrap();
                let rel_path = path
                    .strip_prefix(&root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let analysis = extract::analyze(&source, language, &rel_path).unwrap();

                assert!(
                    analysis
                        .relationships
                        .iter()
                        .all(|r| !is_framework_kind(r.kind)),
                    "unexpected framework edge in {}",
                    path.display()
                );
                assert!(
                    analysis.symbols.iter().all(|s| s.roles.is_empty()),
                    "unexpected role tag in {}",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "expected to check at least one fixture file");
    }

    fn is_framework_kind(kind: RelationshipKind) -> bool {
        matches!(
            kind,
            RelationshipKind::Injects
                | RelationshipKind::RegisteredAs
                | RelationshipKind::HandlesRoute
                | RelationshipKind::Triggers
                | RelationshipKind::PersistsTo
        )
    }

    // --- static grep check (task 3.14) --------------------------------------

    /// No file under `languages/` contains any marker string from this
    /// module's own catalogue (following `scripts/check_licensing.py`'s
    /// grep-based-check precedent, same as typescript.rs's task 2.8 check).
    /// Comments and the `#[cfg(test)]` module are stripped first — both
    /// analyzers legitimately use realistic names (`HttpGet`, ...) as
    /// generic, framework-blind extraction examples in doc comments/tests;
    /// this check is about branching logic, not illustrative fixtures.
    #[test]
    fn no_frameworks_marker_leaks_into_languages_module() {
        for (path, contents) in [
            (
                "src/languages/typescript.rs",
                include_str!("../languages/typescript.rs"),
            ),
            (
                "src/languages/csharp.rs",
                include_str!("../languages/csharp.rs"),
            ),
        ] {
            let production_code = production_code_only(contents);
            for marker in TS_MARKERS.iter().chain(CS_MARKERS.iter()) {
                assert!(
                    !production_code.contains(marker),
                    "{path} contains a frameworks/ marker {marker:?} outside comments/tests"
                );
            }
        }
    }

    /// Drops everything from `#[cfg(test)]` onward and every `//`/`///`
    /// comment (mirrors `typescript.rs`'s task 2.8 helper).
    fn production_code_only(src: &str) -> String {
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        code.lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
