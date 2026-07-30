//! Command implementations. The binary is the composition root (§11.2): it
//! loads config, drives discovery + extraction (`codekurve-analysis`), and
//! persists/queries through `codekurve-store`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use codekurve_analysis::discovery::DiscoveryOptions;
use codekurve_analysis::ir::{EdgeTarget, FileAnalysis};
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Confidence, Config, LanguageId, SourceSpan, Symbol, SymbolKind};
use codekurve_store::db;
use codekurve_store::repo::{
    self, FileInput, RelationshipInput, StoredSymbol, UnresolvedReferenceInput,
};
use codekurve_store::{traverse, Connection};

use crate::query;

/// Structured CLI error (§27, spec "Ambiguous name lookup"/"Query before
/// first index"): code 1 = generic failure — `From<String>` keeps every
/// pre-PR5b command (`index`/`search`/`symbol`/`doctor`) working unchanged;
/// code 4 = no completed index run; code 6 = ambiguous bare-name lookup.
/// Lands in `commands.rs` only, per design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub code: u8,
    pub message: String,
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self { code: 1, message }
    }
}

/// Everything a project-rooted command (`index`, `watch`) needs before it
/// can call the shared `incremental` engine — canonicalized root, parsed
/// config, an open connection, and the project row. Factored out of `index`
/// (PR6) so `watch`'s reconcile-on-start and event-loop batches share one
/// setup path instead of duplicating it.
pub(crate) struct IndexSetup {
    pub root: PathBuf,
    pub config: Config,
    pub conn: Connection,
    pub project_id: String,
    pub options: DiscoveryOptions,
    pub aliases: TsconfigAliases,
}

pub(crate) fn setup_index(root: &Path) -> Result<IndexSetup, String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;
    let options = discovery_options(&config);
    let aliases = load_tsconfig_aliases(&root);

    let db_path = root.join(&config.storage.database);
    let conn = db::open(&db_path).map_err(|e| e.to_string())?;
    let config_text = config.to_toml().map_err(|e| e.to_string())?;
    let project_id = repo::upsert_project(
        &conn,
        &config.project.name,
        &root.to_string_lossy(),
        &repo::config_hash(&config_text),
    )
    .map_err(|e| e.to_string())?;

    Ok(IndexSetup {
        root,
        config,
        conn,
        project_id,
        options,
        aliases,
    })
}

/// `codekurve index --root <path>` (task 5.5, design's shared engine):
/// delegates to [`crate::incremental::detect`] then
/// [`crate::incremental::apply_batch`] instead of always running a full
/// reindex. On a never-indexed project every discovered file is `Created`
/// and the oversized-batch fallback (task 5.4) naturally takes the full
/// [`repo::reindex`] path — no separate bootstrap case needed.
pub fn index(root: &Path) -> Result<(), String> {
    let mut setup = setup_index(root)?;

    let changes = crate::incremental::detect(
        &setup.conn,
        &setup.project_id,
        &setup.root,
        &setup.options,
        None,
    )?;
    if changes.is_empty() {
        println!("index up to date, no changes detected");
        return Ok(());
    }

    let ctx = crate::incremental::IndexContext {
        root: &setup.root,
        project_id: &setup.project_id,
        aliases: &setup.aliases,
        options: &setup.options,
        full_reindex_threshold_pct: setup.config.index.watch.full_reindex_threshold_pct,
    };
    let outcome = crate::incremental::apply_batch(&mut setup.conn, &ctx, &changes)?;

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
    Ok(())
}

/// `codekurve status --root <path> [--json]` (task 7.1, design "`codekurve
/// status` and Stale Warning"): counts plus stored freshness metadata, read
/// without a filesystem walk (spec "Status Command Reports Pending Count and
/// Last Verified Time").
pub fn status(root: &Path, json: bool) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;
    let conn = open_existing_db(&root, &config)?;
    let pid = project_id(&conn, &root)?;

    let s = repo::index_status(&conn, &pid).map_err(|e| e.to_string())?;
    let schema_version =
        codekurve_store::migrations::current_version(&conn).map_err(|e| e.to_string())?;
    let stale = s.pending_files > 0;

    if json {
        let result = serde_json::json!({
            "schema_version": schema_version,
            "files": s.files,
            "symbols": s.symbols,
            "relationships": s.relationships,
            "relationships_unresolved": s.relationships_unresolved,
            "pending_files": s.pending_files,
            "last_verified_at": s.last_verified_at,
            "stale": stale,
        });
        print_envelope(&config.project.name, result, Vec::new(), false);
    } else {
        println!("project: {}", config.project.name);
        println!(
            "database: {} (schema {schema_version})",
            config.storage.database
        );
        println!(
            "files: {}   symbols: {}   relationships: {} ({} unresolved)",
            s.files, s.symbols, s.relationships, s.relationships_unresolved
        );
        println!(
            "last verified: {}",
            s.last_verified_at.as_deref().unwrap_or("never")
        );
        println!("pending files: {}", s.pending_files);
        println!("status: {}", if stale { "stale" } else { "fresh" });
    }
    Ok(())
}

