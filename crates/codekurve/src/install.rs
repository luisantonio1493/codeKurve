//! `codekurve install <client>` (design PR5a): wires the running binary into
//! a supported MCP client's config file directly — no subprocess shell-out
//! (SECURITY_MODEL.md: "never shell out"), matching how `codekurve-mcp`
//! itself is invoked (`codekurve mcp --root <path>`).
//!
//! PR5b adds Codex CLI's TOML writer (`write_codex_toml`, via `toml_edit` to
//! preserve comments/formatting on merge — Codex's `config.toml` is
//! hand-edited by users, unlike a naive `toml`-crate round-trip which would
//! destroy that).
//!
//! ## Verified Codex CLI config: `~/.codex/config.toml`, `[mcp_servers.<name>]`
//!
//! Verified against a live installation on this machine (`codex --version` →
//! `codex-cli 0.146.0`): `~/.codex/config.toml` already contains real
//! `[mcp_servers.*]` tables written by Codex itself for other servers, e.g.
//! `[mcp_servers.engram]` with plain `command`/`args` keys and no
//! `type`/`transport` key — same shape the design's PR5 sketch predicted.
//! `$CODEX_HOME` (unset on this machine) takes priority over `$HOME/.codex`
//! per design D12; Windows uses `%USERPROFILE%\.codex`.
//!
//! ## Verified stdio key: `"type": "stdio"` (Claude Code), no key (Cursor)
//!
//! The design flagged this as the phase's one open question: does Claude
//! Code's `.mcp.json` expect `"type": "stdio"` or `"transport": "stdio"`?
//! Verified conclusively against a live installation on this machine
//! (`claude --version` → `2.1.220 (Claude Code)`):
//!
//! ```text
//! $ claude mcp add --scope project probe-server -- /bin/echo hello
//! $ cat .mcp.json
//! {"mcpServers":{"probe-server":{"type":"stdio","command":"/bin/echo", ...}}}
//! ```
//!
//! `claude mcp add` is the client's own first-party CLI writing its own
//! config format — this is ground truth, not inference. The repo's prior
//! `"transport": "stdio"` in `docs/AGENT_USAGE.md`/`README.md` was wrong and
//! has been corrected to match this writer.
//!
//! Cursor was checked the same way against this machine's real
//! `~/.cursor/mcp.json` (populated by Cursor itself for other servers):
//! entries carry only `command`/`args`, no `type`/`transport` key at all.
//! PR5a therefore does NOT share one entry shape across both clients (the
//! design's sketch did) — each client gets the exact shape its own live
//! config already uses.
//!
//! ## Verified GitHub Copilot (VS Code): `.vscode/mcp.json`, `"servers"` key
//!
//! Checked against this machine's real `~/Library/Application Support/Code/
//! User/mcp.json` (VS Code's *user*-scope MCP config, populated by VS Code
//! itself for other servers): top-level key is `"servers"` (not
//! `"mcpServers"`), entries carry `command`/`args`/`type: "stdio"` for local
//! servers. Project scope uses the same shape at `.vscode/mcp.json` in the
//! workspace root (VS Code's own documented per-project MCP config file) —
//! chosen here to match Claude Code/Cursor's project-scope convention.
//!
//! ## Verified OpenCode: `opencode.json`, `"mcp"` key, `command` is ONE array
//!
//! Checked against this machine's real `~/.config/opencode/opencode.json`
//! (OpenCode's user-scope config) and cross-checked against OpenCode's
//! published schema (`https://opencode.ai/config.json`,
//! `$defs.McpLocalConfig`): top-level key is `"mcp"`, and unlike every other
//! client here, a local server's `command` is a **single array** combining
//! the binary and its arguments (e.g. `["/path/to/codekurve", "mcp",
//! "--root", "/path"]`) — there is no separate `args` key. `type: "local"`
//! is required. Project scope uses the same `Config` schema at
//! `opencode.json` in the project root.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, Table};

pub enum Client {
    ClaudeCode,
    Cursor,
    Codex,
    Copilot,
    OpenCode,
}

