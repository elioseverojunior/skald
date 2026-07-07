// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Minimal YAML MCP server core: transport-agnostic dispatch + newline framing.
//!
//! Implements the [Model Context Protocol](https://modelcontextprotocol.io/)
//! JSON-RPC 2.0 over newline-delimited transport (one JSON object per line).

use serde_json::{Value, json};

// ─── Protocol types ───────────────────────────────────────────────────────────

/// An outgoing MCP message produced by handling an incoming request.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    /// A successful JSON-RPC 2.0 response.
    Response {
        /// The request id this responds to.
        id: Value,
        /// The result payload.
        result: Value,
    },
    /// A JSON-RPC 2.0 error response.
    Error {
        /// The request id this error responds to.
        id: Value,
        /// The error code (JSON-RPC standard: -32600 invalid, -32601 method not found, etc.).
        code: i64,
        /// Human-readable error message.
        message: String,
    },
}

// ─── Server ──────────────────────────────────────────────────────────────────

/// MCP server. Each request is handled purely; the server holds no
/// cross-request state in this subset of the protocol.
#[derive(Default)]
pub struct Server;

impl Server {
    /// Creates a new MCP server.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Handles one incoming JSON-RPC request and returns the outgoing messages.
    ///
    /// `id` is `Some` for requests (which expect a response), `None` for
    /// notifications (which do not).
    pub fn handle(&mut self, method: &str, id: Option<Value>, params: Value) -> Vec<Outgoing> {
        match method {
            "initialize" => vec![response(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "skald-mcp", "version": env!("CARGO_PKG_VERSION") }
                }),
            )],
            "notifications/initialized" => vec![],
            "ping" => vec![response(id, json!({}))],
            "tools/list" => vec![response(id, json!({ "tools": tools_list() }))],
            "tools/call" => vec![response(id, self.tools_call(&params))],
            _ => match id {
                Some(i) => vec![error_response(
                    Some(i),
                    -32601,
                    format!("method not found: {method}"),
                )],
                None => vec![],
            },
        }
    }

    /// Dispatches a `tools/call` request. Returns the MCP content-envelope `Value`.
    fn tools_call(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = &params["arguments"];
        match name {
            "yaml_parse" => tool_parse(args),
            "yaml_validate" => tool_validate(args),
            "yaml_format" => tool_format(args),
            "yaml_edit" => tool_edit(args),
            other => content_error(format!("unknown tool: {other}")),
        }
    }
}

// ─── Tool registry ────────────────────────────────────────────────────────────

/// Returns the list of tools advertised by this server.
///
/// Each entry is a JSON object with `name`, `description`, and `inputSchema`
/// following the MCP `Tool` schema.
#[must_use]
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "yaml_parse",
            "description": "Parse YAML and report whether it is valid; on failure returns the error with line/column.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "YAML source to parse." }
                },
                "required": ["text"]
            }
        }),
        json!({
            "name": "yaml_validate",
            "description": "Validate YAML against a JSON-Schema (YAML/JSON source); returns span-anchored diagnostics.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":   { "type": "string", "description": "YAML source to validate." },
                    "schema": { "type": "string", "description": "JSON Schema (as YAML or JSON string)." }
                },
                "required": ["text", "schema"]
            }
        }),
        json!({
            "name": "yaml_format",
            "description": "Format YAML safely (trailing-whitespace trim + final newline), preserving comments.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "YAML source to format." }
                },
                "required": ["text"]
            }
        }),
        json!({
            "name": "yaml_edit",
            "description": "Set a value at a dotted path (comment-preserving); inserts a top-level key if the path is absent.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":  { "type": "string", "description": "YAML source to edit." },
                    "path":  { "type": "string", "description": "Dotted key path, e.g. `settings.debug`." },
                    "value": { "type": "string", "description": "New scalar value to write." }
                },
                "required": ["text", "path", "value"]
            }
        }),
    ]
}

// ─── Framing ─────────────────────────────────────────────────────────────────

/// Frames a JSON body as a single newline-terminated line for MCP transport.
///
/// MCP uses newline-delimited JSON-RPC (one JSON object per `\n`) rather than
/// the `Content-Length` header framing used by LSP.
#[must_use]
pub fn frame(body: &str) -> String {
    format!("{body}\n")
}

// ─── Envelope helpers ─────────────────────────────────────────────────────────

/// Builds a successful JSON-RPC [`Outgoing::Response`] for the given `id` and `result`.
///
/// This is the transport-layer success builder. The tool-result *content*
/// envelope (`{ "content": [...], "isError": .. }`) is built separately by the
/// `content_ok` / `content_error` helpers and carried as this response's `result`.
#[must_use]
pub fn response(id: Option<Value>, result: Value) -> Outgoing {
    Outgoing::Response {
        id: id.unwrap_or(Value::Null),
        result,
    }
}

