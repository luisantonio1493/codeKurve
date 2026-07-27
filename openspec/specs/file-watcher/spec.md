# File Watcher Specification

## Purpose

Add `codekurve watch`: a foreground-blocking command built on `notify` that debounces filesystem events into batches and applies them through the same change-detection engine `codekurve index` uses, reconciling any changes missed while it was not running before it starts watching (proposal §Intent, §23.2-23.3).

## Requirements

### Requirement: Watch Is Foreground-Blocking, Not a Daemon

`codekurve watch` MUST run in the foreground, blocking the invoking terminal until interrupted. It MUST NOT detach, background itself, write a PID file, or expose any service/daemon management surface.

#### Scenario: Watch blocks the terminal

- GIVEN a user runs `codekurve watch`
- WHEN the command starts
- THEN it does not return control to the shell; it keeps running and processing events until interrupted (e.g. Ctrl+C)

#### Scenario: No daemon artifacts

- GIVEN `codekurve watch` is running
- WHEN inspecting the process and filesystem
- THEN there is no PID file, no background/detached process, and no separate service to start/stop/query

### Requirement: Reconcile on Start

Before entering its event loop, `codekurve watch` MUST run the shared change-detection engine once over the whole project to catch changes made while the watcher was not running, applying any resulting batch(es) before watching for new filesystem events.

#### Scenario: Changes made while watcher was stopped are caught

- GIVEN two files were edited after the last `codekurve index`/`watch` run, while no watcher was running
- WHEN `codekurve watch` starts
- THEN it detects and applies both changes as an initial reconciliation batch before it begins watching for further events

### Requirement: Debounce Coalesces a Burst Into One Batch

Filesystem events MUST be debounced: multiple events, including on different paths, that arrive within one shared quiet window MUST be coalesced into a single batch and applied once change detection settles, rather than triggering one batch per event or per file.

#### Scenario: Bulk checkout coalesces into one batch

- GIVEN a `git checkout` touches 50 files in immediate succession
- WHEN `codekurve watch` observes the resulting filesystem events
- THEN all 50 files are coalesced into one debounced batch and applied as a single incremental update, not 50 separate batches

#### Scenario: Quiet window resets on continued activity

- GIVEN events keep arriving on different paths inside the quiet window
- WHEN the window would otherwise elapse
- THEN the window extends so the whole burst is still captured in one batch, and the batch is only applied once events stop arriving for the configured quiet duration

### Requirement: Watch Applies Batches Through the Shared Incremental Engine

`codekurve watch` MUST apply each debounced batch using the same per-file create/update/delete, per-batch-transaction, and freshness-metadata behavior defined by the `incremental-index` capability — it MUST NOT implement a separate apply path.

#### Scenario: Watch delete behaves identically to index delete

- GIVEN a file tracked by the index is deleted while `codekurve watch` is running
- WHEN the debounce window elapses and the batch is applied
- THEN the deleted file's symbols are removed and any inbound edges become unresolved, exactly as specified for `codekurve index`'s per-file delete

#### Scenario: Interrupting the watcher mid-batch leaves a consistent index

- GIVEN `codekurve watch` is applying a debounced batch
- WHEN the process is interrupted (Ctrl+C) before the batch's transaction commits
- THEN the index reflects the state before the batch started, and the batch's files remain pending, per the `incremental-index` batch-interruption requirement
