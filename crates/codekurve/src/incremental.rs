//! Phase 3 tasks 5.1-5.4 (design "Technical Approach"): the shared
//! change-detection + apply engine. `index` calls it with `filter = None`
//! (full sweep); `watch` (PR6) will call it per debounced batch with
//! `filter = Some(paths)` — same body either way, so there is exactly one
//! "what changed" implementation and one "apply a batch" implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use codekurve_analysis::discovery::{self, DiscoveredFile, DiscoveryOptions, DiscoveryReport};
use codekurve_analysis::extract;
use codekurve_analysis::ir::{EdgeTarget, FileAnalysis};
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{LanguageId, SymbolKind};
use codekurve_store::repo;
use codekurve_store::Connection;
use rayon::prelude::*;

use crate::commands::{self, FileMeta};

/// One file's classification against the shared change-detection engine
/// (design "Interfaces").
#[derive(Debug, Clone, PartialEq)]
pub enum FileChange {
    Created(DiscoveredFile),
    Modified(DiscoveredFile),
    Deleted {
        relative_path: String,
    },
    /// mtime moved but the bytes did not: nothing to re-index, only the
    /// stored mtime to refresh so the fast path applies next time.
    Touched {
        relative_path: String,
        modified_ns: i64,
    },
    /// Stored file the engine could not look at (unreadable directory or
    /// file): its rows are kept, but the index cannot be called verified.
    Unverified {
        relative_path: String,
    },
}

/// Everything `apply_batch` needs beyond `changes` itself (design
/// "Interfaces").
pub struct IndexContext<'a> {
    pub root: &'a Path,
    pub project_id: &'a str,
    pub aliases: &'a TsconfigAliases,
    pub options: &'a DiscoveryOptions,
    /// PR6's `[index.watch]` config (design's File Changes table); `index`
    /// and `watch` both read it from `Config`, so there is one source of
    /// truth instead of a literal duplicated per caller.
    pub full_reindex_threshold_pct: u32,
}

/// Summary of one `apply_batch` call, for `index`'s (and later `watch`'s)
/// summary line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchOutcome {
    pub files_changed: usize,
    pub files_deleted: usize,
    pub fell_back_to_full_reindex: bool,
    /// Indexed files that could not be read or walked this time; their rows
    /// were kept and the index is left marked stale until they can be.
    pub unverified: usize,
}

/// Task 5.1/7.6 (design "Interfaces" + "walk-intersection handles it"):
/// classifies every file the shared engine needs to look at against what's
/// currently stored. `filter = None` is a full sweep (`index`, watch's
/// reconcile-on-start); `filter = Some(paths)` restricts both the
/// walk-derived candidates and the deletion check to a debounced batch. A
/// filter entry matches either a file's exact relative path or a
/// slash-prefix directory of it (`filter_matches`) — macOS FSEvents can
/// report a directory path instead of the individual files that changed
/// inside it, and `discovery::discover`'s walk is what actually knows which
/// files live under that directory (design "walk-intersection"). Fast path:
/// `(size_bytes, modified_ns)` equal to stored -> unchanged, no read.
/// Otherwise confirm via BLAKE3 `content_hash` before calling it `Modified`
/// (spec "Mtime touch without content change is not a false positive").
///
/// `force_full`: treats every stored file as untracked, so every discovered
/// file comes back `Created` — the same shape a never-indexed project
/// already produces, which `apply_batch`'s oversized-batch check naturally
/// routes into a full [`repo::reindex`] (no separate codepath needed). Set
/// by callers that detected an analyzer-version mismatch (new extraction
/// logic, unchanged source files — content hashes alone can't see that).
/// A file read whose bytes and mtime describe the same version of it.
struct StableRead {
    bytes: Vec<u8>,
    text: String,
    modified_ns: i64,
}

fn read_stable(path: &Path) -> Result<StableRead, String> {
    read_stable_with(path, || {})
}

/// Read and analyze one file: the per-file work the parse loops run on the
/// rayon pool. Pure per file (no DB access), so callers fold the results back
/// in discovery order.
fn read_and_analyze(
    absolute: &Path,
    relative_path: &str,
    language: LanguageId,
) -> Result<(FileMeta, FileAnalysis), String> {
    let read = read_stable(absolute)?;
    let analysis =
        extract::analyze(&read.text, language, relative_path).map_err(|e| e.to_string())?;
    let meta = FileMeta {
        language,
        size_bytes: read.bytes.len() as u64,
        content_hash: repo::content_hash(&read.bytes),
        modified_ns: read.modified_ns,
    };
    Ok((meta, analysis))
}

/// Stat, read, stat again; if size or mtime moved, a write raced the read —
/// retry a few times, then fail the file (the watcher retries the batch).
/// Raw bytes plus a lossy UTF-8 view: hash and size come from the bytes
/// (what `detect` compares against), so a Latin-1 file is still analyzed
/// instead of failing the batch. `between` runs after the read, before the
/// second stat (a test seam; a no-op in production).
fn read_stable_with(path: &Path, mut between: impl FnMut()) -> Result<StableRead, String> {
    for _ in 0..3 {
        let before = fs::metadata(path).map_err(|e| e.to_string())?;
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        between();
        let after = fs::metadata(path).map_err(|e| e.to_string())?;
        let (before_ns, after_ns) = (mtime_ns(&before)?, mtime_ns(&after)?);
        if before.len() == after.len() && before_ns == after_ns {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            return Ok(StableRead {
                bytes,
                text,
                modified_ns: after_ns,
            });
        }
    }
    Err(format!(
        "{} kept changing while it was being read",
        path.display()
    ))
}

