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

/// A2: a file whose walk stops at the depth limit still indexes, and the
/// truncation is recorded so `index` can warn about it.
fn assert_reports_depth_truncation(source: String, language: LanguageId, path: &'static str) {
    let analysis = on_small_stack(move || extract::analyze(&source, language, path))
        .expect("deep source should still produce an analysis");
    assert!(
        extract::truncated_at_depth_limit(&analysis),
        "a walk stopped at the depth limit must be recorded for {path}"
    );
    assert!(
        analysis
            .symbols
            .iter()
            .any(|s| s.name == "shallow" || s.name == "Shallow"),
        "symbols above the depth cap must be kept"
    );
}

#[test]
fn typescript_deep_brackets_report_the_depth_truncation() {
    assert_reports_depth_truncation(ts_deep_brackets(), LanguageId::TypeScript, "deep.ts");
}

/// Nesting inside a method body, which the C# analyzer walks itself. A field
/// initializer is only walked by the .NET framework pass, which is not
/// covered by the depth record yet.
fn cs_deep_parens_in_method() -> String {
    format!(
        "namespace Deep;\n// Http\npublic class Shallow {{ public int M() {{ return {open}1{close}; }} }}\n",
        open = "(".repeat(NESTING),
        close = ")".repeat(NESTING),
    )
}

#[test]
fn csharp_deep_parens_in_method_report_the_depth_truncation() {
    assert_reports_depth_truncation(cs_deep_parens_in_method(), LanguageId::CSharp, "Deep.cs");
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
