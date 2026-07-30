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

use crate::extract::NO_SAME_FILE_MATCH_REASON;
use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};

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

/// Reason text for a C# base-list entry (design "Architecture Decisions" —
/// base-list edges are always emitted `Unresolved` with this reason, never
/// routed through `resolve_pending`; PR5's `resolve.rs` reclassifies them to
/// `Inherits`/`Implements` from the resolved candidate's own `SymbolKind`).
pub(crate) const BASE_LIST_REASON: &str = "c# base list entry";

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
            .filter(|s| s.name == rel.target_name && analyzer.kind_matches(rel.kind, s.kind))
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
}
