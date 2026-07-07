// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML serializer.
//!
//! Converts Rust types to YAML text via serde's `Serialize` trait.
//! Builds a [`Node`] tree from serde data-model calls, then emits via the emitter.

use crate::error::{Error, Result};
use serde::ser;
use skald_ast::emitter::EmitterConfig;
use skald_ast::node::{Mapping, Node, Scalar, Sequence};
use skald_core::types::{CollectionStyle, Position, ScalarStyle, Span};

// ─── Helper ──────────────────────────────────────────────────────────

/// Creates a synthetic span (serialized nodes have no source location).
fn synth_span() -> Span {
    Span::point(Position::start())
}

/// Creates a plain scalar node.
fn plain(value: impl Into<String>) -> Node<'static> {
    Node::Scalar(Scalar {
        value: std::borrow::Cow::Owned(value.into()),
        tag: None,
        style: ScalarStyle::Plain,
        span: synth_span(),
    })
}

/// Creates a plain scalar node borrowing a static string (zero allocation).
fn plain_static(value: &'static str) -> Node<'static> {
    Node::Scalar(Scalar {
        value: std::borrow::Cow::Borrowed(value),
        tag: None,
        style: ScalarStyle::Plain,
        span: synth_span(),
    })
}

/// Creates a double-quoted scalar node (for strings that need quoting).
fn quoted(value: impl Into<String>) -> Node<'static> {
    Node::Scalar(Scalar {
        value: std::borrow::Cow::Owned(value.into()),
        tag: None,
        style: ScalarStyle::DoubleQuoted,
        span: synth_span(),
    })
}

/// Determines whether a string needs quoting to be unambiguous in YAML.
fn needs_quoting(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }

    // Values that would be interpreted as null, bool, or numeric
    matches!(
        s,
        "null"
            | "Null"
            | "NULL"
            | "~"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || s.contains('\n')
        || s.contains(": ")
        || s.contains(" #")
        || s.starts_with([
            '&', '*', '!', '|', '>', '%', '@', '`', '{', '}', '[', ']', ',', '?',
        ])
        || s.starts_with("- ")
        || s.ends_with(':')
        || parse_as_number(s)
}

/// Returns true if the string parses as a YAML number.
fn parse_as_number(s: &str) -> bool {
    let trimmed = s.strip_prefix(['+', '-']).unwrap_or(s);
    if trimmed.is_empty() {
        return false;
    }
    // Hex/octal
    if trimmed.starts_with("0x") || trimmed.starts_with("0o") {
        return trimmed[2..].chars().all(|c| c.is_ascii_hexdigit());
    }
    // Decimal int or float
    let mut has_dot = false;
    let mut has_e = false;
    for c in trimmed.chars() {
        match c {
            '0'..='9' => {}
            '.' if !has_dot && !has_e => has_dot = true,
            'e' | 'E' if !has_e => has_e = true,
            '+' | '-' if has_e => {}
            _ => return false,
        }
    }
    true
}

/// Chooses the scalar style for a string value: double-quoted when the text
/// would be ambiguous as a plain scalar (numbers, bools, special tokens),
/// otherwise plain. Single source of truth for both `string_node` and the
/// streaming serializer.
pub(crate) fn scalar_style(value: &str) -> ScalarStyle {
    if needs_quoting(value) {
        ScalarStyle::DoubleQuoted
    } else {
        ScalarStyle::Plain
    }
}

/// Creates a scalar node, choosing plain or quoted style based on content.
fn string_node(value: &str) -> Node<'static> {
    match scalar_style(value) {
        ScalarStyle::DoubleQuoted => quoted(value),
        _ => plain(value),
    }
}

// ─── Public API ──────────────────────────────────────────────────────

/// Serializes a value to a YAML string with default settings.
pub fn to_string<T: ser::Serialize + ?Sized>(value: &T) -> Result<String> {
    to_string_with(value, &EmitterConfig::default())
}

/// Serializes a value to a YAML string with the given emitter configuration.
pub fn to_string_with<T: ser::Serialize + ?Sized>(
    value: &T,
    config: &EmitterConfig,
) -> Result<String> {
    crate::stream_ser::to_string_streaming_with(value, config)
}

/// Serializes a value to YAML, writing to any [`io::Write`](std::io::Write) destination.
pub fn to_writer<T: ser::Serialize + ?Sized, W: std::io::Write>(
    writer: W,
    value: &T,
) -> Result<()> {
    to_writer_with(writer, value, &EmitterConfig::default())
}

