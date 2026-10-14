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
    #[serde(default)]
    pub mcp: Mcp,
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
    /// Additive field (Phase 6): older configs that name `[index]` but
    /// predate this key still parse, via this field-level default rather
    /// than requiring the whole section to be absent.
    #[serde(default = "default_max_total_files")]
    pub max_total_files: usize,
    pub follow_symlinks: bool,
    pub include_hidden: bool,
    pub store_source: bool,
    #[serde(default)]
    pub watch: IndexWatch,
}

/// `codekurve watch` debounce/fallback tuning (Phase 3 design "Debounce" +
/// "Oversized Batch Falls Back to Full Reindex").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexWatch {
    /// Sliding quiet-window: flush once no new event arrives for this long.
    pub debounce_ms: u64,
    /// Hard cap: flush even under continuous events after this long since
    /// the batch's first pending event.
    pub max_batch_wait_ms: u64,
    /// A batch touching more than this percent of tracked files falls back
    /// to a full `reindex` instead of the incremental path.
    pub full_reindex_threshold_pct: u32,
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

/// `[mcp]` config (design "Config"): additive, same `#[serde(default)]`
/// pattern as `[index.watch]` — older config files without this section
/// parse unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mcp {
    #[serde(default)]
    pub allow_reindex: bool,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}

fn default_max_total_files() -> usize {
    50_000
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
            mcp: Mcp::default(),
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
            languages: vec![
                "typescript".to_string(),
                "javascript".to_string(),
                "csharp".to_string(),
            ],
            max_file_size_bytes: 2_097_152,
            max_total_files: default_max_total_files(),
            follow_symlinks: false,
            include_hidden: false,
            store_source: false,
            watch: IndexWatch::default(),
        }
    }
}

impl Default for IndexWatch {
    fn default() -> Self {
        Self {
            debounce_ms: 750,
            max_batch_wait_ms: 5000,
            full_reindex_threshold_pct: 25,
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
    /// The index database path under `root`. A repository ships its own
    /// `.codekurve/config.toml`, so `storage.database` is untrusted: it must
    /// be a relative path without `..` that stays inside the project.
    pub fn database_path(
        &self,
        root: &std::path::Path,
    ) -> std::result::Result<std::path::PathBuf, String> {
        use std::path::Component;

        let rel = std::path::Path::new(&self.storage.database);
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!(
                "storage.database {:?} must be a relative path inside the project root (no absolute path, no `..`)",
                self.storage.database
            ));
        }
        Ok(root.join(rel))
    }

    /// Settings that widen what a server or indexer will touch. A cloned
    /// repository can enable them from its own config, so entry points warn
    /// when any is on.
    pub fn elevated_settings(&self) -> Vec<&'static str> {
        let mut on = Vec::new();
        if self.mcp.allow_reindex {
            on.push("mcp.allow_reindex");
        }
        if self.index.follow_symlinks {
            on.push("index.follow_symlinks");
        }
        on
    }

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
        assert_eq!(parsed.index.max_total_files, 50_000);
    }

    /// Spec scenario "Existing config files parse unchanged": a config file
    /// that omits `[index]` entirely still parses with the built-in
    /// `max_total_files` default, via `Config.index`'s section-level
    /// `#[serde(default)]`.
    #[test]
    fn config_omitting_index_section_uses_default_max_total_files() {
        let parsed = Config::from_toml("version = 1\n[project]\nname = \"x\"\nroot = \".\"\n")
            .expect("parse");
        assert_eq!(parsed.index.max_total_files, 50_000);
    }

    /// `max_total_files` is additive (Phase 6): a config file that *names*
    /// `[index]` — because it predates this field, or only overrides other
    /// `Index` fields — but omits `max_total_files` still parses, via the
    /// field's own `#[serde(default = "default_max_total_files")]`, not
    /// just the whole-section default above.
    #[test]
    fn index_section_omitting_max_total_files_uses_its_own_default() {
        let parsed = Config::from_toml(
            "version = 1\n[project]\nname = \"x\"\nroot = \".\"\n\
             [index]\nlanguages = [\"typescript\"]\nmax_file_size_bytes = 1024\n\
             follow_symlinks = true\ninclude_hidden = true\nstore_source = true\n",
        )
        .expect("parse");
        assert_eq!(parsed.index.max_total_files, 50_000);
        assert_eq!(parsed.index.languages, vec!["typescript".to_string()]);
        assert!(parsed.index.follow_symlinks);
    }

    #[test]
    fn database_path_stays_inside_the_project_root() {
        let root = std::path::Path::new("/proj");
        let mut cfg = Config::default();
        assert_eq!(
            cfg.database_path(root).unwrap(),
            root.join(".codekurve/index.db")
        );
        for bad in ["/etc/index.db", "../index.db", "a/../../index.db"] {
            cfg.storage.database = bad.to_string();
            let err = cfg.database_path(root).unwrap_err();
            assert!(err.contains("storage.database"), "{bad}: {err}");
        }
    }

    #[test]
    fn elevated_settings_lists_only_what_is_switched_on() {
        let mut cfg = Config::default();
        assert!(cfg.elevated_settings().is_empty());
        cfg.mcp.allow_reindex = true;
        cfg.index.follow_symlinks = true;
        assert_eq!(
            cfg.elevated_settings(),
            vec!["mcp.allow_reindex", "index.follow_symlinks"]
        );
    }
}