pub fn detect(
    conn: &Connection,
    project_id: &str,
    root: &Path,
    opts: &DiscoveryOptions,
    filter: Option<&HashSet<String>>,
    force_full: bool,
) -> Result<Vec<FileChange>, String> {
    let report = discovery::discover_report(root, opts).map_err(|e| e.to_string())?;
    let discovered = report.files.clone();
    let snapshot = repo::file_snapshot(conn, project_id).map_err(|e| e.to_string())?;
    // `force_full` re-reads everything (every file looks new) but deletions
    // are still judged against what is really stored.
    let no_prior = HashMap::new();
    let stored = if force_full { &no_prior } else { &snapshot };

    let mut changes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for file in discovered {
        if filter.is_some_and(|f| !filter_matches(f, &file.relative_path)) {
            continue;
        }
        seen.insert(file.relative_path.clone());

        let Some(prior) = stored.get(&file.relative_path) else {
            changes.push(FileChange::Created(file));
            continue;
        };

        // Vanished between the walk and the stat; next sweep sees it as
        // Deleted, so it's safe to just skip it this round.
        let Ok(meta) = fs::metadata(&file.absolute_path) else {
            continue;
        };
        let size = meta.len();
        let mtime = mtime_ns(&meta)?;
        if size == prior.size_bytes && Some(mtime) == prior.modified_ns {
            continue; // fast path: unchanged
        }

        let Ok(bytes) = fs::read(&file.absolute_path) else {
            changes.push(FileChange::Unverified {
                relative_path: file.relative_path.clone(),
            });
            continue;
        };
        if prior.content_hash.as_deref() == Some(repo::content_hash(&bytes).as_str()) {
            // touch-only: mtime moved, content didn't
            changes.push(FileChange::Touched {
                relative_path: file.relative_path.clone(),
                modified_ns: mtime,
            });
            continue;
        }
        changes.push(FileChange::Modified(file));
    }

    for path in snapshot.keys() {
        if seen.contains(path) {
            continue;
        }
        if filter.is_some_and(|f| !filter_matches(f, path)) {
            continue;
        }
        // The walk could not look here: absence is not evidence of deletion.
        if report.is_unverified(path) {
            changes.push(FileChange::Unverified {
                relative_path: path.clone(),
            });
            continue;
        }
        changes.push(FileChange::Deleted {
            relative_path: path.clone(),
        });
    }

    Ok(changes)
}

/// Task 7.6: `relative_path` matches `filter` if it's an exact entry, or a
/// filter entry is a slash-separated directory prefix of it — the
/// walk-intersection the design promises for a directory-level watch event
/// (e.g. filter = `{"src"}`, `relative_path` = `"src/a.ts"` matches; `filter
/// = {"src2"}` does not, since `"src2"` isn't a full path component of
/// `"src/a.ts"`).
fn filter_matches(filter: &HashSet<String>, relative_path: &str) -> bool {
    if filter.contains(relative_path) {
        return true;
    }
    filter.iter().any(|f| {
        relative_path.len() > f.len()
            && relative_path.starts_with(f.as_str())
            && relative_path.as_bytes()[f.len()] == b'/'
    })
}

