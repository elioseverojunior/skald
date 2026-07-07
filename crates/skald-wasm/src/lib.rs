// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use serde_json::{Value, json};
use wasm_bindgen::prelude::wasm_bindgen;

// ─── Pure logic (host-testable; no wasm, no unsafe) ────────────────────────────

fn parse_logic(text: &str) -> Result<(), String> {
    skald::from_str_node(text).map(|_| ()).map_err(|e| {
        let loc = e.span.map_or_else(String::new, |s| {
            format!("{}:{} ", s.start.line, s.start.column)
        });
        format!("{loc}{e}")
    })
}

fn format_logic(text: &str) -> Result<String, String> {
    skald::from_str_node(text).map_err(|e| format!("parse error: {e}"))?;
    Ok(skald::cst::Document::parse(text).reformatted())
}

fn validate_logic(text: &str, schema: &str) -> Result<String, String> {
    let schema_node =
        skald::from_str_node(schema).map_err(|e| format!("schema parse error: {e}"))?;
    let sc = skald::schema::Schema::from_node(&schema_node);
    let data_node = skald::from_str_node(text).map_err(|e| format!("parse error: {e}"))?;
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
    Ok(Value::Array(diags).to_string())
}

fn edit_logic(text: &str, path: &str, value: &str) -> Result<String, String> {
    let mut doc = skald::cst::Document::parse(text);
    match doc.set(path, value) {
        Ok(()) => Ok(doc.to_string()),
        Err(skald::cst::SetError::PathNotFound) if !path.contains('.') => {
            doc.insert(path, value).map_err(|e| e.to_string())?;
            Ok(doc.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// ─── wasm-bindgen delegates (thin) ─────────────────────────────────────────────

/// Returns `undefined` if `text` is valid YAML; throws the parse error otherwise.
#[wasm_bindgen]
pub fn yaml_parse(text: &str) -> Result<(), String> {
    parse_logic(text)
}

/// Formats `text` safely (trailing-whitespace trim + final newline), preserving
/// comments. Throws if `text` is not valid YAML.
#[wasm_bindgen]
pub fn yaml_format(text: &str) -> Result<String, String> {
    format_logic(text)
}

/// Validates `text` against the JSON-Schema `schema` (YAML/JSON source). Returns a
/// JSON-array string of diagnostics (empty array = valid). Throws if either input
/// fails to parse.
#[wasm_bindgen]
pub fn yaml_validate(text: &str, schema: &str) -> Result<String, String> {
    validate_logic(text, schema)
}

/// Sets the scalar at dotted `path` to `value` (comment-preserving); inserts a
/// top-level key if `path` is absent and has no `.`. Returns the new document.
/// Throws on a structural error.
#[wasm_bindgen]
pub fn yaml_edit(text: &str, path: &str, value: &str) -> Result<String, String> {
    edit_logic(text, path, value)
}

/// Returns the `skald-wasm` package version.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_and_err() {
        assert!(parse_logic("a: 1\n").is_ok());
        let e = parse_logic("a: [1, 2\n").unwrap_err();
        assert!(e.contains(':'), "error should carry a location: {e}");
    }

    #[test]
    fn format_trims_and_rejects_invalid() {
        assert_eq!(format_logic("a: 1   \nb: 2\n").unwrap(), "a: 1\nb: 2\n");
        assert!(format_logic("a: [1, 2").is_err());
    }

    #[test]
    fn format_preserves_comment() {
        assert_eq!(format_logic("a: 1  # c\n").unwrap(), "a: 1  # c\n");
    }

    #[test]
    fn validate_reports_and_passes() {
        let schema = "type: object\nproperties:\n  age: {type: integer}\n";
        let diags: Value =
            serde_json::from_str(&validate_logic("age: notnum\n", schema).unwrap()).unwrap();
        assert_eq!(diags.as_array().unwrap().len(), 1);
        let ok: Value = serde_json::from_str(&validate_logic("age: 7\n", schema).unwrap()).unwrap();
        assert_eq!(ok.as_array().unwrap().len(), 0);
    }

    #[test]
    fn validate_schema_parse_error_is_err() {
        assert!(validate_logic("a: 1\n", "").is_err());
    }

    #[test]
    fn edit_sets_existing_preserving_comment() {
        assert_eq!(
            edit_logic("a: 1  # keep\nb: 2\n", "a", "9").unwrap(),
            "a: 9  # keep\nb: 2\n"
        );
    }

    #[test]
    fn edit_inserts_absent_toplevel_key() {
        assert!(edit_logic("a: 1\n", "b", "2").unwrap().contains("b: 2"));
    }

    #[test]
    fn version_is_nonempty() {
        assert!(!version().is_empty());
    }
}