/// Task 7.2 (design "`codekurve status` and Stale Warning"): prints exactly
/// one stderr warning when stored freshness metadata shows pending files,
/// without a filesystem walk. Called from [`require_indexed_project`] (the
/// six graph commands) plus one line each in [`search`]/[`symbol`] — stdout,
/// JSON, and exit codes are never touched (spec "Stale Index Warning on
/// Stderr").
fn warn_if_stale(conn: &Connection, project_id: &str) {
    for w in query::pending_warning(conn, project_id) {
        eprintln!("warning: {w}");
    }
}

/// Per-file metadata `build_file_inputs` needs alongside each `FileAnalysis`
/// — extended in Phase 3 with the change-detection engine's hash/mtime
/// (design "Content Hash Tracked Per File").
pub(crate) struct FileMeta {
    pub language: LanguageId,
    pub size_bytes: u64,
    pub content_hash: String,
    pub modified_ns: i64,
}

/// A symbol standing in for "this module", one per file. `Imports`/`Exports`
/// edges are sourced at the whole-file level, not a symbol
/// (`extract::collect_imports`/`collect_exports`) — but
/// `relationships.source_symbol_id` is `NOT NULL` (§24.2), and no dedicated
/// `Module` symbol is emitted by `extract::analyze` yet (a documented PR4a
/// gap). Registering one here, keyed by the file's own relative path (the
/// same string `source_local_key`/`target_text` already use for file-level
/// edges), closes that gap without touching the analysis crate.
fn module_symbol(relative_path: &str, language: LanguageId) -> Symbol {
    Symbol {
        name: relative_path.to_string(),
        qualified_name: relative_path.to_string(),
        kind: SymbolKind::Module,
        language,
        span: SourceSpan {
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: 0,
        },
        parent: None,
        // A module stand-in has no call signature.
        signature_fingerprint: String::new(),
        visibility: codekurve_core::Visibility::Default,
        is_partial: false,
        is_record: false,
        partial_ordinal: None,
    }
}

/// Builds `repo::FileInput`s and a `(relative_path, lookup_key) ->
/// symbol_id` table, using the same deterministic id functions
/// `repo::reindex` uses internally — so ids computed here match exactly
/// what ends up in the `symbols` table. `pub(crate)`: shared with
/// `crate::incremental`'s per-batch and full-reindex-fallback paths.
///
/// Lookup keys are each symbol's `local_key` (so C# `partial` fragments and
/// fingerprint-disambiguated overloads attribute `Contains`/`Inherits`
/// sources correctly) and, when distinct, `qualified_name` (so
/// `EdgeTarget::Global` resolution still hits). First writer wins on a
/// shared `qualified_name`.
pub(crate) fn build_file_inputs(
    project_id: &str,
    analyses: &[FileAnalysis],
    file_meta: &[FileMeta],
) -> (Vec<FileInput>, HashMap<(String, String), String>) {
    let mut files = Vec::with_capacity(analyses.len());
    let mut symbol_ids: HashMap<(String, String), String> = HashMap::new();

    for (analysis, meta) in analyses.iter().zip(file_meta) {
        let file_id = repo::file_id(project_id, &analysis.file);
        let mut symbols: Vec<Symbol> = analysis
            .symbols
            .iter()
            .map(|s| Symbol {
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind,
                language: s.language,
                span: s.span,
                parent: s.parent.clone(),
                signature_fingerprint: s.signature_fingerprint.clone(),
                visibility: s.visibility,
                is_partial: s.is_partial,
                is_record: s.is_record,
                partial_ordinal: s.partial_ordinal,
            })
            .collect();
        symbols.push(module_symbol(&analysis.file, meta.language));

        for extracted in &analysis.symbols {
            let key = repo::symbol_key(
                meta.language.as_str(),
                &analysis.file,
                extracted.kind.as_str(),
                &extracted.qualified_name,
                &extracted.signature_fingerprint,
                extracted.partial_ordinal,
            );
            let id = repo::symbol_id(&file_id, &key);
            symbol_ids.insert((analysis.file.clone(), extracted.local_key.clone()), id.clone());
            symbol_ids
                .entry((analysis.file.clone(), extracted.qualified_name.clone()))
                .or_insert(id);
        }
        // Module stand-in: edges key the file by its relative path.
        {
            let module = symbols
                .last()
                .expect("module stand-in pushed immediately above");
            let key = repo::symbol_key(
                meta.language.as_str(),
                &analysis.file,
                module.kind.as_str(),
                &module.qualified_name,
                &module.signature_fingerprint,
                module.partial_ordinal,
            );
            symbol_ids.insert(
                (analysis.file.clone(), analysis.file.clone()),
                repo::symbol_id(&file_id, &key),
            );
        }

        files.push(FileInput {
            relative_path: analysis.file.clone(),
            language: meta.language.as_str().to_string(),
            size_bytes: meta.size_bytes,
            content_hash: meta.content_hash.clone(),
            modified_ns: meta.modified_ns,
            symbols,
        });
    }

    (files, symbol_ids)
}

