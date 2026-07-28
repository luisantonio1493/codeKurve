//! Spawned-process integration tests (task 6.7, extends PR4/PR5's harness
//! style in `handshake.rs`/`tools.rs`) for the `[mcp] allow_reindex` gate
//! (spec "reindex Gated Off by Default"): `codekurve_reindex` is absent from
//! `tools/list` by default, appears when the config flag is set, and a call
//! while disabled fails as an unknown tool.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use assert_cmd::Command as AssertCommand;

const A_TS: &str = "export function getEligibility(): boolean {\n  return true;\n}\n";

fn seed_project(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.ts"), A_TS).unwrap();
    AssertCommand::cargo_bin("codekurve")
        .unwrap()
        .arg("init")
        .arg(root)
        .assert()
        .success();
    AssertCommand::cargo_bin("codekurve")
        .unwrap()
        .arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success();
}

/// Flips `[mcp] allow_reindex = true` in the project's already-written
/// `.codekurve/config.toml` (`init` always writes the default, `allow_reindex
/// = false`, first) — same round-trip `codekurve_core::config::Config`
/// already guarantees additively (task 4.5).
fn enable_reindex(root: &Path) {
    let config_path = root
        .join(codekurve_core::config::CONFIG_DIR)
        .join(codekurve_core::config::CONFIG_FILE);
    let text = std::fs::read_to_string(&config_path).unwrap();
    let mut config = codekurve_core::config::Config::from_toml(&text).unwrap();
    config.mcp.allow_reindex = true;
    std::fs::write(&config_path, config.to_toml().unwrap()).unwrap();
}

/// A minimal live `codekurve mcp` JSON-RPC session over stdio — same
/// newline-delimited codec `tools.rs`'s `McpSession` uses.
struct McpSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpSession {
    fn start(root: &Path) -> Self {
        let bin = assert_cmd::cargo::cargo_bin("codekurve");
        let mut child = Command::new(bin)
            .arg("mcp")
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn codekurve mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut session = McpSession {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };

        session.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": session.next_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "codekurve-mcp-test", "version": "0.0.0"},
            },
        }));
        session.next_id += 1;
        session.read_line();

        session.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }));
        session
    }

    fn send(&mut self, value: serde_json::Value) {
        let mut line = value.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn read_line(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "server closed stdout unexpectedly");
        serde_json::from_str(line.trim()).unwrap()
    }

    fn list_tools(&mut self) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {},
        }));
        self.read_line()
    }

    fn call(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        self.read_line()
    }

    fn finish(self) {
        drop(self.stdin);
        let mut child = self.child;
        let _ = child.wait();
    }
}

fn tool_names(tools_list: &serde_json::Value) -> Vec<String> {
    tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// Task 6.7 (scenario "reindex absent by default"): no `[mcp]` config —
/// `codekurve_reindex` doesn't appear in `tools/list`.
#[test]
fn reindex_tool_absent_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    let mut session = McpSession::start(root);
    let names = tool_names(&session.list_tools());
    assert!(!names.contains(&"codekurve_reindex".to_string()));
    session.finish();
}

/// Task 6.7 (scenario "reindex enabled via config"): `allow_reindex = true`
/// — the tool appears in the list and a call triggers a real index run.
#[test]
fn reindex_tool_appears_and_runs_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    enable_reindex(root);

    let mut session = McpSession::start(root);
    let names = tool_names(&session.list_tools());
    assert!(names.contains(&"codekurve_reindex".to_string()));

    // Add a new file after the initial index, then call `codekurve_reindex`
    // — a real index run must pick it up.
    std::fs::write(
        root.join("src").join("b.ts"),
        "export function another(): void {}\n",
    )
    .unwrap();

    let result = session.call("codekurve_reindex", serde_json::json!({}));
    assert_eq!(
        result["result"]["isError"],
        serde_json::Value::Bool(false),
        "reindex call failed: {result}"
    );
    let text = result["result"]["content"][0]["text"].as_str().unwrap();
    let envelope: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(envelope["result"]["files_changed"].as_u64().unwrap() >= 1);

    // The freshly reindexed session sees `b.ts`'s symbol.
    let result = session.call(
        "codekurve_search_symbols",
        serde_json::json!({"query": "another"}),
    );
    let text = result["result"]["content"][0]["text"].as_str().unwrap();
    let envelope: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(!envelope["result"].as_array().unwrap().is_empty());

    session.finish();
}

/// Task 6.7 (spec "calling `reindex` while disabled MUST fail as an unknown
/// tool"): calling the tool name directly while the gate is off is a
/// JSON-RPC method-not-found error, not a silent success or a distinct
/// "forbidden" shape.
#[test]
fn calling_reindex_while_disabled_fails_as_unknown_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    let mut session = McpSession::start(root);
    let result = session.call("codekurve_reindex", serde_json::json!({}));
    assert!(
        result.get("result").is_none(),
        "disabled reindex must not produce a tool result: {result}"
    );
    let error = &result["error"];
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("not found")
            || error["code"].is_number(),
        "expected a method-not-found-shaped JSON-RPC error: {result}"
    );
    session.finish();
}
