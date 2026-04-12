//! Minimal MCP server: reads JSON-RPC from stdin, dispatches to
//! memory tools, writes responses to stdout.
//!
//! All logging goes to stderr — stdout is reserved for JSON-RPC.

use std::io::{BufRead, Write};

use grafeo::GrafeoDB;
use redb::Database;
use serde::{Deserialize, Serialize};

use crate::mcp::tools;

/// A JSON-RPC request (simplified).
///
/// `id` is optional — notifications (like `notifications/initialized`)
/// have no `id` and expect no response.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A JSON-RPC response (simplified).
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Run the MCP server loop.
///
/// Reads JSON-RPC requests from stdin (one per line), dispatches
/// to the appropriate tool, writes responses to stdout.
///
/// # Errors
///
/// Returns when stdin is closed or on I/O error.
#[allow(clippy::significant_drop_tightening)]
pub fn run_server(
    db: &Database,
    grafeo: &GrafeoDB,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let stdin_lock = stdin.lock();
    let mut stdout = std::io::stdout();
    let mut reader = std::io::BufReader::new(stdin_lock);

    loop {
        // Read using Content-Length framing (MCP/LSP protocol),
        // falling back to line-delimited for compatibility.
        let body = match read_message(&mut reader) {
            Ok(Some(b)) => b,
            Ok(None) => break, // EOF
            Err(e) => {
                eprintln!("lobster: read error: {e}");
                break;
            }
        };

        if body.trim().is_empty() {
            continue;
        }

        let req = match serde_json::from_str::<JsonRpcRequest>(&body) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: serde_json::Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {e}"),
                    }),
                };
                let json = serde_json::to_string(&resp)?;
                write_message(&mut stdout, &json)?;
                continue;
            }
        };

        // Notifications (no id) get no response
        if req.id.is_null() {
            continue;
        }

        let response = handle_request(&req, db, grafeo);
        let json = serde_json::to_string(&response)?;
        write_message(&mut stdout, &json)?;
    }

    Ok(())
}

/// Read a message using Content-Length framing or line-delimited fallback.
fn read_message(
    reader: &mut impl BufRead,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut header_line = String::new();
    let n = reader.read_line(&mut header_line)?;
    if n == 0 {
        return Ok(None); // EOF
    }

    let trimmed = header_line.trim();
    if trimmed.is_empty() {
        // Empty line — skip
        return Ok(Some(String::new()));
    }

    // Check for Content-Length header (MCP/LSP framing)
    if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
        let len: usize = len_str
            .trim()
            .parse()
            .map_err(|e| format!("invalid Content-Length: {e}"))?;

        // Read the blank separator line
        let mut blank = String::new();
        reader.read_line(&mut blank)?;

        // Read exactly `len` bytes of body
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        return Ok(Some(String::from_utf8(body)?));
    }

    // Fallback: treat the line itself as the message
    Ok(Some(trimmed.to_string()))
}

/// Write a message with Content-Length framing.
fn write_message(
    writer: &mut impl Write,
    json: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = json.as_bytes();
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

fn handle_request(
    req: &JsonRpcRequest,
    db: &Database,
    grafeo: &GrafeoDB,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => ok_response(req, initialize_result()),
        "tools/list" => ok_response(req, tools_list()),
        "tools/call" => handle_tools_call(req, db, grafeo),
        _ => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
            }),
        },
    }
}

fn ok_response(
    req: &JsonRpcRequest,
    result: serde_json::Value,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: req.id.clone(),
        result: Some(result),
        error: None,
    }
}

