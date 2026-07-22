//! Symbol extraction via Tree-sitter. See CODEKURVE_MASTER_PLAN.md §Fase 1:
//! extract class and top-level function symbols from a source file.

use codekurve_core::error::{Error, Result};
use codekurve_core::{LanguageId, SourceSpan, Symbol, SymbolKind};
use tree_sitter::{Node, Parser};

/// Parse `source` for the given language and extract class and top-level
/// function symbols.
pub fn extract_symbols(source: &str, language: LanguageId) -> Result<Vec<Symbol>> {
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
    collect(tree.root_node(), source.as_bytes(), language, &mut symbols);
    Ok(symbols)
}

fn collect(node: Node, source: &[u8], language: LanguageId, out: &mut Vec<Symbol>) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            push_named(node, source, language, SymbolKind::Class, out);
        }
        "function_declaration" | "generator_function_declaration" if is_top_level(node) => {
            push_named(node, source, language, SymbolKind::Function, out);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, source, language, out);
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

fn push_named(
    node: Node,
    source: &[u8],
    language: LanguageId,
    kind: SymbolKind,
    out: &mut Vec<Symbol>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(source) else {
        return;
    };
    out.push(Symbol {
        name: name.to_string(),
        kind,
        language,
        span: span_of(node),
    });
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
    fn extracts_classes_and_top_level_functions() {
        let symbols = extract_symbols(SOURCE, LanguageId::TypeScript).unwrap();
        let names: Vec<(&str, SymbolKind)> =
            symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();

        assert!(names.contains(&("MemberService", SymbolKind::Class)));
        assert!(names.contains(&("topLevel", SymbolKind::Function)));
        assert!(names.contains(&("alsoTop", SymbolKind::Function)));
        // Nested function and class method are not top-level symbols.
        assert!(!names.iter().any(|(n, _)| *n == "nested"));
        assert!(!names.iter().any(|(n, _)| *n == "find"));
    }

    #[test]
    fn span_points_at_declaration() {
        let symbols = extract_symbols(SOURCE, LanguageId::TypeScript).unwrap();
        let svc = symbols.iter().find(|s| s.name == "MemberService").unwrap();
        assert_eq!(svc.span.start_line, 2);
        assert!(svc.span.end_byte > svc.span.start_byte);
    }
}