/// Maps every resolved `ExtractedRelationship` to a `RelationshipInput`.
/// After `resolve::resolve`, every edge's target is `Local`, `Global`, or
/// `External` (never `Unresolved` — zero-candidate edges moved to
/// `analysis.unresolved` instead, §18.3). `pub(crate)`: shared with
/// `crate::incremental`.
pub(crate) fn build_relationships(
    project_id: &str,
    analyses: &[FileAnalysis],
    symbol_ids: &HashMap<(String, String), String>,
) -> Vec<RelationshipInput> {
    let mut out = Vec::new();
    for analysis in analyses {
        let file_id = repo::file_id(project_id, &analysis.file);
        for rel in &analysis.relationships {
            let Some(source_symbol_id) = symbol_ids
                .get(&(analysis.file.clone(), rel.source_local_key.clone()))
                .cloned()
            else {
                // Post-resolution invariant: every source is either a real
                // symbol or the file's own module symbol. A miss here would
                // be a resolver bug, not a legitimate unresolved case (those
                // already moved to `analysis.unresolved`) — skip defensively
                // rather than violate the NOT NULL `source_symbol_id` column.
                continue;
            };
            let (target_symbol_id, target_external) = match &rel.target {
                EdgeTarget::External(pkg) => (None, Some(pkg.clone())),
                EdgeTarget::Local(key) => (
                    symbol_ids
                        .get(&(analysis.file.clone(), key.clone()))
                        .cloned(),
                    None,
                ),
                EdgeTarget::Global {
                    file,
                    qualified_name,
                } => (
                    symbol_ids
                        .get(&(file.clone(), qualified_name.clone()))
                        .cloned(),
                    None,
                ),
                // Never emitted post-`resolve()`; kept for exhaustiveness. A
                // whole-module placeholder (namespace import/`export *`,
                // `qualified_name == "*"`) also lands here as a no-target
                // row — no `Module`-member id exists to look up yet
                // (documented PR4a gap).
                EdgeTarget::Unresolved(_) => (None, None),
            };
            out.push(RelationshipInput {
                source_symbol_id,
                target_symbol_id,
                target_external,
                kind: rel.kind,
                provenance: rel.provenance,
                confidence: rel.confidence,
                source_file_id: file_id.clone(),
                start_line: Some(rel.span.start_line as u32),
                start_column: Some(rel.span.start_column as u32),
                reason: rel.reason.clone(),
            });
        }
    }
    out
}

/// Maps every `UnresolvedReference` left after resolution (never dropped,
/// §18.3) to an `UnresolvedReferenceInput`. `pub(crate)`: shared with
/// `crate::incremental`.
pub(crate) fn build_unresolved(
    project_id: &str,
    analyses: &[FileAnalysis],
    symbol_ids: &HashMap<(String, String), String>,
) -> Vec<UnresolvedReferenceInput> {
    let mut out = Vec::new();
    for analysis in analyses {
        let file_id = repo::file_id(project_id, &analysis.file);
        for u in &analysis.unresolved {
            let source_symbol_id = symbol_ids
                .get(&(analysis.file.clone(), u.source_local_key.clone()))
                .cloned();
            out.push(UnresolvedReferenceInput {
                source_symbol_id,
                source_file_id: file_id.clone(),
                relationship_kind: u.relationship_kind,
                target_text: u.target_text.clone(),
                context_json: u.context.clone(),
                candidate_count: u.candidate_count as u32,
                reason: u.reason.clone(),
                confidence: u.confidence,
            });
        }
    }
    out
}

/// Composition-root mapping (task 4.4, design "Baseline for
/// re-resolution"): `codekurve-store::repo::resolution_snapshot`'s rows ->
/// `codekurve-analysis::resolve::ProjectBaseline`. `codekurve-store` never
/// depends on `codekurve-analysis`, so this glue lives here, matching
/// `build_file_inputs`'s IR -> store-input mapping just above. `pub(crate)`:
/// `crate::incremental::apply_batch` is its first caller (task 5.3).
pub(crate) fn project_baseline(snapshot: repo::ResolutionSnapshot) -> resolve::ProjectBaseline {
    let symbols = snapshot
        .symbols
        .into_iter()
        .map(|row| resolve::BaselineSymbol {
            name: row.name,
            file: row.relative_path,
            qualified_name: row.qualified_name,
            kind: row.kind,
            language: row.language,
            exported: row.exported,
        })
        .collect();
    resolve::ProjectBaseline::new(snapshot.files, symbols)
}

