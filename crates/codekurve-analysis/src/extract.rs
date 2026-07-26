//! Symbol and intra-file relationship extraction via Tree-sitter. See
//! CODEKURVE_MASTER_PLAN.md §Fase 1 (symbols) and §18/§20 (relationship IR).
//! Cross-file resolution (imports/exports, cross-file calls) lands in a later
//! phase slice — every edge here either resolves to a same-file symbol or
//! carries `EdgeTarget::Unresolved(text)` for a later pass to pick up.

use codekurve_core::error::{Error, Result};
use codekurve_core::{
    Confidence, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind,
};
use tree_sitter::{Node, Parser};

use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};

/// Parse `source` for the given language and extract its symbols plus
/// intra-file relationships into a per-file `FileAnalysis`. `unresolved`
/// stays empty here — it is populated once whole-project resolution (a later
/// phase slice) proves a target has zero candidates anywhere in the project.
pub fn analyze(source: &str, language: LanguageId, relative_path: &str) -> Result<FileAnalysis> {
    // ponytail: one grammar for both languages — the TypeScript grammar parses
    // plain JS as a subset, TSX covers JS/JSX. Add tree-sitter-javascript only
    // if fidelity gaps appear (§12 lists it for later).
    let grammar = match language {
        LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        LanguageId::JavaScript => tree_sitter_typescript::LANGUAGE_TSX,
    };

    let mut parser = Parser::new();
    parser
        .set_language(&grammar.into())
        .map_err(|e| Error::Parse(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("parser returned no tree".to_string()))?;

    let mut ctx = CollectCtx {
        source: source.as_bytes(),
        language,
        relative_path,
        out: Vec::new(),
        out_rels: Vec::new(),
        pending: Vec::new(),
    };
    collect(tree.root_node(), None, None, &mut ctx);
    let mut relationships = ctx.out_rels;
    resolve_pending(&ctx.out, ctx.pending, &mut relationships);

    Ok(FileAnalysis {
        file: relative_path.to_string(),
        symbols: ctx.out,
        relationships,
        unresolved: Vec::new(),
        diagnostics: Vec::new(),
    })
}

/// A heritage/call/construct target discovered while walking, deferred until
/// the whole file's symbols are known (both may be forward references).
struct PendingRel {
    source_key: String,
    kind: RelationshipKind,
    target_name: String,
    span: SourceSpan,
}

/// Everything the recursive walk needs that doesn't change with tree depth —
/// bundled so `collect` stays under clippy's argument-count limit; `node`,
/// `parent` (enclosing class name, for qualified names), and `scope`
/// (enclosing function/method local key, for call/construct attribution)
/// are the only things that vary per call and stay as explicit arguments.
struct CollectCtx<'a> {
    source: &'a [u8],
    language: LanguageId,
    relative_path: &'a str,
    out: Vec<ExtractedSymbol>,
    out_rels: Vec<ExtractedRelationship>,
    pending: Vec<PendingRel>,
}

