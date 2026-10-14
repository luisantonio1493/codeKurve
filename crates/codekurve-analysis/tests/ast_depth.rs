//! SEC-7: pathologically nested sources must index without overflowing the
//! stack. Each case runs on a thread with an explicit 2 MB stack (the default
//! test-thread size) so a recursion overflow aborts the test binary.

use codekurve_analysis::extract;
use codekurve_core::LanguageId;

const TEST_STACK_BYTES: usize = 2 * 1024 * 1024;
const NESTING: usize = 200_000;

fn on_small_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(TEST_STACK_BYTES)
        .spawn(f)
        .expect("spawn test thread")
        .join()
        .expect("analysis thread must not panic or overflow the stack")
}

fn ts_deep_brackets() -> String {
    // The `// @Component` comment is a framework marker, so the Angular
    // re-parse and walk also run over the deep tree.
    format!(
        "// @Component\nexport function shallow() {{ return 1; }}\nconst v = {open}1{close};\n",
        open = "[".repeat(NESTING),
        close = "]".repeat(NESTING),
    )
}

fn ts_deep_binary_chain() -> String {
    let chain = vec!["a"; NESTING].join(" + ");
    format!("export function shallow() {{ return 1; }}\nconst v = {chain};\n")
}

fn cs_deep_parens() -> String {
    format!(
        "namespace Deep;\n// Http\npublic class Shallow {{ public int M() {{ return 1; }} }}\n\
         public class Holder {{ public int X = {open}1{close}; }}\n",
        open = "(".repeat(NESTING),
        close = ")".repeat(NESTING),
    )
}

fn cs_deep_binary_chain() -> String {
    let chain = vec!["a"; NESTING].join(" + ");
    format!(
        "namespace Deep;\npublic class Shallow {{ public int M() {{ return 1; }} }}\n\
         public class Holder {{ public int X(int a) {{ return {chain}; }} }}\n"
    )
}

fn assert_indexes_shallow_symbol(source: String, language: LanguageId, path: &'static str) {
    let analysis = on_small_stack(move || extract::analyze(&source, language, path))
        .expect("deep source should still produce an analysis");
    assert!(
        analysis
            .symbols
            .iter()
            .any(|s| s.name == "shallow" || s.name == "Shallow"),
        "symbols above the depth cap must be kept, got {:?}",
        analysis.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

#[test]
fn typescript_deep_brackets_do_not_overflow_the_stack() {
    assert_indexes_shallow_symbol(ts_deep_brackets(), LanguageId::TypeScript, "deep.ts");
}

#[test]
fn typescript_deep_binary_chain_does_not_overflow_the_stack() {
    assert_indexes_shallow_symbol(ts_deep_binary_chain(), LanguageId::TypeScript, "chain.ts");
}

#[test]
fn csharp_deep_parens_do_not_overflow_the_stack() {
    assert_indexes_shallow_symbol(cs_deep_parens(), LanguageId::CSharp, "Deep.cs");
}

#[test]
fn csharp_deep_binary_chain_does_not_overflow_the_stack() {
    assert_indexes_shallow_symbol(cs_deep_binary_chain(), LanguageId::CSharp, "Chain.cs");
}