/// Minimal `tsconfig.json` `compilerOptions.paths` loader (design's
/// minimal-scope decision): single-`*`-prefix pattern -> first array entry
/// only, no `baseUrl` chains, no full JSON parser (`serde_json` is scoped to
/// PR5b). Absent file, missing `paths`, or anything outside this narrow
/// shape yields an empty map rather than failing the index.
fn load_tsconfig_aliases(root: &Path) -> TsconfigAliases {
    let mut aliases = TsconfigAliases::new();
    let Ok(text) = fs::read_to_string(root.join("tsconfig.json")) else {
        return aliases;
    };
    let Some(paths_idx) = text.find("\"paths\"") else {
        return aliases;
    };
    let Some(rel_start) = text[paths_idx..].find('{') else {
        return aliases;
    };
    let body_start = paths_idx + rel_start + 1;
    let Some(rel_end) = text[body_start..].find('}') else {
        return aliases;
    };
    let body = &text[body_start..body_start + rel_end];

    for entry in split_top_level_commas(body) {
        let Some((key_part, value_part)) = entry.split_once(':') else {
            continue;
        };
        let (Some(pattern), Some(replacement)) =
            (extract_string(key_part), extract_string(value_part))
        else {
            continue;
        };
        aliases.insert(pattern, replacement);
    }
    aliases
}

/// Splits `body` on commas that sit outside both string literals and `[...]`
/// arrays — good enough for a flat `"pattern": ["replacement", ...]` object,
/// which is all `compilerOptions.paths` ever contains.
fn split_top_level_commas(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0usize;
    let bytes = body.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if i == 0 || bytes[i - 1] != b'\\' => in_str = !in_str,
            b'[' if !in_str => depth += 1,
            b']' if !in_str => depth -= 1,
            b',' if !in_str && depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = body[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// The first quoted substring in `s`, e.g. `extract_string(r#"["src/*"]"#)`
/// -> `Some("src/*")`.
fn extract_string(s: &str) -> Option<String> {
    let start = s.find('"')?;
    let rest = &s[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// `codekurve search <query> --root <path>`
pub fn search(root: &Path, query: &str) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;
    let conn = open_existing_db(&root, &config)?;
    let project_id = project_id(&conn, &root)?;
    warn_if_stale(&conn, &project_id);

    let hits = repo::search(&conn, &project_id, query, config.queries.default_limit)
        .map_err(|e| e.to_string())?;
    if hits.is_empty() {
        println!("no matches for {query:?}");
        return Ok(());
    }
    for hit in hits {
        println!(
            "{}  {}  {}  {}:{}{}",
            hit.name,
            hit.kind,
            hit.visibility,
            hit.relative_path,
            hit.span.start_line,
            modifiers_suffix(hit.is_partial, hit.is_record)
        );
    }
    Ok(())
}

fn modifiers_suffix(is_partial: bool, is_record: bool) -> &'static str {
    match (is_partial, is_record) {
        (true, true) => "  [partial, record]",
        (true, false) => "  [partial]",
        (false, true) => "  [record]",
        (false, false) => "",
    }
}

/// `codekurve symbol <name> --root <path>`
pub fn symbol(root: &Path, name: &str) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;
    let conn = open_existing_db(&root, &config)?;
    let project_id = project_id(&conn, &root)?;
    warn_if_stale(&conn, &project_id);

    let hits = repo::find_by_name(&conn, &project_id, name).map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Err(format!("no symbol named {name:?}"));
    }
    for hit in &hits {
        println!(
            "{} ({}) [{}] visibility={}{}",
            hit.name,
            hit.kind,
            hit.language,
            hit.visibility,
            modifiers_suffix(hit.is_partial, hit.is_record)
        );
        println!(
            "  {}:{}:{}-{}:{}",
            hit.relative_path,
            hit.span.start_line,
            hit.span.start_column,
            hit.span.end_line,
            hit.span.end_column
        );
        println!("  --- snippet {} ---", snippet(&root, hit));
    }
    Ok(())
}

/// `codekurve doctor --root <path>`
pub fn doctor(root: &Path) -> Result<(), String> {
    let mut ok = true;

    let probe = db::open_in_memory().map_err(|e| e.to_string())?;
    let fts5 = db::has_fts5(&probe);
    report("sqlite", true, "available (bundled)");
    report("fts5", fts5, if fts5 { "available" } else { "MISSING" });
    ok &= fts5;

    let version =
        codekurve_store::migrations::current_version(&probe).map_err(|e| e.to_string())?;
    let schema_ok = version == codekurve_store::migrations::SCHEMA_VERSION;
    report(
        "schema",
        schema_ok,
        &format!(
            "version {version} (expected {})",
            codekurve_store::migrations::SCHEMA_VERSION
        ),
    );
    ok &= schema_ok;

    match root.canonicalize() {
        Ok(root) => {
            report("project root", true, &root.to_string_lossy());
            match load_config(&root) {
                Ok(_) => report("config", true, ".codekurve/config.toml"),
                Err(msg) => {
                    report("config", false, &msg);
                    ok = false;
                }
            }
        }
        Err(_) => {
            report("project root", false, &root.to_string_lossy());
            ok = false;
        }
    }

    if ok {
        Ok(())
    } else {
        Err("doctor found problems".to_string())
    }
}