/// Serializes a value to YAML with custom emitter configuration, writing to any [`io::Write`](std::io::Write) destination.
///
/// Emits directly into the destination via the streaming serializer — no
/// intermediate `String` allocation.
pub fn to_writer_with<T: ser::Serialize + ?Sized, W: std::io::Write>(
    writer: W,
    value: &T,
    config: &EmitterConfig,
) -> Result<()> {
    let mut adapter = IoWriteAdapter::new(writer);
    let result = crate::stream_ser::to_io_streaming_with(&mut adapter, value, config);
    // Surface a captured I/O error in preference to the generic fmt-bridge error.
    if let Some(io_err) = adapter.into_error() {
        return Err(<Error as serde::ser::Error>::custom(io_err));
    }
    result
}

/// Bridges an [`io::Write`](std::io::Write) destination to [`fmt::Write`](std::fmt::Write)
/// so the streaming emitter (which writes UTF-8 text) can target arbitrary byte sinks.
///
/// `fmt::Write` cannot carry an I/O error payload, so any failure is captured in
/// `error` and surfaced by the caller after serialization completes.
struct IoWriteAdapter<W: std::io::Write> {
    writer: W,
    error: Option<std::io::Error>,
}

impl<W: std::io::Write> IoWriteAdapter<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            error: None,
        }
    }

    fn into_error(self) -> Option<std::io::Error> {
        self.error
    }
}

impl<W: std::io::Write> std::fmt::Write for IoWriteAdapter<W> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match self.writer.write_all(s.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.error = Some(e);
                Err(std::fmt::Error)
            }
        }
    }
}

/// Serializes a value to a YAML `Node<'static>`.
pub fn to_node<T: ser::Serialize + ?Sized>(value: &T) -> Result<Node<'static>> {
    value.serialize(NodeSerializer)
}

// ─── NodeSerializer ──────────────────────────────────────────────────

/// The core serializer that converts serde calls to `Node` values.
struct NodeSerializer;

impl ser::Serializer for NodeSerializer {
    type Ok = Node<'static>;
    type Error = Error;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = TupleVariantBuilder;
    type SerializeMap = MapBuilder;
    type SerializeStruct = MapBuilder;
    type SerializeStructVariant = StructVariantBuilder;

