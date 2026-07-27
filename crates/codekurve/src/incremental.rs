//! Phase 3 tasks 5.1-5.4 (design "Technical Approach"): the shared
//! change-detection + apply engine. `index` calls it with `filter = None`
//! (full sweep); `watch` (PR6) will call it per debounced batch with
//! `filter = Some(paths)` — same body either way, so there is exactly one
//! "what changed" implementation and one "apply a batch" implementation.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use codekurve_analysis::discovery::{self, DiscoveredFile, DiscoveryOptions};
use codekurve_analysis::extract;
use codekurve_analysis::ir::{EdgeTarget, FileAnalysis};
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::LanguageId;
use codekurve_store::repo;
use codekurve_store::Connection;

use crate::commands::{self, FileMeta};

/// One file's classification against the shared change-detection engine
/// (design "Interfaces").
#[derive(Debug, Clone, PartialEq)]
pub enum FileChange {
    Created(DiscoveredFile),
    Modified(DiscoveredFile),
    Deleted { relative_path: String },
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
}

/// Task 5.1 (design "Interfaces"): classifies every file the shared engine
/// needs to look at against what's currently stored. `filter = None` is a
/// full sweep (`index`, watch's reconcile-on-start); `filter = Some(paths)`
/// restricts both the walk-derived candidates and the deletion check to a
/// debounced batch (PR6, not exercised by this PR's only caller). Fast
/// path: `(size_bytes, modified_ns)` equal to stored -> unchanged, no read.
/// Otherwise confirm via BLAKE3 `content_hash` before calling it `Modified`
/// (spec "Mtime touch without content change is not a false positive").
pub fn detect(
    conn: &Connection,
    project_id: &str,
    root: &Path,
    opts: &DiscoveryOptions,
    filter: Option<&HashSet<String>>,
) -> Result<Vec<FileChange>, String> {
    let discovered = discovery::discover(root, opts);
    let stored = repo::file_snapshot(conn, project_id).map_err(|e| e.to_string())?;

    let mut changes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for file in discovered {
        if filter.is_some_and(|f| !f.contains(&file.relative_path)) {
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
            continue;
        };
        if prior.content_hash.as_deref() == Some(repo::content_hash(&bytes).as_str()) {
            continue; // touch-only: mtime moved, content didn't
        }
        changes.push(FileChange::Modified(file));
    }

    for path in stored.keys() {
        if seen.contains(path) {
            continue;
        }
        if filter.is_some_and(|f| !f.contains(path)) {
            continue;
        }
        changes.push(FileChange::Deleted {
            relative_path: path.clone(),
        });
    }

    Ok(changes)
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
    if changes.is_empty() {
        return Ok(BatchOutcome::default());
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
    if is_oversized(changes.len(), tracked, ctx.full_reindex_threshold_pct) {
        return apply_via_full_reindex(conn, ctx);
    }

    apply_incremental_changes(conn, ctx, changes, &ts)
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
    let discovered = discovery::discover(ctx.root, ctx.options);

    let mut analyses: Vec<FileAnalysis> = Vec::new();
    let mut file_meta: Vec<FileMeta> = Vec::new();
    for file in &discovered {
        let Ok(source) = fs::read_to_string(&file.absolute_path) else {
            continue;
        };
        let Ok(analysis) = extract::analyze(&source, file.language, &file.relative_path) else {
            continue;
        };
        let meta = fs::metadata(&file.absolute_path).map_err(|e| e.to_string())?;
        file_meta.push(FileMeta {
            language: file.language,
            size_bytes: source.len() as u64,
            content_hash: repo::content_hash(source.as_bytes()),
            modified_ns: mtime_ns(&meta)?,
        });
        analyses.push(analysis);
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
    let mut analyses: Vec<FileAnalysis> = Vec::new();
    let mut file_meta: Vec<FileMeta> = Vec::new();
    for file in &created_or_modified {
        let source = fs::read_to_string(&file.absolute_path).map_err(|e| e.to_string())?;
        let analysis = extract::analyze(&source, file.language, &file.relative_path)
            .map_err(|e| e.to_string())?;
        let meta = fs::metadata(&file.absolute_path).map_err(|e| e.to_string())?;
        file_meta.push(FileMeta {
            language: file.language,
            size_bytes: source.len() as u64,
            content_hash: repo::content_hash(source.as_bytes()),
            modified_ns: mtime_ns(&meta)?,
        });
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
    dependents.retain(|p| !b_paths.contains(p) && !deleted_set.contains(p));

    let mut d_paths: Vec<String> = dependents.into_iter().collect();
    d_paths.sort();
    for path in &d_paths {
        let absolute = ctx.root.join(path);
        // Vanished since it was last indexed (rare race); skip this cycle,
        // it'll be reconsidered on the next sweep.
        let Ok(source) = fs::read_to_string(&absolute) else {
            continue;
        };
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let Some(language) = LanguageId::from_extension(ext) else {
            continue;
        };
        let Ok(analysis) = extract::analyze(&source, language, path) else {
            continue;
        };
        let meta = fs::metadata(&absolute).map_err(|e| e.to_string())?;
        file_meta.push(FileMeta {
            language,
            size_bytes: source.len() as u64,
            content_hash: repo::content_hash(source.as_bytes()),
            modified_ns: mtime_ns(&meta)?,
        });
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
            languages: vec![LanguageId::TypeScript],
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

        let changes = detect(&conn, &project, root, &discovery_opts(), None).unwrap();

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
        // Exactly touched (skipped) excluded: changed + added + removed = 3.
        assert_eq!(changes.len(), 3);
    }
}
