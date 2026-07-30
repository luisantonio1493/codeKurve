//! Language identifiers. See CODEKURVE_MASTER_PLAN.md §6 (TypeScript/JavaScript
//! first for v0.1).

use serde::{Deserialize, Serialize};

/// A source language CodeKurve can analyze.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanguageId {
    TypeScript,
    JavaScript,
    CSharp,
}

impl LanguageId {
    /// Resolve a language from a file extension (without the leading dot).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "ts" | "tsx" | "mts" | "cts" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "cs" => Some(Self::CSharp),
            _ => None,
        }
    }

    /// Resolve a language from a config `languages` entry (§14).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "typescript" => Some(Self::TypeScript),
            "javascript" => Some(Self::JavaScript),
            "csharp" => Some(Self::CSharp),
            _ => None,
        }
    }

    /// Stable lowercase name used in config and storage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::CSharp => "csharp",
        }
    }
}
