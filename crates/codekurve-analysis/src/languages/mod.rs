//! Language-analyzer seam (design "Module Layout", "Interfaces"): the trait
//! and shared machinery a per-language analyzer needs to plug into
//! `extract::analyze`'s dispatch and `resolve.rs`'s `kind_matches` check,
//! without dictating anything a third language wouldn't also need (Phase 5
//! proposal "Key Decisions" — trait width).
//!
//! `PendingRel`/`resolve_pending`/`push_unresolved_edge` live here rather
//! than in `extract.rs` because `resolve_pending` calls `kind_matches`
//! through `&dyn LanguageAnalyzer` (design "Helper split").

pub mod csharp;
pub mod typescript;

/// Deepest AST level the recursive walkers descend into (SEC-7). Tree-sitter
/// happily builds trees thousands of levels deep (`[[[[...`, `a+a+a+...`),
/// and each walker recursion costs stack; past this cap a walker stops
/// descending but keeps every symbol it already extracted, so the file still
/// indexes and nothing panics. Sized against the debug-build frame cost of the
/// TypeScript walker: 512 overflowed a 2 MB thread stack in the SEC-7 repro,
/// 256 leaves headroom while still covering long fluent call chains.
pub(crate) const MAX_AST_DEPTH: usize = 256;

thread_local! {
    // A2: set by a walker that stops at MAX_AST_DEPTH on this thread.
    // `extract::analyze` clears it before a file's walks and reads it after,
    // so each file reports only its own truncation.
    static DEPTH_LIMIT_HIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Records that a walker stopped at [`MAX_AST_DEPTH`] (A2). The walker keeps
/// what it already extracted and returns as before; this only lets the caller
/// report the file as truncated.
pub(crate) fn note_depth_limit() {
    DEPTH_LIMIT_HIT.with(|hit| hit.set(true));
}

/// Whether a walker stopped at the depth limit since the last call; clears the
/// record (A2).
pub(crate) fn take_depth_limit_hit() -> bool {
    DEPTH_LIMIT_HIT.with(|hit| hit.replace(false))
}

/// Parse budget for one file (SEC-7). A parse that exceeds it yields no tree,
/// which callers already treat as a failed parse.
const PARSE_TIMEOUT_MICROS: u64 = 5_000_000;

/// Fresh tree-sitter parser with the shared parse timeout applied. Every
/// parser in the analysis pipeline is created here so none can run unbounded.
/// `set_timeout_micros` is deprecated upstream in favor of a progress
/// callback; the simple budget is kept because it is the one-line equivalent.
#[allow(deprecated)]
pub(crate) fn new_parser() -> tree_sitter::Parser {
    let mut parser = tree_sitter::Parser::new();
    parser.set_timeout_micros(PARSE_TIMEOUT_MICROS);
    parser
}

// Test-only count of every tree-sitter parse the pipeline runs (PERF-4).
// Thread-local, so parallel test threads never see each other's parses.
#[cfg(test)]
thread_local! {
    pub(crate) static PARSE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    // PERF-4: one parser per grammar and thread, reused across files. A
    // rayon worker (PERF-1) keeps its own three parsers for its lifetime.
    static TS_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    static TSX_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
    static CS_PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Parses `source` once for the whole analysis pipeline (PERF-4). The tree
/// is handed to the language analyzer and then to the framework pass, so no
/// later stage re-parses the file. The TypeScript-family grammar is picked
/// per file (IDX-9): `.tsx` needs the TSX grammar, since the plain grammar
/// turns JSX into ERROR nodes; `.ts` keeps the plain grammar, since TSX reads
/// `<T,>(x: T)` as a JSX tag; JS always uses TSX, which covers JSX.
pub(crate) fn parse_tree(language: LanguageId, relative_path: &str, source: &str) -> Result<Tree> {
    #[cfg(test)]
    PARSE_CALLS.with(|c| c.set(c.get() + 1));

    let (slot, grammar): (
        &'static LocalKey<RefCell<Option<Parser>>>,
        tree_sitter::Language,
    ) = match language {
        LanguageId::CSharp => (&CS_PARSER, tree_sitter_c_sharp::LANGUAGE.into()),
        LanguageId::TypeScript if relative_path.ends_with(".tsx") => {
            (&TSX_PARSER, tree_sitter_typescript::LANGUAGE_TSX.into())
        }
        LanguageId::TypeScript => (
            &TS_PARSER,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        ),
        LanguageId::JavaScript => (&TSX_PARSER, tree_sitter_typescript::LANGUAGE_TSX.into()),
    };

    slot.with(|cell: &RefCell<Option<Parser>>| -> Result<Tree> {
        let mut parser = cell.borrow_mut();
        if parser.is_none() {
            let mut fresh = new_parser();
            fresh
                .set_language(&grammar)
                .map_err(|e| Error::Parse(e.to_string()))?;
            *parser = Some(fresh);
        }
        let active = parser
            .as_mut()
            .ok_or_else(|| Error::Parse("parser unavailable".to_string()))?;
        active
            .parse(source, None)
            .ok_or_else(|| Error::Parse("parser returned no tree".to_string()))
    })
}

use std::cell::RefCell;
use std::collections::HashMap;
use std::thread::LocalKey;

use tree_sitter::{Parser, Tree};

use codekurve_core::error::{Error, Result};
use codekurve_core::{
    Confidence, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind,
};

use crate::extract::NO_SAME_FILE_MATCH_REASON;
use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};

/// What every language analyzer must provide to plug into `extract::analyze`
/// dispatch and `resolve.rs`'s kind-compatibility check. Three methods,
/// fixed — a fourth needs a real TS or C# need, not speculative width.
pub trait LanguageAnalyzer {
    fn language(&self) -> LanguageId;
    /// `tree` is the file's parse, produced once by `parse_tree` (PERF-4).
    fn analyze(&self, source: &str, tree: &Tree, relative_path: &str) -> Result<FileAnalysis>;
    fn kind_matches(&self, rel: RelationshipKind, sym: SymbolKind) -> bool;
}

/// `&'static dyn` lookup over a fixed set of static instances — no registry,
/// no allocation (design "Module Layout").
pub fn analyzer_for(language: LanguageId) -> &'static dyn LanguageAnalyzer {
    match language {
        LanguageId::TypeScript => &typescript::TS,
        LanguageId::JavaScript => &typescript::JS,
        LanguageId::CSharp => &csharp::CS,
    }
}

