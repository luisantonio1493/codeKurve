//! Whole-project resolution (design §Interfaces, plan §20.2/§20.4): turns
//! the by-name/module-specifier `EdgeTarget::Unresolved` edges every
//! `extract::analyze` call leaves behind into `EdgeTarget::Global`/`External`
//! edges (or `UnresolvedReference` rows, never silently dropped — §18.3),
//! once every file in the run has been parsed. Pure library, no filesystem
//! I/O: it only ever looks at the `FileAnalysis.file` paths already produced
//! by discovery + `extract::analyze`, so no project-root parameter is
//! needed here (PR4b's pipeline is what makes that file set complete).
//!
//! PR4a-1 shipped the whole-project symbol table and module resolution
//! (§20.2, still below); this adds reference/call resolution against that
//! table (§20.4): `resolve()` walks every `Unresolved` edge and either
//! resolves it to `Global`/`External`, or moves it to
//! `FileAnalysis.unresolved` when it has zero candidates.

use std::collections::{HashMap, HashSet};

use codekurve_core::{Confidence, Provenance, RelationshipKind, SymbolKind};

use crate::extract::kind_matches;
use crate::ir::{EdgeTarget, ExtractedRelationship, FileAnalysis, UnresolvedReference};

/// Minimal `tsconfig.json` `compilerOptions.paths` alias map: prefix (with a
/// single trailing `*`) -> replacement prefix. Deliberately narrow scope
/// (design's "minimal scope" note) — no `baseUrl` chains, no mid-segment
/// wildcards, no exact (non-wildcard) entries beyond a literal match.
pub type TsconfigAliases = HashMap<String, String>;

/// A project symbol as seen from resolution: enough to build an
/// `EdgeTarget::Global` and to apply a `RelationshipKind`/`SymbolKind`
/// compatibility check (PR4a-2).
#[derive(Debug, Clone)]
pub(crate) struct ProjectSymbol {
    pub(crate) file: String,
    pub(crate) qualified_name: String,
    pub(crate) kind: SymbolKind,
}

/// Whole-project symbol index built from every file's `FileAnalysis`.
pub struct SymbolTable {
    /// Bare name -> every symbol in the project sharing it, for by-name
    /// `Calls`/`Constructs`/`Inherits`/`Implements` resolution (PR4a-2).
    pub(crate) by_name: HashMap<String, Vec<ProjectSymbol>>,
    /// file -> exported name -> symbol, for import/re-export resolution
    /// (PR4a-2).
    pub(crate) exports: HashMap<String, HashMap<String, ProjectSymbol>>,
}

impl SymbolTable {
    pub fn build(files: &[FileAnalysis]) -> Self {
        let mut by_name: HashMap<String, Vec<ProjectSymbol>> = HashMap::new();
        for file in files {
            for sym in &file.symbols {
                by_name
                    .entry(sym.name.clone())
                    .or_default()
                    .push(ProjectSymbol {
                        file: file.file.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind,
                    });
            }
        }

        let mut exports: HashMap<String, HashMap<String, ProjectSymbol>> = HashMap::new();
        for file in files {
            let by_local_key: HashMap<&str, &crate::ir::ExtractedSymbol> = file
                .symbols
                .iter()
                .map(|s| (s.local_key.as_str(), s))
                .collect();
            for rel in &file.relationships {
                if rel.kind != RelationshipKind::Exports {
                    continue;
                }
                let EdgeTarget::Local(key) = &rel.target else {
                    continue;
                };
                let Some(sym) = by_local_key.get(key.as_str()) else {
                    continue;
                };
                exports.entry(file.file.clone()).or_default().insert(
                    sym.name.clone(),
                    ProjectSymbol {
                        file: file.file.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind,
                    },
                );
            }
        }
        // Fallback: `extract.rs` only emits an `Exports` edge for named,
        // default, and re-export forms — a direct declaration export
        // (`export class Foo {}`) leaves no edge at all (see extract.rs's
        // `collect_exports` doc comment), yet that's the most common
        // real-world export style. Without this, ordinary `import { x }
        // from './mod'` against a plain `export function x() {}` would
        // never resolve. Register every top-level Class/Function/Interface
        // symbol under its own name too, when the (authoritative) loop
        // above didn't already claim that name — `is_exported` isn't
        // tracked yet (extract.rs, PR1), so this may register a
        // not-actually-exported top-level symbol; accepted MVP
        // over-inclusion, not a false *identity* match.
        for file in files {
            for sym in &file.symbols {
                if sym.parent.is_none()
                    && matches!(
                        sym.kind,
                        SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
                    )
                {
                    exports
                        .entry(file.file.clone())
                        .or_default()
                        .entry(sym.name.clone())
                        .or_insert_with(|| ProjectSymbol {
                            file: file.file.clone(),
                            qualified_name: sym.qualified_name.clone(),
                            kind: sym.kind,
                        });
                }
            }
        }

        Self { by_name, exports }
    }
}

