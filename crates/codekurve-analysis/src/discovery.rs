//! File discovery. Walks a project root, honoring ignore rules, and yields the
//! source files eligible for indexing. See CODEKURVE_MASTER_PLAN.md §15
//! (discovery) and §15.1 (file rules).

use std::path::{Path, PathBuf};

use codekurve_core::LanguageId;
use ignore::WalkBuilder;

/// A source file selected by discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    /// Absolute path on disk.
    pub absolute_path: PathBuf,
    /// Path relative to the project root, always using `/` separators (§15.1).
    pub relative_path: String,
    pub language: LanguageId,
}

/// Discovery inputs derived from config (§14).
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    pub respect_gitignore: bool,
    pub respect_global_gitignore: bool,
    pub include_hidden: bool,
    pub follow_symlinks: bool,
    pub max_file_size_bytes: u64,
    /// Languages to include. Empty means "all supported".
    pub languages: Vec<LanguageId>,
}

impl DiscoveryOptions {
    fn accepts(&self, language: LanguageId) -> bool {
        self.languages.is_empty() || self.languages.contains(&language)
    }
}

/// Walk `root` and return the eligible source files, sorted by relative path
/// for deterministic output. Unreadable entries are skipped rather than
/// aborting the walk.
pub fn discover(root: &Path, options: &DiscoveryOptions) -> Vec<DiscoveredFile> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!options.include_hidden)
        .git_ignore(options.respect_gitignore)
        .git_exclude(options.respect_gitignore)
        .git_global(options.respect_global_gitignore)
        .parents(options.respect_gitignore)
        // Honor `.gitignore` even when the root is not a git repository (§15.1).
        .require_git(false)
        .follow_links(options.follow_symlinks)
        .max_filesize(Some(options.max_file_size_bytes));

    let mut files = Vec::new();
    for entry in builder.build().flatten() {
        // ponytail: extension filter is enough for TS/JS in Phase 1; explicit
        // binary sniffing (§15.2) lands when non-text extensions matter.
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(language) = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(LanguageId::from_extension)
        else {
            continue;
        };
        if !options.accepts(language) {
            continue;
        }
        let Some(relative_path) = relative_slash_path(root, path) else {
            continue;
        };
        files.push(DiscoveredFile {
            absolute_path: path.to_path_buf(),
            relative_path,
            language,
        });
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    files
}

/// Build a `/`-separated path relative to `root`, or `None` if `path` is not
/// under `root`.
fn relative_slash_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn options() -> DiscoveryOptions {
        DiscoveryOptions {
            respect_gitignore: true,
            respect_global_gitignore: false,
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_bytes: 2_097_152,
            languages: vec![LanguageId::TypeScript, LanguageId::JavaScript],
        }
    }

    #[test]
    fn finds_sources_and_respects_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.ts"), "export class A {}").unwrap();
        fs::write(root.join("b.js"), "function b() {}").unwrap();
        fs::write(root.join("readme.md"), "# doc").unwrap();
        fs::write(root.join("ignored.ts"), "export class X {}").unwrap();
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("node_modules").join("dep.ts"), "").unwrap();
        fs::write(root.join(".gitignore"), "ignored.ts\nnode_modules/\n").unwrap();

        let discovered = discover(root, &options());
        let found: Vec<&str> = discovered
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();

        assert!(found.contains(&"a.ts"));
        assert!(found.contains(&"b.js"));
        assert!(!found.iter().any(|p| p.contains("readme")));
        assert!(!found.iter().any(|p| p.contains("ignored.ts")));
        assert!(!found.iter().any(|p| p.contains("node_modules")));
    }

    #[test]
    fn language_filter_excludes_unselected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.ts"), "").unwrap();
        fs::write(root.join("b.js"), "").unwrap();

        let mut opts = options();
        opts.languages = vec![LanguageId::TypeScript];
        let found = discover(root, &opts);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].relative_path, "a.ts");
        assert_eq!(found[0].language, LanguageId::TypeScript);
    }
}
