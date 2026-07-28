//! Spawned-process integration test (task 4.10, design "stdout Discipline"
//! layer 2): a real MCP handshake over stdio against the `codekurve` binary,
//! asserting every stdout line is valid JSON-RPC — under both a clean env
//! and a deliberately noisy `RUST_LOG=trace` one.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// One line per JSON-RPC message (the codec in `rmcp::transport::io` is
/// newline-delimited); returns the raw lines the server wrote to stdout so
/// the caller can both parse responses and assert protocol purity.
fn run_handshake(root: &std::path::Path, extra_env: &[(&str, &str)]) -> Vec<String> {
    let bin = assert_cmd::cargo::cargo_bin("codekurve");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp")
        .arg("--root")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn codekurve mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let mut send = |value: serde_json::Value| {
        let mut line = value.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
    };
    let read_line = |reader: &mut BufReader<std::process::ChildStdout>| -> String {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "server closed stdout unexpectedly");
        line
    };

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "codekurve-mcp-test", "version": "0.0.0"},
        },
    }));
    let initialize_response = read_line(&mut reader);

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    }));

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {},
    }));
    let tools_list_response = read_line(&mut reader);

    send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "codekurve_project_status", "arguments": {}},
    }));
    let call_tool_response = read_line(&mut reader);

    drop(stdin); // close stdin -> server sees EOF and shuts down
    let _ = child.wait();

    vec![initialize_response, tools_list_response, call_tool_response]
}

#[test]
fn handshake_and_project_status_call_stay_clean_jsonrpc() {
    let tmp = tempfile::tempdir().unwrap();
    codekurve_core::project::init(tmp.path()).unwrap();

    let lines = run_handshake(tmp.path(), &[]);
    assert_all_lines_are_jsonrpc(&lines);

    let tools_list: serde_json::Value = serde_json::from_str(lines[1].trim()).unwrap();
    let tool_names: Vec<&str> = tools_list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(tool_names, vec!["codekurve_project_status"]);

    let call_result: serde_json::Value = serde_json::from_str(lines[2].trim()).unwrap();
    assert_eq!(
        call_result["result"]["isError"],
        serde_json::Value::Bool(false)
    );
    let text = call_result["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(envelope["schema_version"], 1);
    assert!(envelope["result"]["stale"].is_boolean());
}

#[test]
fn noisy_logging_env_does_not_leak_onto_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    codekurve_core::project::init(tmp.path()).unwrap();

    let lines = run_handshake(tmp.path(), &[("RUST_LOG", "trace")]);
    assert_all_lines_are_jsonrpc(&lines);
}

fn assert_all_lines_are_jsonrpc(lines: &[String]) {
    for line in lines {
        let trimmed = line.trim();
        assert!(!trimmed.is_empty(), "blank line on stdout");
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .unwrap_or_else(|e| panic!("non-JSON-RPC line on stdout: {trimmed:?} ({e})"));
        assert_eq!(
            value["jsonrpc"], "2.0",
            "line missing jsonrpc 2.0: {trimmed}"
        );
    }
}