/// Task 5.3/5.4 (design "Batch Atomicity" + "Oversized Batch Falls Back to
/// Full Reindex"): T1 publishes the pending count; an oversized batch falls
/// back to the existing full [`repo::reindex`]; otherwise parses+resolves
/// `B ∪ D` with zero DB writes, then applies everything in one T2
/// transaction.
pub fn apply_batch(
    conn: &mut Connection,
    ctx: &IndexContext,
    changes: &[FileChange],
) -> Result<BatchOutcome, String> {
    // Touch-only entries are bookkeeping, not changes: refresh their stored
    // mtime and carry on with whatever real changes remain.
    let touched: Vec<(&str, i64)> = changes
        .iter()
        .filter_map(|c| match c {
            FileChange::Touched {
                relative_path,
                modified_ns,
            } => Some((relative_path.as_str(), *modified_ns)),
            _ => None,
        })
        .collect();
    if !touched.is_empty() {
        repo::touch_files(conn, ctx.project_id, &touched).map_err(|e| e.to_string())?;
    }
    let unverified = changes
        .iter()
        .filter(|c| matches!(c, FileChange::Unverified { .. }))
        .count();
    let real: Vec<FileChange> = changes
        .iter()
        .filter(|c| {
            !matches!(
                c,
                FileChange::Touched { .. } | FileChange::Unverified { .. }
            )
        })
        .cloned()
        .collect();
    let changes = real.as_slice();
    if changes.is_empty() {
        record_unverified(conn, ctx.project_id, unverified)?;
        return Ok(BatchOutcome {
            unverified,
            ..BatchOutcome::default()
        });
    }
    let ts = repo::now_ts();

    {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        repo::set_pending_files(&tx, ctx.project_id, &ts, changes.len() as i64)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    // Task 5.4: gate on the raw detected-batch size (spec "Oversized Batch
    // Falls Back to Full Reindex" scenario: "a batch whose changed-file
    // count exceeds the configured threshold"), not the derived B∪D
    // affected set — computing D needs B parsed first (it matches against
    // B's fresh symbol names), so checking pre-D avoids that work entirely
    // for the common "this is basically the whole project" case. A batch
    // that's borderline pre-D and only pushed over by D stays incremental;
    // that's a deliberately accepted edge case (D only ever narrows what
    // gets reparsed further, it never breaks correctness).
    let tracked = repo::count_files(conn, ctx.project_id).map_err(|e| e.to_string())?;
    let mut outcome = if is_oversized(changes.len(), tracked, ctx.full_reindex_threshold_pct) {
        apply_via_full_reindex(conn, ctx)?
    } else {
        apply_incremental_changes(conn, ctx, changes, &ts)?
    };
    // The batch above marked the index verified; files we could not look at
    // take that back.
    record_unverified(conn, ctx.project_id, unverified)?;
    outcome.unverified = unverified;
    // Refresh planner statistics after real writes; a hint, never a failure.
    let _ = codekurve_store::db::optimize(conn);
    Ok(outcome)
}

/// Publish the verification state after a pass: with unverifiable files the
/// index is left stale (`pending_files` = how many), otherwise a stale mark
/// left by an earlier failure is cleared.
fn record_unverified(
    conn: &mut Connection,
    project_id: &str,
    unverified: usize,
) -> Result<(), String> {
    let ts = repo::now_ts();
    if unverified > 0 {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        repo::set_pending_files(&tx, project_id, &ts, unverified as i64)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    } else if repo::pending_files(conn, project_id).map_err(|e| e.to_string())? > 0 {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        repo::mark_verified(&tx, project_id, &ts).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// One stderr line when files could not be verified, shared by `index`,
/// `watch` and MCP reindex.
pub fn warn_unverified(outcome: &BatchOutcome) {
    if outcome.unverified > 0 {
        eprintln!(
            "warning: {} indexed file(s) could not be read; their existing rows were kept and the index is marked stale until they can be verified",
            outcome.unverified
        );
    }
}

/// The last path segment an import of this file would end in: the file stem,
/// or the directory name for an `index` file.
fn module_stem(relative_path: &str) -> Option<String> {
    let path = Path::new(relative_path);
    let stem = path.file_stem()?.to_str()?;
    if stem == "index" {
        return path
            .parent()?
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
    }
    Some(stem.to_string())
}

fn file_meta_paths(analyses: &[FileAnalysis]) -> HashSet<&str> {
    analyses.iter().map(|a| a.file.as_str()).collect()
}

fn is_oversized(changed: usize, tracked: usize, threshold_pct: u32) -> bool {
    if tracked == 0 {
        // Bootstrap: nothing tracked yet, so every discovered file reads as
        // `Created` — naturally the whole project, exactly what `reindex`
        // already handles well (task 5.5's "first run" requirement).
        return changed > 0;
    }
    changed * 100 > tracked * threshold_pct as usize
}

/// Task 5.4: the existing full-reindex path, reused rather than duplicated.
/// Ignores `changes` and re-discovers/re-parses the whole project, exactly
/// like `index()` did pre-PR5 — the only path that still needs a full walk.
fn apply_via_full_reindex(
    conn: &mut Connection,
    ctx: &IndexContext,
) -> Result<BatchOutcome, String> {
    let report: DiscoveryReport =
        discovery::discover_report(ctx.root, ctx.options).map_err(|e| e.to_string())?;

    // Read and analyze on the rayon pool; `collect` keeps discovery order, so
    // the fold below builds the same `analyses` and `file_meta` a sequential
    // loop would. Only the DB writes that follow stay single-threaded.
    let parsed: Vec<Result<(FileMeta, FileAnalysis), String>> = report
        .files
        .par_iter()
        .map(|file| read_and_analyze(&file.absolute_path, &file.relative_path, file.language))
        .collect();
    let mut analyses: Vec<FileAnalysis> = Vec::new();
    let mut file_meta: Vec<FileMeta> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (file, result) in report.files.iter().zip(parsed) {
        let Ok((meta, analysis)) = result else {
            skipped.push(file.relative_path.clone());
            continue;
        };
        file_meta.push(meta);
        analyses.push(analysis);
    }

    // `reindex` replaces the whole project, so a file we could not read or
    // walk would silently vanish from the index. Refuse and keep what is
    // stored; files that were never indexed are only warned about.
    let stored = repo::file_snapshot(conn, ctx.project_id).map_err(|e| e.to_string())?;
    let analyzed: HashSet<&str> = file_meta_paths(&analyses);
    let mut lost: Vec<&String> = stored
        .keys()
        .filter(|path| !analyzed.contains(path.as_str()))
        .filter(|path| skipped.contains(path) || report.is_unverified(path))
        .collect();
    if !lost.is_empty() {
        lost.sort();
        return Err(format!(
            "{} previously indexed file(s) could not be read or parsed (e.g. {}); keeping the existing index",
            lost.len(),
            lost[0]
        ));
    }
    for path in &skipped {
        eprintln!("warning: skipped unreadable or unparsable file {path}");
    }
    resolve::resolve(&mut analyses, ctx.aliases);

    let (files, symbol_ids) = commands::build_file_inputs(ctx.project_id, &analyses, &file_meta);
    let relationships = commands::build_relationships(ctx.project_id, &analyses, &symbol_ids);
    let unresolved = commands::build_unresolved(ctx.project_id, &analyses, &symbol_ids);

    let outcome = repo::reindex(conn, ctx.project_id, &files, &relationships, &unresolved)
        .map_err(|e| e.to_string())?;

    Ok(BatchOutcome {
        files_changed: outcome.files,
        files_deleted: 0,
        fell_back_to_full_reindex: true,
        unverified: 0,
    })
}

/// The non-fallback path: parses+resolves only `B` (created/modified) plus
/// `D` (dependents, design "Dependent Re-Resolution Scope") against a
/// baseline of everything else already stored, then applies the whole
/// batch — deletes, B, and D together — in one T2 transaction.
fn apply_incremental_changes(
    conn: &mut Connection,
    ctx: &IndexContext,
    changes: &[FileChange],
    ts: &str,
) -> Result<BatchOutcome, String> {
    let mut created_or_modified: Vec<DiscoveredFile> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    for change in changes {
        match change {
            FileChange::Created(f) | FileChange::Modified(f) => created_or_modified.push(f.clone()),
            FileChange::Deleted { relative_path } => deleted.push(relative_path.clone()),
            FileChange::Touched { .. } | FileChange::Unverified { .. } => {} // handled in `apply_batch`
        }
    }
    let b_paths: HashSet<String> = created_or_modified
        .iter()
        .map(|f| f.relative_path.clone())
        .collect();
    let deleted_set: HashSet<String> = deleted.iter().cloned().collect();

    // Parse B — a parse error here fails the whole batch (spec "A Failed
    // Batch Preserves the Previous Index": "parse error treated as batch
    // failure"), unlike the fallback path's lenient skip-on-error.
    let parsed: Vec<Result<(FileMeta, FileAnalysis), String>> = created_or_modified
        .par_iter()
        .map(|file| read_and_analyze(&file.absolute_path, &file.relative_path, file.language))
        .collect();
    let mut analyses: Vec<FileAnalysis> = Vec::new();
    let mut file_meta: Vec<FileMeta> = Vec::new();
    // Discovery order: the first failure in that order is the one reported.
    for result in parsed {
        let (meta, analysis) = result?;
        file_meta.push(meta);
        analyses.push(analysis);
    }

    // Dependent set D (design "Dependent Re-Resolution Scope"): files
    // outside B/deleted whose stored relationships target a symbol B
    // changed/deleted, plus files with an unresolved reference matching a
    // name B just introduced. ponytail: the unresolved-target lookup only
    // matches by-name text, not an import-specifier reaching a newly
    // created file — the by-name case is what PR4's own test exercises;
    // specifier-based dependents are a documented gap, not attempted here.
    let mut changed_and_deleted: Vec<String> = b_paths.iter().cloned().collect();
    changed_and_deleted.extend(deleted.iter().cloned());
    let old_symbol_ids = repo::symbol_ids_for_files(conn, ctx.project_id, &changed_and_deleted)
        .map_err(|e| e.to_string())?;
    let mut dependents: HashSet<String> = repo::dependents_by_target_symbol(
        conn,
        ctx.project_id,
        &old_symbol_ids,
        &changed_and_deleted,
    )
    .map_err(|e| e.to_string())?
    .into_iter()
    .collect();
    let new_names: Vec<String> = analyses
        .iter()
        .flat_map(|a| a.symbols.iter().map(|s| s.name.clone()))
        .collect();
    dependents.extend(
        repo::dependents_by_unresolved_target(conn, ctx.project_id, &new_names)
            .map_err(|e| e.to_string())?,
    );
    // `using N;` links to every namespace symbol named `N`: a batch file that
    // newly declares `namespace N` adds one, so N's importers are dependents.
    // A file that already declared N keeps its symbol (same id), and its
    // importers keep their edge: re-analyzing them on every edit would be
    // wasted work for a popular namespace.
    let old_declarations =
        repo::namespace_declarations_for_files(conn, ctx.project_id, &changed_and_deleted)
            .map_err(|e| e.to_string())?;
    let namespace_names: Vec<String> = analyses
        .iter()
        .flat_map(|a| {
            a.symbols
                .iter()
                .filter(|s| s.kind == SymbolKind::Namespace)
                .map(move |s| (a.file.clone(), s.name.clone()))
        })
        .filter(|declaration| !old_declarations.contains(declaration))
        .map(|(_, name)| name)
        .collect();
    dependents.extend(
        repo::dependents_by_namespace(conn, ctx.project_id, &namespace_names, &changed_and_deleted)
            .map_err(|e| e.to_string())?,
    );
    // A newly created module can satisfy a relative import that named no
    // symbol, so by-name matching never sees it: match the import specifiers
    // against the new files' module names (`foo.ts` -> `foo`, `a/index.ts`
    // -> `a`).
    let new_modules: Vec<String> = changes
        .iter()
        .filter_map(|c| match c {
            FileChange::Created(f) => module_stem(&f.relative_path),
            _ => None,
        })
        .collect();
    dependents.extend(
        repo::dependents_by_unresolved_module(conn, ctx.project_id, &new_modules)
            .map_err(|e| e.to_string())?,
    );
    dependents.retain(|p| !b_paths.contains(p) && !deleted_set.contains(p));

    let mut d_paths: Vec<String> = dependents.into_iter().collect();
    d_paths.sort();
    let root = ctx.root;
    // A dependent that vanished since it was last indexed (rare race), or no
    // longer parses, is skipped this cycle and reconsidered on the next sweep.
    let d_parsed: Vec<(FileMeta, FileAnalysis)> = d_paths
        .par_iter()
        .filter_map(|path| {
            let ext = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let language = LanguageId::from_extension(ext)?;
            read_and_analyze(&root.join(path), path, language).ok()
        })
        .collect();
    for (meta, analysis) in d_parsed {
        file_meta.push(meta);
        analyses.push(analysis);
    }

    // Baseline: everything NOT being reparsed this batch (B ∪ D ∪ deleted) —
    // deleted files must be excluded too, or a still-present-in-storage
    // deleted symbol would wrongly look resolvable to D's fresh edges.
    let reparsed: HashSet<String> = b_paths
        .iter()
        .cloned()
        .chain(d_paths.iter().cloned())
        .collect();
    let mut excluded = reparsed.clone();
    excluded.extend(deleted_set.iter().cloned());
    let mut snapshot =
        repo::resolution_snapshot(conn, ctx.project_id).map_err(|e| e.to_string())?;
    snapshot.files.retain(|f| !excluded.contains(f));
    snapshot
        .symbols
        .retain(|s| !excluded.contains(&s.relative_path));
    let baseline = commands::project_baseline(snapshot);

    resolve::resolve_with(&mut analyses, ctx.aliases, &baseline);

    let (files, mut symbol_ids) =
        commands::build_file_inputs(ctx.project_id, &analyses, &file_meta);
    let extra_ids = baseline_symbol_ids(conn, ctx.project_id, &analyses, &reparsed)?;
    symbol_ids.extend(extra_ids);
    let relationships = commands::build_relationships(ctx.project_id, &analyses, &symbol_ids);
    let unresolved = commands::build_unresolved(ctx.project_id, &analyses, &symbol_ids);

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    repo::apply_incremental(
        &tx,
        ctx.project_id,
        ts,
        &files,
        &relationships,
        &unresolved,
        &deleted,
    )
    .map_err(|e| e.to_string())?;
    repo::mark_verified(&tx, ctx.project_id, ts).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;

    Ok(BatchOutcome {
        files_changed: b_paths.len(),
        files_deleted: deleted.len(),
        fell_back_to_full_reindex: false,
        unverified: 0,
    })
}

/// Resolves `analyses`' cross-file edge targets that land on a *baseline*
/// file (outside `reparsed`) to their real stored symbol id — those files
/// weren't reparsed this batch, so `build_file_inputs`'s fresh map has no
/// entry for them (design "Baseline for re-resolution").
fn baseline_symbol_ids(
    conn: &Connection,
    project_id: &str,
    analyses: &[FileAnalysis],
    reparsed: &HashSet<String>,
) -> Result<HashMap<(String, String), String>, String> {
    let mut pairs: HashSet<(String, String)> = HashSet::new();
    for a in analyses {
        for rel in &a.relationships {
            if let EdgeTarget::Global {
                file,
                qualified_name,
            } = &rel.target
            {
                if !reparsed.contains(file) {
                    pairs.insert((file.clone(), qualified_name.clone()));
                }
            }
        }
    }
    repo::symbol_ids_by_qualified_names(conn, project_id, &pairs.into_iter().collect::<Vec<_>>())
        .map_err(|e| e.to_string())
}

fn mtime_ns(meta: &fs::Metadata) -> Result<i64, String> {
    let modified = meta.modified().map_err(|e| e.to_string())?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    Ok(dur.as_nanos() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codekurve_store::db;
    use codekurve_store::repo::{upsert_project, FileInput};
    use std::fs as std_fs;
    use std::time::SystemTime;

    fn discovery_opts() -> DiscoveryOptions {
        DiscoveryOptions {
            respect_gitignore: true,
            respect_global_gitignore: false,
            include_hidden: false,
            follow_symlinks: false,
            max_file_size_bytes: 2_097_152,
            max_total_files: 0,
            languages: vec![LanguageId::TypeScript, LanguageId::CSharp],
            ignore_patterns: Vec::new(),
        }
    }

    /// Indexes every `paths` entry's current on-disk content as the "prior"
    /// state `detect` compares against, mirroring what a real `apply_batch`
    /// T2 would have written (content_hash/modified_ns included) — one
    /// `reindex` call, since `reindex` wipes the whole project first.
    fn seed_indexed(conn: &mut Connection, project_id: &str, root: &Path, paths: &[&str]) {
        let files: Vec<FileInput> = paths
            .iter()
            .map(|path| {
                let absolute = root.join(path);
                let bytes = std_fs::read(&absolute).unwrap();
                let meta = std_fs::metadata(&absolute).unwrap();
                FileInput {
                    relative_path: path.to_string(),
                    language: "typescript".to_string(),
                    size_bytes: bytes.len() as u64,
                    content_hash: repo::content_hash(&bytes),
                    modified_ns: mtime_ns(&meta).unwrap(),
                    symbols: vec![],
                }
            })
            .collect();
        repo::reindex(conn, project_id, &files, &[], &[]).unwrap();
    }

    /// Task 5.6: `detect` classifies a touch-only edit (mtime changed,
    /// content byte-identical) as unchanged, a real content edit as
    /// `Modified`, a brand-new file as `Created`, and a removed file as
    /// `Deleted` — never conflating any of the four.
    #[test]
    fn detect_classifies_touch_only_changed_new_and_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::write(root.join("touched.ts"), "export const a = 1;\n").unwrap();
        std_fs::write(root.join("changed.ts"), "export const b = 1;\n").unwrap();
        std_fs::write(root.join("removed.ts"), "export const c = 1;\n").unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        seed_indexed(
            &mut conn,
            &project,
            root,
            &["touched.ts", "changed.ts", "removed.ts"],
        );

        // touch-only: bump mtime, keep bytes identical.
        let touched_path = root.join("touched.ts");
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        std_fs::File::options()
            .write(true)
            .open(&touched_path)
            .unwrap()
            .set_modified(future)
            .unwrap();

        // real content edit — bump mtime explicitly too, so this doesn't
        // depend on the filesystem's mtime-tick granularity vs. this
        // process's write speed (the fast path trusts an unchanged mtime
        // without hashing, by design).
        let changed_path = root.join("changed.ts");
        std_fs::write(&changed_path, "export const b = 2;\n").unwrap();
        std_fs::File::options()
            .write(true)
            .open(&changed_path)
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(7200))
            .unwrap();

        // deletion.
        std_fs::remove_file(root.join("removed.ts")).unwrap();

        // brand new file.
        std_fs::write(root.join("added.ts"), "export const d = 1;\n").unwrap();

        let changes = detect(&conn, &project, root, &discovery_opts(), None, false).unwrap();

        assert!(
            !changes
                .iter()
                .any(|c| matches!(c, FileChange::Modified(f) if f.relative_path == "touched.ts")),
            "touch-only edit must not be classified as changed"
        );
        assert!(changes
            .iter()
            .any(|c| matches!(c, FileChange::Modified(f) if f.relative_path == "changed.ts")));
        assert!(changes
            .iter()
            .any(|c| matches!(c, FileChange::Created(f) if f.relative_path == "added.ts")));
        assert!(changes.iter().any(
            |c| matches!(c, FileChange::Deleted { relative_path } if relative_path == "removed.ts")
        ));
        // The touched file only gets its mtime refreshed (`Touched`), so the
        // real changes are changed + added + removed = 3.
        let real = changes
            .iter()
            .filter(|c| !matches!(c, FileChange::Touched { .. }))
            .count();
        assert_eq!(real, 3);
    }

    fn ctx<'a>(
        root: &'a Path,
        project_id: &'a str,
        aliases: &'a TsconfigAliases,
        options: &'a DiscoveryOptions,
    ) -> IndexContext<'a> {
        IndexContext {
            root,
            project_id,
            aliases,
            options,
            full_reindex_threshold_pct: 25,
        }
    }

    /// Run `detect` + `apply_batch` once, like a real `index` pass.
    fn sync(conn: &mut Connection, project: &str, root: &Path) -> Result<BatchOutcome, String> {
        let options = discovery_opts();
        let aliases = TsconfigAliases::new();
        let changes = detect(conn, project, root, &options, None, false)?;
        apply_batch(conn, &ctx(root, project, &aliases, &options), &changes)
    }

    /// IDX-2: `force_full` treats every discovered file as new, but files
    /// deleted while nothing was watching must still be reported as deleted.
    #[test]
    fn force_full_still_reports_offline_deletions() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["a.ts", "b.ts", "c.ts", "d.ts"] {
            std_fs::write(root.join(name), "export const x = 1;\n").unwrap();
        }
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        seed_indexed(&mut conn, &project, root, &["a.ts", "b.ts", "c.ts", "d.ts"]);
        for name in ["b.ts", "c.ts", "d.ts"] {
            std_fs::remove_file(root.join(name)).unwrap();
        }

        let changes = detect(&conn, &project, root, &discovery_opts(), None, true).unwrap();

        let deleted: Vec<_> = changes
            .iter()
            .filter_map(|c| match c {
                FileChange::Deleted { relative_path } => Some(relative_path.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deleted.len(), 3, "got {changes:?}");
    }

    /// IDX-7: a dependent file that cannot be re-read must not break the
    /// batch (stale edges to symbols about to be replaced used to violate a
    /// foreign key and roll the whole transaction back).
    #[cfg(unix)]
    #[test]
    fn unreadable_dependent_does_not_fail_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..10 {
            std_fs::write(root.join(format!("f{i}.ts")), "export const k = 1;\n").unwrap();
        }
        std_fs::write(root.join("a.ts"), "export function foo() { return 1; }\n").unwrap();
        std_fs::write(
            root.join("b.ts"),
            "import { foo } from './a';\nexport function run() { return foo(); }\n",
        )
        .unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();
        let edges: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relationships WHERE kind = 'calls' AND target_symbol_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(edges >= 1, "fixture must produce a call edge into a.ts");

        // Change a.ts so its symbol is replaced; make its dependent unreadable.
        std_fs::write(root.join("a.ts"), "export function foo() { return 2; }\n").unwrap();
        std_fs::File::options()
            .write(true)
            .open(root.join("a.ts"))
            .unwrap()
            .set_modified(SystemTime::now() + std::time::Duration::from_secs(7200))
            .unwrap();
        if !deny(&root.join("b.ts")) {
            return;
        }
        let result = sync(&mut conn, &project, root);
        allow(&root.join("b.ts"));

        assert!(result.is_ok(), "batch failed: {result:?}");
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relationships r
                 WHERE r.target_symbol_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM symbols s WHERE s.id = r.target_symbol_id)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0);
    }

    /// IDX-6: bytes and mtime must describe the same version of the file. A
    /// write landing between the read and the stat used to store the new
    /// mtime with the old hash, hiding the change from the fast path forever.
    #[test]
    fn read_stable_retries_when_the_file_changes_mid_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.ts");
        std_fs::write(&path, "export const a = 1;\n").unwrap();
        let mut writes = 0;

        let read = read_stable_with(&path, || {
            if writes == 0 {
                writes += 1;
                std_fs::write(&path, "export const a = 22;\n").unwrap();
                std_fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(SystemTime::now() + std::time::Duration::from_secs(10))
                    .unwrap();
            }
        })
        .unwrap();

        assert_eq!(read.text, "export const a = 22;\n");
        assert_eq!(
            read.modified_ns,
            mtime_ns(&std_fs::metadata(&path).unwrap()).unwrap()
        );
    }

    #[test]
    fn read_stable_gives_up_on_a_file_that_keeps_changing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.ts");
        std_fs::write(&path, "x").unwrap();
        let mut n = 0u64;

        let result = read_stable_with(&path, || {
            n += 1;
            std_fs::write(&path, "x".repeat(n as usize + 1)).unwrap();
        });

        assert!(result.is_err());
    }

    fn unresolved_modules(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM unresolved_references WHERE reason = 'module not found in project'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// IDX-8: creating the module a file already imports (even a namespace
    /// import that names no symbol) re-resolves that importer.
    #[test]
    fn creating_an_imported_module_re_resolves_its_importers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..10 {
            std_fs::write(root.join(format!("f{i}.ts")), "export const k = 1;\n").unwrap();
        }
        std_fs::write(
            root.join("b.ts"),
            "import * as M from './missing';\nexport const x = M;\n",
        )
        .unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();
        assert_eq!(
            unresolved_modules(&conn),
            1,
            "fixture needs the dangling import"
        );

        std_fs::write(root.join("missing.ts"), "export const unrelated = 1;\n").unwrap();
        let outcome = sync(&mut conn, &project, root).unwrap();

        assert!(!outcome.fell_back_to_full_reindex, "must stay incremental");
        assert_eq!(unresolved_modules(&conn), 0, "importer was not re-resolved");
    }

    /// IDX-8: unrelated new files do not drag every importer of an external
    /// package into the batch.
    #[test]
    fn creating_an_unrelated_file_does_not_re_resolve_package_importers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..10 {
            std_fs::write(root.join(format!("f{i}.ts")), "export const k = 1;\n").unwrap();
        }
        std_fs::write(
            root.join("b.ts"),
            "import React from 'react';\nexport const x = React;\n",
        )
        .unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();

        std_fs::write(root.join("other.ts"), "export const unrelated = 1;\n").unwrap();
        let outcome = sync(&mut conn, &project, root).unwrap();

        assert_eq!(outcome.files_changed, 1, "only the new file is reindexed");
    }

    /// IDX-10: a touch-only edit (mtime moved, bytes identical) refreshes the
    /// stored mtime, so the fast path applies again instead of re-reading and
    /// re-hashing the file on every later pass.
    #[test]
    fn touch_only_edit_refreshes_the_stored_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::write(root.join("a.ts"), "export const a = 1;\n").unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();

        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        let file = std_fs::File::options()
            .write(true)
            .open(root.join("a.ts"))
            .unwrap();
        file.set_modified(future).unwrap();
        let new_mtime = mtime_ns(&std_fs::metadata(root.join("a.ts")).unwrap()).unwrap();

        sync(&mut conn, &project, root).unwrap();

        let stored = repo::file_snapshot(&conn, &project).unwrap();
        assert_eq!(stored["a.ts"].modified_ns, Some(new_mtime));
        assert_eq!(repo::pending_files(&conn, &project).unwrap(), 0);
    }

    /// IDX-1: a Latin-1 file is indexed (lossy decode) instead of failing the
    /// batch, and its hash is of the raw bytes so the next pass sees no change.
    #[test]
    fn non_utf8_file_is_indexed_and_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for i in 0..10 {
            std_fs::write(root.join(format!("f{i}.ts")), "export const a = 1;\n").unwrap();
        }
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();

        // incremental path (1 change out of 10 tracked, under the 25% threshold)
        std_fs::write(
            root.join("latin1.ts"),
            b"export const caf = 1; // caf\xe9\n".as_slice(),
        )
        .unwrap();
        let outcome = sync(&mut conn, &project, root).expect("non-UTF-8 must not fail the batch");
        assert!(!outcome.fell_back_to_full_reindex);
        assert_eq!(repo::count_files(&conn, &project).unwrap(), 11);

        let again = detect(&conn, &project, root, &discovery_opts(), None, false).unwrap();
        assert!(again.is_empty(), "unchanged bytes re-detected as {again:?}");
    }

    /// IDX-1: the full-reindex fallback keeps non-UTF-8 files too.
    #[test]
    fn full_reindex_keeps_non_utf8_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::write(root.join("ok.ts"), "export const a = 1;\n").unwrap();
        std_fs::write(
            root.join("latin1.ts"),
            b"export const b = 1; // \xe9\n".as_slice(),
        )
        .unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();

        sync(&mut conn, &project, root).unwrap();

        assert_eq!(repo::count_files(&conn, &project).unwrap(), 2);
    }

    #[cfg(unix)]
    fn deny(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std_fs::set_permissions(path, std_fs::Permissions::from_mode(0o000)).unwrap();
        // Running as root ignores the mode bits; the scenario can't be built.
        let readable = if path.is_dir() {
            std_fs::read_dir(path).is_ok()
        } else {
            std_fs::read(path).is_ok()
        };
        if readable {
            std_fs::set_permissions(path, std_fs::Permissions::from_mode(0o755)).unwrap();
        }
        !readable
    }

    #[cfg(unix)]
    fn allow(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std_fs::set_permissions(path, std_fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// IDX-1: a directory the walk cannot read is a failure, not a deletion.
    #[cfg(unix)]
    #[test]
    fn unreadable_directory_does_not_delete_its_indexed_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::create_dir(root.join("locked")).unwrap();
        std_fs::write(root.join("keep.ts"), "export const a = 1;\n").unwrap();
        std_fs::write(root.join("locked/b.ts"), "export const b = 1;\n").unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        seed_indexed(&mut conn, &project, root, &["keep.ts", "locked/b.ts"]);

        if !deny(&root.join("locked")) {
            return;
        }
        let changes = detect(&conn, &project, root, &discovery_opts(), None, false);
        allow(&root.join("locked"));

        let changes = changes.unwrap();
        assert!(
            !changes
                .iter()
                .any(|c| matches!(c, FileChange::Deleted { .. })),
            "unreadable directory read as deletion: {changes:?}"
        );
    }

    fn snapshot_of(conn: &Connection) -> Vec<String> {
        let mut out = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT 'unresolved|' || relationship_kind || '|' || target_text || '|' || reason
                 FROM unresolved_references
                 UNION ALL
                 SELECT 'edge|' || r.kind || '|' || s.qualified_name || '|' ||
                        COALESCE(t.qualified_name, r.target_external, '') || '|' || r.confidence
                 FROM relationships r
                 JOIN symbols s ON s.id = r.source_symbol_id
                 LEFT JOIN symbols t ON t.id = r.target_symbol_id
                 ORDER BY 1",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        for row in rows {
            out.push(row.unwrap());
        }
        out
    }

    /// Index `before` (plus filler so edits stay under the full-reindex
    /// threshold), apply the transition to `after` incrementally, and compare
    /// with a clean index of `after`. The plan's acceptance criterion:
    /// "incremental results equal a clean rebuild".
    fn assert_incremental_equals_clean(before: &[(&str, &str)], after: &[(&str, &str)]) {
        let filler = |root: &Path| {
            for i in 0..12 {
                std_fs::write(root.join(format!("f{i}.ts")), "export const k = 1;\n").unwrap();
            }
        };
        let write = |root: &Path, files: &[(&str, &str)]| {
            for (path, text) in files {
                if let Some(parent) = root.join(path).parent() {
                    std_fs::create_dir_all(parent).unwrap();
                }
                std_fs::write(root.join(path), text).unwrap();
            }
        };

        let incremental = tempfile::tempdir().unwrap();
        filler(incremental.path());
        write(incremental.path(), before);
        let mut conn_a = db::open_in_memory().unwrap();
        let project_a = upsert_project(&conn_a, "demo", "/tmp/a", "hash").unwrap();
        sync(&mut conn_a, &project_a, incremental.path()).unwrap();

        // Transition: delete what `after` no longer has, then write `after`
        // with a bumped mtime so the fast path cannot hide the edit.
        for (path, _) in before {
            if !after.iter().any(|(p, _)| p == path) {
                std_fs::remove_file(incremental.path().join(path)).unwrap();
            }
        }
        write(incremental.path(), after);
        for (path, _) in after {
            std_fs::File::options()
                .write(true)
                .open(incremental.path().join(path))
                .unwrap()
                .set_modified(SystemTime::now() + std::time::Duration::from_secs(7200))
                .unwrap();
        }
        let outcome = sync(&mut conn_a, &project_a, incremental.path()).unwrap();
        assert!(!outcome.fell_back_to_full_reindex, "must stay incremental");

        let clean = tempfile::tempdir().unwrap();
        filler(clean.path());
        write(clean.path(), after);
        let mut conn_b = db::open_in_memory().unwrap();
        let project_b = upsert_project(&conn_b, "demo", "/tmp/b", "hash").unwrap();
        sync(&mut conn_b, &project_b, clean.path()).unwrap();

        assert_eq!(snapshot_of(&conn_a), snapshot_of(&conn_b));
    }

    const UTIL: &str = "export function helper() { return 1; }\n";
    const MAIN: &str =
        "import { helper } from './util';\nexport function main() { return helper(); }\n";

    /// Editing a file that has named imports used to turn them into "not
    /// exported by target module".
    #[test]
    fn equivalence_edit_importer() {
        assert_incremental_equals_clean(
            &[("util.ts", UTIL), ("main.ts", MAIN)],
            &[
                ("util.ts", UTIL),
                ("main.ts", &MAIN.replace("helper()", "helper() + 1")),
            ],
        );
    }

    #[test]
    fn equivalence_rename_exported_symbol() {
        assert_incremental_equals_clean(
            &[("util.ts", UTIL), ("main.ts", MAIN)],
            &[
                ("util.ts", &UTIL.replace("helper", "helper2")),
                ("main.ts", MAIN),
            ],
        );
    }

    #[test]
    fn equivalence_delete_imported_module() {
        assert_incremental_equals_clean(
            &[("util.ts", UTIL), ("main.ts", MAIN)],
            &[("main.ts", MAIN)],
        );
    }

    /// A new C# file in an existing namespace adds a namespace symbol that
    /// unchanged files with `using <namespace>;` must now import too.
    #[test]
    fn equivalence_new_file_in_existing_csharp_namespace() {
        const A: &str = "namespace App.Services;\npublic class A { }\n";
        const B: &str = "namespace App.Services;\npublic class B { }\n";
        const CTL: &str = "namespace App.Web;\nusing App.Services;\npublic class Ctl { }\n";
        assert_incremental_equals_clean(
            &[("Svc/A.cs", A), ("Web/Ctl.cs", CTL)],
            &[("Svc/A.cs", A), ("Svc/B.cs", B), ("Web/Ctl.cs", CTL)],
        );
    }

    #[test]
    fn equivalence_create_missing_module() {
        assert_incremental_equals_clean(
            &[("main.ts", MAIN)],
            &[("util.ts", UTIL), ("main.ts", MAIN)],
        );
    }

    #[test]
    fn equivalence_move_module_and_fix_import() {
        assert_incremental_equals_clean(
            &[("util.ts", UTIL), ("main.ts", MAIN)],
            &[
                ("lib/util.ts", UTIL),
                ("main.ts", &MAIN.replace("./util", "./lib/util")),
            ],
        );
    }

    #[test]
    fn equivalence_class_inheritance_across_files() {
        assert_incremental_equals_clean(
            &[
                ("base.ts", "export class Base { run() { return 1; } }\n"),
                (
                    "child.ts",
                    "import { Base } from './base';\nexport class Child extends Base { go() { return this.run(); } }\n",
                ),
            ],
            &[
                ("base.ts", "export class Base { run() { return 2; } stop() {} }\n"),
                (
                    "child.ts",
                    "import { Base } from './base';\nexport class Child extends Base { go() { return this.run(); } }\n",
                ),
            ],
        );
    }

    /// IDX-1 (degraded status): files the engine could not verify keep their
    /// rows but must not be reported as a verified index.
    #[cfg(unix)]
    #[test]
    fn unverified_files_mark_the_index_stale_until_they_can_be_read() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::create_dir(root.join("locked")).unwrap();
        for i in 0..10 {
            std_fs::write(root.join(format!("f{i}.ts")), "export const k = 1;\n").unwrap();
        }
        std_fs::write(root.join("locked/b.ts"), "export const b = 1;\n").unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();
        assert_eq!(repo::pending_files(&conn, &project).unwrap(), 0);

        if !deny(&root.join("locked")) {
            return;
        }
        let outcome = sync(&mut conn, &project, root);
        let pending = repo::pending_files(&conn, &project).unwrap();
        let rows = repo::count_files(&conn, &project).unwrap();
        allow(&root.join("locked"));

        let outcome = outcome.unwrap();
        assert_eq!(outcome.unverified, 1);
        assert_eq!(
            pending, 1,
            "an unverifiable file must leave the index stale"
        );
        assert_eq!(rows, 11, "its rows are kept");

        let outcome = sync(&mut conn, &project, root).unwrap();
        assert_eq!(outcome.unverified, 0);
        assert_eq!(repo::pending_files(&conn, &project).unwrap(), 0);
    }

    /// A file that changed and then became unreadable is unverified too, not
    /// silently "unchanged".
    #[cfg(unix)]
    #[test]
    fn modified_then_unreadable_file_is_unverified() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::write(root.join("a.ts"), "export const a = 1;\n").unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        seed_indexed(&mut conn, &project, root, &["a.ts"]);

        std_fs::write(root.join("a.ts"), "export const a = 22;\n").unwrap();
        if !deny(&root.join("a.ts")) {
            return;
        }
        let changes = detect(&conn, &project, root, &discovery_opts(), None, false);
        allow(&root.join("a.ts"));

        let changes = changes.unwrap();
        assert!(
            changes.iter().any(
                |c| matches!(c, FileChange::Unverified { relative_path } if relative_path == "a.ts")
            ),
            "got {changes:?}"
        );
    }

    /// IDX-1: a full reindex must not wipe the index when a previously
    /// indexed file can no longer be read.
    #[cfg(unix)]
    #[test]
    fn full_reindex_with_unreadable_indexed_file_keeps_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::write(root.join("a.ts"), "export const a = 1;\n").unwrap();
        std_fs::write(root.join("b.ts"), "export const b = 1;\n").unwrap();
        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        sync(&mut conn, &project, root).unwrap();
        assert_eq!(repo::count_files(&conn, &project).unwrap(), 2);

        if !deny(&root.join("b.ts")) {
            return;
        }
        let options = discovery_opts();
        let aliases = TsconfigAliases::new();
        let result = apply_via_full_reindex(&mut conn, &ctx(root, &project, &aliases, &options));
        allow(&root.join("b.ts"));

        assert!(result.is_err(), "expected an error, got {result:?}");
        assert_eq!(repo::count_files(&conn, &project).unwrap(), 2);
    }

    /// Task 7.6 (design's open question: "`watch` on macOS FSEvents may
    /// report directory-level events; batch flush must tolerate a directory
    /// path in `pending`"). `notify` can hand `watch::relative_paths` a bare
    /// directory (`"src"`) instead of the individual file that changed
    /// inside it; `detect`'s filter must still resolve that file via
    /// walk-intersection instead of silently dropping the whole batch.
    #[test]
    fn directory_level_filter_entry_resolves_files_underneath_via_walk_intersection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std_fs::create_dir_all(root.join("src")).unwrap();
        std_fs::write(root.join("src/a.ts"), "export const a = 1;\n").unwrap();
        std_fs::write(root.join("top.ts"), "export const b = 1;\n").unwrap();

        let mut conn = db::open_in_memory().unwrap();
        let project = upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        seed_indexed(&mut conn, &project, root, &["src/a.ts", "top.ts"]);

        std_fs::write(root.join("src/a.ts"), "export const a = 2;\n").unwrap();

        // A raw directory path, exactly what a macOS FSEvents batch can
        // hand `watch::relative_paths` — not `src/a.ts` itself.
        let filter: HashSet<String> = ["src".to_string()].into_iter().collect();
        let changes = detect(
            &conn,
            &project,
            root,
            &discovery_opts(),
            Some(&filter),
            false,
        )
        .unwrap();

        assert_eq!(
            changes,
            vec![FileChange::Modified(
                discovery::discover(root, &discovery_opts())
                    .unwrap()
                    .into_iter()
                    .find(|f| f.relative_path == "src/a.ts")
                    .unwrap()
            )]
        );
    }

    /// Task 5.4 (design "Oversized Batch Falls Back to Full Reindex"):
    /// `is_oversized`'s threshold arithmetic with a non-zero tracked count —
    /// the bootstrap branch (`tracked == 0`) is covered separately via
    /// `vertical_slice.rs`. `changed * 100 > tracked * threshold_pct` must
    /// stay false right at the threshold and flip true just past it.
    #[test]
    fn is_oversized_crosses_threshold_with_nonzero_tracked_count() {
        // 100 tracked files, 50% threshold: exactly 50 changed is NOT
        // oversized (50*100 == 100*50, not strictly greater); 51 is.
        assert!(!is_oversized(50, 100, 50));
        assert!(is_oversized(51, 100, 50));

        // Small tracked count, integer-division edge: 3 tracked, 50%
        // threshold -> boundary is 1.5 changed files. 1 stays under, 2 tips
        // over.
        assert!(!is_oversized(1, 3, 50));
        assert!(is_oversized(2, 3, 50));

        // Well below threshold stays incremental; well above falls back.
        assert!(!is_oversized(1, 100, 50));
        assert!(is_oversized(100, 100, 50));
    }

    #[test]
    fn filter_matches_exact_path_and_directory_prefix_only() {
        let filter: HashSet<String> = ["src".to_string()].into_iter().collect();
        assert!(filter_matches(&filter, "src/a.ts"));
        assert!(filter_matches(&filter, "src/nested/b.ts"));
        assert!(!filter_matches(&filter, "src2/a.ts"));
        assert!(!filter_matches(&filter, "other.ts"));

        let exact: HashSet<String> = ["top.ts".to_string()].into_iter().collect();
        assert!(filter_matches(&exact, "top.ts"));
        assert!(!filter_matches(&exact, "top.ts.bak"));
    }
}
