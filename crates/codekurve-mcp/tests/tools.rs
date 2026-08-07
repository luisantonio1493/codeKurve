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
/// `callGhost` calls a symbol that exists nowhere in the project, so the
/// analyzer records an `unresolved_references` row instead of an edge — the
/// fixture's populated case for `find_unresolved`. It adds no `Calls` edge,
/// so every other tool's expected counts are unchanged.
const B_TS: &str = "export function callAmbiguous(): boolean {\n  return getEligibility();\n}\n\nexport function callGhost(): void {\n  missingThing();\n}\n";
const CONTROLLER_CS: &str = "[ApiController]\n[Route(\"api/PatientReferrals\")]\npublic class PatientReferralsController\n{\n    [HttpPost(\"Submit\")]\n    public void Submit() {}\n}\n";

/// `init` + `index` via the real `codekurve` binary — same setup CLI golden
/// tests use (`crates/codekurve-bin/tests/graph_queries.rs`), so the MCP
/// server reads back exactly what the indexer produced.
fn seed_project(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("a.ts"), A_TS).unwrap();
    std::fs::write(root.join("src").join("b.ts"), B_TS).unwrap();
    std::fs::write(
        root.join("src").join("PatientReferralsController.cs"),
        CONTROLLER_CS,
    )
    .unwrap();
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

