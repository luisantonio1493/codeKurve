//! Project configuration (`.codekurve/config.toml`). Schema mirrors
//! CODEKURVE_MASTER_PLAN.md §14; only the sections the Phase 1 vertical slice
//! needs are modeled here. Unknown sections are ignored so later phases can
//! extend the file without breaking older readers.

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// On-disk config schema version. Bumped when the format changes (§14).
pub const CONFIG_VERSION: u32 = 1;

/// Directory holding CodeKurve state, relative to the project root.
pub const CONFIG_DIR: &str = ".codekurve";

/// Config file name inside [`CONFIG_DIR`].
pub const CONFIG_FILE: &str = "config.toml";

/// Root project configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub project: Project,
    #[serde(default)]
    pub index: Index,
    #[serde(default)]
    pub ignore: Ignore,
    #[serde(default)]
    pub storage: Storage,
    #[serde(default)]
    pub queries: Queries,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Project root, relative to the config file's directory (§14).
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub languages: Vec<String>,
    pub max_file_size_bytes: u64,
    pub follow_symlinks: bool,
    pub include_hidden: bool,
    pub store_source: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ignore {
    pub respect_gitignore: bool,
    pub respect_global_gitignore: bool,
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Storage {
    pub database: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queries {
    pub default_limit: u32,
    pub max_limit: u32,
    pub max_snippet_bytes: u32,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            project: Project::default(),
            index: Index::default(),
            ignore: Ignore::default(),
            storage: Storage::default(),
            queries: Queries::default(),
        }
    }
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "my-project".to_string(),
            root: "..".to_string(),
        }
    }
}

impl Default for Index {
    fn default() -> Self {
        Self {
            languages: vec!["typescript".to_string(), "javascript".to_string()],
            max_file_size_bytes: 2_097_152,
            follow_symlinks: false,
            include_hidden: false,
            store_source: false,
        }
    }
}

impl Default for Ignore {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            respect_global_gitignore: true,
            patterns: [
                ".codekurve/**",
                "**/node_modules/**",
                "**/dist/**",
                "**/build/**",
                "**/.git/**",
                "**/*.min.js",
                "**/*.map",
                "**/.env",
                "**/.env.*",
                "**/secrets.*",
                "**/*.pfx",
                "**/*.pem",
                "**/*.key",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self {
            database: ".codekurve/index.db".to_string(),
        }
    }
}

impl Default for Queries {
    fn default() -> Self {
        Self {
            default_limit: 50,
            max_limit: 500,
            max_snippet_bytes: 12_000,
        }
    }
}

impl Config {
    /// Parse a config from TOML text.
    pub fn from_toml(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Serialize to pretty TOML text.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml().expect("serialize");
        let parsed = Config::from_toml(&text).expect("parse");
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn partial_config_fills_defaults() {
        let parsed = Config::from_toml("version = 1\n[project]\nname = \"x\"\nroot = \".\"\n")
            .expect("parse");
        assert_eq!(parsed.project.name, "x");
        // Unspecified sections fall back to defaults.
        assert_eq!(parsed.queries.default_limit, 50);
        assert!(parsed.ignore.respect_gitignore);
    }
}
