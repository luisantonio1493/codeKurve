//! File discovery. Walks a project root, honoring ignore rules, and yields the
//! source files eligible for indexing. See CODEKURVE_MASTER_PLAN.md §15
//! (discovery) and §15.1 (file rules).

use std::collections::HashSet;
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

/// What a walk found, plus what it could not look at. A caller that diffs
/// the walk against stored state must not read "not found" as "deleted" for
/// anything under `unreadable` (or at all when `incomplete`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    pub files: Vec<DiscoveredFile>,
    /// Root-relative `/`-separated paths (directories or files) the walk
    /// failed on, e.g. a permission error.
    pub unreadable: Vec<String>,
    /// A walk error carried no usable path: nothing can be ruled deleted.
    pub incomplete: bool,
    /// Root-relative `/`-separated paths reached through a symlink (with
    /// `follow_symlinks`) whose target lies outside the canonical root. They
    /// are never listed in `files`; this is not a walk failure, so it does not
    /// make anything unverified.
    pub outside_root: Vec<String>,
}

impl DiscoveryReport {
    /// Whether `relative_path` lies at or under something the walk failed on.
    pub fn is_unverified(&self, relative_path: &str) -> bool {
        self.incomplete
            || self.unreadable.iter().any(|failed| {
                relative_path == failed
                    || relative_path
                        .strip_prefix(failed.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
    }
}

fn walk_error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } => walk_error_path(err),
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::Partial(errors) => errors.iter().find_map(walk_error_path),
        _ => None,
    }
}

/// [`discover_report`] without the failure detail (callers that only want
/// the files, e.g. a first index).
pub fn discover(
    root: &Path,
    options: &DiscoveryOptions,
) -> Result<Vec<DiscoveredFile>, codekurve_core::Error> {
    discover_report(root, options).map(|report| report.files)
}

/// Walk `root` and return the eligible source files, sorted by relative path
/// for deterministic output. Walk errors are recorded in the report instead
/// of being dropped. Hard-fails with `Error::TooManyFiles` the moment the
/// discovered count exceeds `options.max_total_files` (`0` = unlimited) —
/// mid-walk short-circuit, not a post-walk count check, so an oversized
/// project never finishes an expensive full walk just to be rejected
/// (design "max_total_files enforcement point").
pub fn discover_report(
    root: &Path,
    options: &DiscoveryOptions,
) -> Result<DiscoveryReport, codekurve_core::Error> {
    walk(root, options, None)
}

/// [`discover_report`] restricted to `paths`: root-relative `/`-separated
/// entries, each a file (listed when it exists) or a directory (its whole
/// subtree is listed). The walk enters only the directories on the way to
/// those entries and the subtrees under them, so one changed file costs a
/// listing of its ancestors instead of a walk of the project. Ignore rules,
/// hidden-file and size limits apply exactly as in [`discover_report`]; the
/// `max_total_files` cap counts only what this walk returns.
pub fn discover_report_for(
    root: &Path,
    options: &DiscoveryOptions,
    paths: &HashSet<String>,
) -> Result<DiscoveryReport, codekurve_core::Error> {
    walk(root, options, Some(paths.iter().cloned().collect()))
}

fn walk(
    root: &Path,
    options: &DiscoveryOptions,
    wanted: Option<Vec<String>>,
) -> Result<DiscoveryReport, codekurve_core::Error> {
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
    // Directories off the way to a wanted entry are pruned inside the walker,
    // so ignore and hidden rules still decide what a wanted subtree contains.
    // The walker does not apply this predicate to files when a size limit is
    // set, so files are checked against `wanted` below as well.
    let wanted_files = wanted.clone();
    if let Some(wanted) = wanted {
        let root = root.to_path_buf();
        builder.filter_entry(move |entry| {
            relative_slash_path(&root, entry.path()).is_some_and(|rel| on_the_way(&wanted, &rel))
        });
    }

    // Canonical root for the containment check (A1). `None` when the root
    // cannot be resolved: then every followed file fails closed as outside.
    let canonical_root = options
        .follow_symlinks
        .then(|| root.canonicalize().ok())
        .flatten();
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    let mut outside_root = Vec::new();
    let mut incomplete = false;
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                match walk_error_path(&err).and_then(|path| relative_slash_path(root, path)) {
                    Some(relative) if !relative.is_empty() => unreadable.push(relative),
                    _ => incomplete = true,
                }
                continue;
            }
        };
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
        if wanted_files
            .as_ref()
            .is_some_and(|wanted| !on_the_way(wanted, &relative_path))
        {
            continue;
        }
        if options.follow_symlinks && !resolves_under(canonical_root.as_deref(), path) {
            outside_root.push(relative_path);
            continue;
        }
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
    outside_root.sort();
    Ok(DiscoveryReport {
        files,
        unreadable,
        incomplete,
        outside_root,
    })
}

