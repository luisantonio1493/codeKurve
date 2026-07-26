//! Per-file intermediate representation produced by `extract::analyze`,
//! consumed by whole-project resolution before it is persisted (design
//! §Interfaces, plan §18, §22).

use codekurve_core::{
    Confidence, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind,
};

/// One file's extracted symbols/relationships, pre-resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct FileAnalysis {
    pub file: String,
    pub symbols: Vec<ExtractedSymbol>,
    pub relationships: Vec<ExtractedRelationship>,
    pub unresolved: Vec<UnresolvedReference>,
    pub diagnostics: Vec<String>,
}

/// A symbol extracted from one file, keyed locally until pass 2 assigns it a
/// stable storage identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedSymbol {
    pub local_key: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub language: LanguageId,
    pub span: SourceSpan,
    pub parent: Option<String>,
    pub is_exported: bool,
    /// Whitespace-normalized `type_parameters`/`parameters`/`return_type`
    /// declaration text, `\x1f`-joined; empty for kinds with no call
    /// signature (class/interface). Feeds `symbol_key`'s 5th tuple element
    /// (design "symbol_key and signature_fingerprint").
    pub signature_fingerprint: String,
}

/// A relationship edge extracted from one file, before its target is
/// resolved against the whole-project symbol table.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedRelationship {
    pub source_local_key: String,
    pub target: EdgeTarget,
    pub kind: RelationshipKind,
    pub span: SourceSpan,
    pub provenance: Provenance,
    pub confidence: Confidence,
    pub reason: Option<String>,
}

/// Where a relationship edge points, before/after resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeTarget {
    /// Another symbol in the same file, by its local key.
    Local(String),
    /// A symbol in another file, resolved by relative path + qualified name.
    Global {
        file: String,
        qualified_name: String,
    },
    /// A package outside the project (e.g. `node_modules`), never indexed.
    External(String),
    /// Could not be resolved to any of the above; carries the raw text.
    Unresolved(String),
}

/// A reference/import with zero resolution candidates or insufficient
/// context (spec "Unresolved Reference Handling", §18.3). Never dropped
/// silently.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedReference {
    pub source_local_key: String,
    pub relationship_kind: RelationshipKind,
    pub target_text: String,
    pub context: Option<String>,
    pub candidate_count: usize,
    pub reason: String,
    pub confidence: Confidence,
}