/// Builds a JSON-RPC [`Outgoing::Error`] for the given `id`, error `code`, and `message`.
#[must_use]
pub fn error_response(id: Option<Value>, code: i64, message: impl Into<String>) -> Outgoing {
    Outgoing::Error {
        id: id.unwrap_or(Value::Null),
        code,
        message: message.into(),
    }
}

/// Wraps a successful tool text payload in the MCP content envelope.
fn content_ok(text: impl Into<String>) -> Value {
    json!({ "content": [ { "type": "text", "text": text.into() } ], "isError": false })
}

/// Wraps a tool failure in the MCP content envelope with `isError: true`.
fn content_error(text: impl Into<String>) -> Value {
    json!({ "content": [ { "type": "text", "text": text.into() } ], "isError": true })
}

// ─── Tool implementations ─────────────────────────────────────────────────────

fn tool_parse(args: &Value) -> Value {
    let text = args["text"].as_str().unwrap_or("");
    match skald::from_str_node(text) {
        Ok(_) => content_ok("valid"),
        Err(e) => {
            let loc = e.span.map_or_else(String::new, |s| {
                format!("{}:{} ", s.start.line, s.start.column)
            });
            content_error(format!("{loc}{e}"))
        }
    }
}

fn tool_validate(args: &Value) -> Value {
    let text = args["text"].as_str().unwrap_or("");
    let schema_src = args["schema"].as_str().unwrap_or("");
    let schema_node = match skald::from_str_node(schema_src) {
        Ok(n) => n,
        Err(e) => return content_error(format!("schema parse error: {e}")),
    };
    let sc = skald::schema::Schema::from_node(&schema_node);
    let data_node = match skald::from_str_node(text) {
        Ok(n) => n,
        Err(e) => return content_error(format!("parse error: {e}")),
    };
    let diags: Vec<Value> = match skald::schema::validate(&data_node, &sc) {
        Ok(()) => Vec::new(),
        Err(errs) => errs
            .into_iter()
            .map(|e| {
                json!({
                    "path": e.path,
                    "line": e.span.start.line,
                    "column": e.span.start.column,
                    "message": e.message,
                })
            })
            .collect(),
    };
    // Compact JSON array as the text payload (machine-readable for the agent).
    content_ok(Value::Array(diags).to_string())
}

fn tool_format(args: &Value) -> Value {
    let text = args["text"].as_str().unwrap_or("");
    match skald::from_str_node(text) {
        Ok(_) => content_ok(skald::cst::Document::parse(text).reformatted()),
        Err(e) => content_error(format!("parse error: {e}")),
    }
}

