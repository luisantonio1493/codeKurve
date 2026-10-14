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

    // Register the watcher BEFORE reconciling: events that fire while the
    // (possibly long) reconcile runs queue in the channel and are drained by
    // the debounce loop, instead of falling into the gap between the two.
    let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
    let event_root = setup.root.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let paths = event_paths(&event_root, res);
        if !paths.is_empty() {
            let _ = tx.send(paths);
        }
    })
    .map_err(|e| e.to_string())?;
    watcher
        .watch(&setup.root, RecursiveMode::Recursive)
        .map_err(|e| e.to_string())?;

    reconcile(&mut setup)?;

    let debounce_ms = debounce_ms_override.unwrap_or(setup.config.index.watch.debounce_ms);
    let max_batch_wait_ms = setup.config.index.watch.max_batch_wait_ms;

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

/// Outcome of one flush: `Retry` keeps the paths pending (with backoff)
/// instead of forgetting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flush {
    Done,
    Retry,
}

/// Failed flushes are retried this many times (exponential backoff capped at
/// `max_batch_wait`), then once more as a full sweep, then abandoned with a
/// message; `pending_files` stays non-zero so status keeps saying "stale".
const MAX_RETRIES: u32 = 5;

/// Marker path in a pending set meaning "sweep the whole project".
fn full_sweep_marker() -> PathBuf {
    PathBuf::new()
}

/// Paths to queue for one `notify` result. A watcher error or a rescan
/// request (events were dropped) cannot name what changed, so it asks for a
/// full sweep by queueing the root.
fn event_paths(root: &Path, res: notify::Result<Event>) -> Vec<PathBuf> {
    match res {
        Ok(event) if event.need_rescan() => vec![root.to_path_buf()],
        Ok(event) => event.paths,
        Err(err) => {
            eprintln!("warning: watcher error ({err}); scheduling a full sweep");
            vec![root.to_path_buf()]
        }
    }
}

/// An event on the root itself (or the full-sweep marker) means "look at
/// everything".
fn needs_full_sweep(root: &Path, paths: &HashSet<PathBuf>) -> bool {
    paths.iter().any(|p| p == root || p.as_os_str().is_empty())
}