/// Whether `path`, after resolving every symlink on the way, lies under the
/// canonical project root. An unresolvable path is not contained (A1).
fn resolves_under(canonical_root: Option<&Path>, path: &Path) -> bool {
    match (canonical_root, path.canonicalize()) {
        (Some(root), Ok(target)) => target.starts_with(root),
        _ => false,
    }
}

/// Whether a walk restricted to `wanted` must enter or yield `rel`: it is a
/// wanted entry, an ancestor of one (the walk has to pass through it), or lies
/// under a wanted directory.
fn on_the_way(wanted: &[String], rel: &str) -> bool {
    rel.is_empty()
        || wanted
            .iter()
            .any(|w| w.is_empty() || rel == w || is_below(rel, w) || is_below(w, rel))
}

/// `path` lies strictly below the directory `dir` (both root-relative).
fn is_below(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.starts_with('/'))
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

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_reported_not_swallowed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("locked")).unwrap();
        std::fs::write(dir.path().join("locked/b.ts"), "export const b = 1;\n").unwrap();
        std::fs::write(dir.path().join("ok.ts"), "export const a = 1;\n").unwrap();
        let locked = dir.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let denied = std::fs::read_dir(&locked).is_err();

        let report = discover_report(dir.path(), &options());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !denied {
            return; // running as root: mode bits are ignored
        }

        let report = report.unwrap();
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.unreadable, vec!["locked".to_string()]);
        assert!(report.is_unverified("locked/b.ts"));
        assert!(!report.is_unverified("ok.ts"));
        assert!(!report.is_unverified("lockedx/b.ts"));
    }

    fn listed(report: &DiscoveryReport) -> Vec<&str> {
        report
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect()
    }

    /// PERF-3: a restricted walk never reads siblings of what it was asked
    /// for, so a cap the whole project exceeds is not hit by one file.
    #[test]
    fn restricted_walk_lists_only_the_named_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src").join("a.ts"), "").unwrap();
        fs::write(root.join("src").join("b.ts"), "").unwrap();
        for name in ["c", "d", "e"] {
            fs::write(root.join(format!("{name}.ts")), "").unwrap();
        }
        let mut opts = options();
        opts.max_total_files = 2;
        let paths = HashSet::from(["src/a.ts".to_string()]);

        let report = discover_report_for(root, &opts, &paths).unwrap();

        assert_eq!(listed(&report), vec!["src/a.ts"]);
    }

    /// PERF-3: a named directory yields its whole subtree, and only that.
    #[test]
    fn restricted_walk_lists_a_named_directory_subtree_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src").join("nested")).unwrap();
        fs::create_dir(root.join("other")).unwrap();
        fs::write(root.join("src").join("a.ts"), "").unwrap();
        fs::write(root.join("src").join("nested").join("n.ts"), "").unwrap();
        fs::write(root.join("other").join("o.ts"), "").unwrap();
        let paths = HashSet::from(["src".to_string()]);

        let report = discover_report_for(root, &options(), &paths).unwrap();

        assert_eq!(listed(&report), vec!["src/a.ts", "src/nested/n.ts"]);
    }

    /// PERF-3 guard: a named file inside a gitignored directory is not listed,
    /// exactly as the full walk would not list it.
    #[test]
    fn restricted_walk_keeps_ignore_rules_for_named_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("build")).unwrap();
        fs::write(root.join("build").join("out.ts"), "").unwrap();
        fs::write(root.join(".gitignore"), "build/\n").unwrap();
        let paths = HashSet::from(["build/out.ts".to_string(), "build".to_string()]);

        let report = discover_report_for(root, &options(), &paths).unwrap();

        assert!(listed(&report).is_empty(), "got {:?}", listed(&report));
    }

    /// A1 (SEC): with `follow_symlinks` on, a symlink whose target lies outside
    /// the project root must not be indexed. A symlink that stays inside the
    /// root keeps indexing.
    #[cfg(unix)]
    #[test]
    fn follow_symlinks_does_not_index_targets_outside_the_root() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.ts"), "export const s = 1;\n").unwrap();
        fs::create_dir(outside.path().join("outdir")).unwrap();
        fs::write(outside.path().join("outdir").join("o.ts"), "").unwrap();
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.ts"), "export const a = 1;\n").unwrap();
        fs::write(root.path().join("inner.ts"), "export const i = 1;\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.ts"),
            root.path().join("link.ts"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path().join("outdir"), root.path().join("linkdir"))
            .unwrap();
        std::os::unix::fs::symlink(root.path().join("inner.ts"), root.path().join("inlink.ts"))
            .unwrap();
        let mut opts = options();
        opts.follow_symlinks = true;

        let report = discover_report(root.path(), &opts).unwrap();

        assert_eq!(listed(&report), vec!["a.ts", "inlink.ts", "inner.ts"]);
        assert_eq!(
            report.outside_root,
            vec!["link.ts".to_string(), "linkdir/o.ts".to_string()]
        );
    }
}
