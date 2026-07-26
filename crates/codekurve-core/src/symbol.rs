//! Domain types for extracted code symbols and their relationships. See
//! CODEKURVE_MASTER_PLAN.md §11.2, §17.2-§17.5 (`Symbol`, `SymbolKind`,
//! `RelationshipKind`, `Provenance`, `Confidence`, `SourceSpan`).

use serde::{Deserialize, Serialize};

use crate::language::LanguageId;

/// The kind of a symbol (plan §17.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Module,
    Namespace,
    Class,
    Interface,
    Struct,
    Enum,
    Function,
    Method,
    Constructor,
    Property,
    Field,
    Variable,
    Parameter,
    TypeAlias,
    Import,
    Export,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Namespace => "namespace",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Function => "function",
            Self::Method => "method",
            Self::Constructor => "constructor",
            Self::Property => "property",
            Self::Field => "field",
            Self::Variable => "variable",
            Self::Parameter => "parameter",
            Self::TypeAlias => "typealias",
            Self::Import => "import",
            Self::Export => "export",
        }
    }
}

/// The kind of a relationship edge between two symbols (plan §17.3). The
/// framework-role tags in §17.2 (Controller, Route, ...) and the "Futuro"
/// edge kinds in §17.3 (Injects, Triggers, ...) are out of scope for this
/// phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipKind {
    Defines,
    Contains,
    Imports,
    Exports,
    References,
    Calls,
    Constructs,
    Inherits,
    Implements,
    Overrides,
    UsesType,
    Reads,
    Writes,
}

impl RelationshipKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Defines => "defines",
            Self::Contains => "contains",
            Self::Imports => "imports",
            Self::Exports => "exports",
            Self::References => "references",
            Self::Calls => "calls",
            Self::Constructs => "constructs",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::Overrides => "overrides",
            Self::UsesType => "usestype",
            Self::Reads => "reads",
            Self::Writes => "writes",
        }
    }
}

/// How a relationship edge was determined (plan §17.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Extracted,
    Resolved,
    Heuristic,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "extracted",
            Self::Resolved => "resolved",
            Self::Heuristic => "heuristic",
        }
    }
}

/// How certain a resolved edge (or symbol) is (plan §17.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    High,
    Medium,
    Low,
    Unresolved,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Unresolved => "unresolved",
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
    /// Fully-qualified name computed per §20.3 (`relative_path::Name`,
    /// `relative_path::Class.method`). Never falls back to `name`.
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub language: LanguageId,
    pub span: SourceSpan,
    /// Enclosing symbol name (e.g. the class a method belongs to), if any.
    pub parent: Option<String>,
    /// See `ExtractedSymbol::signature_fingerprint` (codekurve-analysis
    /// ir.rs) — carried through unchanged to feed `symbol_key`'s 5th
    /// tuple element.
    pub signature_fingerprint: String,
}
