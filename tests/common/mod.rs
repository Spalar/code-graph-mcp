//! Shared test utilities for integration/hardening tests.
//!
//! Provides helpers for building JSON-RPC tool calls and parsing responses.

#![allow(dead_code)]

use code_graph_mcp::mcp::server::McpServer;
use tempfile::TempDir;

/// Build a JSON-RPC 2.0 `tools/call` request string.
pub fn tool_call_json(tool_name: &str, args: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": args
        }
    })
    .to_string()
}

/// Extract the parsed tool result from a JSON-RPC response.
///
/// Assumes the response wraps the tool output as a JSON string inside
/// `result.content[0].text`.
pub fn parse_tool_result(response: &Option<String>) -> serde_json::Value {
    let resp = response
        .as_ref()
        .expect("parse_tool_result: response was None");
    let parsed: serde_json::Value = serde_json::from_str(resp)
        .unwrap_or_else(|e| panic!("parse_tool_result: invalid JSON: {e}\nraw: {resp}"));
    let text = parsed["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("parse_tool_result: unexpected response shape: {parsed}"));
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("parse_tool_result: inner text not JSON: {e}\ntext: {text}"))
}

/// Call `rebuild_index` until it is not `busy`, and return its result.
///
/// `busy` is the tool's retry signal: it waits up to 30 s for the background
/// embedding pass and then asks the caller to come back after
/// `retry_after_ms`. Under a loaded machine (a full `--features embed-model`
/// run beside other work) that pass took past 30 s and a test that read the
/// first answer failed with `left: "busy"` (D#56; reproduced with the wait
/// measured at 30,033 ms). A test of what a rebuild does retries, as a client
/// would; the deadline keeps a wedged embedding pass a failure.
pub fn rebuild_until_done(server: &McpServer) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let req = tool_call_json("rebuild_index", serde_json::json!({"confirm": true}));
        let result = parse_tool_result(&server.handle_message(&req).unwrap());
        if result["status"] != "busy" || std::time::Instant::now() > deadline {
            return result;
        }
        let ms = result["retry_after_ms"].as_u64().unwrap_or(2000);
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Create an McpServer from a TempDir project root and send the `initialize` handshake.
pub fn init_server(project: &TempDir) -> McpServer {
    let server = McpServer::from_project_root(project.path()).unwrap();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;
    server.handle_message(init).unwrap();
    server
}
