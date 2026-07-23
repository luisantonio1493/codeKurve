//! Command implementations. The binary is the composition root (§11.2): it
//! loads config, drives discovery + extraction (`codekurve-analysis`), and
//! persists/queries through `codekurve-store`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use codekurve_analysis::discovery::{self, DiscoveryOptions};
use codekurve_analysis::extract;
use codekurve_analysis::ir::{EdgeTarget, FileAnalysis};
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Config, LanguageId, SourceSpan, Symbol, SymbolKind};
use codekurve_store::db;
use codekurve_store::repo::{
    self, FileInput, RelationshipInput, StoredSymbol, UnresolvedReferenceInput,
};
use codekurve_store::Connection;

/// `codekurve index --root <path>` — two-pass pipeline (design §22): pass 1
/// parses every discovered file in isolation; pass 2 resolves every edge
/// against the whole-project symbol table before anything is persisted.
pub fn index(root: &Path) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;

    let options = discovery_options(&config);
    let discovered = discovery::discover(&root, &options);
    let aliases = load_tsconfig_aliases(&root);

    // Pass 1: parse only, no persistence — the whole batch must be in
    // memory before pass 2 can resolve cross-file references (§22.2 accepts
    // this for MVP scale).
    let mut analyses: Vec<FileAnalysis> = Vec::new();
    let mut file_meta: Vec<(LanguageId, u64)> = Vec::new();
    let mut parse_errors = 0usize;
    for file in &discovered {
        let Ok(source) = fs::read_to_string(&file.absolute_path) else {
            parse_errors += 1;
            continue;
        };
        match extract::analyze(&source, file.language, &file.relative_path) {
            Ok(analysis) => {
                file_meta.push((file.language, source.len() as u64));
                analyses.push(analysis);
            }
            Err(_) => parse_errors += 1,
        }
    }

    // Pass 2: resolve every `Unresolved` edge in place against the
    // whole-project symbol table.
    resolve::resolve(&mut analyses, &aliases);

    let db_path = root.join(&config.storage.database);
    let mut conn = db::open(&db_path).map_err(|e| e.to_string())?;
    let config_text = config.to_toml().map_err(|e| e.to_string())?;
    let project_id = repo::upsert_project(
        &conn,
        &config.project.name,
        &root.to_string_lossy(),
        &repo::config_hash(&config_text),
    )
    .map_err(|e| e.to_string())?;

    // Composition-root mapping: analysis IR -> store's persist-input types,
    // so `codekurve-store` never depends on `codekurve-analysis`.
    let (files, symbol_ids) = build_file_inputs(&project_id, &analyses, &file_meta);
    let relationships = build_relationships(&project_id, &analyses, &symbol_ids);
    let unresolved = build_unresolved(&project_id, &analyses, &symbol_ids);

    let outcome = repo::reindex(&mut conn, &project_id, &files, &relationships, &unresolved)
        .map_err(|e| e.to_string())?;

    println!(
        "indexed {} file(s), {} symbol(s), {} relationship(s), {} unresolved{}",
        outcome.files,
        outcome.symbols,
        relationships.len(),
        unresolved.len(),
        if parse_errors > 0 {
            format!(", {parse_errors} skipped")
        } else {
            String::new()
        }
    );
    Ok(())
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
    }
}

/// Builds `repo::FileInput`s and a `(relative_path, qualified_name) ->
/// symbol_id` lookup table, using the same deterministic id functions
/// `repo::reindex` uses internally — so ids computed here match exactly
/// what ends up in the `symbols` table.
fn build_file_inputs(
    project_id: &str,
    analyses: &[FileAnalysis],
    file_meta: &[(LanguageId, u64)],
) -> (Vec<FileInput>, HashMap<(String, String), String>) {
    let mut files = Vec::with_capacity(analyses.len());
    let mut symbol_ids: HashMap<(String, String), String> = HashMap::new();

    for (analysis, (language, size_bytes)) in analyses.iter().zip(file_meta) {
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
            })
            .collect();
        symbols.push(module_symbol(&analysis.file, *language));

        for symbol in &symbols {
            let key = repo::symbol_key(
                language.as_str(),
                &analysis.file,
                symbol.kind.as_str(),
                &symbol.qualified_name,
            );
            symbol_ids.insert(
                (analysis.file.clone(), symbol.qualified_name.clone()),
                repo::symbol_id(&file_id, &key),
            );
        }

        files.push(FileInput {
            relative_path: analysis.file.clone(),
            language: language.as_str().to_string(),
            size_bytes: *size_bytes,
            symbols,
        });
    }

    (files, symbol_ids)
}

/// Maps every resolved `ExtractedRelationship` to a `RelationshipInput`.
/// After `resolve::resolve`, every edge's target is `Local`, `Global`, or
/// `External` (never `Unresolved` — zero-candidate edges moved to
/// `analysis.unresolved` instead, §18.3).
fn build_relationships(
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
/// §18.3) to an `UnresolvedReferenceInput`.
fn build_unresolved(
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

    let hits = repo::search(&conn, &project_id, query, config.queries.default_limit)
        .map_err(|e| e.to_string())?;
    if hits.is_empty() {
        println!("no matches for {query:?}");
        return Ok(());
    }
    for hit in hits {
        println!(
            "{}  {}  {}:{}",
            hit.name, hit.kind, hit.relative_path, hit.span.start_line
        );
    }
    Ok(())
}

/// `codekurve symbol <name> --root <path>`
pub fn symbol(root: &Path, name: &str) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;
    let conn = open_existing_db(&root, &config)?;
    let project_id = project_id(&conn, &root)?;

    let hits = repo::find_by_name(&conn, &project_id, name).map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Err(format!("no symbol named {name:?}"));
    }
    for hit in &hits {
        println!("{} ({}) [{}]", hit.name, hit.kind, hit.language);
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