fn report(check: &str, ok: bool, detail: &str) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("[{mark}] {check}: {detail}");
}

const DEFAULT_MAX_DEPTH: u32 = 10;
const DEFAULT_MAX_NODES: usize = 500;
const DEFAULT_MAX_EDGES: usize = 2000;
const DEFAULT_MAX_DURATION: Duration = Duration::from_secs(5);

/// Shared parameters for the six graph-query commands (§27.2): one query
/// subject via `symbol_id` (used verbatim) or `symbol_name` (bare or
/// qualified, resolved through [`resolve_symbol`]), plus
/// `min_confidence`/`depth`/`limit`/`offset`/`json`. `trace`'s second
/// (target) symbol isn't part of this shared shape — it's a positional CLI
/// argument, resolved the same way in [`trace`].
pub struct QueryArgs<'a> {
    pub root: &'a Path,
    pub symbol_id: Option<&'a str>,
    pub symbol_name: Option<&'a str>,
    pub min_confidence: Option<&'a str>,
    pub depth: Option<u32>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub json: bool,
}

/// `codekurve references --symbol-id <id>|--symbol-name <name> --root <path>`
pub fn references(args: &QueryArgs) -> Result<(), CommandError> {
    relationship_command(args, query::RelKind::References)
}

/// `codekurve callers --symbol-id <id>|--symbol-name <name> --root <path>`
pub fn callers(args: &QueryArgs) -> Result<(), CommandError> {
    relationship_command(args, query::RelKind::Callers)
}

/// `codekurve callees --symbol-id <id>|--symbol-name <name> --root <path>`
pub fn callees(args: &QueryArgs) -> Result<(), CommandError> {
    relationship_command(args, query::RelKind::Callees)
}

/// `codekurve implementations --symbol-id <id>|--symbol-name <name> --root <path>`
pub fn implementations(args: &QueryArgs) -> Result<(), CommandError> {
    relationship_command(args, query::RelKind::Implementations)
}

/// Shared body of `references`/`callers`/`callees`/`implementations`: open a
/// session, run [`query::relationships`], then print either plain text or
/// the §27.5 JSON envelope (`total: None` — CLI stdout stays byte-identical;
/// MCP, PR5+, passes `Some(page.total)`).
fn relationship_command(args: &QueryArgs, kind: query::RelKind) -> Result<(), CommandError> {
    let s = query::Session::open(args.root)?;
    if let query::Session::Indexed {
        conn, project_id, ..
    } = &s
    {
        warn_if_stale(conn, project_id);
    }
    let page = query::relationships(&s, kind, args)?;

    if args.json {
        let result = serde_json::Value::Array(page.rows.iter().map(relationship_json).collect());
        print_envelope(&s.config().project.name, result, Vec::new(), page.truncated);
    } else {
        print_relationships(&page.rows);
    }
    Ok(())
}

/// `codekurve trace <to> --symbol-id <id>|--symbol-name <from> --root <path>`
/// — bounded forward BFS (§26.4) from the resolved source symbol to `to`,
/// also resolved through [`resolve_symbol`] (an ambiguous target exits 6
/// exactly like an ambiguous source). Re-resolves `to` for the text-mode
/// print only — [`query::trace`] already resolved it once internally; both
/// calls are deterministic reads against the same session, so this costs one
/// extra lookup rather than widening `query::trace`'s design-fixed return
/// type.
pub fn trace(args: &QueryArgs, to: &str) -> Result<(), CommandError> {
    let s = query::Session::open(args.root)?;
    if let query::Session::Indexed {
        conn, project_id, ..
    } = &s
    {
        warn_if_stale(conn, project_id);
    }
    let outcome = query::trace(&s, args, to)?;

    if args.json {
        let result = trace_json(&outcome);
        print_envelope(
            &s.config().project.name,
            result,
            Vec::new(),
            outcome.truncated,
        );
    } else {
        let (conn, project_id) = s.indexed()?;
        let target = resolve_symbol(conn, project_id, None, Some(to))?;
        print_trace_result(&outcome, &target);
    }
    Ok(())
}

/// `codekurve impact --symbol-id <id>|--symbol-name <name> --root <path>` —
/// bounded reverse BFS (§26.5): everything that potentially depends on the
/// resolved symbol, never guaranteed, truncated rather than silently
/// incomplete.
pub fn impact(args: &QueryArgs) -> Result<(), CommandError> {
    let s = query::Session::open(args.root)?;
    if let query::Session::Indexed {
        conn, project_id, ..
    } = &s
    {
        warn_if_stale(conn, project_id);
    }
    let outcome = query::impact(&s, args)?;

    if args.json {
        let result = trace_json(&outcome);
        print_envelope(
            &s.config().project.name,
            result,
            Vec::new(),
            outcome.truncated,
        );
    } else {
        print_impact_result(&outcome);
    }
    Ok(())
}

