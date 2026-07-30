//! Phase 3 task 6.2 (design "Data Flow — watch" + "Debounce"): `notify`
//! setup plus a hand-rolled sliding-window + hard-cap debounce loop. Every
//! flushed batch reuses [`crate::incremental::detect`]/`apply_batch` — the
//! same engine `index` uses — so there is exactly one "what changed" and one
//! "apply a batch" implementation (design "Technical Approach").

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Event, RecursiveMode, Watcher};

use crate::commands::{self, IndexSetup};
use crate::incremental::{self, IndexContext};

/// `codekurve watch --root <path> [--debounce-ms <n>]` (task 6.4): reconciles
/// once via a full-sweep `detect` (same as a plain `index` run — catches
/// anything that changed while the watcher wasn't running), then starts the
/// `notify` watcher and blocks on the debounce loop forever.
pub fn run(root: &Path, debounce_ms_override: Option<u64>) -> Result<(), String> {
    let mut setup = commands::setup_index(root)?;
    reconcile(&mut setup)?;

    let debounce_ms = debounce_ms_override.unwrap_or(setup.config.index.watch.debounce_ms);
    let max_batch_wait_ms = setup.config.index.watch.max_batch_wait_ms;

    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            let _ = tx.send(event.paths);
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&setup.root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;

    println!(
        "watching {} (debounce {}ms, max wait {}ms)",
        setup.root.display(),
        debounce_ms,
        max_batch_wait_ms
    );

    debounce_loop(
        &rx,
        Duration::from_millis(debounce_ms),
        Duration::from_millis(max_batch_wait_ms),
        |paths| apply_flush(&mut setup, paths),
    )
}

fn reconcile(setup: &mut IndexSetup) -> Result<(), String> {
    let changes = incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        None,
    )?;
    if changes.is_empty() {
        return Ok(());
    }
    let ctx = IndexContext {
        root: &setup.root,
        project_id: &setup.project_id,
        aliases: &setup.aliases,
        options: &setup.options,
        full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
    };
    let outcome = incremental::apply_batch(&mut setup.conn, &ctx, &changes)?;
    println!(
        "reconciled {} file(s) changed, {} deleted{}",
        outcome.files_changed,
        outcome.files_deleted,
        if outcome.fell_back_to_full_reindex {
            " (full reindex)"
        } else {
            ""
        }
    );
    Ok(())
}

/// One debounced batch: `detect` restricted to the flushed paths, then
/// `apply_batch`. Most errors are logged, not fatal — the watcher keeps
/// running so the next batch (or `pending_files` staying nonzero) can
/// recover. `max_total_files` is the one exception (Phase 6, design risk
/// #4): a project that has genuinely grown past the configured cap will
/// hit the exact same error on every future batch too, so silently
/// logging-and-continuing forever would leave the index falling further
/// behind with no clear signal — this stops the watcher instead, matching
/// startup [`reconcile`]'s fatal behavior for the same condition.
fn apply_flush(setup: &mut IndexSetup, paths: &HashSet<PathBuf>) -> Result<(), String> {
    let filter = relative_paths(&setup.root, paths);
    if filter.is_empty() {
        return Ok(());
    }
    let changes = match incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        Some(&filter),
    ) {
        Ok(changes) => changes,
        Err(e) if e.contains("max_total_files") => return Err(e),
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(());
        }
    };
    if changes.is_empty() {
        return Ok(());
    }
    let ctx = IndexContext {
        root: &setup.root,
        project_id: &setup.project_id,
        aliases: &setup.aliases,
        options: &setup.options,
        full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
    };
    match incremental::apply_batch(&mut setup.conn, &ctx, &changes) {
        Ok(outcome) => println!(
            "indexed {} file(s) changed, {} deleted{}",
            outcome.files_changed,
            outcome.files_deleted,
            if outcome.fell_back_to_full_reindex {
                " (full reindex)"
            } else {
                ""
            }
        ),
        Err(e) => eprintln!("error: {e}"),
    }
    Ok(())
}

/// Absolute event paths -> `discovery`-style relative slash paths, dropping
/// anything outside `root`. A raw directory path (macOS FSEvents) survives
/// this conversion as a directory-relative path; `incremental::detect`'s
/// `filter_matches` treats it as a slash-prefix match against every file
/// underneath it (task 7.6, design "walk-intersection").
fn relative_paths(root: &Path, paths: &HashSet<PathBuf>) -> HashSet<String> {
    paths
        .iter()
        .filter_map(|p| {
            let relative = p.strip_prefix(root).ok()?;
            let parts: Vec<String> = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("/"))
            }
        })
        .collect()
}

