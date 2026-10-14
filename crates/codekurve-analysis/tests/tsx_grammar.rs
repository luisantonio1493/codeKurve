//! IDX-9: `.tsx` files must parse with the TSX grammar. The plain TypeScript
//! grammar turns JSX into ERROR nodes and loses the symbols around it.

use codekurve_analysis::extract;
use codekurve_core::{LanguageId, SymbolKind};

const TSX_SOURCE: &str = "export function Greeting() { return <div className=\"x\">hi</div>; }\n\
export const Other = () => <span/>;\n\
function helper() { return 1; }\n";

fn symbol_names(source: &str, path: &str) -> Vec<String> {
    extract::analyze(source, LanguageId::TypeScript, path)
        .unwrap()
        .symbols
        .into_iter()
        .map(|symbol| symbol.name)
        .collect()
}

/// Grammar-level evidence: the TSX grammar accepts the JSX source without
/// ERROR nodes, while the plain TypeScript grammar does not.
#[test]
fn tsx_grammar_parses_jsx_without_error_nodes() {
    let mut tsx = tree_sitter::Parser::new();
    tsx.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .unwrap();
    assert!(!tsx.parse(TSX_SOURCE, None).unwrap().root_node().has_error());

    let mut plain = tree_sitter::Parser::new();
    plain
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .unwrap();
    assert!(plain
        .parse(TSX_SOURCE, None)
        .unwrap()
        .root_node()
        .has_error());
}

#[test]
fn tsx_file_keeps_component_and_helper_symbols() {
    let names = symbol_names(TSX_SOURCE, "app.tsx");
    assert!(names.iter().any(|name| name == "Greeting"), "{names:?}");
    assert!(names.iter().any(|name| name == "helper"), "{names:?}");
}

#[test]
fn tsx_function_component_is_a_function_symbol() {
    let analysis = extract::analyze(TSX_SOURCE, LanguageId::TypeScript, "app.tsx").unwrap();
    let greeting = analysis
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Greeting")
        .expect("Greeting symbol");
    assert_eq!(greeting.kind, SymbolKind::Function);
}

/// Plain `.ts` keeps the TypeScript grammar: TSX would read `<T,>` as a JSX
/// tag and change how the file parses. Pins today's symbols for `.ts`; the
/// generic arrow `f` is not extracted by the analyzer before or after IDX-9.
#[test]
fn ts_generic_arrow_keeps_plain_typescript_grammar() {
    let source = "const f = <T,>(x: T) => x;\nexport function after() { return 1; }\n";
    assert_eq!(
        symbol_names(source, "generic.ts"),
        vec!["after".to_string()]
    );
}