pub(crate) fn bfs_caps(depth: Option<u32>) -> traverse::BfsCaps {
    traverse::BfsCaps {
        max_depth: depth.unwrap_or(DEFAULT_MAX_DEPTH),
        max_nodes: DEFAULT_MAX_NODES,
        max_edges: DEFAULT_MAX_EDGES,
        max_duration: DEFAULT_MAX_DURATION,
    }
}

/// Fatal half (code 4) of the old `require_indexed_project` preamble:
/// resolve `root`, load config. `pub(crate)`: [`query::Session::open`] calls
/// this first — a missing project root/config is always fatal, degraded or
/// not (design "Missing index").
pub(crate) fn load_project_config(root: &Path) -> Result<(PathBuf, Config), CommandError> {
    let root = canonicalize(root).map_err(|e| CommandError {
        code: 4,
        message: e,
    })?;
    let config = load_config(&root).map_err(|e| CommandError {
        code: 4,
        message: e,
    })?;
    Ok((root, config))
}

/// The other half: open the DB, find the project row. `pub(crate)`:
/// [`query::Session::open`] downgrades a failure here to `NotIndexed`
/// instead of propagating it — the six graph-query commands (via
/// [`query::Session::indexed`]) turn that back into the exact same code-4
/// `CommandError` this function used to return directly, so their behavior
/// is unchanged.
pub(crate) fn open_project_index(
    root: &Path,
    config: &Config,
) -> Result<(Connection, String), CommandError> {
    let conn = open_existing_db(root, config).map_err(|e| CommandError {
        code: 4,
        message: e,
    })?;
    let pid = project_id(&conn, root).map_err(|e| CommandError {
        code: 4,
        message: e,
    })?;
    Ok((conn, pid))
}

/// Resolves one query subject. `--symbol-id` is used verbatim (already
/// disambiguated). `--symbol-name` accepts either a bare name — ambiguous
/// matches (>1 candidate) become `CommandError{code:6}` listing every
/// candidate's qualified name/kind/path (spec "Ambiguous name lookup"), never
/// silently picking one — or a full qualified name (`path::Name` /
/// `path::Class.method`), which narrows to exactly one match (spec
/// "Qualified name disambiguates") by looking up the bare identifier tail
/// then filtering candidates down to the exact qualified-name match.
pub(crate) fn resolve_symbol(
    conn: &Connection,
    project_id: &str,
    symbol_id: Option<&str>,
    symbol_name: Option<&str>,
) -> Result<String, CommandError> {
    if let Some(id) = symbol_id {
        return Ok(id.to_string());
    }
    let name = symbol_name.ok_or_else(|| CommandError {
        code: 1,
        message: "expected --symbol-id or --symbol-name".to_string(),
    })?;

    let is_qualified = name.contains("::");
    let bare = if is_qualified { bare_name(name) } else { name };
    let candidates = repo::find_candidates_by_name(conn, project_id, bare)
        .map_err(|e| CommandError::from(e.to_string()))?;

    if is_qualified {
        return candidates
            .into_iter()
            .find(|c| c.qualified_name == name)
            .map(|c| c.id)
            .ok_or_else(|| CommandError {
                code: 1,
                message: format!("no symbol matching qualified name {name:?}"),
            });
    }
    match candidates.len() {
        0 => Err(CommandError {
            code: 1,
            message: format!("no symbol named {name:?}"),
        }),
        1 => Ok(candidates[0].id.clone()),
        _ => Err(CommandError {
            code: 6,
            message: ambiguous_message(name, &candidates),
        }),
    }
}

/// The bare identifier tail of a qualified name (`path::Class.method` ->
/// `method`; `path::name` -> `name`), matching what `symbols.name` stores.
fn bare_name(qualified: &str) -> &str {
    let local = qualified.rsplit("::").next().unwrap_or(qualified);
    local.rsplit('.').next().unwrap_or(local)
}

fn ambiguous_message(name: &str, candidates: &[repo::SymbolCandidate]) -> String {
    let mut message = format!("ambiguous symbol name {name:?}, candidates:");
    for c in candidates {
        message.push_str(&format!(
            "\n  {} ({}) [{}]",
            c.qualified_name, c.kind, c.relative_path
        ));
    }
    message
}

pub(crate) fn parse_confidence(raw: Option<&str>) -> Result<Option<Confidence>, CommandError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw {
        "exact" => Ok(Some(Confidence::Exact)),
        "high" => Ok(Some(Confidence::High)),
        "medium" => Ok(Some(Confidence::Medium)),
        "low" => Ok(Some(Confidence::Low)),
        "unresolved" => Ok(Some(Confidence::Unresolved)),
        other => Err(CommandError {
            code: 1,
            message: format!("unknown --min-confidence value: {other:?}"),
        }),
    }
}

/// ponytail: applies to the four flat relationship-list commands only —
/// `trace`/`impact`'s BFS result (a path plus a reached-set) isn't a
/// page-able list in the same sense, so they don't call this.
pub(crate) fn paginate<T>(rows: &mut Vec<T>, limit: Option<usize>, offset: Option<usize>) {
    if let Some(offset) = offset {
        let drop_n = offset.min(rows.len());
        rows.drain(0..drop_n);
    }
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
}

