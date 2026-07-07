// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::schema::{JsonType, Schema};
use skald_ast::node::Node;
use skald_core::types::Span;

/// A single validation failure, anchored to the offending node's source span.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaError {
    /// JSON-pointer-style path to the offending node (e.g. `/items/0/age`).
    pub path: String,
    /// Source span of the offending node.
    pub span: Span,
    /// Human-readable message.
    pub message: String,
}

/// Validates `data` against `schema`. Returns all violations (`Ok` if none).
pub fn validate(data: &Node<'_>, schema: &Schema) -> Result<(), Vec<SchemaError>> {
    let mut errors = Vec::new();
    validate_node(data, schema, String::new(), &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Classifies a plain scalar string into a [`JsonType`].
fn classify_scalar(value: &str) -> JsonType {
    match value {
        "null" | "Null" | "NULL" | "~" | "" => JsonType::Null,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => JsonType::Boolean,
        _ => {
            if value.parse::<i64>().is_ok() {
                JsonType::Integer
            } else if value.parse::<f64>().is_ok() {
                JsonType::Number
            } else {
                JsonType::String
            }
        }
    }
}

/// Returns the [`JsonType`] of a node.
fn json_type_of(node: &Node<'_>) -> JsonType {
    match node {
        Node::Scalar(s) => classify_scalar(&s.value),
        Node::Sequence(_) => JsonType::Array,
        Node::Mapping(_) => JsonType::Object,
    }
}

/// Returns `true` if `actual` satisfies `expected`, with the special rule that
/// `Integer` satisfies `Number`.
fn type_matches(expected: JsonType, actual: JsonType) -> bool {
    expected == actual || (expected == JsonType::Number && actual == JsonType::Integer)
}

/// Core recursive validator — appends any violations to `errors`.
fn validate_node(node: &Node<'_>, schema: &Schema, path: String, errors: &mut Vec<SchemaError>) {
    let span = node.span();
    let actual_type = json_type_of(node);

    // ── type ──────────────────────────────────────────────────────────────────
    if let Some(types) = &schema.types {
        if !types.iter().any(|&t| type_matches(t, actual_type)) {
            let type_names: Vec<&str> = types.iter().map(json_type_name).collect();
            errors.push(SchemaError {
                path: path.clone(),
                span,
                message: format!(
                    "expected type {} but got {}",
                    type_names.join(" or "),
                    json_type_name(&actual_type)
                ),
            });
            // Return early: further keyword checks would be noisy / wrong type.
            return;
        }
    }

    // ── const ─────────────────────────────────────────────────────────────────
    if let Some(expected) = &schema.const_value {
        if node.as_str().is_none_or(|s| s != expected) {
            errors.push(SchemaError {
                path: path.clone(),
                span,
                message: format!("value must equal const {expected:?}"),
            });
        }
    }

    // ── enum ──────────────────────────────────────────────────────────────────
    if let Some(allowed) = &schema.enum_values {
        if let Some(text) = node.as_str() {
            if !allowed.iter().any(|v| v == text) {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!(
                        "value {:?} is not one of [{}]",
                        text,
                        allowed
                            .iter()
                            .map(|s| format!("{s:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }
    }

    // ── numeric bounds ────────────────────────────────────────────────────────
    if schema.minimum.is_some() || schema.maximum.is_some() {
        if let Some(text) = node.as_str() {
            if let Ok(num) = text.parse::<f64>() {
                if let Some(min) = schema.minimum {
                    if num < min {
                        errors.push(SchemaError {
                            path: path.clone(),
                            span,
                            message: format!("value {num} is less than minimum {min}"),
                        });
                    }
                }
                if let Some(max) = schema.maximum {
                    if num > max {
                        errors.push(SchemaError {
                            path: path.clone(),
                            span,
                            message: format!("value {num} exceeds maximum {max}"),
                        });
                    }
                }
            }
        }
    }

    // ── string length ─────────────────────────────────────────────────────────
    if schema.min_length.is_some() || schema.max_length.is_some() {
        if let Some(text) = node.as_str() {
            let len = text.chars().count();
            if let Some(min) = schema.min_length {
                if len < min {
                    errors.push(SchemaError {
                        path: path.clone(),
                        span,
                        message: format!("string length {len} is less than minLength {min}"),
                    });
                }
            }
            if let Some(max) = schema.max_length {
                if len > max {
                    errors.push(SchemaError {
                        path: path.clone(),
                        span,
                        message: format!("string length {len} exceeds maxLength {max}"),
                    });
                }
            }
        }
    }

    // ── object keywords ───────────────────────────────────────────────────────
    if let Some(entries) = node.as_mapping() {
        // minProperties / maxProperties
        let prop_count = entries.len();
        if let Some(min) = schema.min_properties {
            if prop_count < min {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!("object has {prop_count} properties, minimum is {min}"),
                });
            }
        }
        if let Some(max) = schema.max_properties {
            if prop_count > max {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!("object has {prop_count} properties, maximum is {max}"),
                });
            }
        }

        // required
        for required_key in &schema.required {
            let present = entries
                .iter()
                .any(|(k, _)| k.as_str().is_some_and(|s| s == required_key));
            if !present {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!("required property {required_key:?} is missing"),
                });
            }
        }

        // properties + additionalProperties
        for (key_node, value_node) in entries {
            let key_str = key_node.as_str().unwrap_or("");
            let child_path = format!("{path}/{key_str}");

            if let Some(prop_schema) = schema.properties.get(key_str) {
                validate_node(value_node, prop_schema, child_path, errors);
            } else if schema.additional_properties == Some(false) {
                errors.push(SchemaError {
                    path: child_path,
                    span: value_node.span(),
                    message: format!("additional property {key_str:?} is not allowed"),
                });
            }
        }
    }

    // ── array keywords ────────────────────────────────────────────────────────
    if let Some(items) = node.as_sequence() {
        let item_count = items.len();

        if let Some(min) = schema.min_items {
            if item_count < min {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!("array has {item_count} items, minimum is {min}"),
                });
            }
        }
        if let Some(max) = schema.max_items {
            if item_count > max {
                errors.push(SchemaError {
                    path: path.clone(),
                    span,
                    message: format!("array has {item_count} items, maximum is {max}"),
                });
            }
        }

        if let Some(item_schema) = &schema.items {
            for (idx, item_node) in items.iter().enumerate() {
                validate_node(item_node, item_schema, format!("{path}/{idx}"), errors);
            }
        }
    }
}

