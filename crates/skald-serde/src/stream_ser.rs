// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Streaming YAML serializer.
//!
//! Drives the push emitter sink ([`skald_ast::emitter::Emitter`]) directly from
//! serde data-model calls, with **no intermediate [`Node`](skald_ast::node::Node)
//! tree**. Output is byte-identical to the `NodeSerializer` path in
//! [`crate::ser`]; that path is the oracle for every layout/scalar decision.
//!
//! The serde [`Serializer`](ser::Serializer) is implemented on
//! `&mut StreamingSerializer` so that nested values can re-borrow the same sink
//! via `value.serialize(&mut *self)`, threading a single emitter through the
//! whole tree.

use std::fmt;

use serde::ser;
use skald_ast::emitter::{Emitter, EmitterConfig};
use skald_core::types::{CollectionStyle, ScalarStyle};

use crate::error::{Error, Result};
use crate::ser::scalar_style;

/// Maps a `fmt::Error` from the sink to the crate error type.
fn fmt_err(_e: fmt::Error) -> Error {
    <Error as ser::Error>::custom("write error")
}

/// A pending per-value style override carried from a reserved
/// `serialize_newtype_struct` (the [`crate::styled`] wrappers) to the next
/// collection-open / scalar call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StyleOverride {
    /// Next collection opens in flow style.
    FlowCollection,
    /// Next scalar emits as a literal block scalar (`|`).
    LiteralScalar,
    /// Next scalar emits as a folded block scalar (`>`).
    FoldedScalar,
}

/// Streaming serde serializer that pushes directly into the emitter sink.
pub(crate) struct StreamingSerializer<'c, W: fmt::Write> {
    emitter: Emitter<'c, W>,
    /// A style override for the immediately following node, set by a reserved
    /// newtype-struct (`FlowSeq`/`FlowMap`/`LitStr`/`FoldStr`).
    style_override: Option<StyleOverride>,
}

impl<'c, W: fmt::Write> StreamingSerializer<'c, W> {
    /// Creates a streaming serializer writing into `writer` with `cfg`.
    pub(crate) fn new(writer: W, cfg: &'c EmitterConfig) -> Self {
        Self {
            emitter: Emitter::new(writer, cfg),
            style_override: None,
        }
    }

    /// Finalizes the sink (no-op beyond consuming it; all bytes already written).
    pub(crate) fn finish(self) -> Result<()> {
        self.emitter.finish().map_err(fmt_err)
    }

    /// Resolves the collection style for the next `begin_*`, consuming any
    /// pending flow override.
    fn take_collection_style(&mut self) -> CollectionStyle {
        if self.style_override == Some(StyleOverride::FlowCollection) {
            self.style_override = None;
            CollectionStyle::Flow
        } else {
            CollectionStyle::Block
        }
    }

    /// Emits a scalar, honoring a pending literal/folded override if present.
    fn emit_scalar(&mut self, value: &str, default_style: ScalarStyle) -> Result<()> {
        let style = match self.style_override {
            Some(StyleOverride::LiteralScalar) => {
                self.style_override = None;
                ScalarStyle::Literal
            }
            Some(StyleOverride::FoldedScalar) => {
                self.style_override = None;
                ScalarStyle::Folded
            }
            _ => default_style,
        };
        self.emitter.scalar(value, style).map_err(fmt_err)
    }
}

// ─── Crate-internal entry points ─────────────────────────────────────

/// Serializes a value to a YAML string via the streaming sink (default config).
///
/// Test-only convenience: the public `to_string`/`to_writer` paths go through
/// `to_string_streaming_with` / `to_io_streaming_with` with an explicit config.
#[cfg(test)]
pub(crate) fn to_string_streaming<T: ser::Serialize + ?Sized>(value: &T) -> Result<String> {
    to_string_streaming_with(value, &EmitterConfig::default())
}

/// Serializes a value to a YAML string via the streaming sink with `config`.
pub(crate) fn to_string_streaming_with<T: ser::Serialize + ?Sized>(
    value: &T,
    config: &EmitterConfig,
) -> Result<String> {
    let mut out = String::with_capacity(256);
    to_io_streaming_with(&mut out, value, config)?;
    Ok(out)
}