    fn serialize_bool(self, v: bool) -> Result<Node<'static>> {
        Ok(plain(if v { "true" } else { "false" }))
    }

    fn serialize_i8(self, v: i8) -> Result<Node<'static>> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i16(self, v: i16) -> Result<Node<'static>> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i32(self, v: i32) -> Result<Node<'static>> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i64(self, v: i64) -> Result<Node<'static>> {
        Ok(plain(v.to_string()))
    }

    fn serialize_i128(self, v: i128) -> Result<Node<'static>> {
        Ok(plain(v.to_string()))
    }

    fn serialize_u8(self, v: u8) -> Result<Node<'static>> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u16(self, v: u16) -> Result<Node<'static>> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u32(self, v: u32) -> Result<Node<'static>> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u64(self, v: u64) -> Result<Node<'static>> {
        Ok(plain(v.to_string()))
    }

    fn serialize_u128(self, v: u128) -> Result<Node<'static>> {
        Ok(plain(v.to_string()))
    }

    fn serialize_f32(self, v: f32) -> Result<Node<'static>> {
        self.serialize_f64(f64::from(v))
    }

    fn serialize_f64(self, v: f64) -> Result<Node<'static>> {
        if v.is_nan() {
            return Ok(plain_static(".nan"));
        }
        if v.is_infinite() {
            return Ok(plain_static(if v.is_sign_positive() {
                ".inf"
            } else {
                "-.inf"
            }));
        }
        if v == 0.0 {
            return Ok(plain_static(if v.is_sign_negative() {
                "-0.0"
            } else {
                "0.0"
            }));
        }
        // Ensure there's always a decimal point so it's distinguishable from int.
        let s = v.to_string();
        if s.contains('.') || s.contains('e') || s.contains('E') {
            Ok(plain(s))
        } else {
            let mut s = s;
            s.push_str(".0");
            Ok(plain(s))
        }
    }

    fn serialize_char(self, v: char) -> Result<Node<'static>> {
        self.serialize_str(&v.to_string())
    }

    fn serialize_str(self, v: &str) -> Result<Node<'static>> {
        Ok(string_node(v))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Node<'static>> {
        // Emit bytes as a YAML sequence of integers (same as serde_yaml).
        let items: Vec<Node<'static>> = v.iter().map(|b| plain(b.to_string())).collect();
        Ok(Node::Sequence(Sequence {
            items,
            tag: None,
            style: CollectionStyle::Flow,
            span: synth_span(),
        }))
    }

    fn serialize_none(self) -> Result<Node<'static>> {
        Ok(plain("null"))
    }

    fn serialize_some<T: ser::Serialize + ?Sized>(self, value: &T) -> Result<Node<'static>> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Node<'static>> {
        Ok(plain("null"))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Node<'static>> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Node<'static>> {
        Ok(string_node(variant))
    }

    fn serialize_newtype_struct<T: ser::Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Node<'static>> {
        let mut node = value.serialize(NodeSerializer)?;
        match name {
            crate::styled::FLOW_SEQ => {
                if let Node::Sequence(ref mut s) = node {
                    s.style = CollectionStyle::Flow;
                }
            }
            crate::styled::FLOW_MAP => {
                if let Node::Mapping(ref mut m) = node {
                    m.style = CollectionStyle::Flow;
                }
            }
            crate::styled::LIT_STR => {
                if let Node::Scalar(ref mut sc) = node {
                    sc.style = ScalarStyle::Literal;
                }
            }
            crate::styled::FOLD_STR => {
                if let Node::Scalar(ref mut sc) = node {
                    sc.style = ScalarStyle::Folded;
                }
            }
            _ => {}
        }
        Ok(node)
    }

    fn serialize_newtype_variant<T: ser::Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Node<'static>> {
        let inner = value.serialize(NodeSerializer)?;
        Ok(Node::Mapping(Mapping {
            entries: vec![(string_node(variant), inner)],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder> {
        Ok(SeqBuilder {
            items: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqBuilder> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleVariantBuilder> {
        Ok(TupleVariantBuilder {
            variant,
            items: Vec::with_capacity(len),
        })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<MapBuilder> {
        Ok(MapBuilder {
            entries: Vec::with_capacity(len.unwrap_or(0)),
            pending_key: None,
        })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<MapBuilder> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructVariantBuilder> {
        Ok(StructVariantBuilder {
            variant,
            entries: Vec::with_capacity(len),
        })
    }
}

// ─── Sequence builder ────────────────────────────────────────────────

/// Accumulates sequence elements.
pub struct SeqBuilder {
    items: Vec<Node<'static>>,
}

impl ser::SerializeSeq for SeqBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_element<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Node<'static>> {
        Ok(Node::Sequence(Sequence {
            items: self.items,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }
}

impl ser::SerializeTuple for SeqBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_element<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Node<'static>> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SeqBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Node<'static>> {
        ser::SerializeSeq::end(self)
    }
}

// ─── Tuple variant builder ───────────────────────────────────────────

/// Accumulates elements for a tuple variant (e.g. `Enum::Variant(a, b)`).
pub struct TupleVariantBuilder {
    variant: &'static str,
    items: Vec<Node<'static>>,
}

impl ser::SerializeTupleVariant for TupleVariantBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(value.serialize(NodeSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Node<'static>> {
        let seq = Node::Sequence(Sequence {
            items: self.items,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        Ok(Node::Mapping(Mapping {
            entries: vec![(string_node(self.variant), seq)],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }
}

// ─── Map / struct builder ────────────────────────────────────────────

/// Accumulates map/struct entries.
pub struct MapBuilder {
    entries: Vec<(Node<'static>, Node<'static>)>,
    pending_key: Option<Node<'static>>,
}

impl ser::SerializeMap for MapBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_key<T: ser::Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.pending_key = Some(key.serialize(NodeSerializer)?);
        Ok(())
    }

    fn serialize_value<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self
            .pending_key
            .take()
            .expect("serialize_value called before serialize_key");
        self.entries.push((key, value.serialize(NodeSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Node<'static>> {
        Ok(Node::Mapping(Mapping {
            entries: self.entries,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }
}

impl ser::SerializeStruct for MapBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .push((string_node(key), value.serialize(NodeSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Node<'static>> {
        ser::SerializeMap::end(self)
    }
}

// ─── Struct variant builder ──────────────────────────────────────────

/// Accumulates fields for a struct variant (e.g. `Enum::Variant { x, y }`).
pub struct StructVariantBuilder {
    variant: &'static str,
    entries: Vec<(Node<'static>, Node<'static>)>,
}

impl ser::SerializeStructVariant for StructVariantBuilder {
    type Ok = Node<'static>;
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.entries
            .push((string_node(key), value.serialize(NodeSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Node<'static>> {
        let inner = Node::Mapping(Mapping {
            entries: self.entries,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        Ok(Node::Mapping(Mapping {
            entries: vec![(string_node(self.variant), inner)],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[test]
    fn serialize_bool() {
        assert_eq!(to_string(&true).unwrap(), "true\n");
        assert_eq!(to_string(&false).unwrap(), "false\n");
    }

    #[test]
    fn serialize_integers() {
        assert_eq!(to_string(&42i32).unwrap(), "42\n");
        assert_eq!(to_string(&-7i64).unwrap(), "-7\n");
        assert_eq!(to_string(&0u8).unwrap(), "0\n");
        assert_eq!(to_string(&255u64).unwrap(), "255\n");
    }

    #[test]
    fn serialize_floats() {
        assert_eq!(to_string(&1.23f64).unwrap(), "1.23\n");
        assert_eq!(to_string(&0.0f64).unwrap(), "0.0\n");
        assert_eq!(to_string(&f64::INFINITY).unwrap(), ".inf\n");
        assert_eq!(to_string(&f64::NEG_INFINITY).unwrap(), "-.inf\n");
        assert_eq!(to_string(&f64::NAN).unwrap(), ".nan\n");
    }

    #[test]
    fn serialize_string() {
        assert_eq!(to_string("hello").unwrap(), "hello\n");
    }

    #[test]
    fn serialize_string_needs_quoting() {
        assert_eq!(to_string("true").unwrap(), "\"true\"\n");
        assert_eq!(to_string("null").unwrap(), "\"null\"\n");
        assert_eq!(to_string("42").unwrap(), "\"42\"\n");
        assert_eq!(to_string("").unwrap(), "\"\"\n");
    }

    #[test]
    fn serialize_none() {
        let v: Option<i32> = None;
        assert_eq!(to_string(&v).unwrap(), "null\n");
    }

    #[test]
    fn serialize_some() {
        let v: Option<i32> = Some(42);
        assert_eq!(to_string(&v).unwrap(), "42\n");
    }

    #[test]
    fn serialize_unit() {
        assert_eq!(to_string(&()).unwrap(), "null\n");
    }

    #[test]
    fn serialize_seq() {
        let v = vec![1, 2, 3];
        assert_eq!(to_string(&v).unwrap(), "- 1\n- 2\n- 3\n");
    }

    #[test]
    fn serialize_tuple() {
        let v = (1, "two", true);
        assert_eq!(to_string(&v).unwrap(), "- 1\n- two\n- true\n");
    }

    #[test]
    fn serialize_map() {
        let mut m = BTreeMap::new();
        m.insert("name", "skald");
        m.insert("version", "0.1.0");
        let yaml = to_string(&m).unwrap();
        assert!(yaml.contains("name: skald"));
        assert!(yaml.contains("version: 0.1.0"));
    }

    #[test]
    fn serialize_struct() {
        #[derive(Serialize)]
        struct Config {
            name: String,
            debug: bool,
            count: u32,
        }
        let c = Config {
            name: "test".into(),
            debug: true,
            count: 5,
        };
        let yaml = to_string(&c).unwrap();
        assert!(yaml.contains("name: test"));
        assert!(yaml.contains("debug: true"));
        assert!(yaml.contains("count: 5"));
    }

    #[test]
    fn serialize_nested_struct() {
        #[derive(Serialize)]
        struct Inner {
            x: i32,
        }
        #[derive(Serialize)]
        struct Outer {
            inner: Inner,
            tag: String,
        }
        let v = Outer {
            inner: Inner { x: 42 },
            tag: "hello".into(),
        };
        let yaml = to_string(&v).unwrap();
        assert!(yaml.contains("inner:"));
        assert!(yaml.contains("  x: 42"));
        assert!(yaml.contains("tag: hello"));
    }

    #[test]
    fn serialize_enum_unit_variant() {
        #[derive(Serialize)]
        enum Color {
            Red,
            Green,
        }
        assert_eq!(to_string(&Color::Red).unwrap(), "Red\n");
        assert_eq!(to_string(&Color::Green).unwrap(), "Green\n");
    }

    #[test]
    fn serialize_enum_newtype_variant() {
        #[derive(Serialize)]
        enum Value {
            Int(i32),
            Text(String),
        }
        assert_eq!(to_string(&Value::Int(42)).unwrap(), "Int: 42\n");
        assert_eq!(to_string(&Value::Text("hi".into())).unwrap(), "Text: hi\n");
    }

    #[test]
    fn serialize_enum_tuple_variant() {
        #[derive(Serialize)]
        enum Point {
            TwoD(f64, f64),
        }
        let yaml = to_string(&Point::TwoD(1.0, 2.0)).unwrap();
        assert!(yaml.contains("TwoD:"));
        assert!(yaml.contains("- 1.0"));
        assert!(yaml.contains("- 2.0"));
    }

    #[test]
    fn serialize_enum_struct_variant() {
        #[derive(Serialize)]
        enum Shape {
            Rect { width: u32, height: u32 },
        }
        let yaml = to_string(&Shape::Rect {
            width: 10,
            height: 20,
        })
        .unwrap();
        assert!(yaml.contains("Rect:"));
        assert!(yaml.contains("  width: 10"));
        assert!(yaml.contains("  height: 20"));
    }

    #[test]
    fn serialize_bytes_via_node() {
        // Test the serialize_bytes path directly via to_node on a byte sequence.
        use serde::Serializer as _;
        let node = NodeSerializer.serialize_bytes(&[1, 2, 3]).unwrap();
        assert!(node.is_sequence());
        let items = node.as_sequence().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_str(), Some("1"));
    }

    #[test]
    fn serialize_empty_seq() {
        let v: Vec<i32> = vec![];
        assert_eq!(to_string(&v).unwrap(), "[]\n");
    }

    #[test]
    fn serialize_empty_map() {
        let m: BTreeMap<String, String> = BTreeMap::new();
        assert_eq!(to_string(&m).unwrap(), "{}\n");
    }

    #[test]
    fn serialize_with_config() {
        let v = vec![1, 2];
        let config = EmitterConfig {
            indent: 4,
            ..EmitterConfig::default()
        };
        let yaml = to_string_with(&v, &config).unwrap();
        assert_eq!(yaml, "- 1\n- 2\n");
    }

    #[test]
    fn serialize_char() {
        assert_eq!(to_string(&'a').unwrap(), "a\n");
        assert_eq!(to_string(&'!').unwrap(), "\"!\"\n");
    }

    #[test]
    fn serialize_negative_zero() {
        assert_eq!(to_string(&(-0.0f64)).unwrap(), "-0.0\n");
    }

    #[test]
    fn to_node_produces_valid_tree() {
        #[derive(Serialize)]
        struct Pair {
            key: String,
            value: i32,
        }
        let node = to_node(&Pair {
            key: "hello".into(),
            value: 42,
        })
        .unwrap();
        assert!(node.is_mapping());
        let entries = node.as_mapping().unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn serialize_serde_rename() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Config {
            api_key: String,
            max_retries: u32,
        }
        let yaml = to_string(&Config {
            api_key: "secret".into(),
            max_retries: 3,
        })
        .unwrap();
        assert!(yaml.contains("apiKey: secret"));
        assert!(yaml.contains("maxRetries: 3"));
    }

    #[test]
    fn serialize_option_fields() {
        #[derive(Serialize)]
        struct Record {
            name: String,
            email: Option<String>,
            age: Option<u32>,
        }
        let yaml = to_string(&Record {
            name: "Alice".into(),
            email: None,
            age: Some(30),
        })
        .unwrap();
        assert!(yaml.contains("name: Alice"));
        assert!(yaml.contains("email: null"));
        assert!(yaml.contains("age: 30"));
    }

    #[test]
    fn serialize_to_writer() {
        let v = vec![1, 2, 3];
        let mut buf = Vec::new();
        to_writer(&mut buf, &v).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), "- 1\n- 2\n- 3\n");
    }

    #[test]
    fn serialize_to_writer_with_config() {
        let v = vec![1, 2];
        let config = EmitterConfig {
            explicit_document: true,
            ..EmitterConfig::default()
        };
        let mut buf = Vec::new();
        to_writer_with(&mut buf, &v, &config).unwrap();
        let yaml = std::str::from_utf8(&buf).unwrap();
        assert!(yaml.starts_with("---"));
    }

    #[test]
    fn serialize_small_int_widths() {
        // i8 / i16 forward to serialize_i64.
        assert_eq!(to_string(&-5i8).unwrap(), "-5\n");
        assert_eq!(to_string(&-300i16).unwrap(), "-300\n");
        // i128 has its own arm.
        assert_eq!(
            to_string(&170141183460469231731687303715884105727i128).unwrap(),
            "170141183460469231731687303715884105727\n"
        );
    }

    #[test]
    fn serialize_small_uint_widths() {
        // u16 / u128 widths.
        assert_eq!(to_string(&300u16).unwrap(), "300\n");
        assert_eq!(
            to_string(&340282366920938463463374607431768211455u128).unwrap(),
            "340282366920938463463374607431768211455\n"
        );
    }

    #[test]
    fn serialize_f32_forwards_to_f64() {
        // f32 forwards to serialize_f64.
        let yaml = to_string(&1.5f32).unwrap();
        assert_eq!(yaml, "1.5\n");
    }

    #[test]
    fn serialize_unit_struct_yields_null() {
        #[derive(Serialize)]
        struct Unit;
        assert_eq!(to_string(&Unit).unwrap(), "null\n");
    }

    #[test]
    fn serialize_newtype_struct_unwraps_inner() {
        #[derive(Serialize)]
        struct Meters(i32);
        assert_eq!(to_string(&Meters(42)).unwrap(), "42\n");
    }

    #[test]
    fn serialize_tuple_struct_as_sequence() {
        #[derive(Serialize)]
        struct Pair(i32, String);
        let yaml = to_string(&Pair(1, "two".into())).unwrap();
        assert_eq!(yaml, "- 1\n- two\n");
    }

    #[test]
    fn needs_quoting_sign_only_string_is_not_a_number() {
        // A bare "+" or "-" trims to empty in parse_as_number → not a number,
        // and isn't otherwise special, so it serializes plain.
        assert_eq!(to_string("+").unwrap(), "+\n");
        assert_eq!(to_string("-").unwrap(), "-\n");
    }

    #[test]
    fn needs_quoting_hex_string_is_quoted() {
        // "0xFF" parses as a hex number → must be quoted to stay a string.
        assert_eq!(to_string("0xFF").unwrap(), "\"0xFF\"\n");
        assert_eq!(to_string("0o17").unwrap(), "\"0o17\"\n");
    }

    #[test]
    fn needs_quoting_scientific_notation_is_quoted() {
        // "1e+5" exercises the '+'/'-'-after-exponent arm of parse_as_number.
        assert_eq!(to_string("1e+5").unwrap(), "\"1e+5\"\n");
        assert_eq!(to_string("1e-5").unwrap(), "\"1e-5\"\n");
    }

    #[test]
    fn serialize_to_writer_struct() {
        #[derive(Serialize)]
        struct Point {
            x: i32,
            y: i32,
        }
        let mut buf = Vec::new();
        to_writer(&mut buf, &Point { x: 1, y: 2 }).unwrap();
        let yaml = std::str::from_utf8(&buf).unwrap();
        assert!(yaml.contains("x: 1"));
        assert!(yaml.contains("y: 2"));
    }

    #[test]
    fn flow_seq_renders_flow_style() {
        use crate::styled::FlowSeq;
        let out = crate::to_string(&FlowSeq(vec![1, 2, 3])).unwrap();
        assert!(out.contains('['), "expected flow seq, got: {out:?}");
        assert!(out.trim_end().ends_with(']'), "got: {out:?}");
    }

    #[test]
    fn flow_map_renders_flow_style() {
        use crate::styled::FlowMap;
        use std::collections::BTreeMap;
        let mut m = BTreeMap::new();
        m.insert("a", 1);
        m.insert("b", 2);
        let out = crate::to_string(&FlowMap(m)).unwrap();
        assert!(out.contains('{') && out.contains('}'), "got: {out:?}");
    }

    #[test]
    fn lit_str_renders_literal_block() {
        use crate::styled::LitStr;
        let out = crate::to_string(&LitStr("line1\nline2\n")).unwrap();
        assert!(out.contains('|'), "expected literal block, got: {out:?}");
    }

    #[test]
    fn fold_str_renders_folded_block() {
        use crate::styled::FoldStr;
        let out = crate::to_string(&FoldStr("a b c\n")).unwrap();
        assert!(out.contains('>'), "expected folded block, got: {out:?}");
    }

    #[test]
    fn scalar_style_quotes_numeric_and_special_strings() {
        assert_eq!(scalar_style("123"), ScalarStyle::DoubleQuoted);
        assert_eq!(scalar_style("true"), ScalarStyle::DoubleQuoted);
        assert_eq!(scalar_style(""), ScalarStyle::DoubleQuoted);
        assert_eq!(scalar_style("hello"), ScalarStyle::Plain);
        assert_eq!(scalar_style("a normal phrase"), ScalarStyle::Plain);
    }

    // ── NodeSerializer (`to_node`) direct coverage ──
    //
    // `to_string` routes through the streaming serializer, so the
    // `NodeSerializer` integer/float/tuple delegations are only reachable via
    // `to_node`. These tests exercise each narrow-width and delegation arm.

    #[test]
    fn to_node_narrow_signed_widths() {
        // serialize_i8 / serialize_i16 delegate to serialize_i64.
        assert_eq!(to_node(&3i8).unwrap().as_str(), Some("3"));
        assert_eq!(to_node(&-300i16).unwrap().as_str(), Some("-300"));
        assert_eq!(to_node(&70000i32).unwrap().as_str(), Some("70000"));
        // serialize_i128 has its own arm.
        assert_eq!(to_node(&-5i128).unwrap().as_str(), Some("-5"));
    }

    #[test]
    fn to_node_narrow_unsigned_widths() {
        // serialize_u8 / serialize_u16 / serialize_u32 delegate to serialize_u64.
        assert_eq!(to_node(&3u8).unwrap().as_str(), Some("3"));
        assert_eq!(to_node(&300u16).unwrap().as_str(), Some("300"));
        assert_eq!(to_node(&70000u32).unwrap().as_str(), Some("70000"));
        // serialize_u128 has its own arm.
        assert_eq!(to_node(&9u128).unwrap().as_str(), Some("9"));
    }

    #[test]
    fn to_node_f32_forwards_to_f64() {
        // serialize_f32 forwards to serialize_f64.
        assert_eq!(to_node(&1.5f32).unwrap().as_str(), Some("1.5"));
    }

    #[test]
    fn to_node_bytes_is_flow_sequence() {
        use serde::Serializer as _;
        let node = NodeSerializer.serialize_bytes(&[1u8, 2, 3]).unwrap();
        assert!(node.is_sequence());
        assert_eq!(node.as_sequence().unwrap().len(), 3);
    }

    #[test]
    fn to_node_unit_struct_yields_null() {
        #[derive(Serialize)]
        struct Unit;
        // serialize_unit_struct delegates to serialize_unit.
        assert_eq!(to_node(&Unit).unwrap().as_str(), Some("null"));
    }

    #[test]
    fn to_node_plain_newtype_struct_unwraps_inner() {
        // A non-styled newtype struct hits the `_ => {}` arm of
        // serialize_newtype_struct (no style override).
        #[derive(Serialize)]
        struct Meters(i32);
        assert_eq!(to_node(&Meters(42)).unwrap().as_str(), Some("42"));
    }

    #[test]
    fn to_node_tuple_is_block_sequence() {
        // serialize_tuple delegates to serialize_seq; SeqBuilder's
        // SerializeTuple element/end arms run here.
        let node = to_node(&(1i32, 2i32, 3i32)).unwrap();
        assert!(node.is_sequence());
        assert_eq!(node.as_sequence().unwrap().len(), 3);
    }

    #[test]
    fn to_node_tuple_struct_is_block_sequence() {
        // serialize_tuple_struct delegates to serialize_seq; SeqBuilder's
        // SerializeTupleStruct field/end arms run here.
        #[derive(Serialize)]
        struct Pair(i32, i32);
        let node = to_node(&Pair(7, 8)).unwrap();
        assert!(node.is_sequence());
        assert_eq!(node.as_sequence().unwrap().len(), 2);
    }

    /// An `io::Write` sink that always fails, to exercise the I/O error path
    /// of `to_writer_with` (`IoWriteAdapter::write_str` Err arm + the captured
    /// error surfaced at line 172).
    struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn to_writer_surfaces_io_error() {
        let err = to_writer(FailingWriter, &vec![1i32, 2, 3]).unwrap_err();
        assert!(
            err.to_string().contains("boom"),
            "expected captured I/O error, got: {err}"
        );
    }
}
