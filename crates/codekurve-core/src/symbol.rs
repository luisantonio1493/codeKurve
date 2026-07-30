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
    /// An attribute (C#) or, later, a TS decorator applied to a declaration
    /// (design "Attributes"). Target text = the attribute's original name;
    /// span = the attribute's own span, not its enclosing list.
    Decorates,
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
            Self::Decorates => "decorates",
        }
    }
}

/// Language-neutral symbol visibility (design "Visibility"), independent of
/// `is_exported` — `is_exported` means only "declared with the TypeScript
/// `export` keyword" and is unrelated to any language's access modifiers.
/// `Default` means "no modifier written", never a guessed implicit default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Protected,
    Internal,
    Private,
    ProtectedInternal,
    PrivateProtected,
    Default,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Protected => "protected",
            Self::Internal => "internal",
            Self::Private => "private",
            Self::ProtectedInternal => "protectedinternal",
            Self::PrivateProtected => "privateprotected",
            Self::Default => "default",
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
    /// Language-neutral access modifier (design "Visibility"). `Default` for
    /// every TypeScript/JavaScript symbol (no such modifier concept there).
    pub visibility: Visibility,
    /// C# `partial` declaration (design "Partial identity"). Always `false`
    /// for non-C# symbols.
    pub is_partial: bool,
    /// C# `record`/`record struct` (design "Records": no new `SymbolKind`,
    /// `record` folds into `Class`/`Struct` + this flag). Always `false` for
    /// non-C# symbols.
    pub is_record: bool,
    /// Disambiguates multiple `partial` fragments of the same type in one
    /// file for `symbol_key` (design "symbol_key"). `None` for every
    /// non-partial declaration, keeping the hashed input byte-identical to
    /// the pre-Phase-5 five-component tuple.
    pub partial_ordinal: Option<u32>,
}