/// Task 6.2 (design "Debounce"): sliding quiet-window `debounce`, hard cap
/// `max_batch_wait` since the batch's first pending event. Blocks until `rx`
/// disconnects or `flush` returns a fatal error. `flush` runs on the same
/// thread — batches are never applied concurrently.
fn debounce_loop<F: FnMut(&HashSet<PathBuf>) -> Result<(), String>>(
    rx: &Receiver<Vec<PathBuf>>,
    debounce: Duration,
    max_batch_wait: Duration,
    mut flush: F,
) -> Result<(), String> {
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;

    loop {
        let now = Instant::now();
        let deadline = match (first, last) {
            (Some(f), Some(l)) => (l + debounce).min(f + max_batch_wait),
            _ => now + debounce,
        };
        let timeout = deadline.saturating_duration_since(now);

        match rx.recv_timeout(timeout) {
            Ok(paths) => {
                let now = Instant::now();
                pending.extend(paths);
                first.get_or_insert(now);
                last = Some(now);
            }
            Err(RecvTimeoutError::Timeout) => {
                if !pending.is_empty() {
                    flush(&pending)?;
                    pending.clear();
                }
                first = None;
                last = None;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// Task 6.5: a burst of events fired milliseconds apart (each one resets
    /// the sliding window) coalesces into exactly one flush, containing
    /// every path from the burst — mirrors the design's "50-file git
    /// checkout -> one batch" example, no filesystem involved.
    #[test]
    fn burst_coalesces_into_one_batch() {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let flushes: Arc<Mutex<Vec<HashSet<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
        let flushes_clone = flushes.clone();

        let handle = thread::spawn(move || {
            debounce_loop(
                &rx,
                Duration::from_millis(30),
                Duration::from_millis(1000),
                |paths| {
                    flushes_clone.lock().unwrap().push(paths.clone());
                    Ok(())
                },
            )
            .unwrap();
        });

        for i in 0..5 {
            tx.send(vec![PathBuf::from(format!("file{i}.ts"))]).unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        // Let the 30ms sliding window elapse and flush before disconnecting
        // — otherwise the channel closing races the debounce timeout and
        // the loop can exit via `Disconnected` before ever flushing.
        thread::sleep(Duration::from_millis(60));
        drop(tx);

        handle.join().unwrap();

        let flushes = flushes.lock().unwrap();
        assert_eq!(flushes.len(), 1, "burst must coalesce into one batch");
        assert_eq!(flushes[0].len(), 5);
    }

    /// Task 6.5: under continuous events (each one arriving before the
    /// sliding window elapses), the hard `max_batch_wait` cap still forces a
    /// flush instead of starving forever (design "Pure sliding starves under
    /// a continuously-writing process").
    #[test]
    fn max_batch_wait_caps_continuous_events() {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let flushes: Arc<Mutex<Vec<HashSet<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
        let flushes_clone = flushes.clone();

        let handle = thread::spawn(move || {
            debounce_loop(
                &rx,
                Duration::from_millis(50),
                Duration::from_millis(120),
                |paths| {
                    flushes_clone.lock().unwrap().push(paths.clone());
                    Ok(())
                },
            )
            .unwrap();
        });

        // Send an event every 20ms (< 50ms debounce, so the sliding window
        // never elapses on its own) for 300ms — long enough to blow past
        // the 120ms hard cap at least once.
        let start = Instant::now();
        let mut i = 0;
        while start.elapsed() < Duration::from_millis(300) {
            tx.send(vec![PathBuf::from(format!("file{i}.ts"))]).unwrap();
            i += 1;
            thread::sleep(Duration::from_millis(20));
        }
        drop(tx);

        handle.join().unwrap();

        let flushes = flushes.lock().unwrap();
        assert!(
            flushes.len() >= 2,
            "max_batch_wait must force at least one flush before the sender stops, got {}",
            flushes.len()
        );
    }

    /// Phase 6, design risk #4 (WARNING follow-up): a fatal `flush` error
    /// (e.g. `max_total_files` exceeded) stops the loop immediately instead
    /// of being logged-and-continued, and is propagated to the caller —
    /// `apply_flush` inherits this by returning `Err` only for that one
    /// condition (see its doc comment).
    #[test]
    fn fatal_flush_error_stops_the_loop_and_is_propagated() {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let flush_count = Arc::new(Mutex::new(0u32));
        let flush_count_clone = flush_count.clone();

        let handle = thread::spawn(move || {
            debounce_loop(
                &rx,
                Duration::from_millis(10),
                Duration::from_millis(1000),
                |_paths| {
                    *flush_count_clone.lock().unwrap() += 1;
                    Err("project exceeds index.max_total_files (2)".to_string())
                },
            )
        });

        tx.send(vec![PathBuf::from("a.ts")]).unwrap();
        // Second batch, sent after the fatal error should already have
        // stopped the loop — must never reach `flush`.
        thread::sleep(Duration::from_millis(50));
        let _ = tx.send(vec![PathBuf::from("b.ts")]);

        let result = handle.join().unwrap();
        assert!(result.is_err(), "fatal flush error must propagate");
        assert_eq!(
            *flush_count.lock().unwrap(),
            1,
            "loop must stop after the first fatal flush, not keep retrying"
        );
    }
}
