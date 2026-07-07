// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Minimal YAML Language Server core: transport-agnostic dispatch + framing.

use serde_json::{Value, json};
use std::collections::HashMap;

/// An outgoing LSP message produced by handling an incoming one.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    /// A response to a request, carrying the request `id` and a `result`.
    Response {
        /// The request id this responds to.
        id: Value,
        /// The result payload.
        result: Value,
    },
    /// A server-initiated notification (method + params).
    Notification {
        /// The notification method name.
        method: String,
        /// The notification params.
        params: Value,
    },
}

/// LSP server state: open documents keyed by URI.
#[derive(Default)]
pub struct Server {
    /// Open documents (uri -> full text).
    docs: HashMap<String, String>,
}

impl Server {
    /// Creates an empty server.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles one incoming message. `id` is `Some` for requests, `None` for
    /// notifications. Returns the outgoing messages to emit (responses + notifications).
    pub fn handle(&mut self, method: &str, id: Option<Value>, params: Value) -> Vec<Outgoing> {
        match method {
            "initialize" => vec![Outgoing::Response {
                id: id.unwrap_or(Value::Null),
                result: json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "documentFormattingProvider": true
                    }
                }),
            }],
            "initialized" | "exit" => vec![],
            "shutdown" => vec![Outgoing::Response {
                id: id.unwrap_or(Value::Null),
                result: Value::Null,
            }],
            "textDocument/didOpen" => self.did_open(&params), // Task 2 (stub now)
            "textDocument/didChange" => self.did_change(&params), // Task 2 (stub now)
            "textDocument/formatting" => vec![Outgoing::Response {
                id: id.unwrap_or(Value::Null),
                result: self.format(&params), // Task 2 (stub now)
            }],
            _ => match id {
                Some(i) => vec![Outgoing::Response {
                    id: i,
                    result: Value::Null,
                }], // unknown request → null result
                None => vec![], // unknown notification → silent
            },
        }
    }

    fn did_open(&mut self, params: &Value) -> Vec<Outgoing> {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let text = params["textDocument"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        self.docs.insert(uri.clone(), text);
        self.publish_diagnostics(&uri)
    }

    fn did_change(&mut self, params: &Value) -> Vec<Outgoing> {
        let uri = params["textDocument"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        // Full sync: take the last contentChange's full text.
        if let Some(text) = params["contentChanges"]
            .as_array()
            .and_then(|c| c.last())
            .and_then(|c| c["text"].as_str())
            .map(str::to_string)
        {
            self.docs.insert(uri.clone(), text);
        }
        self.publish_diagnostics(&uri)
    }

    fn publish_diagnostics(&self, uri: &str) -> Vec<Outgoing> {
        let text = self.docs.get(uri).map(String::as_str).unwrap_or("");
        let diagnostics: Vec<Value> = match skald::from_str_node(text) {
            Ok(_) => Vec::new(),
            Err(e) => {
                // skald Position: line is 1-based, column is 1-based.
                // LSP positions are both 0-based.
                let (line, ch) = e.span.map_or((0u64, 0u64), |s| {
                    (
                        u64::from(s.start.line.saturating_sub(1)),
                        u64::from(s.start.column.saturating_sub(1)),
                    )
                });
                vec![json!({
                    "range": {
                        "start": { "line": line, "character": ch },
                        "end":   { "line": line, "character": ch + 1 }
                    },
                    "severity": 1,
                    "source": "skald",
                    "message": e.to_string(),
                })]
            }
        };
        vec![Outgoing::Notification {
            method: "textDocument/publishDiagnostics".to_string(),
            params: json!({ "uri": uri, "diagnostics": diagnostics }),
        }]
    }

    fn format(&mut self, params: &Value) -> Value {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        let Some(text) = self.docs.get(uri) else {
            return json!([]);
        };
        let formatted = skald::cst::Document::parse(text).reformatted();
        let end_line = text.matches('\n').count() as u64 + 1;
        json!([{
            "range": {
                "start": { "line": 0, "character": 0 },
                "end":   { "line": end_line, "character": 0 }
            },
            "newText": formatted,
        }])
    }
}

/// Frames a JSON body with the LSP `Content-Length` header.
#[must_use]
pub fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frames_roundtrip() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"x"}"#;
        let framed = frame(body);
        assert!(framed.starts_with("Content-Length: "));
        assert!(framed.ends_with(body));
    }

    #[test]
    fn initialize_advertises_capabilities() {
        let mut s = Server::new();
        let out = s.handle("initialize", Some(json!(1)), json!({}));
        let resp = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Response { id, result } if *id == json!(1) => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            resp["capabilities"]["documentFormattingProvider"],
            json!(true)
        );
        assert_eq!(resp["capabilities"]["textDocumentSync"], json!(1));
    }

    #[test]
    fn shutdown_returns_null_result() {
        let mut s = Server::new();
        let out = s.handle("shutdown", Some(json!(2)), json!(null));
        assert!(out.iter().any(
            |o| matches!(o, Outgoing::Response { id, result } if *id == json!(2) && result.is_null())
        ));
    }

    #[test]
    fn unknown_notification_is_silent() {
        let mut s = Server::new();
        assert!(s.handle("$/cancelRequest", None, json!({})).is_empty());
    }

    #[test]
    fn unknown_request_returns_null_result() {
        // An unrecognized method WITH an id is a request: reply with a null
        // result echoing the id (the `Some(i)` arm of the catch-all).
        let mut s = Server::new();
        let out = s.handle("textDocument/hover", Some(json!(42)), json!({}));
        assert!(out.iter().any(
            |o| matches!(o, Outgoing::Response { id, result } if *id == json!(42) && result.is_null())
        ));
    }

    #[test]
    fn did_open_invalid_yaml_publishes_diagnostic() {
        let mut s = Server::new();
        let out = s.handle(
            "textDocument/didOpen",
            None,
            json!({
                "textDocument": { "uri": "file:///t.yaml", "text": "a: [1, 2\n" }
            }),
        );
        let note = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Notification { method, params }
                    if method == "textDocument/publishDiagnostics" =>
                {
                    Some(params)
                }
                _ => None,
            })
            .expect("publishDiagnostics");
        assert_eq!(note["uri"], json!("file:///t.yaml"));
        let diags = note["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert!(diags[0]["range"]["start"]["line"].is_number());
        assert!(diags[0]["message"].is_string());
    }

    #[test]
    fn did_open_valid_yaml_publishes_empty_diagnostics() {
        let mut s = Server::new();
        let out = s.handle(
            "textDocument/didOpen",
            None,
            json!({
                "textDocument": { "uri": "file:///ok.yaml", "text": "a: 1\n" }
            }),
        );
        let note = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Notification { params, .. } => Some(params),
                _ => None,
            })
            .unwrap();
        assert_eq!(note["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn did_change_updates_and_rediagnoses() {
        let mut s = Server::new();
        s.handle(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///c.yaml", "text": "a: 1\n" } }),
        );
        let out = s.handle(
            "textDocument/didChange",
            None,
            json!({
                "textDocument": { "uri": "file:///c.yaml" },
                "contentChanges": [ { "text": "a: [1, 2\n" } ]
            }),
        );
        let note = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Notification { params, .. } => Some(params),
                _ => None,
            })
            .unwrap();
        assert_eq!(note["diagnostics"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn formatting_returns_text_edit() {
        let mut s = Server::new();
        s.handle(
            "textDocument/didOpen",
            None,
            json!({ "textDocument": { "uri": "file:///f.yaml", "text": "a: 1   \n" } }),
        );
        let out = s.handle(
            "textDocument/formatting",
            Some(json!(9)),
            json!({ "textDocument": { "uri": "file:///f.yaml" } }),
        );
        let edits = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Response { id, result } if *id == json!(9) => Some(result),
                _ => None,
            })
            .unwrap();
        let arr = edits.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["newText"], json!("a: 1\n"));
    }

    #[test]
    fn formatting_unknown_doc_returns_empty() {
        let mut s = Server::new();
        let out = s.handle(
            "textDocument/formatting",
            Some(json!(1)),
            json!({ "textDocument": { "uri": "file:///nope.yaml" } }),
        );
        let edits = out
            .iter()
            .find_map(|o| match o {
                Outgoing::Response { result, .. } => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(edits.as_array().unwrap().len(), 0);
    }
}