fn tool_edit(args: &Value) -> Value {
    let text = args["text"].as_str().unwrap_or("");
    let path = args["path"].as_str().unwrap_or("");
    let value = args["value"].as_str().unwrap_or("");
    let mut doc = skald::cst::Document::parse(text);
    match doc.set(path, value) {
        Ok(()) => content_ok(doc.to_string()),
        Err(skald::cst::SetError::PathNotFound) if !path.contains('.') => {
            match doc.insert(path, value) {
                Ok(()) => content_ok(doc.to_string()),
                Err(e) => content_error(e.to_string()),
            }
        }
        Err(e) => content_error(e.to_string()),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frame_appends_newline() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let framed = frame(body);
        assert_eq!(framed, format!("{body}\n"));
        assert!(framed.ends_with('\n'));
    }

    #[test]
    fn initialize_returns_protocol_version_and_capabilities() {
        let mut s = Server::new();
        let out = s.handle("initialize", Some(json!(1)), json!({}));
        let result = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Response { id, result } if *id == json!(1) => Some(result),
                _ => None,
            })
            .expect("initialize response");
        assert_eq!(result["protocolVersion"], json!("2024-11-05"));
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], json!("skald-mcp"));
    }

    #[test]
    fn ping_returns_empty_object() {
        let mut s = Server::new();
        let out = s.handle("ping", Some(json!(2)), json!(null));
        assert!(out.iter().any(
            |o| matches!(o, Outgoing::Response { id, result } if *id == json!(2) && result == &json!({}))
        ));
    }

    #[test]
    fn tools_list_response_contains_four_tools() {
        let mut s = Server::new();
        let out = s.handle("tools/list", Some(json!(3)), json!({}));
        let result = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Response { id, result } if *id == json!(3) => Some(result),
                _ => None,
            })
            .expect("tools/list response");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"yaml_parse"));
        assert!(names.contains(&"yaml_validate"));
        assert!(names.contains(&"yaml_format"));
        assert!(names.contains(&"yaml_edit"));
    }

    #[test]
    fn unknown_method_returns_error() {
        let mut s = Server::new();
        let out = s.handle("nonexistent/method", Some(json!(4)), json!({}));
        assert!(out.iter().any(
            |o| matches!(o, Outgoing::Error { id, code, .. } if *id == json!(4) && *code == -32601)
        ));
    }

    #[test]
    fn unknown_notification_is_silent() {
        let mut s = Server::new();
        let out = s.handle("$/something", None, json!({}));
        assert!(out.is_empty());
    }

    // ─── Task 2: tool dispatch tests ─────────────────────────────────────────

    fn call(s: &mut Server, name: &str, args: Value) -> Value {
        let out = s.handle(
            "tools/call",
            Some(json!(7)),
            json!({ "name": name, "arguments": args }),
        );
        out.into_iter()
            .find_map(|o| match o {
                Outgoing::Response { result, .. } => Some(result),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn parse_valid_is_ok() {
        let mut s = Server::new();
        let r = call(&mut s, "yaml_parse", json!({ "text": "a: 1\n" }));
        assert_eq!(r["isError"], json!(false));
    }

    #[test]
    fn parse_invalid_is_error_with_location() {
        let mut s = Server::new();
        let r = call(&mut s, "yaml_parse", json!({ "text": "a: [1, 2\n" }));
        assert_eq!(r["isError"], json!(true));
        assert!(r["content"][0]["text"].as_str().unwrap().contains(':'));
    }

    #[test]
    fn validate_reports_type_error() {
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_validate",
            json!({
                "text": "age: notnum\n",
                "schema": "type: object\nproperties:\n  age: {type: integer}\n"
            }),
        );
        let text = r["content"][0]["text"].as_str().unwrap();
        let diags: Value = serde_json::from_str(text).unwrap();
        assert_eq!(diags.as_array().unwrap().len(), 1);
        assert!(diags[0]["message"].is_string());
        assert!(diags[0]["line"].is_number());
    }

    #[test]
    fn validate_ok_is_empty_array() {
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_validate",
            json!({
                "text": "name: skald\n",
                "schema": "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n"
            }),
        );
        let diags: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(diags.as_array().unwrap().len(), 0);
        assert_eq!(r["isError"], json!(false));
    }

    #[test]
    fn format_trims_trailing_ws_and_preserves_comment() {
        let mut s = Server::new();
        let r = call(&mut s, "yaml_format", json!({ "text": "a: 1   # c\n" }));
        assert_eq!(r["isError"], json!(false));
        // The scanner bakes trailing spaces before a comment into the token;
        // reformatted() preserves inline comments verbatim, so the exact input
        // is unchanged (no trailing ws after the comment, comment preserved).
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("# c"));
        assert!(!text.ends_with("  \n"));
    }

    #[test]
    fn format_invalid_is_error() {
        let mut s = Server::new();
        let r = call(&mut s, "yaml_format", json!({ "text": "a: [1, 2" }));
        assert_eq!(r["isError"], json!(true));
    }

    #[test]
    fn edit_sets_existing_value_preserving_comment() {
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_edit",
            json!({ "text": "a: 1  # keep\nb: 2\n", "path": "a", "value": "9" }),
        );
        assert_eq!(r["isError"], json!(false));
        assert_eq!(
            r["content"][0]["text"].as_str().unwrap(),
            "a: 9  # keep\nb: 2\n"
        );
    }

    #[test]
    fn edit_inserts_absent_toplevel_key() {
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_edit",
            json!({ "text": "a: 1\n", "path": "b", "value": "2" }),
        );
        assert_eq!(r["isError"], json!(false));
        assert!(r["content"][0]["text"].as_str().unwrap().contains("b: 2"));
    }

    #[test]
    fn unknown_tool_is_error() {
        let mut s = Server::new();
        let r = call(&mut s, "nope", json!({}));
        assert_eq!(r["isError"], json!(true));
    }

    #[test]
    fn validate_invalid_schema_is_error() {
        // A malformed schema document hits the schema parse-error arm.
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_validate",
            json!({ "text": "a: 1\n", "schema": "schema: [1, 2\n" }),
        );
        assert_eq!(r["isError"], json!(true));
        assert!(
            r["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("schema parse error")
        );
    }

    #[test]
    fn validate_invalid_data_is_error() {
        // A valid schema but malformed data hits the data parse-error arm.
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_validate",
            json!({ "text": "a: [1, 2\n", "schema": "type: object\n" }),
        );
        assert_eq!(r["isError"], json!(true));
        assert!(
            r["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("parse error")
        );
    }

    #[test]
    fn edit_insert_into_non_mapping_root_is_error() {
        // set → PathNotFound (no dot) → insert → NotAMapping (scalar root) → the
        // inner insert Err arm.
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_edit",
            json!({ "text": "just a scalar\n", "path": "newkey", "value": "v" }),
        );
        assert_eq!(r["isError"], json!(true));
    }

    #[test]
    fn edit_unresolved_dotted_path_is_error() {
        // A dotted path that does not resolve returns PathNotFound WITH a dot,
        // which falls through to the outer set Err arm (no insert attempt).
        let mut s = Server::new();
        let r = call(
            &mut s,
            "yaml_edit",
            json!({ "text": "a: 1\n", "path": "a.b.c", "value": "v" }),
        );
        assert_eq!(r["isError"], json!(true));
    }
}