impl Client {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "claude-code" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "codex-cli" => Some(Self::Codex),
            "copilot" => Some(Self::Copilot),
            "opencode" => Some(Self::OpenCode),
            _ => None,
        }
    }

    /// Client-relative config path (project scope for every client here
    /// except Codex, which has no project-scoped config — see
    /// `codex_config_path`).
    fn config_path(&self, root: &Path) -> PathBuf {
        match self {
            Self::ClaudeCode => root.join(".mcp.json"),
            Self::Cursor => root.join(".cursor").join("mcp.json"),
            Self::Copilot => root.join(".vscode").join("mcp.json"),
            Self::OpenCode => root.join("opencode.json"),
            Self::Codex => unreachable!("Codex path is resolved via codex_config_path"),
        }
    }

    /// `true` for clients whose live config carries a `"type": "stdio"` key
    /// (Claude Code and Copilot, both verified above); Cursor's live config
    /// carries none.
    fn includes_stdio_type(&self) -> bool {
        matches!(self, Self::ClaudeCode | Self::Copilot)
    }

    fn scope(&self) -> &'static str {
        match self {
            Self::ClaudeCode | Self::Cursor | Self::Copilot | Self::OpenCode => "project scope",
            Self::Codex => "user scope",
        }
    }
}

const SUPPORTED_CLIENTS: &str = "claude-code, cursor, codex-cli, copilot, opencode";

fn manual_instructions(exe: &str, root: &str) -> String {
    format!(
        "supported clients: {SUPPORTED_CLIENTS}\n\
         to configure Claude Code/Cursor manually, add this entry to the \
         client's MCP config under \"mcpServers\":\n\
         \"codekurve\": {{\"command\": \"{exe}\", \"args\": [\"mcp\", \"--root\", \"{root}\"]}}\n\
         to configure Codex CLI manually, add this table to config.toml:\n\
         [mcp_servers.codekurve]\n\
         command = \"{exe}\"\n\
         args = [\"mcp\", \"--root\", \"{root}\"]\n\
         to configure GitHub Copilot (VS Code) manually, add this entry to \
         .vscode/mcp.json under \"servers\":\n\
         \"codekurve\": {{\"command\": \"{exe}\", \"args\": [\"mcp\", \"--root\", \"{root}\"], \"type\": \"stdio\"}}\n\
         to configure OpenCode manually, add this entry to opencode.json \
         under \"mcp\":\n\
         \"codekurve\": {{\"type\": \"local\", \"command\": [\"{exe}\", \"mcp\", \"--root\", \"{root}\"]}}"
    )
}

/// `$CODEX_HOME/config.toml`, else `$HOME/.codex/config.toml`
/// (`%USERPROFILE%\.codex\config.toml` on Windows) — design D12.
fn codex_config_path() -> Result<PathBuf, String> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(home_var).map_err(|_| {
        format!("could not resolve Codex config: neither $CODEX_HOME nor ${home_var} is set")
    })?;
    Ok(PathBuf::from(home).join(".codex").join("config.toml"))
}

/// `codekurve install <client> [--root <path>]`.
pub fn run(root: &Path, client: Option<&str>) -> Result<(), String> {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("failed to resolve codekurve binary path: {e}"))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("failed to resolve --root: {e}"))?;

    let Some(client_name) = client else {
        return Err(format!(
            "codekurve install requires a client name\n{}",
            manual_instructions(&exe.to_string_lossy(), &root.to_string_lossy())
        ));
    };

    let Some(client) = Client::parse(client_name) else {
        return Err(format!(
            "unsupported client \"{client_name}\"\n{}",
            manual_instructions(&exe.to_string_lossy(), &root.to_string_lossy())
        ));
    };

    let config_path = match client {
        Client::Codex => codex_config_path()?,
        _ => client.config_path(&root),
    };
    match client {
        Client::Codex => write_codex_toml(&config_path, &exe, &root)?,
        Client::OpenCode => write_opencode_json(&config_path, &exe, &root)?,
        Client::Copilot => write_json_client(&config_path, &exe, &root, "servers", true)?,
        _ => write_json_client(
            &config_path,
            &exe,
            &root,
            "mcpServers",
            client.includes_stdio_type(),
        )?,
    }
    println!(
        "installed codekurve MCP server into {} ({})",
        config_path.display(),
        client.scope()
    );
    Ok(())
}

fn server_entry(exe: &Path, root: &Path, include_type: bool) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "command".to_string(),
        Value::String(exe.to_string_lossy().into_owned()),
    );
    entry.insert(
        "args".to_string(),
        Value::Array(vec![
            Value::String("mcp".to_string()),
            Value::String("--root".to_string()),
            Value::String(root.to_string_lossy().into_owned()),
        ]),
    );
    if include_type {
        entry.insert("type".to_string(), Value::String("stdio".to_string()));
    }
    Value::Object(entry)
}

