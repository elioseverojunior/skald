// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::Value;
use skald_mcp::{Outgoing, Server, frame};
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut server = Server::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                let _ = out.write_all(frame(&err.to_string()).as_bytes());
                let _ = out.flush();
                continue;
            }
        };

        let method = msg["method"].as_str().unwrap_or("").to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let is_exit = method == "exit";

        let responses = server.handle(&method, id, params);
        for outgoing in responses {
            let json_obj = match outgoing {
                Outgoing::Response { id, result } => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                }),
                Outgoing::Error { id, code, message } => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code, "message": message }
                }),
            };
            let _ = out.write_all(frame(&json_obj.to_string()).as_bytes());
        }
        let _ = out.flush();

        if is_exit {
            break;
        }
    }
}
