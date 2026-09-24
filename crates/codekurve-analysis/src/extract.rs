//! Dispatcher over per-language analyzers, plus the handful of helpers that
//! are genuinely tree-shape-agnostic (design "Module Layout", "Helper
//! split"). Contains no C#-specific and no TypeScript-specific node-kind
//! string — everything that names a node kind lives in `languages/`.

use codekurve_core::error::Result;
use codekurve_core::LanguageId;
use tree_sitter::Node;

use codekurve_core::SourceSpan;

use crate::ir::FileAnalysis;
use crate::languages::analyzer_for;

/// Parse `source` for the given language and extract its symbols plus
/// intra-file relationships into a per-file `FileAnalysis`. Keeps its public
/// signature; every existing caller (`commands.rs`, `incremental.rs`) is
/// untouched by the seam refactor (design "Technical Approach").
///
/// D3: `frameworks::recognize` runs here, immediately after the per-language
/// analyzer returns — this is the single entry point every caller already
/// routes through, and the only place holding both the source text and the
/// finished per-file symbol list.
///
/// A file deeper than [`crate::languages::MAX_SYNTAX_DEPTH`] comes back
/// empty with a diagnostic ([`FileAnalysis::skipped_too_deep`]) and skips
/// framework recognition, which would re-parse and walk it recursively.
/// Below that limit extraction still recurses per tree level: run it on
/// [`on_analysis_stack`].
pub fn analyze(source: &str, language: LanguageId, relative_path: &str) -> Result<FileAnalysis> {
    let mut analysis = analyzer_for(language).analyze(source, relative_path)?;
    if !analysis.skipped_too_deep() {
        crate::frameworks::recognize(source, language, &mut analysis);
    }
    Ok(analysis)
}

/// Stack reserved for [`on_analysis_stack`]. A reservation, not an
/// allocation: pages are only committed as recursion actually touches them.
/// Sized so a tree right at [`crate::languages::MAX_SYNTAX_DEPTH`] fits with
/// margin even in a debug build (~4 KiB per level measured for TypeScript).
pub const ANALYSIS_STACK_BYTES: usize = 256 << 20;

/// Runs `work` (a batch of [`analyze`] calls and whatever follows them) on a
/// dedicated thread with an [`ANALYSIS_STACK_BYTES`] stack, and returns its
/// result. Callers' own stacks vary: 8 MiB on the main thread, 2 MiB on
/// tokio's blocking pool where the MCP server reindexes, which a few hundred
/// nested levels already overflowed. One thread per batch, not per file.
/// A panic in `work` is re-raised on the caller's thread.
pub fn on_analysis_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("codekurve-analysis".to_string())
            .stack_size(ANALYSIS_STACK_BYTES)
            .spawn_scoped(scope, work)
            .expect("spawn the analysis thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

/// Reason text for a deferred (`PendingRel`) target with zero same-file
/// candidates. Shared with `resolve.rs` so cross-file resolution can tell a
/// same-file-miss `Exports` edge apart from a module-specifier re-export
/// without re-parsing.
pub(crate) const NO_SAME_FILE_MATCH_REASON: &str = "no matching same-file symbol";

// ponytail: not `Iterator::find` (clippy's own suggestion) — tree-sitter's
// cursor-borrowed `Node` items don't outlive the function that way here.
#[allow(clippy::manual_find)]
pub(crate) fn find_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

pub(crate) fn span_of(node: Node) -> SourceSpan {
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

/// design "symbol_key and signature_fingerprint": whitespace-normalized
/// declaration text of the given `fields`, `\x1f`-joined. Empty for
/// declarations without any of those fields (e.g. class/interface). The
/// field *names* differ per language (C# uses `type`, not `return_type`);
/// this normalize-and-join logic does not — generalized from the old
/// TS-only `signature_fingerprint` so a second language can pass its own
/// field list (design "Helper split").
pub(crate) fn fingerprint_fields(node: Node, source: &[u8], fields: &[&str]) -> String {
    fields
        .iter()
        .filter_map(|f| node.child_by_field_name(f)?.utf8_text(source).ok())
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}