/// Serializes a value straight into a [`fmt::Write`] sink with `config`, with no
/// intermediate `String` allocation. Used by the public `to_writer*` path via an
/// `io::Write`→`fmt::Write` adapter.
///
/// Mirrors [`skald_ast::emitter::emit`]: when `config.explicit_document` is set,
/// the body is wrapped in `---` / `...` document markers (the body itself always
/// ends with a trailing newline, so `...` lands on its own line).
pub(crate) fn to_io_streaming_with<W: fmt::Write, T: ser::Serialize + ?Sized>(
    writer: &mut W,
    value: &T,
    config: &EmitterConfig,
) -> Result<()> {
    if config.explicit_document {
        writer.write_str("---\n").map_err(fmt_err)?;
    }
    {
        let mut ser = StreamingSerializer::new(&mut *writer, config);
        value.serialize(&mut ser)?;
        ser.finish()?;
    }
    if config.explicit_document {
        writer.write_str("...\n").map_err(fmt_err)?;
    }
    Ok(())
}

// ─── Serializer impl ─────────────────────────────────────────────────

impl<'a, 'c, W: fmt::Write> ser::Serializer for &'a mut StreamingSerializer<'c, W> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = SeqEmitter<'a, 'c, W>;
    type SerializeTuple = SeqEmitter<'a, 'c, W>;
    type SerializeTupleStruct = SeqEmitter<'a, 'c, W>;
    type SerializeTupleVariant = VariantSeqEmitter<'a, 'c, W>;
    type SerializeMap = MapEmitter<'a, 'c, W>;
    type SerializeStruct = MapEmitter<'a, 'c, W>;
    type SerializeStructVariant = VariantMapEmitter<'a, 'c, W>;

    fn serialize_bool(self, v: bool) -> Result<()> {
        self.emit_scalar(if v { "true" } else { "false" }, ScalarStyle::Plain)
    }

    fn serialize_i8(self, v: i8) -> Result<()> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i16(self, v: i16) -> Result<()> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i32(self, v: i32) -> Result<()> {
        self.serialize_i64(i64::from(v))
    }

    fn serialize_i64(self, v: i64) -> Result<()> {
        self.emit_scalar(&v.to_string(), ScalarStyle::Plain)
    }

    fn serialize_i128(self, v: i128) -> Result<()> {
        self.emit_scalar(&v.to_string(), ScalarStyle::Plain)
    }

    fn serialize_u8(self, v: u8) -> Result<()> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u16(self, v: u16) -> Result<()> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u32(self, v: u32) -> Result<()> {
        self.serialize_u64(u64::from(v))
    }

    fn serialize_u64(self, v: u64) -> Result<()> {
        self.emit_scalar(&v.to_string(), ScalarStyle::Plain)
    }

    fn serialize_u128(self, v: u128) -> Result<()> {
        self.emit_scalar(&v.to_string(), ScalarStyle::Plain)
    }

    fn serialize_f32(self, v: f32) -> Result<()> {
        self.serialize_f64(f64::from(v))
    }

    fn serialize_f64(self, v: f64) -> Result<()> {
        // Mirror NodeSerializer::serialize_f64 exactly.
        if v.is_nan() {
            return self.emit_scalar(".nan", ScalarStyle::Plain);
        }
        if v.is_infinite() {
            let s = if v.is_sign_positive() {
                ".inf"
            } else {
                "-.inf"
            };
            return self.emit_scalar(s, ScalarStyle::Plain);
        }
        if v == 0.0 {
            let s = if v.is_sign_negative() { "-0.0" } else { "0.0" };
            return self.emit_scalar(s, ScalarStyle::Plain);
        }
        let s = v.to_string();
        if s.contains('.') || s.contains('e') || s.contains('E') {
            self.emit_scalar(&s, ScalarStyle::Plain)
        } else {
            let mut s = s;
            s.push_str(".0");
            self.emit_scalar(&s, ScalarStyle::Plain)
        }
    }

    fn serialize_char(self, v: char) -> Result<()> {
        self.serialize_str(&v.to_string())
    }

    fn serialize_str(self, v: &str) -> Result<()> {
        self.emit_scalar(v, scalar_style(v))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<()> {
        // Mirror NodeSerializer::serialize_bytes: a flow sequence of integers.
        self.emitter
            .begin_seq(CollectionStyle::Flow, Some(v.len()))
            .map_err(fmt_err)?;
        for b in v {
            self.emitter.before_elem().map_err(fmt_err)?;
            self.emitter
                .scalar(&b.to_string(), ScalarStyle::Plain)
                .map_err(fmt_err)?;
        }
        self.emitter.end_seq().map_err(fmt_err)
    }

    fn serialize_none(self) -> Result<()> {
        self.emit_scalar("null", ScalarStyle::Plain)
    }

    fn serialize_some<T: ser::Serialize + ?Sized>(self, value: &T) -> Result<()> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<()> {
        self.emit_scalar("null", ScalarStyle::Plain)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<()> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<()> {
        self.emit_scalar(variant, scalar_style(variant))
    }

    fn serialize_newtype_struct<T: ser::Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<()> {
        // Reserved style wrappers set a pending override consumed by the inner
        // node's serialize call (a collection-open or a scalar).
        match name {
            crate::styled::FLOW_SEQ | crate::styled::FLOW_MAP => {
                self.style_override = Some(StyleOverride::FlowCollection);
            }
            crate::styled::LIT_STR => {
                self.style_override = Some(StyleOverride::LiteralScalar);
            }
            crate::styled::FOLD_STR => {
                self.style_override = Some(StyleOverride::FoldedScalar);
            }
            _ => {}
        }
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ser::Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        // Externally tagged: a single-entry block map {variant: value}.
        self.emitter
            .begin_map(CollectionStyle::Block, Some(1))
            .map_err(fmt_err)?;
        self.emitter.before_key().map_err(fmt_err)?;
        self.emitter
            .scalar(variant, scalar_style(variant))
            .map_err(fmt_err)?;
        self.emitter.before_value().map_err(fmt_err)?;
        value.serialize(&mut *self)?;
        self.emitter.end_map().map_err(fmt_err)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq> {
        let style = self.take_collection_style();
        self.emitter.begin_seq(style, len).map_err(fmt_err)?;
        Ok(SeqEmitter { ser: self })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        // {variant: [ .. ]} — open the outer map and the inner block sequence.
        self.emitter
            .begin_map(CollectionStyle::Block, Some(1))
            .map_err(fmt_err)?;
        self.emitter.before_key().map_err(fmt_err)?;
        self.emitter
            .scalar(variant, scalar_style(variant))
            .map_err(fmt_err)?;
        self.emitter.before_value().map_err(fmt_err)?;
        self.emitter
            .begin_seq(CollectionStyle::Block, Some(len))
            .map_err(fmt_err)?;
        Ok(VariantSeqEmitter { ser: self })
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap> {
        let style = self.take_collection_style();
        self.emitter.begin_map(style, len).map_err(fmt_err)?;
        Ok(MapEmitter { ser: self })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Self::SerializeStruct> {
        self.serialize_map(Some(len))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant> {
        // {variant: { .. }} — open the outer map and the inner block map.
        self.emitter
            .begin_map(CollectionStyle::Block, Some(1))
            .map_err(fmt_err)?;
        self.emitter.before_key().map_err(fmt_err)?;
        self.emitter
            .scalar(variant, scalar_style(variant))
            .map_err(fmt_err)?;
        self.emitter.before_value().map_err(fmt_err)?;
        self.emitter
            .begin_map(CollectionStyle::Block, Some(len))
            .map_err(fmt_err)?;
        Ok(VariantMapEmitter { ser: self })
    }
}

// ─── Sequence / tuple emitter ────────────────────────────────────────

/// Drives a (block or flow) sequence into the sink.
pub(crate) struct SeqEmitter<'a, 'c, W: fmt::Write> {
    ser: &'a mut StreamingSerializer<'c, W>,
}

impl<W: fmt::Write> ser::SerializeSeq for SeqEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.ser.emitter.before_elem().map_err(fmt_err)?;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<()> {
        self.ser.emitter.end_seq().map_err(fmt_err)
    }
}

impl<W: fmt::Write> ser::SerializeTuple for SeqEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<()> {
        ser::SerializeSeq::end(self)
    }
}

impl<W: fmt::Write> ser::SerializeTupleStruct for SeqEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        ser::SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<()> {
        ser::SerializeSeq::end(self)
    }
}

// ─── Tuple-variant emitter ───────────────────────────────────────────

/// Drives the inner block sequence of a tuple variant (`{variant: [ .. ]}`),
/// closing both the sequence and the outer map at `end`.
pub(crate) struct VariantSeqEmitter<'a, 'c, W: fmt::Write> {
    ser: &'a mut StreamingSerializer<'c, W>,
}

impl<W: fmt::Write> ser::SerializeTupleVariant for VariantSeqEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.ser.emitter.before_elem().map_err(fmt_err)?;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<()> {
        self.ser.emitter.end_seq().map_err(fmt_err)?;
        self.ser.emitter.end_map().map_err(fmt_err)
    }
}

// ─── Map / struct emitter ────────────────────────────────────────────

/// Drives a (block or flow) mapping into the sink.
pub(crate) struct MapEmitter<'a, 'c, W: fmt::Write> {
    ser: &'a mut StreamingSerializer<'c, W>,
}

impl<W: fmt::Write> ser::SerializeMap for MapEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: ser::Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.ser.emitter.before_key().map_err(fmt_err)?;
        key.serialize(&mut *self.ser)
    }

    fn serialize_value<T: ser::Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.ser.emitter.before_value().map_err(fmt_err)?;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<()> {
        self.ser.emitter.end_map().map_err(fmt_err)
    }
}

