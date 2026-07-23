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

/// Resolves every deferred heritage/call/construct target against the file's
/// full symbol list, now that forward references are visible. A same-file
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
                reason: Some("no matching same-file symbol".to_string()),
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
/// not a same-named function).
fn kind_matches(rel_kind: RelationshipKind, sym_kind: SymbolKind) -> bool {
    match rel_kind {
        RelationshipKind::Constructs => sym_kind == SymbolKind::Class,
        RelationshipKind::Calls => matches!(
            sym_kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
        ),
        RelationshipKind::Inherits | RelationshipKind::Implements => {
            matches!(sym_kind, SymbolKind::Class | SymbolKind::Interface)
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
    });
    Some(name)
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
}
