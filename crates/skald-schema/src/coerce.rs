// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::schema::{JsonType, Schema};
use skald_cst::Document;

/// A single value coercion that was applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Coercion {
    /// Dotted path to the coerced value.
    pub path: String,
    /// The original value text.
    pub from: String,
    /// The new (canonical) value text.
    pub to: String,
}

/// Coerces scalar values in `doc` to match `schema`'s declared scalar types,
/// editing the CST in place (comments preserved). Returns the coercions applied.
///
/// Handles object properties, array items (indexed until a gap), multi-type
/// schemas (first matching type wins), and top-level arrays.
#[must_use]
pub fn coerce_to_schema(doc: &mut Document, schema: &Schema) -> Vec<Coercion> {
    let mut changes = Vec::new();
    coerce_at(doc, schema, "", &mut changes);
    changes
}

fn coerce_at(doc: &mut Document, schema: &Schema, path: &str, changes: &mut Vec<Coercion>) {
    // Object: recurse into each declared property.
    if !schema.properties.is_empty() {
        for (name, sub) in &schema.properties {
            let child = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            coerce_at(doc, sub, &child, changes);
        }
        return;
    }
    // Array: coerce each item against `items` (indices until a gap).
    if let Some(item_schema) = &schema.items {
        let mut i = 0usize;
        loop {
            let item_path = if path.is_empty() {
                i.to_string()
            } else {
                format!("{path}.{i}")
            };
            if doc.get(&item_path).is_none() {
                break;
            }
            coerce_at(doc, item_schema, &item_path, changes);
            i += 1;
        }
        return;
    }
    // Scalar leaf.
    if path.is_empty() {
        return;
    }
    let Some(types) = schema.types.as_deref() else {
        return;
    };
    let Some(current) = doc.get(path).map(str::to_string) else {
        return;
    };
    if let Some(canonical) = coerce_value_multi(&current, types)
        && canonical != current
        && doc.set(path, &canonical).is_ok()
    {
        changes.push(Coercion {
            path: path.to_string(),
            from: current,
            to: canonical,
        });
    }
}

/// Tries each declared type in order, returning the first successful coercion.
fn coerce_value_multi(text: &str, types: &[JsonType]) -> Option<String> {
    types.iter().find_map(|ty| coerce_value(text, *ty))
}