/// Where a relative/aliased import specifier resolved to (§20.2 order).
pub(crate) enum ModuleResolution {
    Project(String),
    External(String),
    Unresolved,
}

/// §20.2: relative path -> exact file -> implicit `.ts/.tsx/.js/.jsx` ->
/// `index.*` -> tsconfig alias -> external node (never indexed).
pub(crate) fn resolve_module(
    importer: &str,
    specifier: &str,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
) -> ModuleResolution {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let candidate = join_relative(importer, specifier);
        return match find_file(&candidate, known_files) {
            Some(path) => ModuleResolution::Project(path),
            None => ModuleResolution::Unresolved,
        };
    }
    if let Some(candidate) = apply_alias(specifier, aliases) {
        return match find_file(&candidate, known_files) {
            Some(path) => ModuleResolution::Project(path),
            None => ModuleResolution::Unresolved,
        };
    }
    ModuleResolution::External(specifier.to_string())
}

/// Joins a relative specifier against the importer's directory, resolving
/// `.`/`..` segments. Forward-slash only (relative paths in this IR are
/// always `/`-separated, see `extract::qualified_name`), so this stays a
/// plain string operation rather than `std::path::Path` (which is
/// platform-separator-sensitive).
fn join_relative(importer: &str, specifier: &str) -> String {
    let mut parts: Vec<&str> = importer
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    for seg in specifier.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Tries `base` as-is, then with each implicit extension, then as an
/// `index.*` directory entry, per §20.2's order.
fn find_file(base: &str, known_files: &HashSet<String>) -> Option<String> {
    if known_files.contains(base) {
        return Some(base.to_string());
    }
    for ext in [".ts", ".tsx", ".js", ".jsx"] {
        let candidate = format!("{base}{ext}");
        if known_files.contains(&candidate) {
            return Some(candidate);
        }
    }
    for index in ["index.ts", "index.tsx", "index.js", "index.jsx"] {
        let candidate = if base.is_empty() {
            index.to_string()
        } else {
            format!("{base}/{index}")
        };
        if known_files.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A single-`*`-prefix `compilerOptions.paths` alias mapping (e.g.
/// `"@app/*": "src/*"`), or a literal (no-wildcard) exact match.
fn apply_alias(specifier: &str, aliases: &TsconfigAliases) -> Option<String> {
    for (pattern, replacement) in aliases {
        match pattern.strip_suffix('*') {
            Some(prefix) => {
                if let Some(rest) = specifier.strip_prefix(prefix) {
                    let repl_prefix = replacement.strip_suffix('*').unwrap_or(replacement);
                    return Some(format!("{repl_prefix}{rest}"));
                }
            }
            None if pattern == specifier => return Some(replacement.clone()),
            None => {}
        }
    }
    None
}

/// Counts of how many edges resolution produced, for `codekurve index`'s
/// summary output (PR4b).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionReport {
    pub resolved: usize,
    pub unresolved: usize,
}

/// Resolves every `EdgeTarget::Unresolved` edge left by `extract::analyze`
/// against the whole-project `SymbolTable`, in place. Zero-candidate edges
/// move to `FileAnalysis.unresolved` (never dropped, §18.3); multi-candidate
/// edges become one Low-confidence edge per candidate (never pick first,
/// §20.4).
pub fn resolve(files: &mut [FileAnalysis], aliases: &TsconfigAliases) -> ResolutionReport {
    let table = SymbolTable::build(&*files);
    let known_files: HashSet<String> = files.iter().map(|f| f.file.clone()).collect();
    let mut report = ResolutionReport::default();

    for file in files.iter_mut() {
        let file_name = file.file.clone();
        let old_rels = std::mem::take(&mut file.relationships);
        let mut new_rels = Vec::with_capacity(old_rels.len());
        for rel in old_rels {
            resolve_one(
                &file_name,
                rel,
                &table,
                &known_files,
                aliases,
                &mut new_rels,
                &mut file.unresolved,
                &mut report,
            );
        }
        file.relationships = new_rels;
    }
    report
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    file: &str,
    rel: ExtractedRelationship,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let text = match &rel.target {
        EdgeTarget::Unresolved(t) => t.clone(),
        _ => {
            new_rels.push(rel);
            return;
        }
    };

    match rel.kind {
        RelationshipKind::Imports => resolve_import(
            file,
            &rel,
            &text,
            table,
            known_files,
            aliases,
            new_rels,
            unresolved,
            report,
        ),
        RelationshipKind::Exports => resolve_export(
            file,
            &rel,
            &text,
            table,
            known_files,
            aliases,
            new_rels,
            unresolved,
            report,
        ),
        RelationshipKind::Calls
        | RelationshipKind::Constructs
        | RelationshipKind::Inherits
        | RelationshipKind::Implements
        | RelationshipKind::References => {
            resolve_by_name(&rel, &text, table, new_rels, unresolved, report)
        }
        // `Defines`/`Overrides`/`UsesType`/`Reads`/`Writes` aren't produced
        // by `extract::analyze` yet — nothing to resolve, pass through
        // unchanged.
        _ => new_rels.push(rel),
    }
}

/// §20.4 tiers for a project-wide by-name lookup (`Calls`/`Constructs`
/// same-file misses, `Inherits`/`Implements` same-file misses).
fn resolve_by_name(
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let matches: Vec<&ProjectSymbol> = table
        .by_name
        .get(text)
        .into_iter()
        .flatten()
        .filter(|ps| kind_matches(rel.kind, ps.kind))
        .collect();

    match matches.as_slice() {
        [] => {
            unresolved.push(unresolved_ref(rel, text, "no matching symbol in project"));
            report.unresolved += 1;
        }
        [only] => {
            push_global(
                new_rels,
                rel,
                &only.file,
                &only.qualified_name,
                Provenance::Resolved,
                Confidence::High,
            );
            report.resolved += 1;
        }
        many => {
            // Never silently pick one (§20.4/§27.4): one Low-confidence,
            // Heuristic-provenance edge per candidate — extraction collapses
            // both `this.foo()`/`obj.foo()` member calls and bare `foo()`
            // calls to the same by-name target, so a multi-candidate hit is
            // always a genuine "receiver undeterminable" case here.
            for candidate in many {
                push_global(
                    new_rels,
                    rel,
                    &candidate.file,
                    &candidate.qualified_name,
                    Provenance::Heuristic,
                    Confidence::Low,
                );
                report.resolved += 1;
            }
        }
    }
}

/// An `Imports` edge, or an `Exports` edge with a `from` module specifier.
#[allow(clippy::too_many_arguments)]
fn resolve_import(
    file: &str,
    rel: &ExtractedRelationship,
    specifier: &str,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    match resolve_module(file, specifier, known_files, aliases) {
        ModuleResolution::External(pkg) => {
            push_external(new_rels, rel, pkg);
            report.resolved += 1;
        }
        ModuleResolution::Unresolved => {
            unresolved.push(unresolved_ref(
                rel,
                specifier,
                "module not found in project",
            ));
            report.unresolved += 1;
        }
        ModuleResolution::Project(target_file) => resolve_binding(
            rel,
            &target_file,
            rel.reason.as_deref(),
            table,
            new_rels,
            unresolved,
            report,
        ),
    }
}

/// An `Exports` edge: either a same-file export with zero local candidates
/// (extract.rs already searched the file — can never resolve elsewhere), or
/// a module-specifier re-export (`export { x } from './mod'` / `export *
/// from './mod'`).
#[allow(clippy::too_many_arguments)]
fn resolve_export(
    file: &str,
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    if rel.reason.as_deref() == Some(crate::extract::NO_SAME_FILE_MATCH_REASON) {
        unresolved.push(unresolved_ref(rel, text, "not declared in this file"));
        report.unresolved += 1;
        return;
    }

    match resolve_module(file, text, known_files, aliases) {
        ModuleResolution::External(pkg) => {
            push_external(new_rels, rel, pkg);
            report.resolved += 1;
        }
        ModuleResolution::Unresolved => {
            unresolved.push(unresolved_ref(rel, text, "module not found in project"));
            report.unresolved += 1;
        }
        ModuleResolution::Project(target_file) => resolve_binding(
            rel,
            &target_file,
            rel.reason.as_deref(),
            table,
            new_rels,
            unresolved,
            report,
        ),
    }
}

/// Resolves a named binding (`Some(name)`), namespace import/blanket
/// re-export (`Some("*")`/`None`) against a module already resolved to a
/// project file.
fn resolve_binding(
    rel: &ExtractedRelationship,
    target_file: &str,
    binding: Option<&str>,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    match binding {
        Some("*") | None => {
            // Namespace import / `export * from`: binds the whole module,
            // not one symbol — no `Module` symbol kind is emitted yet
            // (ponytail: add one if per-namespace-member access is needed).
            push_global(
                new_rels,
                rel,
                target_file,
                "*",
                Provenance::Resolved,
                Confidence::High,
            );
            report.resolved += 1;
        }
        Some(name) => match table.exports.get(target_file).and_then(|m| m.get(name)) {
            Some(sym) => {
                push_global(
                    new_rels,
                    rel,
                    &sym.file,
                    &sym.qualified_name,
                    Provenance::Resolved,
                    Confidence::Exact,
                );
                report.resolved += 1;
            }
            None => {
                // A `"default"` reason also lands here: extraction doesn't
                // tag which local export is the module's `export default`
                // target (see extract.rs's `collect_exports` doc comment),
                // so a literal `"default"` key is never present — reported
                // as unresolved rather than guessed.
                unresolved.push(unresolved_ref(rel, name, "not exported by target module"));
                report.unresolved += 1;
            }
        },
    }
}

fn push_global(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    file: &str,
    qualified_name: &str,
    provenance: Provenance,
    confidence: Confidence,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::Global {
            file: file.to_string(),
            qualified_name: qualified_name.to_string(),
        },
        kind: rel.kind,
        span: rel.span,
        provenance,
        confidence,
        reason: None,
    });
}

fn push_external(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    pkg: String,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::External(pkg),
        kind: rel.kind,
        span: rel.span,
        provenance: Provenance::Resolved,
        confidence: Confidence::Exact,
        reason: rel.reason.clone(),
    });
}

