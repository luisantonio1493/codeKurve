//! File discovery. Walks a project root, honoring ignore rules, and yields the
//! source files eligible for indexing. See CODEKURVE_MASTER_PLAN.md §15
//! (discovery) and §15.1 (file rules).

use std::path::{Path, PathBuf};

use codekurve_core::LanguageId;
use ignore::overrides::OverrideBuilder;
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
    /// Hard cap on discovered file count; `0` disables the check (Phase 6,
    /// design "max_total_files enforcement point").
    pub max_total_files: usize,
    /// Languages to include. Empty means "all supported".
    pub languages: Vec<LanguageId>,
    /// Gitignore-style globs (`config.ignore.patterns`) excluded from the walk
    /// on top of the ignore-file rules. Empty applies no extra exclusions.
    pub ignore_patterns: Vec<String>,
}

impl DiscoveryOptions {
    fn accepts(&self, language: LanguageId) -> bool {
        self.languages.is_empty() || self.languages.contains(&language)
    }
}

/// Walk `root` and return the eligible source files, sorted by relative path
/// for deterministic output. Unreadable entries are skipped rather than
/// aborting the walk. Hard-fails with `Error::TooManyFiles` the moment the
/// discovered count exceeds `options.max_total_files` (`0` = unlimited) —
/// mid-walk short-circuit, not a post-walk count check, so an oversized
/// project never finishes an expensive full walk just to be rejected
/// (design "max_total_files enforcement point").
pub fn discover(
    root: &Path,
    options: &DiscoveryOptions,
) -> Result<Vec<DiscoveredFile>, codekurve_core::Error> {
    let mut builder = WalkBuilder::new(root);
    if !options.ignore_patterns.is_empty() {
        builder.overrides(build_ignore_overrides(root, &options.ignore_patterns)?);
    }
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
        // ponytail: extension filter is enough for TS/JS/C# (`LanguageId::
        // from_extension` covers `.cs` since Phase 5); explicit binary
        // sniffing (§15.2) lands when non-text extensions matter.
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
        if options.max_total_files > 0 && files.len() > options.max_total_files {
            return Err(codekurve_core::Error::TooManyFiles {
                limit: options.max_total_files,
            });
        }
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}

/// Compile `ignore.patterns` into walk overrides. Override globs are a
/// whitelist by default, so each pattern is added as a `!` negation: matching
/// paths are excluded and every other path stays eligible.
fn build_ignore_overrides(
    root: &Path,
    patterns: &[String],
) -> Result<ignore::overrides::Override, codekurve_core::Error> {
    let invalid = |pattern: &str, err: ignore::Error| codekurve_core::Error::InvalidIgnorePattern {
        pattern: pattern.to_string(),
        reason: err.to_string(),
    };
    let mut builder = OverrideBuilder::new(root);
    for pattern in patterns {
        builder
            .add(&format!("!{pattern}"))
            .map_err(|err| invalid(pattern, err))?;
    }
    builder
        .build()
        .map_err(|err| codekurve_core::Error::InvalidIgnorePattern {
            pattern: patterns.join(", "),
            reason: err.to_string(),
        })
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
            max_total_files: 0,
            languages: vec![LanguageId::TypeScript, LanguageId::JavaScript],
            ignore_patterns: Vec::new(),
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

        let discovered = discover(root, &options()).unwrap();
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
        let found = discover(root, &opts).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].relative_path, "a.ts");
        assert_eq!(found[0].language, LanguageId::TypeScript);
    }

    /// Phase 6: exactly `max_total_files` discovered files is accepted, not
    /// rejected — the limit is inclusive.
    #[test]
    fn at_limit_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["a", "b", "c"] {
            fs::write(root.join(format!("{name}.ts")), "").unwrap();
        }

        let mut opts = options();
        opts.max_total_files = 3;
        let found = discover(root, &opts).unwrap();

        assert_eq!(found.len(), 3);
    }

    /// Phase 6 (design "max_total_files enforcement point"): the moment
    /// discovery finds one file more than the configured cap, it hard-fails
    /// with `Error::TooManyFiles` instead of returning a truncated list.
    #[test]
    fn over_limit_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["a", "b", "c"] {
            fs::write(root.join(format!("{name}.ts")), "").unwrap();
        }

        let mut opts = options();
        opts.max_total_files = 2;
        let err = discover(root, &opts).unwrap_err();

        assert!(matches!(
            err,
            codekurve_core::Error::TooManyFiles { limit: 2 }
        ));
    }

    /// `max_total_files: 0` means unlimited — never short-circuits, however
    /// many files are discovered.
    #[test]
    fn zero_disables_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["a", "b", "c"] {
            fs::write(root.join(format!("{name}.ts")), "").unwrap();
        }

        let mut opts = options();
        opts.max_total_files = 0;
        let found = discover(root, &opts).unwrap();

        assert_eq!(found.len(), 3);
    }

    /// `ignore.patterns` (default secret/minified/build globs) must exclude
    /// matching files even when they are not gitignored. Hidden files are
    /// included so `.env.ts` is only excluded by the pattern.
    #[test]
    fn ignore_patterns_exclude_secret_minified_and_build_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("dist")).unwrap();
        fs::write(root.join("src").join("a.ts"), "export class A {}").unwrap();
        fs::write(root.join("src").join("app.min.js"), "function a(){}").unwrap();
        fs::write(root.join(".env.ts"), "export const k = 1;").unwrap();
        fs::write(root.join("secrets.ts"), "export const s = 1;").unwrap();
        fs::write(root.join("dist").join("x.ts"), "export class X {}").unwrap();

        let mut opts = options();
        opts.include_hidden = true;
        opts.ignore_patterns = ["**/.env*", "**/secrets.*", "**/dist/**", "**/*.min.js"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let found: Vec<String> = discover(root, &opts)
            .unwrap()
            .into_iter()
            .map(|f| f.relative_path)
            .collect();

        assert_eq!(found, vec!["src/a.ts".to_string()]);
    }

    /// A pattern that is not a valid glob fails discovery, naming the pattern,
    /// instead of silently indexing files it was meant to exclude.
    #[test]
    fn invalid_ignore_pattern_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.ts"), "").unwrap();

        let mut opts = options();
        opts.ignore_patterns = vec!["src/[unclosed".to_string()];
        let err = discover(root, &opts).unwrap_err();

        assert!(matches!(
            err,
            codekurve_core::Error::InvalidIgnorePattern { ref pattern, .. }
                if pattern == "src/[unclosed"
        ));
    }
}