fn relationship_json(r: &repo::StoredRelationship) -> serde_json::Value {
    serde_json::json!({
        "source_symbol_id": r.source_symbol_id,
        "source_qualified_name": r.source_qualified_name,
        "target_symbol_id": r.target_symbol_id,
        "target_qualified_name": r.target_qualified_name,
        "target_external": r.target_external,
        "kind": r.kind,
        "provenance": r.provenance,
        "confidence": r.confidence,
        "start_line": r.start_line,
        "start_column": r.start_column,
    })
}

fn print_relationships(rows: &[repo::StoredRelationship]) {
    if rows.is_empty() {
        println!("no results");
        return;
    }
    for r in rows {
        let target = r
            .target_qualified_name
            .as_deref()
            .or(r.target_external.as_deref())
            .unwrap_or("?");
        println!(
            "{} -> {}  [{}, {}, {}]  {}:{}",
            r.source_qualified_name,
            target,
            r.kind,
            r.confidence,
            r.provenance,
            r.start_line.unwrap_or(0),
            r.start_column.unwrap_or(0)
        );
    }
}

fn edge_json(e: &traverse::Edge) -> serde_json::Value {
    serde_json::json!({
        "neighbor_symbol_id": e.neighbor_symbol_id,
        "kind": e.kind,
        "confidence": e.confidence,
        "provenance": e.provenance,
    })
}

fn reached_json(r: &traverse::Reached) -> serde_json::Value {
    serde_json::json!({
        "symbol_id": r.symbol_id,
        "depth": r.depth,
        "via": r.via.as_ref().map(edge_json),
        "predecessor": r.predecessor,
    })
}

fn trace_json(outcome: &traverse::BfsOutcome) -> serde_json::Value {
    serde_json::json!({
        "path": outcome.path.as_ref().map(|p| p.iter().map(edge_json).collect::<Vec<_>>()),
        "reached": outcome.reached.iter().map(reached_json).collect::<Vec<_>>(),
        "truncated_reason": outcome.truncated_reason.map(truncation_reason_str),
    })
}

fn truncation_reason_str(r: traverse::TruncationReason) -> &'static str {
    match r {
        traverse::TruncationReason::MaxDepth => "max_depth",
        traverse::TruncationReason::MaxNodes => "max_nodes",
        traverse::TruncationReason::MaxEdges => "max_edges",
        traverse::TruncationReason::MaxDuration => "max_duration",
    }
}

fn print_trace_result(outcome: &traverse::BfsOutcome, target: &str) {
    match &outcome.path {
        Some(path) => {
            println!("path to {target} found ({} hop(s)):", path.len());
            for edge in path {
                println!(
                    "  -> {} [{}, {}]",
                    edge.neighbor_symbol_id, edge.kind, edge.confidence
                );
            }
        }
        None => println!("no path to {target} found within the given caps"),
    }
    print_truncation(outcome);
}

fn print_impact_result(outcome: &traverse::BfsOutcome) {
    println!("{} node(s) reached", outcome.reached.len());
    for r in &outcome.reached {
        println!("  {} (depth {})", r.symbol_id, r.depth);
    }
    print_truncation(outcome);
}

fn print_truncation(outcome: &traverse::BfsOutcome) {
    if outcome.truncated {
        println!(
            "truncated: {}",
            outcome
                .truncated_reason
                .map(truncation_reason_str)
                .unwrap_or("unknown")
        );
    }
}

/// §27.5 JSON envelope: `schema_version`, `project`, `result`, `warnings`,
/// `truncated` — every field, every command, every time. Delegates to
/// [`query::envelope`] with `total: None`, so CLI `--json` output stays
/// byte-identical (the `total` key is only ever emitted when `Some`).
fn print_envelope(
    project: &str,
    result: serde_json::Value,
    warnings: Vec<String>,
    truncated: bool,
) {
    println!(
        "{}",
        query::envelope(project, result, warnings, truncated, None)
    );
}

fn discovery_options(config: &Config) -> DiscoveryOptions {
    let languages = config
        .index
        .languages
        .iter()
        .filter_map(|name| LanguageId::from_name(name))
        .collect();
    DiscoveryOptions {
        respect_gitignore: config.ignore.respect_gitignore,
        respect_global_gitignore: config.ignore.respect_global_gitignore,
        include_hidden: config.index.include_hidden,
        follow_symlinks: config.index.follow_symlinks,
        max_file_size_bytes: config.index.max_file_size_bytes,
        languages,
    }
}