/// TS↔JS still resolve together (existing behavior, unchanged by this PR);
/// C# is its own resolution domain, so cross-language name collisions never
/// produce an edge (design "Cross-language candidate filter").
pub fn same_resolution_domain(a: LanguageId, b: LanguageId) -> bool {
    use LanguageId::*;
    matches!(
        (a, b),
        (TypeScript | JavaScript, TypeScript | JavaScript) | (CSharp, CSharp)
    )
}

/// D6: framework-level kinds (`Injects`, `RegisteredAs`, `HandlesRoute`,
/// `Triggers`, `PersistsTo`) are answered by `frameworks::kind_matches`
/// *before* falling through to `analyzer`'s own table — every call site
/// that used to call `analyzer.kind_matches` directly now goes through this
/// wrapper instead, so a framework edge is never silently accepted or
/// rejected by a per-language table that has never heard of it. Neither
/// `TypeScriptAnalyzer::kind_matches` nor `CSharpAnalyzer::kind_matches`
/// changes — `frameworks::kind_matches` returns `None` for every grammar-
/// level kind those tables already own.
pub(crate) fn kind_matches(
    analyzer: &dyn LanguageAnalyzer,
    rel: RelationshipKind,
    sym: SymbolKind,
) -> bool {
    crate::frameworks::kind_matches(rel, sym).unwrap_or_else(|| analyzer.kind_matches(rel, sym))
}

/// Reason text for a C# base-list entry (design "Architecture Decisions" —
/// base-list edges are always emitted `Unresolved` with this reason, never
/// routed through `resolve_pending`; PR5's `resolve.rs` reclassifies them to
/// `Inherits`/`Implements` from the resolved candidate's own `SymbolKind`).
pub(crate) const BASE_LIST_REASON: &str = "c# base list entry";

/// Reason text for a C# property/field declared type (nav-property support —
/// same `Unresolved` + reason-tagged shape as `BASE_LIST_REASON`, resolved
/// through `resolve_base_entry` too since both name a `Class`/`Struct`/
/// `Interface` the declaring type references, never a call-site usage).
pub(crate) const PROPERTY_TYPE_REASON: &str = "c# property/field declared type";

/// A heritage/call/construct target discovered while walking, deferred until
/// the whole file's symbols are known (both may be forward references).
pub(crate) struct PendingRel {
    pub(crate) source_key: String,
    pub(crate) kind: RelationshipKind,
    pub(crate) target_name: String,
    pub(crate) span: SourceSpan,
}

