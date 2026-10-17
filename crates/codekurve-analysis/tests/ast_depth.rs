//! SEC-7: pathologically nested sources must index without overflowing the
//! stack. Each case runs on a thread with an explicit 2 MB stack (the default
//! test-thread size) so a recursion overflow aborts the test binary. Past
//! `MAX_SYNTAX_DEPTH` the file is skipped with a diagnostic instead of walked.

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

/// A file past the depth limit comes back empty with the skip recorded, and
/// nothing overflows the stack on the way there.
fn assert_skipped_without_overflow(source: String, language: LanguageId, path: &'static str) {
    let analysis = on_small_stack(move || extract::analyze(&source, language, path))
        .expect("deep source should still produce an analysis");
    assert!(
        analysis.skipped_too_deep(),
        "a file past the depth limit must be skipped for {path}"
    );
    assert!(
        analysis.symbols.is_empty(),
        "a skipped file indexes no symbols"
    );
}

#[test]
fn typescript_deep_brackets_do_not_overflow_the_stack() {
    assert_skipped_without_overflow(ts_deep_brackets(), LanguageId::TypeScript, "deep.ts");
}

#[test]
fn typescript_deep_binary_chain_does_not_overflow_the_stack() {
    assert_skipped_without_overflow(ts_deep_binary_chain(), LanguageId::TypeScript, "chain.ts");
}

#[test]
fn csharp_deep_parens_do_not_overflow_the_stack() {
    assert_skipped_without_overflow(cs_deep_parens(), LanguageId::CSharp, "Deep.cs");
}

#[test]
fn csharp_deep_binary_chain_does_not_overflow_the_stack() {
    assert_skipped_without_overflow(cs_deep_binary_chain(), LanguageId::CSharp, "Chain.cs");
}
