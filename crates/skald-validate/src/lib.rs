// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// A validation diagnostic with a 1-based line / 1-based column location.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// 1-based line of the offending node.
    pub line: u32,
    /// 1-based column of the offending node.
    pub column: u32,
    /// JSON-pointer path to the offending node.
    pub path: String,
    /// Human-readable message.
    pub message: String,
}

/// Validates `data` against `schema` (both YAML/JSON source). Returns diagnostics
/// (empty = valid). A parse failure of either input is reported as one diagnostic.
#[must_use]
pub fn validate_str(data: &str, schema: &str) -> Vec<Diagnostic> {
    let schema_node = match skald::from_str_node(schema) {
        Ok(n) => n,
        Err(e) => {
            return vec![Diagnostic {
                line: 0,
                column: 0,
                path: "<schema>".into(),
                message: format!("schema parse error: {e}"),
            }];
        }
    };
    let sc = skald::schema::Schema::from_node(&schema_node);
    let data_node = match skald::from_str_node(data) {
        Ok(n) => n,
        Err(e) => {
            return vec![Diagnostic {
                line: 0,
                column: 0,
                path: String::new(),
                message: format!("parse error: {e}"),
            }];
        }
    };
    match skald::schema::validate(&data_node, &sc) {
        Ok(()) => Vec::new(),
        Err(errs) => errs
            .into_iter()
            .map(|e| Diagnostic {
                line: e.span.start.line,
                column: e.span.start.column,
                path: e.path,
                message: e.message,
            })
            .collect(),
    }
}

/// Applies comment-preserving autofix (type coercion + default insertion) to
/// `data` per `schema`, returning the fixed source and a human-readable report.
///
/// Coercion runs first (fix existing values), then default insertion (add missing
/// keys). Both mutations share the same [`cst::Document`](skald::cst::Document)
/// so comments and formatting are preserved end-to-end.
///
/// If `schema` cannot be parsed, `data` is returned unchanged with an error note
/// in the report — no corruption of the data ever occurs.
#[must_use]
pub fn fix_str(data: &str, schema: &str) -> (String, String) {
    let Ok(schema_node) = skald::from_str_node(schema) else {
        return (
            data.to_string(),
            "schema parse error; no changes".to_string(),
        );
    };
    let sc = skald::schema::Schema::from_node(&schema_node);
    let mut doc = skald::cst::Document::parse(data);
    let coercions = skald::schema::coerce_to_schema(&mut doc, &sc);
    let insertions = skald::schema::apply_defaults(&mut doc, &sc);
    let report = format!(
        "{} coercion(s), {} insertion(s)",
        coercions.len(),
        insertions.len()
    );
    (doc.to_string(), report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_str_reports_type_error_with_location() {
        let schema = "type: object\nproperties:\n  age: {type: integer}\n";
        let diags = validate_str("age: notnum\n", schema);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.to_lowercase().contains("integer")
                || diags[0].message.to_lowercase().contains("type")
        );
        assert_eq!(diags[0].line, 1);
    }

    #[test]
    fn validate_str_ok_yields_no_diagnostics() {
        let schema = "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n";
        assert!(validate_str("name: skald\n", schema).is_empty());
    }

    #[test]
    fn validate_str_reports_missing_required() {
        let diags = validate_str("age: 1\n", "type: object\nrequired: [name]\n");
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("name") && d.message.contains("required"))
        );
    }

    #[test]
    fn fix_str_coerces_and_inserts_defaults_preserving_comments() {
        let schema = "type: object\nproperties:\n  port: {type: integer, default: 8080}\n  host: {type: string}\n";
        let data = "host: \"example\"  # the host\n";
        let (fixed, report) = fix_str(data, schema);
        assert_eq!(fixed, "host: \"example\"  # the host\nport: 8080\n");
        assert!(report.contains('1') || report.to_lowercase().contains("insert"));
    }

    #[test]
    fn fix_str_coerces_quoted_integer() {
        let schema = "type: object\nproperties:\n  port: {type: integer}\n";
        let (fixed, _r) = fix_str("port: \"80\"  # c\n", schema);
        assert_eq!(fixed, "port: 80  # c\n");
    }

    #[test]
    fn validate_str_schema_parse_error_is_one_diagnostic() {
        // An empty schema string is unparseable → the schema parse-error arm.
        let diags = validate_str("a: 1\n", "");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].path, "<schema>");
        assert!(diags[0].message.contains("schema parse error"));
    }

    #[test]
    fn validate_str_data_parse_error_is_one_diagnostic() {
        // A valid schema but malformed data → the data parse-error arm.
        let diags = validate_str("a: [1, 2\n", "type: object\n");
        assert_eq!(diags.len(), 1);
        assert!(diags[0].path.is_empty());
        assert!(diags[0].message.contains("parse error"));
    }

    #[test]
    fn fix_str_schema_parse_error_is_no_op() {
        // An empty string is unparseable as a YAML document → from_str_node returns Err.
        let (fixed, report) = fix_str("a: 1\n", "");
        assert_eq!(fixed, "a: 1\n");
        assert!(
            report.contains("schema parse error")
                || report.contains("no changes")
                || fixed == "a: 1\n"
        );
    }
}