/// `project_overview`/`doctor`/`project_status` don't paginate, so their
/// envelope has no `total` key (`query::envelope(.., None)`, task 2.9's
/// "no `total` key" shape) — the same five fields [`assert_envelope_shape`]
/// checks, minus `total`.
fn assert_envelope_shape_without_total(envelope: &serde_json::Value) {
    let obj = envelope.as_object().expect("envelope must be an object");
    for field in [
        "schema_version",
        "project",
        "result",
        "warnings",
        "truncated",
    ] {
        assert!(
            obj.contains_key(field),
            "missing envelope field {field:?}: {envelope}"
        );
    }
    assert!(
        !obj.contains_key("total"),
        "unexpected total key: {envelope}"
    );
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

/// Golden coverage for all read tools, one fixture project,
/// covering both the "populated" and "small/empty result" shapes per tool
/// where the fixture allows it.
#[test]
fn all_read_tools_return_the_28_3_envelope() {
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
            "codekurve_doctor",
            "codekurve_find_callees",
            "codekurve_find_callers",
            "codekurve_find_implementations",
            "codekurve_find_references",
            "codekurve_find_routes",
            "codekurve_find_unresolved",
            "codekurve_get_symbol",
            "codekurve_project_overview",
            "codekurve_project_status",
            "codekurve_search_symbols",
            "codekurve_trace_path",
        ]
    );
    let search_schema = tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "codekurve_search_symbols")
        .unwrap();
    assert!(search_schema["inputSchema"]["properties"]
        .get("kinds")
        .is_none());
    assert!(search_schema["inputSchema"]["properties"]
        .get("languages")
        .is_none());
    assert!(search_schema["inputSchema"]["properties"]
        .get("path_prefix")
        .is_none());
    // `codekurve_reindex` stays absent — `[mcp] allow_reindex` defaults to
    // off (task 6.4, covered end-to-end in `tests/reindex.rs`).
    assert!(!tool_names.contains(&"codekurve_reindex"));

    let result = session.call(
        "codekurve_search_symbols",
        serde_json::json!({"query": "SubmitReferral"}),
    );
    let envelope = envelope_of(&result);
    assert!(envelope["result"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["name"] == "Submit"));

    let result = session.call(
        "codekurve_find_routes",
        serde_json::json!({"query": "POST /api/PatientReferrals/Submit"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(
        envelope["result"][0]["target_external"],
        "POST api/PatientReferrals/Submit"
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
    let rows = envelope["result"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(envelope["total"], 2);
    assert_eq!(envelope["truncated"], false);
    // The fixed anchor (getEligibility, what every caller row points at)
    // lives once at `result.anchor` — never repeated per row (§ token-cost:
    // it used to be 3 fields x every row, same value each time).
    assert_eq!(
        envelope["result"]["anchor"]["qualified_name"],
        "src/a.ts::getEligibility"
    );
    for field in ["path", "start_line", "confidence", "provenance"] {
        assert!(
            rows[0].get(field).is_some(),
            "caller row missing {field:?}: {}",
            rows[0]
        );
    }
    assert!(
        rows[0].get("target_qualified_name").is_none(),
        "caller row should not repeat the anchor: {}",
        rows[0]
    );

    // find_callers: `unused` has zero callers (small/empty case).
    let result = session.call(
        "codekurve_find_callers",
        serde_json::json!({"symbol_name": "src/a.ts::unused"}),
    );
    let envelope = envelope_of(&result);
    assert_eq!(envelope["result"]["rows"].as_array().unwrap().len(), 0);
    assert_eq!(envelope["result"]["anchor"], serde_json::Value::Null);
    assert_eq!(envelope["total"], 0);
    assert_eq!(envelope["truncated"], false);

    // find_callees: callLocal calls exactly getEligibility.
    let result = session.call(
        "codekurve_find_callees",
        serde_json::json!({"symbol_name": "src/a.ts::callLocal"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"]["rows"].as_array().unwrap().len(), 1);
    // Anchor-is-source here (callees points FROM callLocal) — no `external`
    // field, the anchor is always a project symbol.
    assert_eq!(
        envelope["result"]["anchor"]["qualified_name"],
        "src/a.ts::callLocal"
    );
    assert!(envelope["result"]["anchor"].get("external").is_none());

    // find_references: same underlying rows as find_callers here (no
    // non-call references in the fixture).
    let result = session.call(
        "codekurve_find_references",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"]["rows"].as_array().unwrap().len(), 2);

    // find_implementations: fixture has no interfaces/classes -> empty, not
    // an error.
    let result = session.call(
        "codekurve_find_implementations",
        serde_json::json!({"symbol_name": "src/a.ts::getEligibility"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["result"]["rows"].as_array().unwrap().len(), 0);

    // find_unresolved: the row `find_callers`/`find_references` can never
    // return, because the analyzer refused to guess an edge for it — the
    // §28.3 envelope plus the recorded `reason`.
    let result = session.call("codekurve_find_unresolved", serde_json::json!({}));
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    let rows = envelope["result"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(envelope["total"], 1);
    assert_eq!(rows[0]["target_text"], "missingThing");
    assert_eq!(rows[0]["source_qualified_name"], "src/b.ts::callGhost");
    for field in ["path", "kind", "reason", "confidence", "candidate_count"] {
        assert!(
            rows[0].get(field).is_some(),
            "unresolved row missing {field:?}: {}",
            rows[0]
        );
    }
    assert!(rows[0]["reason"].as_str().unwrap().contains("no matching"));

    // …and the exact-target filter, plus a miss returning an empty page
    // rather than an error.
    let result = session.call(
        "codekurve_find_unresolved",
        serde_json::json!({"target_text": "missingThing"}),
    );
    assert_eq!(envelope_of(&result)["result"].as_array().unwrap().len(), 1);
    let result = session.call(
        "codekurve_find_unresolved",
        serde_json::json!({"target_text": "missing"}),
    );
    let envelope = envelope_of(&result);
    assert_envelope_shape(&envelope);
    assert_eq!(envelope["total"], 0);

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

/// Unsupported filters are absent from the schema and rejected as unknown
/// fields instead of being advertised and then refused at runtime.
#[test]
fn search_symbols_rejects_unknown_filters() {
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
        let message = result["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("expected an invalid-params response for {args}: {result}"));
        assert!(
            message.contains("unknown field"),
            "unexpected error message for {args}: {message}"
        );
    }

    session.finish();
}

/// Route discovery must reject unsupported input rather than silently ignoring
/// it, and its bounded pages must still let a client retrieve the remainder.
#[test]
fn find_routes_paginates_and_rejects_unknown_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    let actions = (0..51)
        .map(|index| {
            format!("    [HttpGet(\"Route{index:02}\")]\n    public void Route{index:02}() {{}}\n")
        })
        .collect::<String>();
    std::fs::write(
        root.join("src").join("MoreRoutesController.cs"),
        format!(
            "[ApiController]\n[Route(\"api/MoreRoutes\")]\npublic class MoreRoutesController\n{{\n{actions}}}\n"
        ),
    )
    .unwrap();
    AssertCommand::cargo_bin("codekurve")
        .unwrap()
        .arg("index")
        .arg("--root")
        .arg(root)
        .assert()
        .success();

    let mut session = McpSession::start(root);
    let first =
        envelope_of(&session.call("codekurve_find_routes", serde_json::json!({"limit": 500})));
    assert_eq!(first["result"].as_array().unwrap().len(), 50);
    assert_eq!(first["total"], 52);
    assert_eq!(first["truncated"], true);

    let second = envelope_of(&session.call(
        "codekurve_find_routes",
        serde_json::json!({"limit": 500, "offset": 50}),
    ));
    assert_eq!(second["result"].as_array().unwrap().len(), 2);
    assert_eq!(second["total"], 52);
    assert_eq!(second["truncated"], false);
    assert_ne!(first["result"][0], second["result"][0]);

    let invalid = session.call("codekurve_find_routes", serde_json::json!({"page": 1}));
    let message = invalid["result"]["content"][0]["text"]
        .as_str()
        .expect("expected an invalid-params response");
    assert!(
        message.contains("unknown field"),
        "unexpected error: {message}"
    );

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
    let rows = envelope["result"]["rows"].as_array().unwrap();
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

/// Task 6.5: `NotIndexed` session (config present, no `codekurve index` run
/// yet) — query tools answer degraded (never a hard MCP protocol error) with
/// a warning, and never trigger an index run themselves.
#[test]
fn not_indexed_session_answers_degraded_without_auto_indexing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    codekurve_core::project::init(root).unwrap();
    let mut session = McpSession::start(root);

    // project_status: degraded data, not an error, with the NotIndexed
    // reason as the one warning.
    let result = session.call("codekurve_project_status", serde_json::json!({}));
    let envelope = envelope_of(&result);
    assert_eq!(envelope["result"]["stale"], true);
    assert_eq!(envelope["result"]["files"], 0);
    let warnings = envelope["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);

    // doctor: still answers, `index` check fails, no auto-index triggered.
    let result = session.call("codekurve_doctor", serde_json::json!({}));
    let envelope = envelope_of(&result);
    assert_eq!(envelope["result"]["ok"], false);
    let checks = envelope["result"]["checks"].as_array().unwrap();
    let index_check = checks
        .iter()
        .find(|c| c["name"] == "index")
        .expect("doctor must report an `index` check when NotIndexed");
    assert_eq!(index_check["ok"], false);

    // A query tool that needs a real index reports an error (not a silent
    // index run) — same `Session::indexed`/code-4 message every other tool
    // body already surfaces via `McpError::internal_error` (existing
    // convention, e.g. `search_symbols_rejects_each_unsupported_filter`'s
    // `invalid_params` case).
    let result = session.call(
        "codekurve_search_symbols",
        serde_json::json!({"query": "anything"}),
    );
    assert!(
        result["error"]["message"].as_str().is_some(),
        "search_symbols on a NotIndexed session must report an error, not success: {result}"
    );

    // Still no index run happened — no db file, no `.codekurve/index.db`.
    assert!(!root.join(".codekurve").join("index.db").exists());

    session.finish();
}

/// Task 6.6: stale index (`pending_files > 0`) is served as-is, warning set
/// — a query tool call must never trigger a reindex on its own.
#[test]
fn stale_index_is_served_as_is_never_auto_reindexed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);

    let db_path = root.join(".codekurve").join("index.db");
    let conn = codekurve_store::db::open(&db_path).unwrap();
    conn.execute("UPDATE index_state SET pending_files = 5", [])
        .unwrap();
    drop(conn);

    let mut session = McpSession::start(root);
    let result = session.call(
        "codekurve_search_symbols",
        serde_json::json!({"query": "getEligibility"}),
    );
    let envelope = envelope_of(&result);
    let warnings = envelope["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].as_str().unwrap().contains("stale"));
    // Results are still served from the existing (stale) index, not blocked.
    assert!(!envelope["result"].as_array().unwrap().is_empty());
    session.finish();

    // `pending_files` is untouched by the query call — no reindex ran.
    let conn = codekurve_store::db::open(&db_path).unwrap();
    let pending: i64 = conn
        .query_row("SELECT pending_files FROM index_state LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(pending, 5);
}

/// Task 6.8: golden coverage for `project_overview` and `doctor`.
#[test]
fn project_overview_and_doctor_return_the_28_3_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    seed_project(root);
    let mut session = McpSession::start(root);

    let result = session.call("codekurve_project_overview", serde_json::json!({}));
    let envelope = envelope_of(&result);
    // `project_overview`/`doctor` don't paginate — no `total` key (same
    // `envelope(.., None)` shape `codekurve_project_status` already uses).
    assert_envelope_shape_without_total(&envelope);
    assert_eq!(envelope["result"]["files"], 3);
    assert!(envelope["result"]["symbols"].as_u64().unwrap() > 0);
    let languages = envelope["result"]["languages"].as_array().unwrap();
    assert!(languages
        .iter()
        .any(|l| l["language"] == "typescript" && l["files"] == 2));
    assert_eq!(
        envelope["result"]["entry_points"][0]["target_external"],
        "POST api/PatientReferrals/Submit"
    );

    let result = session.call("codekurve_doctor", serde_json::json!({}));
    let envelope = envelope_of(&result);
    assert_envelope_shape_without_total(&envelope);
    assert_eq!(envelope["result"]["ok"], true);
    let checks = envelope["result"]["checks"].as_array().unwrap();
    for name in ["sqlite", "fts5", "schema", "project root", "config"] {
        assert!(
            checks.iter().any(|c| c["name"] == name),
            "doctor missing {name:?} check: {checks:?}"
        );
    }
    assert!(!checks.iter().any(|c| c["name"] == "index"));

    session.finish();
}