fn unresolved_ref(
    rel: &ExtractedRelationship,
    target_text: &str,
    reason: &str,
) -> UnresolvedReference {
    UnresolvedReference {
        source_local_key: rel.source_local_key.clone(),
        relationship_kind: rel.kind,
        target_text: target_text.to_string(),
        context: rel.reason.clone(),
        candidate_count: 0,
        reason: reason.to_string(),
        confidence: Confidence::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::analyze;
    use codekurve_core::LanguageId;

    fn analyzed(path: &str, source: &str) -> FileAnalysis {
        analyze(source, LanguageId::TypeScript, path).unwrap()
    }

    /// §20.2 relative-path resolution, exercised through `resolve_module`
    /// directly: implicit extension, then `index.*` directory resolution.
    #[test]
    fn relative_import_resolves_implicit_extension_and_index() {
        let known: HashSet<String> = ["src/app.ts", "src/utils.ts", "src/ui/index.ts"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let aliases = TsconfigAliases::new();

        let utils = resolve_module("src/app.ts", "./utils", &known, &aliases);
        assert!(matches!(utils, ModuleResolution::Project(p) if p == "src/utils.ts"));

        let ui = resolve_module("src/app.ts", "./ui", &known, &aliases);
        assert!(matches!(ui, ModuleResolution::Project(p) if p == "src/ui/index.ts"));
    }

    /// A relative specifier that matches no project file (spec
    /// scenario-adjacent to "Zero-candidate import").
    #[test]
    fn relative_import_to_missing_file_is_unresolved() {
        let known: HashSet<String> = ["src/app.ts".to_string()].into_iter().collect();
        let result = resolve_module(
            "src/app.ts",
            "./nonexistent",
            &known,
            &TsconfigAliases::new(),
        );
        assert!(matches!(result, ModuleResolution::Unresolved));
    }

    /// A single-`*`-prefix tsconfig alias resolves like a relative import.
    #[test]
    fn tsconfig_alias_resolves_to_project_file() {
        let known: HashSet<String> = ["src/main.ts", "src/utils.ts"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut aliases = TsconfigAliases::new();
        aliases.insert("@app/*".to_string(), "src/*".to_string());

        let result = resolve_module("src/main.ts", "@app/utils", &known, &aliases);
        assert!(matches!(result, ModuleResolution::Project(p) if p == "src/utils.ts"));
    }

    /// A bare package specifier resolves to an external node (spec
    /// "External package import"), never a project/unresolved outcome.
    #[test]
    fn bare_specifier_resolves_external() {
        let known: HashSet<String> = ["src/main.ts".to_string()].into_iter().collect();
        let result = resolve_module("src/main.ts", "zod", &known, &TsconfigAliases::new());
        assert!(matches!(result, ModuleResolution::External(pkg) if pkg == "zod"));
    }

    /// `SymbolTable::build`'s direct-declaration-export fallback: a plain
    /// `export function x() {}` (no `Exports` edge from extract.rs) is
    /// still importable by name, and registered in `by_name` too (project-
    /// wide by-name lookup, PR4a-2).
    #[test]
    fn symbol_table_registers_direct_declaration_exports() {
        let files = vec![analyzed("src/utils.ts", "export function helper() {}\n")];
        let table = SymbolTable::build(&files);

        let export = table
            .exports
            .get("src/utils.ts")
            .and_then(|m| m.get("helper"))
            .unwrap();
        assert_eq!(export.file, "src/utils.ts");
        assert_eq!(export.qualified_name, "src/utils.ts::helper");
        assert_eq!(export.kind, SymbolKind::Function);

        let by_name = table.by_name.get("helper").unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].qualified_name, "src/utils.ts::helper");
    }

    /// Relative import + implicit extension/`index.*`, end to end through
    /// `resolve()` (spec "Implicit extension resolution").
    #[test]
    fn resolve_named_import_across_files() {
        let mut files = vec![
            analyzed("src/app.ts", "import { helper } from './utils';\n"),
            analyzed("src/utils.ts", "export function helper() {}\n"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);

        let edge = files[0].relationships.first().unwrap();
        assert_eq!(
            edge.target,
            EdgeTarget::Global {
                file: "src/utils.ts".to_string(),
                qualified_name: "src/utils.ts::helper".to_string(),
            }
        );
        assert_eq!(edge.confidence, Confidence::Exact);
    }

    /// A bare package specifier resolves to an external node, never an
    /// `UnresolvedReference` row (spec "External package import").
    #[test]
    fn external_package_import_has_no_unresolved_row() {
        let mut files = vec![analyzed("src/main.ts", "import { z } from 'zod';\n")];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);
        assert!(files[0].unresolved.is_empty());
        assert_eq!(
            files[0].relationships.first().unwrap().target,
            EdgeTarget::External("zod".to_string())
        );
    }

    /// A same-file-unresolved `Calls` edge resolves to the single cross-file
    /// candidate at High confidence (spec-adjacent to "Exact local call").
    #[test]
    fn cross_file_call_resolves_to_single_candidate() {
        let mut files = vec![
            analyzed("src/caller.ts", "function run() { return doWork(); }\n"),
            analyzed("src/worker.ts", "export function doWork() { return 1; }\n"),
        ];

        resolve(&mut files, &TsconfigAliases::new());

        let call = files[0]
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(
            call.target,
            EdgeTarget::Global {
                file: "src/worker.ts".to_string(),
                qualified_name: "src/worker.ts::doWork".to_string(),
            }
        );
        assert_eq!(call.provenance, Provenance::Resolved);
        assert_eq!(call.confidence, Confidence::High);
    }

    /// Two cross-file candidates -> two Low-confidence edges, never pick
    /// one (spec "Multi-candidate call is not unresolved").
    #[test]
    fn multi_candidate_call_produces_one_low_confidence_edge_per_candidate() {
        let mut files = vec![
            analyzed("src/caller.ts", "function run() { return doWork(); }\n"),
            analyzed("src/worker1.ts", "export function doWork() { return 1; }\n"),
            analyzed("src/worker2.ts", "export function doWork() { return 2; }\n"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);

        let calls: Vec<_> = files[0]
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .collect();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|r| r.confidence == Confidence::Low));
        assert!(calls.iter().all(|r| r.provenance == Provenance::Heuristic));
        assert!(files[0].unresolved.is_empty());
    }

    /// Zero candidates anywhere in the project -> `UnresolvedReference`,
    /// never dropped and never left as a relationships-table row (spec
    /// "Zero-candidate import" principle applies equally to calls).
    #[test]
    fn zero_candidate_call_becomes_unresolved_reference() {
        let mut files = vec![analyzed(
            "src/lonely.ts",
            "function run() { return neverDefined(); }\n",
        )];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);

        assert!(!files[0]
            .relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::Calls));

        let unresolved = files[0].unresolved.first().unwrap();
        assert_eq!(unresolved.relationship_kind, RelationshipKind::Calls);
        assert_eq!(unresolved.target_text, "neverDefined");
        assert_eq!(unresolved.candidate_count, 0);
        assert_eq!(unresolved.confidence, Confidence::Unresolved);
    }
}