/// Reads (or starts fresh), merges the `codekurve` entry into the top-level
/// `servers_key` object (`"mcpServers"` for Claude Code/Cursor, `"servers"`
/// for Copilot/VS Code) without disturbing sibling entries, backs up any
/// pre-existing file, then writes. Rule 4 (design): an unparseable file, or
/// a `servers_key` key present but not an object, aborts with no write at
/// all.
fn write_json_client(
    path: &Path,
    exe: &Path,
    root: &Path,
    servers_key: &str,
    include_type: bool,
) -> Result<(), String> {
    let existing_bytes = fs::read(path).ok();

    let mut doc: Map<String, Value> = match &existing_bytes {
        None => Map::new(),
        Some(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            let parsed: Value = serde_json::from_str(&text).map_err(|e| {
                format!(
                    "{} is not valid JSON ({e}); no changes made.\n{}",
                    path.display(),
                    manual_snippet(exe, root, servers_key, include_type)
                )
            })?;
            match parsed {
                Value::Object(map) => map,
                _ => {
                    return Err(format!(
                        "{} does not contain a JSON object at the top level; no changes made.\n{}",
                        path.display(),
                        manual_snippet(exe, root, servers_key, include_type)
                    ));
                }
            }
        }
    };

    let servers = match doc.get_mut(servers_key) {
        None => {
            doc.insert(servers_key.to_string(), Value::Object(Map::new()));
            doc.get_mut(servers_key).unwrap().as_object_mut().unwrap()
        }
        Some(Value::Object(_)) => doc.get_mut(servers_key).unwrap().as_object_mut().unwrap(),
        Some(_) => {
            return Err(format!(
                "{} has a \"{servers_key}\" key that is not an object; no changes made.\n{}",
                path.display(),
                manual_snippet(exe, root, servers_key, include_type)
            ));
        }
    };
    servers.insert(
        "codekurve".to_string(),
        server_entry(exe, root, include_type),
    );

    if let Some(bytes) = &existing_bytes {
        backup(path, bytes)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    fs::write(path, out).map_err(|e| e.to_string())
}

fn manual_snippet(exe: &Path, root: &Path, servers_key: &str, include_type: bool) -> String {
    let entry = server_entry(exe, root, include_type);
    format!(
        "add this entry to the file's \"{servers_key}\" object manually:\n\"codekurve\": {}",
        serde_json::to_string_pretty(&entry).unwrap_or_default()
    )
}

/// Reads (or starts fresh), merges the `codekurve` entry into OpenCode's
/// top-level `"mcp"` object, backs up any pre-existing file, then writes.
/// OpenCode's `McpLocalConfig` shape (verified against
/// `https://opencode.ai/config.json`) differs from every other client here:
/// `command` is one array holding the binary *and* its arguments, there is
/// no separate `args` key, and `type: "local"` is required.
fn write_opencode_json(path: &Path, exe: &Path, root: &Path) -> Result<(), String> {
    let existing_bytes = fs::read(path).ok();

    let mut doc: Map<String, Value> = match &existing_bytes {
        None => Map::new(),
        Some(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            let parsed: Value = serde_json::from_str(&text).map_err(|e| {
                format!(
                    "{} is not valid JSON ({e}); no changes made.\n{}",
                    path.display(),
                    manual_snippet_opencode(exe, root)
                )
            })?;
            match parsed {
                Value::Object(map) => map,
                _ => {
                    return Err(format!(
                        "{} does not contain a JSON object at the top level; no changes made.\n{}",
                        path.display(),
                        manual_snippet_opencode(exe, root)
                    ));
                }
            }
        }
    };

    let servers = match doc.get_mut("mcp") {
        None => {
            doc.insert("mcp".to_string(), Value::Object(Map::new()));
            doc.get_mut("mcp").unwrap().as_object_mut().unwrap()
        }
        Some(Value::Object(_)) => doc.get_mut("mcp").unwrap().as_object_mut().unwrap(),
        Some(_) => {
            return Err(format!(
                "{} has a \"mcp\" key that is not an object; no changes made.\n{}",
                path.display(),
                manual_snippet_opencode(exe, root)
            ));
        }
    };
    servers.insert("codekurve".to_string(), opencode_entry(exe, root));

    if let Some(bytes) = &existing_bytes {
        backup(path, bytes)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let out = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    fs::write(path, out).map_err(|e| e.to_string())
}

fn opencode_entry(exe: &Path, root: &Path) -> Value {
    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String("local".to_string()));
    entry.insert(
        "command".to_string(),
        Value::Array(vec![
            Value::String(exe.to_string_lossy().into_owned()),
            Value::String("mcp".to_string()),
            Value::String("--root".to_string()),
            Value::String(root.to_string_lossy().into_owned()),
        ]),
    );
    Value::Object(entry)
}

fn manual_snippet_opencode(exe: &Path, root: &Path) -> String {
    let entry = opencode_entry(exe, root);
    format!(
        "add this entry to the file's \"mcp\" object manually:\n\"codekurve\": {}",
        serde_json::to_string_pretty(&entry).unwrap_or_default()
    )
}

/// Reads (or starts fresh), merges the `codekurve` entry into
/// `[mcp_servers]` without disturbing sibling entries or existing
/// comments/formatting elsewhere in the file, backs up any pre-existing file,
/// then writes. Uses `toml_edit` (design D10) rather than the `toml` crate's
/// serialize round-trip, which would destroy comments/ordering in a
/// hand-edited `config.toml`.
fn write_codex_toml(path: &Path, exe: &Path, root: &Path) -> Result<(), String> {
    let existing_bytes = fs::read(path).ok();

    let mut doc: DocumentMut = match &existing_bytes {
        None => DocumentMut::new(),
        Some(bytes) => {
            let text = String::from_utf8_lossy(bytes);
            text.parse::<DocumentMut>().map_err(|e| {
                format!(
                    "{} is not valid TOML ({e}); no changes made.\n{}",
                    path.display(),
                    manual_snippet_toml(exe, root)
                )
            })?
        }
    };

    let servers = doc
        .as_table_mut()
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()));
    let servers_table = servers.as_table_mut().ok_or_else(|| {
        format!(
            "{} has a \"mcp_servers\" key that is not a table; no changes made.\n{}",
            path.display(),
            manual_snippet_toml(exe, root)
        )
    })?;

    let mut entry = Table::new();
    entry["command"] = toml_edit::value(exe.to_string_lossy().into_owned());
    let mut args = toml_edit::Array::new();
    args.push("mcp");
    args.push("--root");
    args.push(root.to_string_lossy().into_owned());
    entry["args"] = toml_edit::value(args);
    servers_table.insert("codekurve", Item::Table(entry));

    if let Some(bytes) = &existing_bytes {
        backup(path, bytes)?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, doc.to_string()).map_err(|e| e.to_string())
}

