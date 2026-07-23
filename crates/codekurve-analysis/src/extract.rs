//! Symbol extraction via Tree-sitter. See CODEKURVE_MASTER_PLAN.md §Fase 1:
//! extract class, top-level function, and class-method symbols from a source
//! file into the per-file IR (§18, §20.3).

use codekurve_core::error::{Error, Result};
use codekurve_core::{LanguageId, SourceSpan, SymbolKind};
use tree_sitter::{Node, Parser};

use crate::ir::{ExtractedSymbol, FileAnalysis};

/// Parse `source` for the given language and extract its symbols into a
/// per-file `FileAnalysis`. Relationship extraction lands in a later phase
/// slice; `relationships`/`unresolved` are always empty here.
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

    let mut symbols = Vec::new();
    collect(
        tree.root_node(),
        source.as_bytes(),
        language,
        relative_path,
        None,
        &mut symbols,
    );
    Ok(FileAnalysis {
        file: relative_path.to_string(),
        symbols,
        relationships: Vec::new(),
        unresolved: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn collect(
    node: Node,
    source: &[u8],
    language: LanguageId,
    relative_path: &str,
    parent: Option<&str>,
    out: &mut Vec<ExtractedSymbol>,
) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            let name = push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Class,
                parent,
                out,
            );
            // Descend with the class name as the new parent so members get a
            // `Class.member` qualified name; this replaces the generic
            // recursion below for this subtree.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, source, language, relative_path, name.as_deref(), out);
            }
            return;
        }
        "method_definition" if parent.is_some() => {
            let kind = method_kind(node, source);
            push_named(node, source, language, relative_path, kind, parent, out);
            // A method body can contain its own nested functions/classes;
            // recurse into it without the enclosing class as parent.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, source, language, relative_path, None, out);
            }
            return;
        }
        "function_declaration" | "generator_function_declaration" if is_top_level(node) => {
            push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Function,
                None,
                out,
            );
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, source, language, relative_path, parent, out);
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
    let qualified_name = match parent {
        Some(p) => format!("{relative_path}::{p}.{name}"),
        None => format!("{relative_path}::{name}"),
    };
    out.push(ExtractedSymbol {
        // ponytail: local_key = qualified_name for PR1 (single-file scope,
        // no cross-file collisions yet); revisit if pass 2 needs a distinct
        // pre-resolution key.
        local_key: qualified_name.clone(),
        name: name.clone(),
        qualified_name,
        kind,
        language,
        span: span_of(node),
        parent: parent.map(str::to_string),
        // ponytail: export detection deferred to relationship extraction
        // (Imports/Exports edges land in PR3).
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
}