impl<W: fmt::Write> ser::SerializeStruct for MapEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.ser.emitter.before_key().map_err(fmt_err)?;
        self.ser
            .emitter
            .scalar(key, scalar_style(key))
            .map_err(fmt_err)?;
        self.ser.emitter.before_value().map_err(fmt_err)?;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<()> {
        ser::SerializeMap::end(self)
    }
}

// ─── Struct-variant emitter ──────────────────────────────────────────

/// Drives the inner block map of a struct variant (`{variant: { .. }}`),
/// closing both the inner map and the outer map at `end`.
pub(crate) struct VariantMapEmitter<'a, 'c, W: fmt::Write> {
    ser: &'a mut StreamingSerializer<'c, W>,
}

impl<W: fmt::Write> ser::SerializeStructVariant for VariantMapEmitter<'_, '_, W> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: ser::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.ser.emitter.before_key().map_err(fmt_err)?;
        self.ser
            .emitter
            .scalar(key, scalar_style(key))
            .map_err(fmt_err)?;
        self.ser.emitter.before_value().map_err(fmt_err)?;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<()> {
        self.ser.emitter.end_map().map_err(fmt_err)?;
        self.ser.emitter.end_map().map_err(fmt_err)
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::collections::BTreeMap;

    /// Asserts the streaming output equals the oracle (Node path) for `v`.
    fn same<T: Serialize>(v: &T) {
        let streaming = to_string_streaming(v).unwrap();
        let node = crate::ser::to_node(v).unwrap();
        let oracle = skald_ast::emitter::emit_to_string(&node, &EmitterConfig::default());
        assert_eq!(streaming, oracle, "streaming diverged from oracle");
    }

    #[test]
    fn scalars_match_oracle() {
        same(&42i64);
        same(&(-7i64));
        same(&255u64);
        same(&true);
        same(&false);
        same(&1.23f64);
        same(&0.0f64);
        same(&(-0.0f64));
        same(&f64::NAN);
        same(&f64::INFINITY);
        same(&f64::NEG_INFINITY);
        same(&'a');
        same(&'!');
    }

    #[test]
    fn strings_match_oracle() {
        same(&"hello");
        same(&"a normal phrase");
        same(&"123");
        same(&"true");
        same(&"null");
        same(&String::from("owned"));
        same(&String::new());
    }

    #[test]
    fn option_and_unit_match_oracle() {
        same(&Some(7i64));
        let none: Option<i64> = None;
        same(&none);
        same(&());
    }

    #[test]
    fn sequences_match_oracle() {
        let empty: Vec<i64> = vec![];
        same(&empty);
        same(&vec![1i64, 2, 3]);
        same(&vec![vec![1i64, 2], vec![3, 4]]);
        let nested_empty: Vec<Vec<i64>> = vec![vec![], vec![1]];
        same(&nested_empty);
    }

    #[test]
    fn maps_match_oracle() {
        let mut m: BTreeMap<String, i64> = BTreeMap::new();
        m.insert("alpha".into(), 1);
        m.insert("beta".into(), 2);
        same(&m);
        let empty: BTreeMap<String, i64> = BTreeMap::new();
        same(&empty);
    }

    #[derive(Serialize)]
    struct Point {
        x: i64,
        y: i64,
    }

    #[test]
    fn struct_matches_oracle() {
        same(&Point { x: 3, y: 4 });
    }

    #[derive(Serialize)]
    struct Nested {
        items: Vec<i64>,
        point: Point,
        label: String,
    }

    #[test]
    fn nested_struct_matches_oracle() {
        same(&Nested {
            items: vec![1, 2, 3],
            point: Point { x: 1, y: 2 },
            label: "deep".into(),
        });
    }

    #[derive(Serialize)]
    enum Shape {
        Unit,
        Number(i64),
        Pair(f64, f64),
        Rect { width: u32, height: u32 },
    }

    #[test]
    fn enum_variants_match_oracle() {
        same(&Shape::Unit);
        same(&Shape::Number(42));
        same(&Shape::Pair(1.0, 2.0));
        same(&Shape::Rect {
            width: 10,
            height: 20,
        });
    }

    #[test]
    fn bytes_match_oracle() {
        // serde_bytes-like path: drive serialize_bytes directly.
        use serde::Serializer as _;
        let mut out = String::new();
        {
            let cfg = EmitterConfig::default();
            let mut ser = StreamingSerializer::new(&mut out, &cfg);
            (&mut ser).serialize_bytes(&[1u8, 2, 3]).unwrap();
            ser.finish().unwrap();
        }
        // Oracle for the same byte slice.
        let node = crate::ser::to_node(&serde_bytes_helper()).unwrap();
        let oracle = skald_ast::emitter::emit_to_string(&node, &EmitterConfig::default());
        assert_eq!(out, oracle);
    }

    /// Helper producing a value whose `to_node` exercises `serialize_bytes`.
    fn serde_bytes_helper() -> Bytes {
        Bytes([1, 2, 3])
    }

    struct Bytes([u8; 3]);
    impl Serialize for Bytes {
        fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
            s.serialize_bytes(&self.0)
        }
    }

    // ── Explicit expected-string assertions ──

    #[test]
    fn explicit_seq_output() {
        same(&vec![1i64, 2, 3]);
        assert_eq!(
            to_string_streaming(&vec![1i64, 2, 3]).unwrap(),
            "- 1\n- 2\n- 3\n"
        );
    }

    #[test]
    fn explicit_empty_seq_output() {
        let empty: Vec<i64> = vec![];
        assert_eq!(to_string_streaming(&empty).unwrap(), "[]\n");
    }

    #[test]
    fn explicit_scalar_output() {
        assert_eq!(to_string_streaming(&true).unwrap(), "true\n");
        assert_eq!(to_string_streaming(&f64::NAN).unwrap(), ".nan\n");
        assert_eq!(to_string_streaming(&"42").unwrap(), "\"42\"\n");
        assert_eq!(to_string_streaming(&(-0.0f64)).unwrap(), "-0.0\n");
    }

    #[test]
    fn explicit_struct_output() {
        assert_eq!(
            to_string_streaming(&Point { x: 1, y: 2 }).unwrap(),
            "x: 1\ny: 2\n"
        );
    }

    #[test]
    fn explicit_enum_outputs() {
        assert_eq!(to_string_streaming(&Shape::Unit).unwrap(), "Unit\n");
        assert_eq!(
            to_string_streaming(&Shape::Number(42)).unwrap(),
            "Number: 42\n"
        );
    }

    #[test]
    fn styled_wrappers_match_oracle() {
        use crate::styled::{FlowMap, FlowSeq, FoldStr, LitStr};
        same(&FlowSeq(vec![1i64, 2, 3]));
        let mut m: BTreeMap<&str, i64> = BTreeMap::new();
        m.insert("a", 1);
        m.insert("b", 2);
        same(&FlowMap(m));
        same(&LitStr("line1\nline2\n"));
        same(&FoldStr("a b c\n"));
    }

    /// A `fmt::Write` sink that always fails, to exercise `fmt_err` (the
    /// sink-`fmt::Error` → crate-error bridge) via the streaming entry point.
    struct FailingFmtWriter;

    impl fmt::Write for FailingFmtWriter {
        fn write_str(&mut self, _s: &str) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn streaming_surfaces_fmt_error() {
        let mut w = FailingFmtWriter;
        let err =
            to_io_streaming_with(&mut w, &vec![1i64, 2, 3], &EmitterConfig::default()).unwrap_err();
        assert!(
            err.to_string().contains("write error"),
            "expected fmt_err bridge, got: {err}"
        );
    }

    #[test]
    fn config_threads_through() {
        // `indent` and `sort_keys` are honored by the sink itself (document
        // markers are added by `emit`, not the sink, so they are out of scope
        // for the streaming entry point until Task 13 wires `to_string_with`).
        let mut m: BTreeMap<String, i64> = BTreeMap::new();
        m.insert("beta".into(), 2);
        m.insert("alpha".into(), 1);
        let config = EmitterConfig {
            indent: 4,
            sort_keys: true,
            ..EmitterConfig::default()
        };
        let streaming = to_string_streaming_with(&m, &config).unwrap();
        let node = crate::ser::to_node(&m).unwrap();
        let oracle = skald_ast::emitter::emit_to_string(&node, &config);
        assert_eq!(streaming, oracle);
    }
}