fn manual_snippet_toml(exe: &Path, root: &Path) -> String {
    format!(
        "add this table to the file manually:\n\
         [mcp_servers.codekurve]\n\
         command = \"{}\"\n\
         args = [\"mcp\", \"--root\", \"{}\"]",
        exe.display(),
        root.display()
    )
}

/// `<file>.bak` written before any modification (design rule 2), overwriting
/// a prior backup — lets a user roll back without git.
fn backup(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let bak = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.bak", ext.to_string_lossy()),
        None => "bak".to_string(),
    });
    fs::write(&bak, bytes).map_err(|e| format!("failed to back up {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn exe() -> PathBuf {
        PathBuf::from("/usr/local/bin/codekurve")
    }
    fn root() -> PathBuf {
        PathBuf::from("/home/user/project")
    }

    #[test]
    fn missing_file_is_created_no_backup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"]["codekurve"]["type"], "stdio");
        assert_eq!(
            value["mcpServers"]["codekurve"]["command"],
            "/usr/local/bin/codekurve"
        );
        assert!(!path.with_extension("json.bak").exists());
    }

    #[test]
    fn preserves_foreign_servers_and_backs_up() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let original = r#"{"mcpServers":{"other":{"command":"foo"}}}"#;
        fs::write(&path, original).unwrap();

        write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "foo");
        assert_eq!(value["mcpServers"]["codekurve"]["type"], "stdio");

        let bak_path = path.with_extension("json.bak");
        assert_eq!(fs::read_to_string(&bak_path).unwrap(), original);
    }

    #[test]
    fn install_twice_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap();
        write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn cursor_entry_has_no_type_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        write_json_client(&path, &exe(), &root(), "mcpServers", false).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["mcpServers"]["codekurve"].get("type").is_none());
    }

    #[test]
    fn malformed_json_is_rejected_and_file_untouched() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let original = "{ not valid json";
        fs::write(&path, original).unwrap();

        let err = write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap_err();
        assert!(err.contains("not valid JSON"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!path.with_extension("json.bak").exists());
    }

    #[test]
    fn wrong_shape_mcp_servers_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let original = r#"{"mcpServers": "not-an-object"}"#;
        fs::write(&path, original).unwrap();

        let err = write_json_client(&path, &exe(), &root(), "mcpServers", true).unwrap_err();
        assert!(err.contains("not an object"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn unsupported_client_makes_no_filesystem_changes() {
        let dir = tempdir().unwrap();
        let err = run(dir.path(), Some("vscode")).unwrap_err();
        assert!(err.contains("unsupported client"));
        assert!(err.contains("claude-code, cursor"));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn cursor_config_lands_under_dot_cursor_dir() {
        let dir = tempdir().unwrap();
        run(dir.path(), Some("cursor")).unwrap();
        assert!(dir.path().join(".cursor").join("mcp.json").exists());
    }

    #[test]
    fn claude_code_config_lands_at_root() {
        let dir = tempdir().unwrap();
        run(dir.path(), Some("claude-code")).unwrap();
        assert!(dir.path().join(".mcp.json").exists());
    }

    #[test]
    fn copilot_config_lands_under_dot_vscode_dir_under_servers_key() {
        let dir = tempdir().unwrap();
        run(dir.path(), Some("copilot")).unwrap();
        let path = dir.path().join(".vscode").join("mcp.json");
        assert!(path.exists());
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["servers"]["codekurve"]["type"], "stdio");
        assert!(value.get("mcpServers").is_none());
    }

    #[test]
    fn opencode_config_lands_at_root_with_single_command_array() {
        let dir = tempdir().unwrap();
        run(dir.path(), Some("opencode")).unwrap();
        let path = dir.path().join("opencode.json");
        assert!(path.exists());
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["mcp"]["codekurve"]["type"], "local");
        let command = value["mcp"]["codekurve"]["command"].as_array().unwrap();
        assert_eq!(command[1], "mcp");
        assert_eq!(command[2], "--root");
        assert!(value["mcp"]["codekurve"].get("args").is_none());
    }

    #[test]
    fn opencode_preserves_foreign_mcp_entries_and_backs_up() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        let original = r#"{"mcp":{"other":{"type":"remote","url":"https://example.com"}}}"#;
        fs::write(&path, original).unwrap();

        write_opencode_json(&path, &exe(), &root()).unwrap();

        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["mcp"]["other"]["type"], "remote");
        assert_eq!(value["mcp"]["codekurve"]["type"], "local");
        let bak_path = path.with_extension("json.bak");
        assert_eq!(fs::read_to_string(&bak_path).unwrap(), original);
    }

    #[test]
    fn codex_toml_created_fresh_no_backup() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_codex_toml(&path, &exe(), &root()).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["codekurve"]["command"].as_str().unwrap(),
            "/usr/local/bin/codekurve"
        );
        assert!(!path.with_extension("toml.bak").exists());
    }

    #[test]
    fn codex_toml_preserves_comments_and_foreign_servers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original =
            "# user comment\nmodel = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"foo\"\n";
        fs::write(&path, original).unwrap();

        write_codex_toml(&path, &exe(), &root()).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# user comment"));
        assert!(text.contains("model = \"gpt-5\""));

        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["other"]["command"].as_str().unwrap(),
            "foo"
        );
        assert_eq!(
            doc["mcp_servers"]["codekurve"]["command"].as_str().unwrap(),
            "/usr/local/bin/codekurve"
        );

        let bak_path = path.with_extension("toml.bak");
        assert_eq!(fs::read_to_string(&bak_path).unwrap(), original);
    }

    #[test]
    fn codex_toml_install_twice_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_codex_toml(&path, &exe(), &root()).unwrap();
        write_codex_toml(&path, &exe(), &root()).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["mcp_servers"].as_table().unwrap().len(), 1);
    }

    #[test]
    fn malformed_codex_toml_is_rejected_and_file_untouched() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "this is not [ valid toml";
        fs::write(&path, original).unwrap();

        let err = write_codex_toml(&path, &exe(), &root()).unwrap_err();
        assert!(err.contains("not valid TOML"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!path.with_extension("toml.bak").exists());
    }

    #[test]
    fn codex_toml_wrong_shape_mcp_servers_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "mcp_servers = \"not-a-table\"\n";
        fs::write(&path, original).unwrap();

        let err = write_codex_toml(&path, &exe(), &root()).unwrap_err();
        assert!(err.contains("not a table"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn codex_home_override_is_honored() {
        // ponytail: mutates a process-global env var; safe here because no
        // other test in this crate reads or writes CODEX_HOME.
        let dir = tempdir().unwrap();
        std::env::set_var("CODEX_HOME", dir.path());
        let resolved = codex_config_path().unwrap();
        std::env::remove_var("CODEX_HOME");
        assert_eq!(resolved, dir.path().join("config.toml"));
    }

    #[test]
    fn unsupported_client_message_lists_codex_cli() {
        let dir = tempdir().unwrap();
        let err = run(dir.path(), Some("vscode")).unwrap_err();
        assert!(err.contains("claude-code, cursor, codex-cli"));
    }
}
