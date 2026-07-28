//! Spawned-process integration tests (task 5.8, extends PR4's `handshake.rs`
//! harness style) for the eight PR5 read tools. One fixture project — see
//! `seed_project` — gives every scenario a controlled shape: `getEligibility`
//! (called by both `callLocal`, same file, and `callAmbiguous`, cross-file)
//! plus an unrelated `unused` symbol with zero callers, so `find_callers`
//! covers both a populated and an empty/small result.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use assert_cmd::Command as AssertCommand;

const A_TS: &str = "export function getEligibility(): boolean {\n  return true;\n}\n\nexport function callLocal(): boolean {\n  return getEligibility();\n}\n\nexport function unused(): void {}\n";
const B_TS: &str = "export function callAmbiguous(): boolean {\n  return getEligibility();\n}\n";

/// `init` + `index` via the real `codekurve` binary — same setup CLI golden
/// tests use (`crates/codekurve-bin/tests/graph_queries.rs`), so the MCP
/// server reads back exactly what the indexer produced.
fn seed_project(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.ts"), A_TS).unwrap();
    std::fs::write(root.join("src").join("b.ts"), B_TS).unwrap();
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

/// A live `codekurve mcp` JSON-RPC session over stdio (one line per
/// message, per `rmcp::transport::io`'s newline-delimited codec).
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
        drop(self.stdin); // close stdin -> server sees EOF and shuts down
        let mut child = self.child;
        let _ = child.wait();
    }
}