fn collect(node: Node, parent: Option<&str>, scope: Option<&str>, ctx: &mut CollectCtx) {
    let (source, language, relative_path) = (ctx.source, ctx.language, ctx.relative_path);
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            let name = push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Class,
                parent,
                &mut ctx.out,
            );
            if let Some(class_name) = &name {
                collect_heritage(node, source, relative_path, class_name, &mut ctx.pending);
            }
            // Descend with the class name as the new parent so members get a
            // `Class.member` qualified name; this replaces the generic
            // recursion below for this subtree.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, name.as_deref(), scope, ctx);
            }
            return;
        }
        "interface_declaration" => {
            // Falls through to the shared recursion below (no early
            // `return`) — interface members (`property_signature`/
            // `method_signature`) aren't extracted as symbols in this phase,
            // only the interface itself, so it can be a resolution target
            // for `implements` clauses (spec "Class extends and implements").
            push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Interface,
                parent,
                &mut ctx.out,
            );
        }
        "method_definition" if parent.is_some() => {
            let Some(class_name) = parent else { return };
            let kind = method_kind(node, source);
            let Some(method_name) = push_named(
                node,
                source,
                language,
                relative_path,
                kind,
                parent,
                &mut ctx.out,
            ) else {
                return;
            };
            let class_key = qualified_name(relative_path, None, class_name);
            let method_key = qualified_name(relative_path, Some(class_name), &method_name);
            ctx.out_rels.push(ExtractedRelationship {
                source_local_key: class_key,
                target: EdgeTarget::Local(method_key.clone()),
                kind: RelationshipKind::Contains,
                span: span_of(node),
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                reason: None,
            });
            // A method body can contain its own nested functions/classes;
            // recurse into it without the enclosing class as parent, but with
            // this method as the call/construct attribution scope.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, None, Some(method_key.as_str()), ctx);
            }
            return;
        }
        "function_declaration" | "generator_function_declaration" if is_top_level(node) => {
            let Some(name) = push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Function,
                None,
                &mut ctx.out,
            ) else {
                return;
            };
            let key = qualified_name(relative_path, None, &name);
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, parent, Some(key.as_str()), ctx);
            }
            return;
        }
        "import_statement" => {
            collect_imports(node, source, relative_path, &mut ctx.out_rels);
        }
        "export_statement" => {
            collect_exports(
                node,
                source,
                relative_path,
                &mut ctx.out_rels,
                &mut ctx.pending,
            );
        }
        "call_expression" => {
            if let (Some(source_key), Some(target_name)) = (scope, callee_name(node, source)) {
                ctx.pending.push(PendingRel {
                    source_key: source_key.to_string(),
                    kind: RelationshipKind::Calls,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        "new_expression" => {
            if let (Some(source_key), Some(target_name)) = (scope, constructor_name(node, source)) {
                ctx.pending.push(PendingRel {
                    source_key: source_key.to_string(),
                    kind: RelationshipKind::Constructs,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        // A type annotation (`: Foo`) on a variable/parameter/return type
        // that names a real declared type — not a primitive, union, or
        // other compound form (kept intentionally narrow). Falls through to
        // the shared recursion below; nothing further needs custom handling
        // inside a `type_annotation` node.
        "type_annotation" => {
            if let Some(target_name) = referenced_type_name(node, source) {
                ctx.pending.push(PendingRel {
                    source_key: reference_scope(parent, scope, relative_path),
                    kind: RelationshipKind::References,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, parent, scope, ctx);
    }
}

/// Resolves `Inherits`/`Implements` (extends/implements clauses) once the
/// class's own key is known; targets aren't known as symbols yet (forward
/// declarations, other files), so this is deferred to `resolve_pending`.
fn collect_heritage(
    node: Node,
    source: &[u8],
    relative_path: &str,
    class_name: &str,
    pending: &mut Vec<PendingRel>,
) {
    let Some(heritage) = find_child(node, "class_heritage") else {
        return;
    };
    let class_key = qualified_name(relative_path, None, class_name);

    if let Some(extends) = find_child(heritage, "extends_clause") {
        if let Some(value) = extends.child_by_field_name("value") {
            if let Ok(target_name) = value.utf8_text(source) {
                pending.push(PendingRel {
                    source_key: class_key.clone(),
                    kind: RelationshipKind::Inherits,
                    target_name: target_name.to_string(),
                    span: span_of(extends),
                });
            }
        }
    }

    if let Some(implements) = find_child(heritage, "implements_clause") {
        let mut cursor = implements.walk();
        for ty in implements.named_children(&mut cursor) {
            if let Some(target_name) = type_name(ty, source) {
                pending.push(PendingRel {
                    source_key: class_key.clone(),
                    kind: RelationshipKind::Implements,
                    target_name,
                    span: span_of(ty),
                });
            }
        }
    }
}

/// Emits one `Imports` edge per imported binding (default/named/namespace)
/// in an `import_statement`. Module resolution (specifier → concrete file)
/// is a later phase slice (PR4a) — the target always carries the raw module
/// specifier text. `reason` carries the imported name (`"default"` for a
/// default import, `"*"` for a namespace import) so a later pass can look it
/// up in the target module's exports without re-parsing source.
fn collect_imports(
    node: Node,
    source: &[u8],
    relative_path: &str,
    out: &mut Vec<ExtractedRelationship>,
) {
    let Some(specifier) = module_specifier(node, source) else {
        return;
    };
    let Some(clause) = find_child(node, "import_clause") else {
        return;
    };
    let source_key = relative_path.to_string();

    let mut cursor = clause.walk();
    for part in clause.named_children(&mut cursor) {
        match part.kind() {
            "identifier" => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Imports,
                &specifier,
                span_of(part),
                Some("default".to_string()),
            ),
            "namespace_import" => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Imports,
                &specifier,
                span_of(part),
                Some("*".to_string()),
            ),
            "named_imports" => {
                let mut ic = part.walk();
                for spec in part.named_children(&mut ic) {
                    let Some(name_node) = spec.child_by_field_name("name") else {
                        continue;
                    };
                    let Ok(name) = name_node.utf8_text(source) else {
                        continue;
                    };
                    push_unresolved_edge(
                        out,
                        &source_key,
                        RelationshipKind::Imports,
                        &specifier,
                        span_of(spec),
                        Some(name.to_string()),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Emits `Exports` edges from an `export_statement`. Re-exports (a `from`
/// clause, or `export * from`) target `EdgeTarget::Unresolved(module_specifier)`
/// since the source module isn't known until whole-project resolution
/// (PR4a). Local named/default exports defer through `pending` so they
/// resolve against this file's own symbol list, same as heritage edges.
/// Direct declaration exports (`export class Foo {}`) are intentionally not
/// covered — `is_exported` wiring is a later phase slice (see
/// `push_named`'s comment); their symbols still extract normally via the
/// shared recursion below.
fn collect_exports(
    node: Node,
    source: &[u8],
    relative_path: &str,
    out: &mut Vec<ExtractedRelationship>,
    pending: &mut Vec<PendingRel>,
) {
    let source_key = relative_path.to_string();
    let specifier = module_specifier(node, source);

    if let Some(clause) = find_child(node, "export_clause") {
        let mut cursor = clause.walk();
        for spec in clause.named_children(&mut cursor) {
            if spec.kind() != "export_specifier" {
                continue;
            }
            let Some(name_node) = spec.child_by_field_name("name") else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(source) else {
                continue;
            };
            match &specifier {
                Some(module) => push_unresolved_edge(
                    out,
                    &source_key,
                    RelationshipKind::Exports,
                    module,
                    span_of(spec),
                    Some(name.to_string()),
                ),
                None => pending.push(PendingRel {
                    source_key: source_key.clone(),
                    kind: RelationshipKind::Exports,
                    target_name: name.to_string(),
                    span: span_of(spec),
                }),
            }
        }
        return;
    }

    if let Some(module) = &specifier {
        // `export * from './mod'` (and `export * as ns from './mod'` — the
        // `ns` alias is dropped, this is extraction not resolution).
        push_unresolved_edge(
            out,
            &source_key,
            RelationshipKind::Exports,
            module,
            span_of(node),
            None,
        );
        return;
    }

    if is_default_export(node, source) {
        match export_default_name(node, source) {
            Some(name) => pending.push(PendingRel {
                source_key,
                kind: RelationshipKind::Exports,
                target_name: name,
                span: span_of(node),
            }),
            None => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Exports,
                "default",
                span_of(node),
                Some("anonymous default export".to_string()),
            ),
        }
    }
}

/// Pushes an `Extracted`/`Unresolved` relationship — the shared shape for
/// import/export edges whose target isn't a same-file symbol (module
/// specifier, or an anonymous default export placeholder).
fn push_unresolved_edge(
    out: &mut Vec<ExtractedRelationship>,
    source_key: &str,
    kind: RelationshipKind,
    target_text: &str,
    span: SourceSpan,
    reason: Option<String>,
) {
    out.push(ExtractedRelationship {
        source_local_key: source_key.to_string(),
        target: EdgeTarget::Unresolved(target_text.to_string()),
        kind,
        span,
        provenance: Provenance::Extracted,
        confidence: Confidence::Unresolved,
        reason,
    });
}

/// The module specifier text from an `import_statement`/`export_statement`'s
/// `source` string field, with surrounding quotes stripped.
fn module_specifier(node: Node, source: &[u8]) -> Option<String> {
    let string_node = node.child_by_field_name("source")?;
    let text = string_node.utf8_text(source).ok()?;
    Some(text.trim_matches(['\'', '"', '`']).to_string())
}

/// Whether an `export_statement` node is an `export default ...` (no
/// dedicated field distinguishes it in this grammar — the `default` keyword
/// is an anonymous token).
fn is_default_export(node: Node, source: &[u8]) -> bool {
    node.utf8_text(source)
        .map(|t| t.trim_start().starts_with("export default"))
        .unwrap_or(false)
}

/// The exported name for `export default X`, when determinable: a named
/// class/function declaration's own name, or a bare identifier value
/// (`export default someIdentifier;`). `None` for anonymous
/// declarations/expressions (`export default class {}`, `export default 42`).
fn export_default_name(node: Node, source: &[u8]) -> Option<String> {
    if let Some(decl) = node.child_by_field_name("declaration") {
        return decl
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string);
    }
    let value = node.child_by_field_name("value")?;
    if value.kind() == "identifier" {
        return value.utf8_text(source).ok().map(str::to_string);
    }
    None
}

/// Reason text for a deferred (`PendingRel`) target with zero same-file
/// candidates. Shared with `resolve.rs` so cross-file resolution (PR4a-2)
/// can tell a same-file-miss `Exports` edge apart from a module-specifier
/// re-export without re-parsing.
pub(crate) const NO_SAME_FILE_MATCH_REASON: &str = "no matching same-file symbol";

/// Resolves every deferred heritage/call/construct/local-export target
/// against the file's full symbol list, now that forward references are
/// visible. A same-file
/// name+kind match becomes `EdgeTarget::Local`; zero matches become
/// `EdgeTarget::Unresolved(text)` (never dropped — §18.3, though this is not
/// yet an `UnresolvedReference` row, that decision needs the whole-project
/// view a later phase slice adds); multiple matches emit one Low-confidence
/// edge per candidate rather than silently pick one (§20.4 principle).
fn resolve_pending(
    symbols: &[ExtractedSymbol],
    pending: Vec<PendingRel>,
    out: &mut Vec<ExtractedRelationship>,
) {
    for rel in pending {
        let matches: Vec<&ExtractedSymbol> = symbols
            .iter()
            .filter(|s| s.name == rel.target_name && kind_matches(rel.kind, s.kind))
            .collect();
        match matches.as_slice() {
            [] => out.push(ExtractedRelationship {
                source_local_key: rel.source_key,
                target: EdgeTarget::Unresolved(rel.target_name),
                kind: rel.kind,
                span: rel.span,
                provenance: Provenance::Extracted,
                confidence: Confidence::Unresolved,
                reason: Some(NO_SAME_FILE_MATCH_REASON.to_string()),
            }),
            [only] => out.push(ExtractedRelationship {
                source_local_key: rel.source_key,
                target: EdgeTarget::Local(only.local_key.clone()),
                kind: rel.kind,
                span: rel.span,
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                reason: None,
            }),
            many => {
                for candidate in many {
                    out.push(ExtractedRelationship {
                        source_local_key: rel.source_key.clone(),
                        target: EdgeTarget::Local(candidate.local_key.clone()),
                        kind: rel.kind,
                        span: rel.span,
                        provenance: Provenance::Extracted,
                        confidence: Confidence::Low,
                        reason: Some("ambiguous: multiple same-file candidates".to_string()),
                    });
                }
            }
        }
    }
}

/// Which symbol kinds a relationship kind may target, so a bare-name match
/// doesn't cross semantic categories (e.g. `new Foo()` must target a class,
/// not a same-named function). `pub(crate)` so `resolve.rs` (PR4a-2) applies
/// the same rule to cross-file candidates.
pub(crate) fn kind_matches(rel_kind: RelationshipKind, sym_kind: SymbolKind) -> bool {
    match rel_kind {
        RelationshipKind::Constructs => sym_kind == SymbolKind::Class,
        RelationshipKind::Calls => matches!(
            sym_kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
        ),
        RelationshipKind::Inherits | RelationshipKind::Implements => {
            matches!(sym_kind, SymbolKind::Class | SymbolKind::Interface)
        }
        RelationshipKind::References => {
            matches!(
                sym_kind,
                SymbolKind::Class | SymbolKind::Interface | SymbolKind::TypeAlias
            )
        }
        RelationshipKind::Exports => {
            matches!(
                sym_kind,
                SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
            )
        }
        _ => true,
    }
}

// ponytail: not `Iterator::find` (clippy's own suggestion) — tree-sitter's
// cursor-borrowed `Node` items don't outlive the function that way here.
#[allow(clippy::manual_find)]
fn find_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// The callee name of a `call_expression`: the bare identifier for
/// `foo()`, or the accessed property for `this.foo()`/`obj.foo()`.
fn callee_name(node: Node, source: &[u8]) -> Option<String> {
    let func = node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => func.utf8_text(source).ok().map(str::to_string),
        "member_expression" => func
            .child_by_field_name("property")
            .and_then(|p| p.utf8_text(source).ok())
            .map(str::to_string),
        _ => None,
    }
}

/// The constructed type name of a `new_expression` (`new Foo()`).
fn constructor_name(node: Node, source: &[u8]) -> Option<String> {
    let ctor = node.child_by_field_name("constructor")?;
    match ctor.kind() {
        "identifier" => ctor.utf8_text(source).ok().map(str::to_string),
        _ => None,
    }
}

/// A `type_identifier` or `generic_type` (e.g. `Comparable<T>`) name from an
/// `implements_clause` entry.
fn type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "generic_type" => node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string),
        _ => node.utf8_text(source).ok().map(str::to_string),
    }
}

/// The plain named-type target of a `type_annotation` (`: Foo`), when it's a
/// simple type reference. Deliberately narrow — primitives (`string`,
/// `number`, ...), unions, and generics (`Array<Foo>`) are skipped so
/// `References` edges only ever target a real declared type, not TypeScript
/// built-ins or compound type expressions.
fn referenced_type_name(node: Node, source: &[u8]) -> Option<String> {
    let ty = node.named_child(0)?;
    if ty.kind() == "type_identifier" {
        return ty.utf8_text(source).ok().map(str::to_string);
    }
    None
}

/// The best-effort enclosing symbol for a `References` edge: the innermost
/// function/method scope (parameter/return type), else the enclosing class
/// (a field's type annotation), else the file itself (a top-level
/// variable's type annotation).
fn reference_scope(parent: Option<&str>, scope: Option<&str>, relative_path: &str) -> String {
    scope
        .map(str::to_string)
        .or_else(|| parent.map(|p| qualified_name(relative_path, None, p)))
        .unwrap_or_else(|| relative_path.to_string())
}

/// A function is top-level when it sits directly under the module, optionally
/// wrapped in an `export` statement.
fn is_top_level(node: Node) -> bool {
    match node.parent() {
        None => false,
        Some(parent) if parent.kind() == "program" => true,
        Some(parent) if parent.kind() == "export_statement" => {
            parent.parent().is_some_and(|g| g.kind() == "program")
        }
        Some(_) => false,
    }
}

fn method_kind(node: Node, source: &[u8]) -> SymbolKind {
    let is_constructor = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|name| name == "constructor");
    if is_constructor {
        SymbolKind::Constructor
    } else {
        SymbolKind::Method
    }
}

fn qualified_name(relative_path: &str, parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(p) => format!("{relative_path}::{p}.{name}"),
        None => format!("{relative_path}::{name}"),
    }
}

/// Extracts `node`'s name and pushes an `ExtractedSymbol`, returning the name
/// so callers can use it as the `parent` for a nested scope (e.g. a class
/// body). Returns `None` (and pushes nothing) when the node has no name
/// field.
fn push_named(
    node: Node,
    source: &[u8],
    language: LanguageId,
    relative_path: &str,
    kind: SymbolKind,
    parent: Option<&str>,
    out: &mut Vec<ExtractedSymbol>,
) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source).ok()?.to_string();
    let qualified = qualified_name(relative_path, parent, &name);
    out.push(ExtractedSymbol {
        // ponytail: local_key = qualified_name for PR1 (single-file scope,
        // no cross-file collisions yet); revisit if pass 2 needs a distinct
        // pre-resolution key.
        local_key: qualified.clone(),
        name: name.clone(),
        qualified_name: qualified,
        kind,
        language,
        span: span_of(node),
        parent: parent.map(str::to_string),
        // ponytail: export detection deferred to import/export edge
        // extraction (whole-project resolution, later phase slice).
        is_exported: false,
        signature_fingerprint: signature_fingerprint(node, source),
    });
    Some(name)
}

/// design "symbol_key and signature_fingerprint": whitespace-normalized
/// declaration text of `type_parameters`/`parameters`/`return_type`,
/// `\x1f`-joined. Empty for declarations without a call signature
/// (class/interface/module stand-in).
fn signature_fingerprint(node: Node, source: &[u8]) -> String {
    ["type_parameters", "parameters", "return_type"]
        .iter()
        .filter_map(|f| node.child_by_field_name(f)?.utf8_text(source).ok())
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn span_of(node: Node) -> SourceSpan {
    let start = node.start_position();
    let end = node.end_position();
    SourceSpan {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: start.row + 1,
        start_column: start.column,
        end_line: end.row + 1,
        end_column: end.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"
export class MemberService {
  find(id: string) { return id; }
}

export function topLevel(): void {}

function alsoTop() {
  function nested() {}
  return nested;
}
"#;

    #[test]
    fn extracts_classes_functions_and_methods() {
        let analysis = analyze(SOURCE, LanguageId::TypeScript, "src/member.ts").unwrap();
        let names: Vec<(&str, SymbolKind)> = analysis
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();

        assert!(names.contains(&("MemberService", SymbolKind::Class)));
        assert!(names.contains(&("topLevel", SymbolKind::Function)));
        assert!(names.contains(&("alsoTop", SymbolKind::Function)));
        assert!(names.contains(&("find", SymbolKind::Method)));
        // Nested function inside a function body is not extracted.
        assert!(!names.iter().any(|(n, _)| *n == "nested"));
    }

    #[test]
    fn span_points_at_declaration() {
        let analysis = analyze(SOURCE, LanguageId::TypeScript, "src/member.ts").unwrap();
        let svc = analysis
            .symbols
            .iter()
            .find(|s| s.name == "MemberService")
            .unwrap();
        assert_eq!(svc.span.start_line, 2);
        assert!(svc.span.end_byte > svc.span.start_byte);
    }

    /// Spec scenario "Nested member qualified name" (Requirement "Real
    /// Qualified Name Computation").
    #[test]
    fn nested_method_qualified_name() {
        let source =
            "export class MemberService {\n  getEligibility(id: string) { return id; }\n}\n";
        let analysis = analyze(
            source,
            LanguageId::TypeScript,
            "src/services/member.service.ts",
        )
        .unwrap();

        let method = analysis
            .symbols
            .iter()
            .find(|s| s.name == "getEligibility")
            .unwrap();
        assert_eq!(
            method.qualified_name,
            "src/services/member.service.ts::MemberService.getEligibility"
        );
        assert_eq!(method.parent.as_deref(), Some("MemberService"));
    }

    /// Spec scenario "Contains hierarchy" (Requirement "Relationship Kind
    /// Extraction").
    #[test]
    fn contains_edges_link_class_to_methods() {
        let source = "class Box {\n  a() {}\n  b() {}\n}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/box.ts").unwrap();

        let contains: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Contains)
            .collect();
        assert_eq!(contains.len(), 2);
        for edge in &contains {
            assert_eq!(edge.source_local_key, "src/box.ts::Box");
            assert_eq!(edge.provenance, Provenance::Extracted);
            assert_eq!(edge.confidence, Confidence::Exact);
        }
    }

    /// Spec scenario "Class extends and implements" (Requirement
    /// "Relationship Kind Extraction").
    #[test]
    fn heritage_edges_extends_and_implements() {
        let source = "class Base {}\nclass Foo extends Base implements IFoo {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/heritage.ts").unwrap();

        let extends = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Inherits)
            .unwrap();
        assert_eq!(extends.source_local_key, "src/heritage.ts::Foo");
        assert_eq!(
            extends.target,
            EdgeTarget::Local("src/heritage.ts::Base".to_string())
        );
        assert_eq!(extends.confidence, Confidence::Exact);

        // No same-file symbol named IFoo (interfaces aren't extracted as
        // symbols yet) — the edge still exists, per §18.3, rather than being
        // silently dropped.
        let implements = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Implements)
            .unwrap();
        assert_eq!(implements.source_local_key, "src/heritage.ts::Foo");
        assert_eq!(
            implements.target,
            EdgeTarget::Unresolved("IFoo".to_string())
        );
        assert_eq!(implements.confidence, Confidence::Unresolved);
    }

    /// A method calling a sibling method resolves to a same-file `Calls`
    /// edge (spec "Exact local call").
    #[test]
    fn method_call_to_sibling_resolves_locally() {
        let source =
            "class Service {\n  run() { return this.helper(); }\n  helper() { return 1; }\n}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/service.ts").unwrap();

        let call = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(call.source_local_key, "src/service.ts::Service.run");
        assert_eq!(
            call.target,
            EdgeTarget::Local("src/service.ts::Service.helper".to_string())
        );
        assert_eq!(call.confidence, Confidence::Exact);
    }

    /// A `new` expression targeting a same-file class resolves to a
    /// `Constructs` edge (spec "Exact local call" tier applies equally to
    /// constructs).
    #[test]
    fn new_expression_resolves_to_local_class() {
        let source = "class Widget {}\nfunction build() { return new Widget(); }\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/widget.ts").unwrap();

        let construct = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Constructs)
            .unwrap();
        assert_eq!(construct.source_local_key, "src/widget.ts::build");
        assert_eq!(
            construct.target,
            EdgeTarget::Local("src/widget.ts::Widget".to_string())
        );
        assert_eq!(construct.confidence, Confidence::Exact);
    }

    /// An `interface_declaration` is extracted as a `SymbolKind::Interface`
    /// symbol, and a same-file `implements` clause now resolves to it
    /// (spec "Class extends and implements" — the in-project case).
    #[test]
    fn interface_is_extracted_and_implements_resolves_locally() {
        let source = "interface IFoo {}\nclass Foo implements IFoo {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/iface.ts").unwrap();

        let iface = analysis
            .symbols
            .iter()
            .find(|s| s.name == "IFoo")
            .expect("IFoo extracted as a symbol");
        assert_eq!(iface.kind, SymbolKind::Interface);

        let implements = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Implements)
            .unwrap();
        assert_eq!(
            implements.target,
            EdgeTarget::Local("src/iface.ts::IFoo".to_string())
        );
        assert_eq!(implements.confidence, Confidence::Exact);
    }

    /// A parameter/return type annotation naming a real declared type emits
    /// a `References` edge (spec "Relationship Kind Extraction" — `Foo` is
    /// one of the MUST-extracted kinds); a primitive type (`string`) does
    /// not.
    #[test]
    fn type_annotation_emits_references_edge() {
        let source =
            "class Widget {}\nfunction build(): Widget { return new Widget(); }\nfunction id(x: string): string { return x; }\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/refs.ts").unwrap();

        let references: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::References)
            .collect();
        assert_eq!(references.len(), 1, "only Widget is a real type reference");
        let reference = references[0];
        assert_eq!(reference.source_local_key, "src/refs.ts::build");
        assert_eq!(
            reference.target,
            EdgeTarget::Local("src/refs.ts::Widget".to_string())
        );
        assert_eq!(reference.confidence, Confidence::Exact);
    }

    /// design "symbol_key and signature_fingerprint": params/return-type
    /// text feeds the fingerprint (whitespace-normalized); a class has no
    /// call signature, so its fingerprint is empty.
    #[test]
    fn signature_fingerprint_reflects_params_and_return_type() {
        let source = "function id(x: string): string { return x; }\nclass Empty {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/sig.ts").unwrap();

        let func = analysis.symbols.iter().find(|s| s.name == "id").unwrap();
        assert_eq!(func.signature_fingerprint, "(x: string)\u{1f}: string");

        let class = analysis.symbols.iter().find(|s| s.name == "Empty").unwrap();
        assert_eq!(class.signature_fingerprint, "");
    }

    /// Extra internal whitespace within the parameter list (that doesn't
    /// shift what touches the surrounding parens/brackets) collapses to the
    /// same fingerprint — `split_whitespace().join(" ")` normalizes runs of
    /// whitespace, not token adjacency.
    #[test]
    fn signature_fingerprint_ignores_extra_internal_whitespace() {
        let a = analyze(
            "function f(x: string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        let b = analyze(
            "function f(x:    string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        assert_eq!(
            a.symbols[0].signature_fingerprint,
            b.symbols[0].signature_fingerprint
        );
    }

    /// A genuine signature change (parameter added) must change the
    /// fingerprint (spec "Rename changes identity" applies equally to
    /// signature edits feeding `symbol_key`).
    #[test]
    fn signature_fingerprint_changes_when_parameters_differ() {
        let a = analyze(
            "function f(x: string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        let b = analyze(
            "function f(x: string, y: number): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        assert_ne!(
            a.symbols[0].signature_fingerprint,
            b.symbols[0].signature_fingerprint
        );
    }
}