/// Returns a human-readable name for a [`JsonType`].
fn json_type_name(t: &JsonType) -> &'static str {
    match t {
        JsonType::Null => "null",
        JsonType::Boolean => "boolean",
        JsonType::Integer => "integer",
        JsonType::Number => "number",
        JsonType::String => "string",
        JsonType::Array => "array",
        JsonType::Object => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Schema;

    fn node(s: &str) -> skald_ast::node::Node<'static> {
        skald_ast::composer::Composer::new(s)
            .next()
            .unwrap()
            .unwrap()
            .into_owned()
    }

    fn schema(s: &str) -> Schema {
        Schema::from_node(&node(s))
    }

    #[test]
    fn valid_object_passes() {
        let sc = schema(
            "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n  age: {type: integer}\n",
        );
        assert!(validate(&node("name: alice\nage: 30\n"), &sc).is_ok());
    }

    #[test]
    fn missing_required_fails_with_span() {
        let sc = schema("type: object\nrequired: [name]\n");
        let data = node("age: 30\n");
        let errs = validate(&data, &sc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.contains("name") && e.message.contains("required"))
        );
    }

    #[test]
    fn wrong_type_fails() {
        let sc = schema("type: object\nproperties:\n  age: {type: integer}\n");
        let errs = validate(&node("age: not_a_number\n"), &sc).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.message.to_lowercase().contains("integer")
                    || e.message.to_lowercase().contains("type"))
        );
    }

    #[test]
    fn enum_and_numeric_bounds() {
        let sc = schema(
            "type: object\nproperties:\n  color: {enum: [red, green, blue]}\n  n: {type: integer, minimum: 1, maximum: 10}\n",
        );
        assert!(validate(&node("color: red\nn: 5\n"), &sc).is_ok());
        assert!(validate(&node("color: purple\nn: 5\n"), &sc).is_err());
        assert!(validate(&node("color: red\nn: 99\n"), &sc).is_err());
    }

    #[test]
    fn additional_properties_false_rejects_extras() {
        let sc = schema(
            "type: object\nadditionalProperties: false\nproperties:\n  a: {type: integer}\n",
        );
        assert!(validate(&node("a: 1\n"), &sc).is_ok());
        assert!(validate(&node("a: 1\nb: 2\n"), &sc).is_err());
    }

    #[test]
    fn array_items_and_length() {
        let sc = schema("type: array\nminItems: 1\nmaxItems: 3\nitems: {type: integer}\n");
        assert!(validate(&node("[1, 2, 3]\n"), &sc).is_ok());
        assert!(validate(&node("[]\n"), &sc).is_err());
        assert!(validate(&node("[1, 2, 3, 4]\n"), &sc).is_err());
        assert!(validate(&node("[1, two, 3]\n"), &sc).is_err()); // wrong item type
    }

    #[test]
    fn integer_satisfies_number_type() {
        let sc = schema("type: number\n");
        assert!(validate(&node("42\n"), &sc).is_ok());
    }

    #[test]
    fn string_length_bounds() {
        let sc = schema("type: string\nminLength: 2\nmaxLength: 4\n");
        assert!(validate(&node("abc\n"), &sc).is_ok());
        assert!(validate(&node("a\n"), &sc).is_err());
        assert!(validate(&node("abcde\n"), &sc).is_err());
    }

    #[test]
    fn float_scalar_classified_as_number() {
        // A non-integer float scalar exercises classify_scalar's Number branch
        // and satisfies a `type: number` schema.
        let sc = schema("type: number\n");
        assert!(validate(&node("1.5\n"), &sc).is_ok());
        // The mismatch path also names the actual type "number".
        let strict = schema("type: integer\n");
        let errs = validate(&node("1.5\n"), &strict).unwrap_err();
        assert!(errs[0].message.contains("number"));
    }

    #[test]
    fn const_mismatch_reports_error() {
        // A value differing from `const` triggers the const-check error arm.
        let sc = schema("const: fixed\n");
        assert!(validate(&node("fixed\n"), &sc).is_ok());
        let errs = validate(&node("other\n"), &sc).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("const")));
    }

    #[test]
    fn minimum_violation_reported() {
        // A value below `minimum` exercises the `num < min` branch.
        let sc = schema("type: integer\nminimum: 10\n");
        let errs = validate(&node("3\n"), &sc).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("less than minimum")));
    }

    #[test]
    fn min_and_max_properties_violations() {
        let too_few = schema("type: object\nminProperties: 2\n");
        let errs = validate(&node("a: 1\n"), &too_few).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("minimum is 2")));

        let too_many = schema("type: object\nmaxProperties: 1\n");
        let errs = validate(&node("a: 1\nb: 2\n"), &too_many).unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("maximum is 1")));
    }

    #[test]
    fn json_type_name_covers_all_variants() {
        // Drive type-mismatch errors whose expected/actual names span every
        // JsonType arm of json_type_name.
        // expected null, actual boolean
        let e = validate(&node("true\n"), &schema("type: 'null'\n")).unwrap_err();
        assert!(e[0].message.contains("null") && e[0].message.contains("boolean"));
        // expected integer, actual string
        let e = validate(&node("hello\n"), &schema("type: integer\n")).unwrap_err();
        assert!(e[0].message.contains("integer") && e[0].message.contains("string"));
        // expected array, actual object
        let e = validate(&node("a: 1\n"), &schema("type: array\n")).unwrap_err();
        assert!(e[0].message.contains("array") && e[0].message.contains("object"));
        // expected object, actual array
        let e = validate(&node("[1]\n"), &schema("type: object\n")).unwrap_err();
        assert!(e[0].message.contains("object") && e[0].message.contains("array"));
    }
}