#[cfg(test)]
thread_local! {
    /// Test-only: symbol candidates `resolve_pending` examined on this thread.
    pub(crate) static RESOLVE_CANDIDATE_CHECKS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Resolves every deferred heritage/call/construct/local-export target
/// against the file's full symbol list, now that forward references are
/// visible. A same-file name+kind match becomes `EdgeTarget::Local`; zero
/// matches become `EdgeTarget::Unresolved(text)` (never dropped — §18.3);
/// multiple matches emit one Low-confidence edge per candidate rather than
/// silently pick one (§20.4 principle). `kind_matches` dispatches through
/// `analyzer` rather than a shared free function, so a third language never
/// silently inherits TypeScript's rules.
pub(crate) fn resolve_pending(
    symbols: &[ExtractedSymbol],
    pending: Vec<PendingRel>,
    out: &mut Vec<ExtractedRelationship>,
    analyzer: &dyn LanguageAnalyzer,
) {
    // PERF-6: one name -> symbols map per file, built in symbol order so each
    // bucket keeps the candidate order the full scan produced.
    let mut by_name: HashMap<&str, Vec<&ExtractedSymbol>> = HashMap::new();
    for symbol in symbols {
        by_name
            .entry(symbol.name.as_str())
            .or_default()
            .push(symbol);
    }
    for rel in pending {
        let candidates: &[&ExtractedSymbol] = by_name
            .get(rel.target_name.as_str())
            .map_or(&[], Vec::as_slice);
        let matches: Vec<&ExtractedSymbol> = candidates
            .iter()
            .copied()
            .filter(|s| {
                #[cfg(test)]
                RESOLVE_CANDIDATE_CHECKS.with(|c| c.set(c.get() + 1));
                kind_matches(analyzer, rel.kind, s.kind)
            })
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

/// Pushes an `Extracted`/`Unresolved` relationship — the shared shape for
/// import/export edges whose target isn't a same-file symbol (module
/// specifier, or an anonymous default export placeholder).
pub(crate) fn push_unresolved_edge(
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

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_RELATIONSHIP_KINDS: [RelationshipKind; 14] = [
        RelationshipKind::Defines,
        RelationshipKind::Contains,
        RelationshipKind::Imports,
        RelationshipKind::Exports,
        RelationshipKind::References,
        RelationshipKind::Calls,
        RelationshipKind::Constructs,
        RelationshipKind::Inherits,
        RelationshipKind::Implements,
        RelationshipKind::Overrides,
        RelationshipKind::UsesType,
        RelationshipKind::Reads,
        RelationshipKind::Writes,
        RelationshipKind::Decorates,
    ];

    const ALL_SYMBOL_KINDS: [SymbolKind; 16] = [
        SymbolKind::Module,
        SymbolKind::Namespace,
        SymbolKind::Class,
        SymbolKind::Interface,
        SymbolKind::Struct,
        SymbolKind::Enum,
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Constructor,
        SymbolKind::Property,
        SymbolKind::Field,
        SymbolKind::Variable,
        SymbolKind::Parameter,
        SymbolKind::TypeAlias,
        SymbolKind::Import,
        SymbolKind::Export,
    ];

    /// The pre-refactor `extract::kind_matches` table, hardcoded here as the
    /// expectation matrix (design "Resolution Changes" — `TypeScriptAnalyzer
    /// ::kind_matches` must be byte-for-byte the same answers, task 2.9).
    fn pre_refactor_kind_matches(rel_kind: RelationshipKind, sym_kind: SymbolKind) -> bool {
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

    /// Task 2.9: exhaustive `(RelationshipKind, SymbolKind)` sweep — every
    /// combination answers identically to the pre-refactor table for both
    /// the TS and JS static instances.
    #[test]
    fn typescript_analyzer_kind_matches_matches_pre_refactor_table() {
        for analyzer in [
            analyzer_for(LanguageId::TypeScript),
            analyzer_for(LanguageId::JavaScript),
        ] {
            for &rel_kind in &ALL_RELATIONSHIP_KINDS {
                for &sym_kind in &ALL_SYMBOL_KINDS {
                    assert_eq!(
                        analyzer.kind_matches(rel_kind, sym_kind),
                        pre_refactor_kind_matches(rel_kind, sym_kind),
                        "mismatch for ({rel_kind:?}, {sym_kind:?})"
                    );
                }
            }
        }
    }

    /// Task 2.10: `same_resolution_domain` table test.
    #[test]
    fn same_resolution_domain_table() {
        use LanguageId::*;
        assert!(same_resolution_domain(TypeScript, JavaScript));
        assert!(same_resolution_domain(JavaScript, TypeScript));
        assert!(same_resolution_domain(TypeScript, TypeScript));
        assert!(same_resolution_domain(JavaScript, JavaScript));
        assert!(same_resolution_domain(CSharp, CSharp));
        assert!(!same_resolution_domain(CSharp, TypeScript));
        assert!(!same_resolution_domain(TypeScript, CSharp));
        assert!(!same_resolution_domain(CSharp, JavaScript));
    }

    fn fixture_symbol(key: &str, name: &str, kind: SymbolKind) -> ExtractedSymbol {
        ExtractedSymbol {
            local_key: key.to_string(),
            name: name.to_string(),
            qualified_name: key.to_string(),
            kind,
            language: LanguageId::TypeScript,
            span: SourceSpan {
                start_byte: 0,
                end_byte: 0,
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
            },
            parent: None,
            is_exported: false,
            signature_fingerprint: String::new(),
            visibility: codekurve_core::Visibility::Default,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
            roles: Vec::new(),
        }
    }

    fn fixture_pending(source: &str, kind: RelationshipKind, target: &str) -> PendingRel {
        PendingRel {
            source_key: source.to_string(),
            kind,
            target_name: target.to_string(),
            span: SourceSpan {
                start_byte: 0,
                end_byte: 0,
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
            },
        }
    }

    /// Several same-named symbols (mixed kinds), an unknown name and repeated
    /// names, shared by the PERF-6 guard and the candidate-count test.
    fn same_name_fixture() -> (Vec<ExtractedSymbol>, Vec<PendingRel>) {
        let symbols = vec![
            fixture_symbol("k1", "Helper", SymbolKind::Class),
            fixture_symbol("k2", "Helper", SymbolKind::Function),
            fixture_symbol("k3", "Helper", SymbolKind::Class),
            fixture_symbol("k4", "Make", SymbolKind::Function),
            fixture_symbol("k5", "Make", SymbolKind::Method),
            fixture_symbol("k6", "Other", SymbolKind::Class),
        ];
        let pending = vec![
            fixture_pending("src1", RelationshipKind::Inherits, "Helper"),
            fixture_pending("src2", RelationshipKind::Calls, "Make"),
            fixture_pending("src3", RelationshipKind::Constructs, "Helper"),
            fixture_pending("src4", RelationshipKind::Calls, "Missing"),
            fixture_pending("src5", RelationshipKind::Calls, "Other"),
            fixture_pending("src6", RelationshipKind::Inherits, "Helper"),
        ];
        (symbols, pending)
    }

    /// PERF-6 equivalence guard: several same-named symbols (mixed kinds),
    /// unknown names and repeated names, pinned on the pre-change scan.
    #[test]
    fn resolve_pending_same_name_fixture_output_is_pinned() {
        let (symbols, pending) = same_name_fixture();
        let mut out = Vec::new();
        resolve_pending(
            &symbols,
            pending,
            &mut out,
            analyzer_for(LanguageId::TypeScript),
        );
        let lines: Vec<String> = out
            .iter()
            .map(|r| {
                format!(
                    "{} {:?} {:?} {:?} {:?}",
                    r.source_local_key, r.target, r.kind, r.confidence, r.reason
                )
            })
            .collect();
        let expected: Vec<&str> = vec![
            r#"src1 Local("k1") Inherits Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src1 Local("k3") Inherits Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src2 Local("k4") Calls Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src2 Local("k5") Calls Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src3 Local("k1") Constructs Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src3 Local("k3") Constructs Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src4 Unresolved("Missing") Calls Unresolved Some("no matching same-file symbol")"#,
            r#"src5 Unresolved("Other") Calls Unresolved Some("no matching same-file symbol")"#,
            r#"src6 Local("k1") Inherits Low Some("ambiguous: multiple same-file candidates")"#,
            r#"src6 Local("k3") Inherits Low Some("ambiguous: multiple same-file candidates")"#,
        ];
        assert_eq!(lines, expected);
    }

    /// PERF-6 work guard: each pending target is compared only with symbols
    /// of its own name (Helper 3x3, Make 2, Missing 0, Other 1 = 12 checks).
    #[test]
    fn resolve_pending_checks_only_same_name_candidates() {
        let (symbols, pending) = same_name_fixture();
        RESOLVE_CANDIDATE_CHECKS.with(|c| c.set(0));
        let mut out = Vec::new();
        resolve_pending(
            &symbols,
            pending,
            &mut out,
            analyzer_for(LanguageId::TypeScript),
        );
        assert_eq!(RESOLVE_CANDIDATE_CHECKS.with(|c| c.get()), 12);
    }
}
