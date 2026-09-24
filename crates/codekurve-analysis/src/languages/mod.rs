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

use codekurve_core::error::Result;
use codekurve_core::{
    Confidence, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind,
};

use tree_sitter::Tree;

use crate::extract::NO_SAME_FILE_MATCH_REASON;
use crate::ir::{
    EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis, TOO_DEEP_DIAGNOSTIC,
};

/// Deepest syntax tree the extractors will walk. They recurse once per tree
/// level, and a stack overflow aborts the process (it cannot be caught), so
/// a crafted file (10k nested `[` is ~20 KB) could kill `index`, `watch` or
/// the MCP server. Deeper files are skipped with a diagnostic instead.
///
/// Real code rarely passes a few hundred levels, but long operator chains
/// (`a + b + c + ...`, one level per term) in generated code can, so the
/// limit is generous and extraction runs on [`crate::extract::on_analysis_stack`],
/// which is sized for it. Measured on a 2 MiB stack: TypeScript overflowed
/// at ~500 levels in debug and ~2,000 in release (C#: ~1,500 / ~6,000).
pub const MAX_SYNTAX_DEPTH: usize = 10_000;

/// Whether `tree` is more than `limit` levels deep. Iterative (a cursor, no
/// recursion), so it is safe on any stack however deep the tree is.
pub(crate) fn syntax_depth_exceeds(tree: &Tree, limit: usize) -> bool {
    let mut cursor = tree.walk();
    let mut depth = 0usize;
    loop {
        if depth > limit {
            return true;
        }
        if cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return false;
            }
            depth -= 1;
        }
    }
}

/// The empty analysis an analyzer returns for a file past
/// [`MAX_SYNTAX_DEPTH`]: `Ok`, not `Err`, because an `Err` fails the whole
/// incremental batch and would fail it again on every later batch.
pub(crate) fn too_deep(language: LanguageId, relative_path: &str) -> FileAnalysis {
    FileAnalysis {
        file: relative_path.to_string(),
        language,
        symbols: Vec::new(),
        relationships: Vec::new(),
        unresolved: Vec::new(),
        diagnostics: vec![format!("{TOO_DEEP_DIAGNOSTIC} {MAX_SYNTAX_DEPTH} levels")],
    }
}

/// What every language analyzer must provide to plug into `extract::analyze`
/// dispatch and `resolve.rs`'s kind-compatibility check. Three methods,
/// fixed — a fourth needs a real TS or C# need, not speculative width.
pub trait LanguageAnalyzer {
    fn language(&self) -> LanguageId;
    fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis>;
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
    for rel in pending {
        let matches: Vec<&ExtractedSymbol> = symbols
            .iter()
            .filter(|s| s.name == rel.target_name && kind_matches(analyzer, rel.kind, s.kind))
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

    fn reference_depth(node: tree_sitter::Node) -> usize {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .map(|child| 1 + reference_depth(child))
            .max()
            .unwrap_or(0)
    }

    /// `syntax_depth_exceeds(tree, limit)` is true exactly when the tree is
    /// deeper than `limit`, matching a straightforward recursive depth on
    /// trees small enough for recursion to be safe.
    #[test]
    fn syntax_depth_exceeds_matches_recursive_depth_at_the_boundary() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        for source in [
            "",
            "let a = 1;",
            "function f() { if (a) { return [[1, [2]], g(h(3))]; } }",
            &format!("const x = {}{};", "[".repeat(60), "]".repeat(60)),
        ] {
            let tree = parser.parse(source, None).unwrap();
            let depth = reference_depth(tree.root_node());
            assert!(!syntax_depth_exceeds(&tree, depth), "{source:?} at {depth}");
            if depth > 0 {
                assert!(
                    syntax_depth_exceeds(&tree, depth - 1),
                    "{source:?} at {depth}"
                );
            }
        }
    }
}
