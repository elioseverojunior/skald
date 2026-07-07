// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::BTreeMap;

use skald_ast::node::Node;

/// JSON value types recognized by the `type` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    /// null
    Null,
    /// boolean
    Boolean,
    /// integer
    Integer,
    /// number
    Number,
    /// string
    String,
    /// array
    Array,
    /// object
    Object,
}

/// A parsed JSON-Schema (draft 2020-12 subset). Unknown keywords are ignored.
#[derive(Debug, Clone, Default)]
pub struct Schema {
    /// Allowed `type`(s).
    pub types: Option<Vec<JsonType>>,
    /// Required property names.
    pub required: Vec<String>,
    /// Per-property subschemas.
    pub properties: BTreeMap<String, Schema>,
    /// `additionalProperties: false` rejects unlisted keys.
    pub additional_properties: Option<bool>,
    /// Subschema applied to every array item.
    pub items: Option<Box<Schema>>,
    /// Allowed scalar values (`enum`), compared as text.
    pub enum_values: Option<Vec<String>>,
    /// Required exact scalar value (`const`), as text.
    pub const_value: Option<String>,
    /// Inclusive numeric lower bound.
    pub minimum: Option<f64>,
    /// Inclusive numeric upper bound.
    pub maximum: Option<f64>,
    /// Minimum string length.
    pub min_length: Option<usize>,
    /// Maximum string length.
    pub max_length: Option<usize>,
    /// Minimum array length.
    pub min_items: Option<usize>,
    /// Maximum array length.
    pub max_items: Option<usize>,
    /// Minimum object property count.
    pub min_properties: Option<usize>,
    /// Maximum object property count.
    pub max_properties: Option<usize>,
    /// Default scalar value (`default`), as source text. Object/array defaults are ignored.
    pub default: Option<String>,
}

/// Maps a `type` string to a [`JsonType`].
#[must_use]
pub fn parse_json_type(s: &str) -> Option<JsonType> {
    Some(match s {
        "null" => JsonType::Null,
        "boolean" => JsonType::Boolean,
        "integer" => JsonType::Integer,
        "number" => JsonType::Number,
        "string" => JsonType::String,
        "array" => JsonType::Array,
        "object" => JsonType::Object,
        _ => return None,
    })
}

impl Schema {
    /// Parses a `Schema` from a schema-document node. A non-mapping node yields
    /// an empty (accept-all) schema; unknown keywords are ignored.
    #[must_use]
    pub fn from_node(node: &Node<'_>) -> Schema {
        let mut s = Schema::default();
        let Some(entries) = node.as_mapping() else {
            return s;
        };
        for (k, v) in entries {
            match k.as_str() {
                Some("type") => s.types = parse_types(v),
                Some("required") => s.required = parse_string_list(v),
                Some("properties") => {
                    if let Some(props) = v.as_mapping() {
                        for (pk, pv) in props {
                            if let Some(name) = pk.as_str() {
                                s.properties.insert(name.to_string(), Schema::from_node(pv));
                            }
                        }
                    }
                }
                Some("additionalProperties") => {
                    s.additional_properties = v.as_str().map(|x| x == "true");
                }
                Some("items") => s.items = Some(Box::new(Schema::from_node(v))),
                Some("enum") => s.enum_values = Some(parse_string_list(v)),
                Some("const") => s.const_value = v.as_str().map(str::to_string),
                Some("minimum") => s.minimum = v.as_str().and_then(|x| x.parse().ok()),
                Some("maximum") => s.maximum = v.as_str().and_then(|x| x.parse().ok()),
                Some("minLength") => s.min_length = v.as_str().and_then(|x| x.parse().ok()),
                Some("maxLength") => s.max_length = v.as_str().and_then(|x| x.parse().ok()),
                Some("minItems") => s.min_items = v.as_str().and_then(|x| x.parse().ok()),
                Some("maxItems") => s.max_items = v.as_str().and_then(|x| x.parse().ok()),
                Some("minProperties") => {
                    s.min_properties = v.as_str().and_then(|x| x.parse().ok());
                }
                Some("maxProperties") => {
                    s.max_properties = v.as_str().and_then(|x| x.parse().ok());
                }
                Some("default") => s.default = v.as_str().map(str::to_string),
                _ => {}
            }
        }
        s
    }
}

fn parse_types(v: &Node<'_>) -> Option<Vec<JsonType>> {
    if let Some(t) = v.as_str() {
        parse_json_type(t).map(|jt| vec![jt])
    } else {
        v.as_sequence().map(|items| {
            items
                .iter()
                .filter_map(|n| n.as_str().and_then(parse_json_type))
                .collect()
        })
    }
}

fn parse_string_list(v: &Node<'_>) -> Vec<String> {
    v.as_sequence()
        .map(|items| {
            items
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(s: &str) -> skald_ast::node::Node<'static> {
        skald_ast::composer::Composer::new(s)
            .next()
            .unwrap()
            .unwrap()
            .into_owned()
    }

    #[test]
    fn parses_object_schema() {
        let n = node(
            "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n  age: {type: integer}\n",
        );
        let s = Schema::from_node(&n);
        assert!(
            s.types
                .as_ref()
                .is_some_and(|t| t.contains(&JsonType::Object))
        );
        assert_eq!(s.required, vec!["name".to_string()]);
        assert!(s.properties.contains_key("name"));
        assert!(s.properties.contains_key("age"));
    }

    #[test]
    fn parses_bounds_and_enum() {
        let n = node("type: integer\nminimum: 1\nmaximum: 10\n");
        let s = Schema::from_node(&n);
        assert_eq!(s.minimum, Some(1.0));
        assert_eq!(s.maximum, Some(10.0));
        let e = Schema::from_node(&node("enum: [a, b, c]\n"));
        assert_eq!(e.enum_values.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn unknown_type_string_yields_none() {
        // An unrecognized `type` value hits parse_json_type's `_ => None` arm;
        // parse_types maps that None through, leaving `types` as None.
        let s = Schema::from_node(&node("type: bogus\n"));
        assert!(s.types.is_none());
        assert_eq!(parse_json_type("bogus"), None);
    }

    #[test]
    fn non_mapping_node_is_accept_all_schema() {
        // A scalar schema document is not a mapping → empty (accept-all) schema.
        let s = Schema::from_node(&node("just a scalar\n"));
        assert!(s.types.is_none());
        assert!(s.properties.is_empty());
    }

    #[test]
    fn parses_property_count_bounds() {
        let s = Schema::from_node(&node("type: object\nminProperties: 1\nmaxProperties: 5\n"));
        assert_eq!(s.min_properties, Some(1));
        assert_eq!(s.max_properties, Some(5));
    }

    #[test]
    fn unknown_keyword_is_ignored() {
        // An unrecognized keyword hits the `_ => {}` arm and is silently skipped.
        let s = Schema::from_node(&node("type: integer\n$comment: hello\nfoo: bar\n"));
        assert!(
            s.types
                .as_ref()
                .is_some_and(|t| t.contains(&JsonType::Integer))
        );
    }
}