/// Successful `tools/call` responses carry the §27.5/§28.3 envelope as the
/// first content block's text.
fn envelope_of(call_result: &serde_json::Value) -> serde_json::Value {
    assert_eq!(
        call_result["result"]["isError"],
        serde_json::Value::Bool(false),
        "tool call reported an error: {call_result}"
    );
    let text = call_result["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("missing content[0].text: {call_result}"));
    serde_json::from_str(text).unwrap()
}

fn assert_envelope_shape(envelope: &serde_json::Value) {
    let obj = envelope.as_object().expect("envelope must be an object");
    for field in [
        "schema_version",
        "project",
        "result",
        "warnings",
        "truncated",
        "total",
    ] {
        assert!(
            obj.contains_key(field),
            "missing envelope field {field:?}: {envelope}"
        );
    }
}

/// Task 5.8: golden coverage for all eight tools, one fixture project,
/// covering both the "populated" and "small/empty result" shapes per tool
/// where the fixture allows it.
#[test]
fn all_eight_tools_return_the_28_3_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    let mut session = McpSession::start(root);

    let tools_list = session.list_tools();
    let mut tool_names: Vec<&str> = tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    tool_names.sort();
    assert_eq!(
        tool_names,
        vec![
            "codekurve_analyze_impact",
            "codekurve_find_callees",
            "codekurve_find_callers",
            "codekurve_find_implementations",
            "codekurve_find_references",
            "codekurve_get_symbol",
            "codekurve_project_status",
            "codekurve_search_symbols",
            "codekurve_trace_path",
        ]
    );

    // search_symbols
    let result = session.call(
        "codekurve_search_symbols",
        serde_json::json!({"query": "getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    let hits = envelope["result"].as_array().unwrap();
    assert!(!hits.is_empty());
    let hit = &hits[0];
    for field in ["path", "start_line", "end_line", "confidence", "provenance"] {
        assert!(
            hit.get(field).is_some(),
            "search row missing {field:?}: {hit}"
        );
    }
    let symbol_id = hit["id"].as_str().unwrap().to_string();

    // get_symbol, chained off the search hit's id (design "SymbolHit carries
    // id").
    let result = session.call("codekurve_get_symbol", serde_json::json!({"id": symbol_id}));
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(
        envelope["result"]["qualified_name"],
        "src/a.ts::getEligibility"
    );
    assert!(envelope["result"]["source"]
        .as_str()
        .unwrap()
        .contains("getEligibility"));
    assert_eq!(envelope["result"]["stale"], false);

    // find_callers: getEligibility has 2 callers (populated case).
    let result = session.call(
        "codekurve_find_callers",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    let rows = envelope["result"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(envelope["total"], 2);
    assert_eq!(envelope["truncated"], false);
    for field in ["path", "start_line", "confidence", "provenance"] {
        assert!(
            rows[0].get(field).is_some(),
            "caller row missing {field:?}: {}",
            rows[0]
        );
    }

    // find_callers: `unused` has zero callers (small/empty case).
    let result = session.call(
        "codekurve_find_callers",
        serde_json::json!({"symbol_name": "src/a.ts::unused"}),
    );
    let envelope = envelope_of(&result);
    assert_eq!(envelope["result"].as_array().unwrap().len(), 0);
    assert_eq!(envelope["total"], 0);
    assert_eq!(envelope["truncated"], false);

    // find_callees: callLocal calls exactly getEligibility.
    let result = session.call(
        "codekurve_find_callees",
        serde_json::json!({"symbol_name": "src/a.ts::callLocal"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"].as_array().unwrap().len(), 1);

    // find_references: same underlying rows as find_callers here (no
    // non-call references in the fixture).
    let result = session.call(
        "codekurve_find_references",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"].as_array().unwrap().len(), 2);

    // find_implementations: fixture has no interfaces/classes -> empty, not
    // an error.
    let result = session.call(
        "codekurve_find_implementations",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"].as_array().unwrap().len(), 0);

    // trace_path
    let result = session.call(
        "codekurve_trace_path",
        serde_json::json!({"symbol_name": "src/a.ts::callLocal", "to": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"]["path_found"], true);
    let reached = envelope["result"]["reached"].as_array().unwrap();
    assert!(!reached.is_empty());
    for field in ["path", "start_line", "end_line"] {
        assert!(
            reached[0].get(field).is_some(),
            "reached row missing {field:?}: {}",
            reached[0]
        );
    }

    // analyze_impact
    let result = session.call(
        "codekurve_analyze_impact",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert!(!envelope["result"]["reached"].as_array().unwrap().is_empty());

    session.finish();
}

/// Task 5.9: one unsupported-filter case per filter — `search_symbols` must
/// reject explicitly, not silently drop the filter.
#[test]
fn search_symbols_rejects_each_unsupported_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    let mut session = McpSession::start(root);

    for args in [
        serde_json::json!({"query": "getEligibility", "kinds": ["class"]}),
        serde_json::json!({"query": "getEligibility", "languages": ["typescript"]}),
        serde_json::json!({"query": "getEligibility", "path_prefix": "src/"}),
    ] {
        let result = session.call("codekurve_search_symbols", args.clone());
        let message = result["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a JSON-RPC error for {args}: {result}"));
        assert!(
            message.contains("filter not supported yet (supported: query, limit)"),
            "unexpected error message for {args}: {message}"
        );
    }

    session.finish();
}

/// Task 5.10: `--limit 1` on `find_callers` (2 real callers) caps the
/// result — `truncated: true` and `total` greater than the returned rows.
#[test]
fn capped_result_is_marked_truncated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    let mut session = McpSession::start(root);

    let result = session.call(
        "codekurve_find_callers",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility", "limit": 1}),
    );
    let envelope = envelope_of(&result);
    let rows = envelope["result"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(envelope["total"], 2);
    assert!(envelope["total"].as_u64().unwrap() > rows.len() as u64);
    assert_eq!(envelope["truncated"], true);

    session.finish();
}

/// Task 5.11: stale warning present when `pending_files > 0`, absent when
/// `0` — asserted via a direct `codekurve_project_status` call, never a
/// filesystem walk.
#[test]
fn stale_warning_reflects_pending_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    // Fresh index: no warning.
    let mut session = McpSession::start(root);
    let result = session.call("codekurve_project_status", serde_json::json!({}));
    let envelope = envelope_of(&result);
    assert_eq!(envelope["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(envelope["result"]["stale"], false);
    session.finish();

    // Force `pending_files` stale directly in the DB (same technique
    // `query.rs`'s own `warnings_wording_identical_regardless_of_caller`
    // test uses), then re-open a session against the same root. `init`'s
    // default config always writes the DB at `.codekurve/index.db`
    // (`codekurve_core::config::Storage::default`).
    let db_path = root.join(".codekurve").join("index.db");
    let conn = codekurve_store::db::open(&db_path).unwrap();
    conn.execute("UPDATE index_state SET pending_files = 3", [])
        .unwrap();
    drop(conn);

    let mut session = McpSession::start(root);
    let result = session.call("codekurve_project_status", serde_json::json!({}));
    let envelope = envelope_of(&result);
    let warnings = envelope["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().contains("stale"));
    assert_eq!(envelope["result"]["stale"], true);
    session.finish();
}