/// MCP `initialize` response: declare protocol version and capabilities.
fn initialize_result() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "lobster",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// MCP `tools/list` response: declare available tools with schemas.
fn tools_list() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "memory_context",
                "description": "Task-oriented context bundle: returns ranked decisions, summaries, tasks, and entities for the current situation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural-language query describing the current task or question"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memory_recent",
                "description": "List the newest ready artifacts (episodes, decisions, tasks).",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "memory_search",
                "description": "Search memory for ranked hits with snippets and confidence scores.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memory_decisions",
                "description": "Return decision timeline with rationale.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            },
            {
                "name": "memory_neighbors",
                "description": "Graph neighbor traversal from a given entity node.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "node_id": {
                            "type": "string",
                            "description": "ID of the node to get neighbors for"
                        }
                    },
                    "required": ["node_id"]
                }
            },
            {
                "name": "memory_status",
                "description": "Processing state diagnostics: episode counts, artifacts, pending/failed status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        ]
    })
}

/// Dispatch a `tools/call` request to the appropriate tool.
fn handle_tools_call(
    req: &JsonRpcRequest,
    db: &Database,
    grafeo: &GrafeoDB,
) -> JsonRpcResponse {
    let tool_name = req
        .params
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let arguments = req
        .params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let tool_result = match tool_name {
        "memory_context" => {
            let query = arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let bundle = tools::memory_context(query, db, grafeo);
            serde_json::to_value(bundle).ok()
        }
        "memory_recent" => {
            let result = tools::memory_recent(db, None);
            serde_json::to_value(result).ok()
        }
        "memory_search" => {
            let query = arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let result = tools::memory_search(query, db, grafeo);
            serde_json::to_value(result).ok()
        }
        "memory_decisions" => {
            let result = tools::memory_decisions(db);
            serde_json::to_value(result).ok()
        }
        "memory_neighbors" => {
            let node_id = arguments
                .get("node_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let result = tools::memory_neighbors(grafeo, node_id);
            serde_json::to_value(result).ok()
        }
        "memory_status" => {
            let report = crate::app::status::scan(db);
            serde_json::to_value(serde_json::json!({
                "episodes_total": report.total_episodes(),
                "ready": report.ready,
                "pending": report.pending,
                "retry_queued": report.retry_queued,
                "failed_final": report.failed_final,
                "summary_artifacts": report.summary_artifacts,
                "extraction_artifacts": report.extraction_artifacts,
            }))
            .ok()
        }
        _ => None,
    };

    tool_result.map_or_else(
        || JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!("Unknown tool: {tool_name}")
                }],
                "isError": true
            })),
            error: None,
        },
        |value| {
            // MCP tools/call response wraps result in content array
            let content = serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&value).unwrap_or_default()
                }]
            });
            ok_response(req, content)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: method.into(),
            params,
        }
    }

    #[test]
    fn test_initialize() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = make_req("initialize", serde_json::json!({}));
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "lobster");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn test_tools_list() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = make_req("tools/list", serde_json::json!({}));
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tool_list = result["tools"].as_array().unwrap();
        assert_eq!(tool_list.len(), 6);

        let names: Vec<&str> = tool_list
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"memory_context"));
        assert!(names.contains(&"memory_search"));
        assert!(names.contains(&"memory_status"));
    }

    #[test]
    fn test_tools_call_memory_context() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = make_req(
            "tools/call",
            serde_json::json!({
                "name": "memory_context",
                "arguments": {"query": "test"}
            }),
        );
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["content"].is_array());
        assert_eq!(result["content"][0]["type"], "text");
    }

    #[test]
    fn test_tools_call_memory_recent() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = make_req(
            "tools/call",
            serde_json::json!({"name": "memory_recent", "arguments": {}}),
        );
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["content"].is_array());
    }

    #[test]
    fn test_tools_call_unknown_tool() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req =
            make_req("tools/call", serde_json::json!({"name": "nonexistent"}));
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_none()); // MCP errors go in result, not JSON-RPC error
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn test_unknown_method() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = make_req("nonexistent", serde_json::json!({}));
        let resp = handle_request(&req, &db, &grafeo);

        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_response_serializes() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"items": []})),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_notification_has_null_id() {
        let json = r#"{
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert!(req.id.is_null());
    }
}
