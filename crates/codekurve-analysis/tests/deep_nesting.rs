//! A pathologically nested file must never abort the process. The
//! extractors recurse once per syntax-tree level, and a stack overflow is not
//! catchable: before the depth guard, ~20 KB of nested `[` killed
//! `codekurve index` on the main thread, and far less did on the 2 MiB
//! threads tokio's blocking pool gives the MCP server.

use codekurve_analysis::extract::{self, on_analysis_stack};
use codekurve_analysis::languages::MAX_SYNTAX_DEPTH;
use codekurve_core::LanguageId;

/// The stack tokio's blocking pool (and Rust's default spawned thread) uses.
const SMALL_STACK: usize = 2 << 20;

fn nested_ts(depth: usize) -> String {
    format!(
        "export const x = {}{};\nexport function afterIt() {{}}\n",
        "[".repeat(depth),
        "]".repeat(depth)
    )
}

fn nested_cs(depth: usize) -> String {
    format!(
        "class C {{ int M() {{ return {}1{}; }} }}\n",
        "(".repeat(depth),
        ")".repeat(depth)
    )
}

fn on_small_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(work)
        .unwrap()
        .join()
        .unwrap()
}

/// Past the limit the guard skips the file before any recursion, so this
/// holds on a small stack even without `on_analysis_stack`.
#[test]
fn file_past_the_limit_is_skipped_not_fatal_even_on_a_small_stack() {
    for (language, source) in [
        (LanguageId::TypeScript, nested_ts(100_000)),
        (LanguageId::CSharp, nested_cs(100_000)),
    ] {
        let analysis = on_small_stack(move || extract::analyze(&source, language, "deep").unwrap());
        assert!(analysis.skipped_too_deep(), "{language:?}");
        assert!(analysis.symbols.is_empty(), "{language:?}");
        assert!(analysis.relationships.is_empty(), "{language:?}");
    }
}

/// Below the limit the file is analyzed normally, recursion and all, which
/// needs the analysis stack: this depth overflowed 2 MiB by ~20x in a debug
/// build. Proves `ANALYSIS_STACK_BYTES` covers the limit in the worst
/// (debug) case.
#[test]
fn file_just_under_the_limit_is_analyzed_on_the_analysis_stack() {
    let depth = MAX_SYNTAX_DEPTH - 1_000;
    for (language, source) in [
        (LanguageId::TypeScript, nested_ts(depth)),
        (LanguageId::CSharp, nested_cs(depth)),
    ] {
        let analysis =
            on_analysis_stack(|| extract::analyze(&source, language, "deep_ok").unwrap());
        assert!(!analysis.skipped_too_deep(), "{language:?}");
        assert!(!analysis.symbols.is_empty(), "{language:?}");
    }
}

#[test]
fn ordinary_file_is_not_flagged() {
    let analysis = extract::analyze(
        "export function a() { return [[1], [2]]; }\n",
        LanguageId::TypeScript,
        "ok.ts",
    )
    .unwrap();
    assert!(!analysis.skipped_too_deep());
    assert!(analysis.diagnostics.is_empty());
}

/// A panic inside the work still reaches the caller.
#[test]
#[should_panic(expected = "boom")]
fn on_analysis_stack_propagates_panics() {
    on_analysis_stack(|| panic!("boom"));
}