/// Canonical text for `text` under `ty`, or `None` if not coercible / already canonical.
fn coerce_value(text: &str, ty: JsonType) -> Option<String> {
    let inner = strip_quotes(text);
    match ty {
        JsonType::Integer => inner.parse::<i64>().ok().map(|i| i.to_string()),
        JsonType::Number => inner.parse::<f64>().ok().map(|_| inner.to_string()),
        JsonType::Boolean => match inner.to_ascii_lowercase().as_str() {
            "true" => Some("true".to_string()),
            "false" => Some("false".to_string()),
            _ => None,
        },
        JsonType::String => {
            if text.starts_with('"') || text.starts_with('\'') {
                None
            } else if inner.parse::<f64>().is_ok()
                || matches!(
                    inner.to_ascii_lowercase().as_str(),
                    "true" | "false" | "null"
                )
            {
                Some(format!("\"{inner}\""))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A default value that was inserted into the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Insertion {
    /// Dotted path of the inserted property.
    pub path: String,
    /// The inserted value text.
    pub value: String,
}

/// Inserts properties that declare a scalar `default` and are absent from
/// `doc`, recursing into nested object subschemas whose parent key is present.
/// Edits the CST comment-preservingly and returns the insertions made.
#[must_use]
pub fn apply_defaults(doc: &mut Document, schema: &Schema) -> Vec<Insertion> {
    let mut inserted = Vec::new();
    apply_defaults_at(doc, schema, "", &mut inserted);
    inserted
}

fn apply_defaults_at(
    doc: &mut Document,
    schema: &Schema,
    parent_path: &str,
    inserted: &mut Vec<Insertion>,
) {
    for (name, sub) in &schema.properties {
        let path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{parent_path}.{name}")
        };
        if let Some(def) = &sub.default {
            if doc.get(&path).is_none() && doc.insert_at(parent_path, name, def).is_ok() {
                inserted.push(Insertion {
                    path: path.clone(),
                    value: def.clone(),
                });
            }
        } else if !sub.properties.is_empty() && doc.get(&path).is_some() {
            apply_defaults_at(doc, sub, &path, inserted);
        }
    }
}

fn strip_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Schema;

    fn schema(s: &str) -> Schema {
        Schema::from_node(
            &skald_ast::composer::Composer::new(s)
                .next()
                .unwrap()
                .unwrap()
                .into_owned(),
        )
    }

    #[test]
    fn coerces_quoted_integer_and_preserves_comment() {
        let sc = schema("type: object\nproperties:\n  port: {type: integer}\n");
        let mut doc = skald_cst::Document::parse("port: \"8080\"  # the port\n");
        let changes = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "port: 8080  # the port\n");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "port");
    }

    #[test]
    fn coerces_boolean_case() {
        let sc = schema("type: object\nproperties:\n  flag: {type: boolean}\n");
        let mut doc = skald_cst::Document::parse("flag: True\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "flag: true\n");
    }

    #[test]
    fn coerces_number_to_string_by_quoting() {
        let sc = schema("type: object\nproperties:\n  id: {type: string}\n");
        let mut doc = skald_cst::Document::parse("id: 123\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "id: \"123\"\n");
    }

    #[test]
    fn no_change_when_already_canonical() {
        let sc =
            schema("type: object\nproperties:\n  port: {type: integer}\n  name: {type: string}\n");
        let mut doc = skald_cst::Document::parse("port: 8080\nname: \"alice\"\n");
        let changes = coerce_to_schema(&mut doc, &sc);
        assert!(changes.is_empty());
        assert_eq!(doc.to_string(), "port: 8080\nname: \"alice\"\n");
    }

    #[test]
    fn recurses_into_nested_objects() {
        let sc = schema(
            "type: object\nproperties:\n  server: {type: object, properties: {port: {type: integer}}}\n",
        );
        let mut doc = skald_cst::Document::parse("server:\n  port: \"9090\"  # keep\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "server:\n  port: 9090  # keep\n");
    }

    #[test]
    fn apply_defaults_inserts_missing_property() {
        let sc = schema(
            "type: object\nproperties:\n  port: {type: integer, default: 8080}\n  host: {type: string, default: localhost}\n",
        );
        let mut doc = skald_cst::Document::parse("host: example.com  # explicit\n");
        let inserted = apply_defaults(&mut doc, &sc);
        assert_eq!(
            doc.to_string(),
            "host: example.com  # explicit\nport: 8080\n"
        );
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].path, "port");
    }

    #[test]
    fn apply_defaults_no_op_when_all_present() {
        let sc = schema("type: object\nproperties:\n  a: {default: 1}\n");
        let mut doc = skald_cst::Document::parse("a: 5\n");
        assert!(apply_defaults(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "a: 5\n");
    }

    #[test]
    fn apply_defaults_into_empty_doc() {
        let sc = schema("type: object\nproperties:\n  x: {default: hello}\n");
        let mut doc = skald_cst::Document::parse("");
        let _ = apply_defaults(&mut doc, &sc);
        assert_eq!(doc.to_string(), "x: hello\n");
    }

    #[test]
    fn coerces_array_of_quoted_integers() {
        let sc =
            schema("type: object\nproperties:\n  ports: {type: array, items: {type: integer}}\n");
        let mut doc = skald_cst::Document::parse("ports:\n  - \"80\"  # http\n  - \"443\"\n");
        let changes = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "ports:\n  - 80  # http\n  - 443\n");
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn coerces_array_of_objects() {
        let sc = schema(
            "type: object\nproperties:\n  servers: {type: array, items: {type: object, properties: {port: {type: integer}}}}\n",
        );
        let mut doc =
            skald_cst::Document::parse("servers:\n  - port: \"8080\"\n  - port: \"9090\"  # alt\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(
            doc.to_string(),
            "servers:\n  - port: 8080\n  - port: 9090  # alt\n"
        );
    }

    #[test]
    fn multi_type_coerces_to_first_matching() {
        let sc = schema("type: object\nproperties:\n  v: {type: [integer, string]}\n");
        let mut doc = skald_cst::Document::parse("v: \"42\"\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "v: 42\n");
    }

    #[test]
    fn multi_type_leaves_non_numeric_string_alone() {
        let sc = schema("type: object\nproperties:\n  v: {type: [integer, string]}\n");
        let mut doc = skald_cst::Document::parse("v: hello\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "v: hello\n");
    }

    #[test]
    fn top_level_array_coerced() {
        let sc = schema("type: array\nitems: {type: integer}\n");
        let mut doc = skald_cst::Document::parse("- \"1\"\n- \"2\"\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "- 1\n- 2\n");
    }

    #[test]
    fn apply_defaults_nested() {
        let sc = schema(
            "type: object\nproperties:\n  server: {type: object, properties: {host: {default: localhost}, port: {type: integer, default: 8080}}}\n",
        );
        let mut doc = skald_cst::Document::parse("server:\n  host: example.com  # explicit\n");
        let inserted = apply_defaults(&mut doc, &sc);
        assert_eq!(
            doc.to_string(),
            "server:\n  host: example.com  # explicit\n  port: 8080\n"
        );
        assert_eq!(
            inserted.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(),
            vec!["server.port"]
        );
    }

    #[test]
    fn apply_defaults_skips_missing_parent() {
        let sc = schema(
            "type: object\nproperties:\n  server: {type: object, properties: {port: {default: 8080}}}\n  name: {default: app}\n",
        );
        let mut doc = skald_cst::Document::parse("name: x\n");
        let inserted = apply_defaults(&mut doc, &sc);
        assert!(inserted.is_empty());
        assert_eq!(doc.to_string(), "name: x\n");
    }

    #[test]
    fn apply_defaults_top_level_still_works() {
        let sc = schema("type: object\nproperties:\n  port: {default: 8080}\n");
        let mut doc = skald_cst::Document::parse("name: x\n");
        let _ = apply_defaults(&mut doc, &sc);
        assert_eq!(doc.to_string(), "name: x\nport: 8080\n");
    }

    #[test]
    fn root_scalar_leaf_is_left_alone() {
        // A scalar schema at the root path ("") hits the `path.is_empty()` early
        // return in the scalar-leaf branch.
        let sc = schema("type: integer\n");
        let mut doc = skald_cst::Document::parse("\"5\"\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "\"5\"\n");
    }

    #[test]
    fn property_without_declared_type_is_skipped() {
        // A property schema with no `type` leaves the value untouched (the
        // `schema.types` None early return).
        let sc = schema("type: object\nproperties:\n  v: {description: free}\n");
        let mut doc = skald_cst::Document::parse("v: \"7\"\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "v: \"7\"\n");
    }

    #[test]
    fn coerces_quoted_number_unquotes_it() {
        // A quoted value under `type: number` strips the quotes to the canonical
        // numeric text (the Number coercion arm).
        let sc = schema("type: object\nproperties:\n  ratio: {type: number}\n");
        let mut doc = skald_cst::Document::parse("ratio: \"1.5\"\n");
        let changes = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "ratio: 1.5\n");
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn coerces_quoted_false_boolean() {
        // Exercises the boolean "false" arm.
        let sc = schema("type: object\nproperties:\n  flag: {type: boolean}\n");
        let mut doc = skald_cst::Document::parse("flag: \"FALSE\"\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "flag: false\n");
    }

    #[test]
    fn boolean_non_bool_value_is_not_coerced() {
        // A non-boolean value under `type: boolean` hits the `_ => None` arm.
        let sc = schema("type: object\nproperties:\n  flag: {type: boolean}\n");
        let mut doc = skald_cst::Document::parse("flag: maybe\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "flag: maybe\n");
    }

    #[test]
    fn string_type_quotes_bareword_booleans_and_null() {
        // A bare `true`/`null` under `type: string` is quoted (the special-word
        // `matches!` arm of the String coercion).
        let sc = schema("type: object\nproperties:\n  a: {type: string}\n  b: {type: string}\n");
        let mut doc = skald_cst::Document::parse("a: true\nb: null\n");
        let _ = coerce_to_schema(&mut doc, &sc);
        assert_eq!(doc.to_string(), "a: \"true\"\nb: \"null\"\n");
    }

    #[test]
    fn typed_property_absent_from_doc_is_skipped() {
        // The schema declares a typed property that the document does not contain;
        // `doc.get(path)` is None → the scalar-leaf early return.
        let sc = schema("type: object\nproperties:\n  missing: {type: integer}\n");
        let mut doc = skald_cst::Document::parse("present: 1\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "present: 1\n");
    }

    #[test]
    fn null_typed_value_is_not_coercible() {
        // `type: null` has no canonical coercion → the `_ => None` arm of
        // coerce_value. The value is left untouched.
        let sc = schema("type: object\nproperties:\n  v: {type: 'null'}\n");
        let mut doc = skald_cst::Document::parse("v: anything\n");
        assert!(coerce_to_schema(&mut doc, &sc).is_empty());
        assert_eq!(doc.to_string(), "v: anything\n");
    }
}
