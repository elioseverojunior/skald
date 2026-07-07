// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::Value;
use skald_lsp::{Outgoing, Server, frame};
use std::io::{Read, Write};

fn main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut server = Server::new();
    loop {
        // Read headers until blank line; parse Content-Length.
        let mut content_length = 0usize;
        let mut header = Vec::new();
        // read byte-by-byte until "\r\n\r\n"
        loop {
            let mut b = [0u8; 1];
            if stdin.read_exact(&mut b).is_err() {
                return;
            } // EOF
            header.push(b[0]);
            if header.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header_str = String::from_utf8_lossy(&header);
        for line in header_str.split("\r\n") {
            if let Some(v) = line.strip_prefix("Content-Length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        if content_length == 0 {
            continue;
        }
        let mut body = vec![0u8; content_length];
        if stdin.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        if method == "exit" {
            return;
        }
        for out in server.handle(&method, id, params) {
            let obj = match out {
                Outgoing::Response { id, result } => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
                }
                Outgoing::Notification { method, params } => {
                    serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params })
                }
            };
            let framed = frame(&obj.to_string());
            let _ = stdout.write_all(framed.as_bytes());
            let _ = stdout.flush();
        }
    }
}
