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
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
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

        let response = match serde_json::from_str::<JsonRpcRequest>(&body) {
            Ok(req) => handle_request(&req, db, grafeo),
            Err(e) => JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: serde_json::Value::Null,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {e}"),
                }),
            },
        };

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
    let result = match req.method.as_str() {
        "memory_context" => {
            let query = req
                .params
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
            let query = req
                .params
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
            let node_id = req
                .params
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

    result.map_or_else(
        || JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {}", req.method),
            }),
        },
        |val| JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: Some(val),
            error: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jsonrpc_request() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "memory_context",
            "params": {"query": "storage decision"}
        }"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "memory_context");
        assert_eq!(req.params["query"].as_str().unwrap(), "storage decision");
    }

    #[test]
    fn test_handle_memory_context() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "memory_context".into(),
            params: serde_json::json!({"query": "test"}),
        };

        let resp = handle_request(&req, &db, &grafeo);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_memory_recent() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(2),
            method: "memory_recent".into(),
            params: serde_json::json!({}),
        };

        let resp = handle_request(&req, &db, &grafeo);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_handle_unknown_method() {
        let db = crate::store::db::open_in_memory().unwrap();
        let grafeo = crate::graph::db::new_in_memory();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(3),
            method: "nonexistent".into(),
            params: serde_json::json!({}),
        };

        let resp = handle_request(&req, &db, &grafeo);
        assert!(resp.result.is_none());
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
        // error should not appear when None
        assert!(!json.contains("error"));
    }
}