fn reconcile(setup: &mut IndexSetup) -> Result<(), String> {
    let fingerprint = commands::analysis_fingerprint(&setup.config, &setup.aliases);
    let force_full =
        commands::analyzer_version_changed(&setup.conn, &setup.project_id, &fingerprint)?;
    let changes = incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        None,
        force_full,
    )?;
    if changes.is_empty() {
        if force_full {
            codekurve_store::repo::set_analyzer_version(
                &setup.conn,
                &setup.project_id,
                &fingerprint,
            )
            .map_err(|e| e.to_string())?;
        }
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
    codekurve_store::repo::set_analyzer_version(&setup.conn, &setup.project_id, &fingerprint)
        .map_err(|e| e.to_string())?;
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
fn apply_flush(setup: &mut IndexSetup, paths: &HashSet<PathBuf>) -> Result<Flush, String> {
    let sweep = needs_full_sweep(&setup.root, paths);
    let filter = relative_paths(&setup.root, paths);
    if !sweep && filter.is_empty() {
        return Ok(Flush::Done);
    }
    let changes = match incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        if sweep { None } else { Some(&filter) },
        false,
    ) {
        Ok(changes) => changes,
        Err(e) if e.contains("max_total_files") => return Err(e),
        Err(e) => {
            eprintln!("error: {e}");
            return Ok(Flush::Retry);
        }
    };
    if changes.is_empty() {
        return Ok(Flush::Done);
    }
    let ctx = IndexContext {
        root: &setup.root,
        project_id: &setup.project_id,
        aliases: &setup.aliases,
        options: &setup.options,
        full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
    };
    match incremental::apply_batch(&mut setup.conn, &ctx, &changes) {
        Ok(outcome) => {
            println!(
                "indexed {} file(s) changed, {} deleted{}",
                outcome.files_changed,
                outcome.files_deleted,
                if outcome.fell_back_to_full_reindex {
                    " (full reindex)"
                } else {
                    ""
                }
            );
            Ok(Flush::Done)
        }
        Err(e) => {
            eprintln!("error: {e}");
            Ok(Flush::Retry)
        }
    }
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
fn debounce_loop<F: FnMut(&HashSet<PathBuf>) -> Result<Flush, String>>(
    rx: &Receiver<Vec<PathBuf>>,
    debounce: Duration,
    max_batch_wait: Duration,
    mut flush: F,
) -> Result<(), String> {
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;
    let mut retries: u32 = 0;
    let mut not_before: Option<Instant> = None;

    loop {
        let now = Instant::now();
        let mut deadline = match (first, last) {
            (Some(f), Some(l)) => (l + debounce).min(f + max_batch_wait),
            _ => now + debounce,
        };
        if let Some(retry_at) = not_before {
            deadline = deadline.max(retry_at);
        }
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
                    match flush(&pending)? {
                        Flush::Done => {
                            pending.clear();
                            retries = 0;
                            not_before = None;
                        }
                        Flush::Retry => {
                            retries += 1;
                            if retries > MAX_RETRIES {
                                eprintln!(
                                    "error: giving up after {MAX_RETRIES} retries; \
                                     run `codekurve index` to reconcile"
                                );
                                pending.clear();
                                retries = 0;
                                not_before = None;
                            } else {
                                if retries == MAX_RETRIES {
                                    // Last resort: the specific paths keep
                                    // failing, so sweep everything once.
                                    pending = HashSet::from([full_sweep_marker()]);
                                }
                                let backoff = debounce
                                    .saturating_mul(1u32 << retries.min(16))
                                    .min(max_batch_wait);
                                not_before = Some(Instant::now() + backoff);
                            }
                        }
                    }
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
                Duration::from_millis(100),
                Duration::from_millis(1000),
                |paths| {
                    flushes_clone.lock().unwrap().push(paths.clone());
                    Ok(Flush::Done)
                },
            )
            .unwrap();
        });

        // 5ms between sends against a 100ms sliding window — wide margin so
        // CI scheduling jitter between two sends can't exceed the window and
        // force a premature flush mid-burst (seen with a tighter 30ms window).
        for i in 0..5 {
            tx.send(vec![PathBuf::from(format!("file{i}.ts"))]).unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        // Poll for the flush instead of a fixed sleep: a hardcoded wait races
        // the debounce timeout under CI scheduling jitter (seen failing both
        // as "too early, got 0" and "too late, split into 2" on macOS
        // runners) — waiting on the actual condition has no such ceiling.
        let deadline = Instant::now() + Duration::from_secs(5);
        while flushes.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
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
                    Ok(Flush::Done)
                },
            )
            .unwrap();
        });

        // Send an event every 20ms (< 50ms debounce, so the sliding window
        // never elapses on its own) for 600ms — long enough to blow past
        // the 120ms hard cap several times even under CI scheduling jitter
        // (a 300ms window proved flaky on loaded macOS runners: only 1 flush).
        let start = Instant::now();
        let mut i = 0;
        while start.elapsed() < Duration::from_millis(600) {
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

    /// IDX-5: a flush that fails is retried with the same paths merged with
    /// whatever arrived since, instead of being forgotten.
    #[test]
    fn failed_flush_is_retried_with_the_same_paths() {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let flushes: Arc<Mutex<Vec<HashSet<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
        let flushes_clone = flushes.clone();

        let handle = thread::spawn(move || {
            debounce_loop(
                &rx,
                Duration::from_millis(10),
                Duration::from_millis(50),
                |paths| {
                    let mut seen = flushes_clone.lock().unwrap();
                    seen.push(paths.clone());
                    Ok(if seen.len() == 1 {
                        Flush::Retry
                    } else {
                        Flush::Done
                    })
                },
            )
            .unwrap();
        });

        tx.send(vec![PathBuf::from("a.ts")]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while flushes.lock().unwrap().len() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        drop(tx);
        handle.join().unwrap();

        let flushes = flushes.lock().unwrap();
        assert_eq!(flushes.len(), 2, "one failure, one successful retry");
        assert_eq!(flushes[0], flushes[1]);
    }

    /// IDX-5: retries are bounded; after `MAX_RETRIES` the loop widens to a
    /// full sweep once and then gives up instead of spinning forever.
    #[test]
    fn retries_are_bounded() {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let count = Arc::new(Mutex::new(0usize));
        let count_clone = count.clone();

        let handle = thread::spawn(move || {
            debounce_loop(
                &rx,
                Duration::from_millis(5),
                Duration::from_millis(20),
                |_paths| {
                    *count_clone.lock().unwrap() += 1;
                    Ok(Flush::Retry)
                },
            )
            .unwrap();
        });

        tx.send(vec![PathBuf::from("a.ts")]).unwrap();
        thread::sleep(Duration::from_millis(1500));
        drop(tx);
        handle.join().unwrap();

        let calls = *count.lock().unwrap();
        assert!(
            calls <= MAX_RETRIES as usize + 2,
            "loop kept retrying: {calls} flushes"
        );
        assert!(calls > 1, "failure must be retried at least once");
    }

    /// IDX-4: a watcher error or a rescan request becomes a full sweep
    /// instead of vanishing.
    #[test]
    fn watcher_errors_and_rescan_flags_request_a_full_sweep() {
        let root = Path::new("/proj");

        let err = event_paths(root, Err(notify::Error::generic("overflow")));
        assert!(needs_full_sweep(root, &err.into_iter().collect()));

        let rescan = Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/proj/a.ts"))
            .set_flag(notify::event::Flag::Rescan);
        let paths = event_paths(root, Ok(rescan));
        assert!(needs_full_sweep(root, &paths.into_iter().collect()));

        let plain = Event::new(notify::EventKind::Any).add_path(PathBuf::from("/proj/a.ts"));
        let paths: HashSet<PathBuf> = event_paths(root, Ok(plain)).into_iter().collect();
        assert!(!needs_full_sweep(root, &paths));
        assert_eq!(paths, HashSet::from([PathBuf::from("/proj/a.ts")]));
    }

    /// IDX-4: an event on the root itself is a full sweep, not a dropped path.
    #[test]
    fn event_on_the_root_is_a_full_sweep() {
        let root = Path::new("/proj");
        let paths = HashSet::from([PathBuf::from("/proj")]);
        assert!(needs_full_sweep(root, &paths));
    }
}
