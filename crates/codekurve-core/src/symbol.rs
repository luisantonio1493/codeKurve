//! Domain types for extracted code symbols. See CODEKURVE_MASTER_PLAN.md §11.2
//! (`Symbol`, `SymbolKind`, `SourceSpan`). Phase 1 extracts classes and
//! top-level functions only (§Fase 1 scope).

use serde::{Deserialize, Serialize};

use crate::language::LanguageId;

/// The kind of a symbol. Extended in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Class,
    Function,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Function => "function",
        }
    }
}

/// A location in a source file. Byte offsets address the file for snippet
/// extraction; line/column are for human display. Lines are 1-based, columns
/// are 0-based byte offsets within the line (as reported by the parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// An extracted symbol, independent of its storage identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub language: LanguageId,
    pub span: SourceSpan,
}
