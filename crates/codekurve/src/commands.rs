//! Command implementations. The binary is the composition root (§11.2): it
//! loads config, drives discovery + extraction (`codekurve-analysis`), and
//! persists/queries through `codekurve-store`.

use std::fs;
use std::path::{Path, PathBuf};

use codekurve_analysis::discovery::{self, DiscoveryOptions};
use codekurve_analysis::extract;
use codekurve_core::{Config, LanguageId, Symbol};
use codekurve_store::db;
use codekurve_store::repo::{self, FileInput, StoredSymbol};
use codekurve_store::Connection;

/// `codekurve index --root <path>`
pub fn index(root: &Path) -> Result<(), String> {
    let root = canonicalize(root)?;
    let config = load_config(&root)?;

    let options = discovery_options(&config);
    let discovered = discovery::discover(&root, &options);

    let mut files = Vec::new();
    let mut parse_errors = 0usize;
    for file in &discovered {
        let Ok(source) = fs::read_to_string(&file.absolute_path) else {
            parse_errors += 1;
            continue;
        };
        let analysis = match extract::analyze(&source, file.language, &file.relative_path) {
            Ok(analysis) => analysis,
            Err(_) => {
                parse_errors += 1;
                continue;
            }
        };
        // Composition-root mapping: analysis IR -> store's persist-input
        // type, so `codekurve-store` never depends on `codekurve-analysis`.
        let symbols: Vec<Symbol> = analysis
            .symbols
            .into_iter()
            .map(|s| Symbol {
                name: s.name,
                qualified_name: s.qualified_name,
                kind: s.kind,
                language: s.language,
                span: s.span,
                parent: s.parent,
            })
            .collect();
        files.push(FileInput {
            relative_path: file.relative_path.clone(),
            language: file.language.as_str().to_string(),
            size_bytes: source.len() as u64,
            symbols,
        });
    }

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
    let outcome = repo::reindex(&mut conn, &project_id, &files).map_err(|e| e.to_string())?;

    println!(
        "indexed {} file(s), {} symbol(s){}",
        outcome.files,
        outcome.symbols,
        if parse_errors > 0 {
            format!(", {parse_errors} skipped")
        } else {
            String::new()
        }
    );
    Ok(())
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