fn snippet(root: &Path, symbol: &StoredSymbol) -> String {
    let path = root.join(&symbol.relative_path);
    let Ok(bytes) = fs::read(&path) else {
        return "(unavailable: file not found) ---".to_string();
    };
    // ponytail: bounds check is the Phase 1 staleness signal; hash-based
    // staleness (§25) arrives with file hashing in Phase 3.
    if symbol.span.end_byte > bytes.len() {
        return "(stale: file changed since index; run `codekurve index`) ---".to_string();
    }
    match std::str::from_utf8(&bytes[symbol.span.start_byte..symbol.span.end_byte]) {
        Ok(text) => format!("(live) ---\n{text}"),
        Err(_) => "(unavailable: non-utf8 span) ---".to_string(),
    }
}

fn canonicalize(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize()
        .map_err(|_| format!("path does not exist: {}", root.display()))
}

fn config_path(root: &Path) -> PathBuf {
    root.join(".codekurve").join("config.toml")
}

fn load_config(root: &Path) -> Result<Config, String> {
    let path = config_path(root);
    let text = fs::read_to_string(&path).map_err(|_| {
        format!(
            "not a codekurve project (missing {}). run `codekurve init` first.",
            path.display()
        )
    })?;
    Config::from_toml(&text).map_err(|e| e.to_string())
}

fn open_existing_db(root: &Path, config: &Config) -> Result<Connection, String> {
    let db_path = root.join(&config.storage.database);
    if !db_path.exists() {
        return Err(format!(
            "no index found ({}). run `codekurve index` first.",
            db_path.display()
        ));
    }
    db::open(&db_path).map_err(|e| e.to_string())
}

fn project_id(conn: &Connection, root: &Path) -> Result<String, String> {
    repo::find_project(conn, &root.to_string_lossy())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "project not indexed yet. run `codekurve index` first.".to_string())
}

#[cfg(test)]
mod symbol_ids_tests {
    use super::*;
    use codekurve_analysis::extract;
    use codekurve_core::RelationshipKind;

    fn meta() -> FileMeta {
        FileMeta {
            language: LanguageId::CSharp,
            size_bytes: 1,
            content_hash: "hash".to_string(),
            modified_ns: 0,
        }
    }

    #[test]
    fn partial_csharp_edges_survive_symbol_ids_lookup() {
        let source = "namespace Acme;\npublic partial class Widget : Base {\n  public void Alpha() {}\n}\npublic class Base {}\n";
        let mut analysis =
            extract::analyze(source, LanguageId::CSharp, "src/widget.cs").expect("analyze");
        resolve::resolve(std::slice::from_mut(&mut analysis), &TsconfigAliases::new());

        let (_files, symbol_ids) =
            build_file_inputs("proj", &[analysis.clone()], &[meta()]);
        let rels = build_relationships("proj", &[analysis], &symbol_ids);

        let contains_alpha = rels.iter().any(|r| {
            r.kind == RelationshipKind::Contains
                && r.target_symbol_id.as_ref().is_some_and(|tid| {
                    symbol_ids
                        .iter()
                        .any(|(k, id)| k.1.contains("Alpha") && id == tid)
                })
        });
        assert!(
            contains_alpha,
            "partial Widget must keep Contains→Alpha through composition-root ids"
        );

        let inherits = rels
            .iter()
            .any(|r| r.kind == RelationshipKind::Inherits && r.target_symbol_id.is_some());
        assert!(
            inherits,
            "partial Widget must keep Inherits→Base through composition-root ids"
        );
        assert!(
            !rels.iter().any(|r| r.target_symbol_id.is_none()
                && r.target_external.is_none()
                && matches!(
                    r.kind,
                    RelationshipKind::Contains | RelationshipKind::Inherits
                )),
            "no dangling Contains/Inherits rows"
        );
    }

    #[test]
    fn overload_contains_targets_distinct_symbol_ids() {
        let source =
            "namespace Acme;\npublic class Widget {\n  public Widget() {}\n  public Widget(int x) {}\n}\n";
        let mut analysis =
            extract::analyze(source, LanguageId::CSharp, "src/widget.cs").expect("analyze");
        resolve::resolve(std::slice::from_mut(&mut analysis), &TsconfigAliases::new());

        let (_files, symbol_ids) =
            build_file_inputs("proj", &[analysis.clone()], &[meta()]);
        let rels = build_relationships("proj", &[analysis.clone()], &symbol_ids);

        let ctor_targets: Vec<_> = analysis
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Constructor)
            .map(|s| symbol_ids.get(&(analysis.file.clone(), s.local_key.clone())))
            .collect();
        assert_eq!(ctor_targets.len(), 2);
        assert!(ctor_targets.iter().all(|t| t.is_some()));
        assert_ne!(ctor_targets[0], ctor_targets[1]);

        let contains_to_ctors: Vec<_> = rels
            .iter()
            .filter(|r| {
                r.kind == RelationshipKind::Contains
                    && ctor_targets.iter().any(|t| *t == r.target_symbol_id.as_ref())
            })
            .collect();
        assert_eq!(
            contains_to_ctors.len(),
            2,
            "each overload must keep its own Contains edge"
        );
        assert_ne!(
            contains_to_ctors[0].target_symbol_id,
            contains_to_ctors[1].target_symbol_id
        );
    }
}
