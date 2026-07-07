// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A serde-compatible YAML value type.
//!
//! [`Value`] wraps [`Node<'static>`] to provide `Serialize` and `Deserialize`
//! implementations. This is needed because Rust's orphan rule prevents implementing
//! foreign traits on foreign types directly.
//!
//! Use `Value::from(node)` and `value.into_node()` to convert between the types.

use crate::de::Deserializer;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, SerializeMap, SerializeSeq};
use skald_ast::node::{Mapping, Node, Scalar, Sequence};
use skald_core::types::{CollectionStyle, Position, ScalarStyle, Span};
use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

fn synth_span() -> Span {
    Span::point(Position::start())
}

/// A serde-compatible YAML value.
///
/// Wraps a [`Node<'static>`] and implements `Serialize` + `Deserialize`.
#[derive(Debug, Clone, PartialEq)]
pub struct Value(Node<'static>);

impl Value {
    /// Creates a new `Value` from a `Node`.
    #[must_use]
    pub fn new(node: Node<'static>) -> Self {
        Self(node)
    }

    /// Consumes this `Value` and returns the inner `Node`.
    #[must_use]
    pub fn into_node(self) -> Node<'static> {
        self.0
    }

    /// Returns a reference to the inner `Node`.
    #[must_use]
    pub fn as_node(&self) -> &Node<'static> {
        &self.0
    }

    /// Returns the YAML tag attached to this value, if any.
    ///
    /// Custom tags (e.g. `!mytag`) are surfaced here as data — the tag text is
    /// preserved verbatim from the source document. Skald never maps a tag to a
    /// code path (RCE-safe by construction).
    ///
    /// # Tag retention by API path
    ///
    /// - **Node path** (`from_str_node` → `Value::from`): tags are preserved.
    ///   The composer stores the raw tag text on the `Node`, and `Value` wraps
    ///   it without modification.  This method returns `Some(tag)` for any
    ///   tagged scalar, sequence, or mapping.
    ///
    /// - **Typed serde path** (`from_str::<T>`): tags are silently normalized
    ///   away.  Serde's data model has no tag concept; the deserializer resolves
    ///   a tagged plain scalar to its underlying value type (e.g. `!mytag hello`
    ///   → `String("hello")`).  If you need the tag, use the node path instead.
    #[must_use]
    pub fn tag(&self) -> Option<&skald_core::types::Tag<'static>> {
        self.0.tag()
    }
}

impl From<Node<'static>> for Value {
    fn from(node: Node<'static>) -> Self {
        Self(node)
    }
}

impl From<Value> for Node<'static> {
    fn from(value: Value) -> Self {
        value.0
    }
}

impl Deref for Value {
    type Target = Node<'static>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ─── Serialize ───────────────────────────────────────────────────────

impl ser::Serialize for Value {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_node(&self.0, serializer)
    }
}

/// Zero-copy serialization wrapper — borrows a `Node` without cloning.
struct NodeRef<'a>(&'a Node<'a>);

impl ser::Serialize for NodeRef<'_> {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_node(self.0, serializer)
    }
}

fn serialize_node<S: ser::Serializer>(node: &Node<'_>, serializer: S) -> Result<S::Ok, S::Error> {
    match node {
        Node::Scalar(s) => serializer.serialize_str(&s.value),
        Node::Sequence(seq) => {
            let mut state = serializer.serialize_seq(Some(seq.items.len()))?;
            for item in &seq.items {
                state.serialize_element(&NodeRef(item))?;
            }
            state.end()
        }
        Node::Mapping(map) => {
            let mut state = serializer.serialize_map(Some(map.entries.len()))?;
            for (k, v) in &map.entries {
                state.serialize_entry(&NodeRef(k), &NodeRef(v))?;
            }
            state.end()
        }
    }
}

// ─── Deserialize ─────────────────────────────────────────────────────

impl<'de> de::Deserialize<'de> for Value {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor).map(Value)
    }
}

struct ValueVisitor;

/// Builds a synthetic scalar node with the given value and style.
///
/// Shared by every `ValueVisitor::visit_*` method — keeping the
/// `Node::Scalar` construction in one place (DRY) and on a single line.
fn scalar_node(value: Cow<'static, str>, style: ScalarStyle) -> Node<'static> {
    Node::Scalar(Scalar {
        value,
        tag: None,
        style,
        span: synth_span(),
    })
}

/// A plain (unquoted) scalar — used for booleans, numbers, and null.
fn plain_scalar(value: Cow<'static, str>) -> Node<'static> {
    scalar_node(value, ScalarStyle::Plain)
}

/// A double-quoted scalar — used for strings, so they never re-resolve as
/// another type on a round-trip.
fn quoted_scalar(value: Cow<'static, str>) -> Node<'static> {
    scalar_node(value, ScalarStyle::DoubleQuoted)
}

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Node<'static>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a YAML value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node<'static>, E> {
        Ok(plain_scalar(Cow::Borrowed(if v {
            "true"
        } else {
            "false"
        })))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node<'static>, E> {
        Ok(plain_scalar(Cow::Owned(v.to_string())))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node<'static>, E> {
        Ok(plain_scalar(Cow::Owned(v.to_string())))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node<'static>, E> {
        let value = if v.is_nan() {
            Cow::Borrowed(".nan")
        } else if v.is_infinite() {
            Cow::Borrowed(if v.is_sign_positive() {
                ".inf"
            } else {
                "-.inf"
            })
        } else {
            Cow::Owned(v.to_string())
        };
        Ok(plain_scalar(value))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node<'static>, E> {
        Ok(quoted_scalar(Cow::Owned(v.to_owned())))
    }

    /// Handles `&'de str` slices from borrowing deserializers.
    ///
    /// `Value` wraps `Node<'static>` so we always own the string — one
    /// allocation is unavoidable here.  This override exists so the serde
    /// machinery dispatches to `visit_str` via the default forwarding rather
    /// than hitting a missing implementation; it also signals to the
    /// `Deserializer` that borrowed scalars are accepted for other `T`.
    fn visit_borrowed_str<E: de::Error>(self, v: &str) -> Result<Node<'static>, E> {
        self.visit_str(v)
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Node<'static>, E> {
        Ok(quoted_scalar(Cow::Owned(v)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Node<'static>, E> {
        Ok(plain_scalar(Cow::Borrowed("null")))
    }

    fn visit_some<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Node<'static>, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node<'static>, E> {
        self.visit_none()
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Node<'static>, A::Error> {
        let mut items = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some(Value(node)) = access.next_element()? {
            items.push(node);
        }
        Ok(Node::Sequence(Sequence {
            items,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Node<'static>, A::Error> {
        let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
        while let Some((Value(key), Value(value))) = access.next_entry()? {
            entries.push((key, value));
        }
        Ok(Node::Mapping(Mapping {
            entries,
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }))
    }
}

// ─── Convenience functions ───────────────────────────────────────────

/// Serializes a `Node` to a YAML `Node<'static>` via serde's data model.
pub fn node_to_value(node: &Node<'_>) -> crate::error::Result<Value> {
    let v = Value::new(node.clone().into_owned());
    // Round-trip through serde to normalize the representation
    let yaml = crate::ser::to_string(&v)?;
    crate::de::from_str(&yaml)
}

/// Deserializes a `Value` from a `Node` via our deserializer.
pub fn value_from_node(node: &Node<'_>) -> crate::error::Result<Value> {
    let mut de = Deserializer::from_node(node);
    let v: Value = serde::Deserialize::deserialize(&mut de)?;
    Ok(v)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::de;
    use crate::ser;

    #[test]
    fn value_from_node_conversion() {
        let node = Node::Scalar(Scalar {
            value: Cow::Owned("hello".into()),
            tag: None,
            style: ScalarStyle::Plain,
            span: synth_span(),
        });
        let value = Value::from(node.clone());
        assert_eq!(value.as_str(), Some("hello"));
        let back: Node<'static> = value.into();
        assert_eq!(back, node);
    }

    #[test]
    fn serialize_value_scalar() {
        let value = Value::new(Node::Scalar(Scalar {
            value: Cow::Owned("hello".into()),
            tag: None,
            style: ScalarStyle::Plain,
            span: synth_span(),
        }));
        let yaml = ser::to_string(&value).unwrap();
        assert_eq!(yaml, "hello\n");
    }

    #[test]
    fn serialize_value_sequence() {
        let value = Value::new(Node::Sequence(Sequence {
            items: vec![
                Node::Scalar(Scalar {
                    value: Cow::Owned("a".into()),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: synth_span(),
                }),
                Node::Scalar(Scalar {
                    value: Cow::Owned("b".into()),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: synth_span(),
                }),
            ],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }));
        let yaml = ser::to_string(&value).unwrap();
        assert!(yaml.contains("- a"));
        assert!(yaml.contains("- b"));
    }

    #[test]
    fn serialize_value_mapping() {
        let value = Value::new(Node::Mapping(Mapping {
            entries: vec![(
                Node::Scalar(Scalar {
                    value: Cow::Owned("key".into()),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: synth_span(),
                }),
                Node::Scalar(Scalar {
                    value: Cow::Owned("val".into()),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: synth_span(),
                }),
            )],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        }));
        let yaml = ser::to_string(&value).unwrap();
        assert!(yaml.contains("key: val"));
    }

    #[test]
    fn deserialize_value_from_yaml() {
        let value: Value = de::from_str("hello: world").unwrap();
        assert!(value.is_mapping());
        assert_eq!(value.as_mapping().unwrap().len(), 1);
    }

    /// A borrowing deserializer can hand the `ValueVisitor` a `&'de str` via
    /// `visit_borrowed_str`; it must forward to `visit_str`, producing an owned
    /// double-quoted scalar. Drive the visitor method directly since skald's own
    /// deserializer always calls `visit_str`.
    #[test]
    fn value_visitor_visit_borrowed_str_forwards_to_visit_str() {
        use serde::de::Visitor as _;
        let node = ValueVisitor
            .visit_borrowed_str::<serde::de::value::Error>("borrowed")
            .unwrap();
        match node {
            Node::Scalar(s) => {
                assert_eq!(&*s.value, "borrowed");
                assert_eq!(s.style, ScalarStyle::DoubleQuoted);
            }
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_value_sequence() {
        let value: Value = de::from_str("- 1\n- 2\n- 3").unwrap();
        assert!(value.is_sequence());
        assert_eq!(value.as_sequence().unwrap().len(), 3);
    }

    #[test]
    fn roundtrip_value_through_yaml() {
        let original: Value = de::from_str("name: skald\nversion: 1").unwrap();
        let yaml = ser::to_string(&original).unwrap();
        let roundtripped: Value = de::from_str(&yaml).unwrap();
        assert_eq!(
            original.as_mapping().unwrap().len(),
            roundtripped.as_mapping().unwrap().len()
        );
    }

    #[test]
    fn value_deref_to_node() {
        let value = Value::new(Node::Scalar(Scalar {
            value: Cow::Owned("test".into()),
            tag: None,
            style: ScalarStyle::Plain,
            span: synth_span(),
        }));
        // Deref lets us call Node methods directly
        assert!(value.is_scalar());
        assert_eq!(value.as_str(), Some("test"));
    }

    #[test]
    fn deserialize_nested_value() {
        let yaml = "server:\n  host: localhost\n  port: 8080";
        let value: Value = de::from_str(yaml).unwrap();
        assert!(value.is_mapping());
        let entries = value.as_mapping().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].1.is_mapping());
    }

    #[test]
    fn value_from_node_function() {
        let node = Node::Sequence(Sequence {
            items: vec![Node::Scalar(Scalar {
                value: Cow::Owned("item".into()),
                tag: None,
                style: ScalarStyle::Plain,
                span: synth_span(),
            })],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let value = value_from_node(&node).unwrap();
        assert!(value.is_sequence());
        assert_eq!(value.as_sequence().unwrap().len(), 1);
    }

    // ─── Direct method-call tests ────────────────────────────────────
    // These exercise Value's accessor methods directly so tarpaulin can
    // count their bodies (the YAML round-trip tests above never call
    // `into_node` or `as_node` explicitly because of `Deref` shadowing).

    fn sample_scalar() -> Node<'static> {
        Node::Scalar(Scalar {
            value: Cow::Owned("x".into()),
            tag: None,
            style: ScalarStyle::Plain,
            span: synth_span(),
        })
    }

    #[test]
    fn value_new_then_into_node_returns_original() {
        let original = sample_scalar();
        let v = Value::new(original.clone());
        assert_eq!(v.into_node(), original);
    }

    #[test]
    fn value_as_node_borrows_inner() {
        let original = sample_scalar();
        let v = Value::new(original.clone());
        assert_eq!(v.as_node(), &original);
    }

    #[test]
    fn node_to_value_round_trips_scalar() {
        let node = sample_scalar();
        let v = node_to_value(&node).unwrap();
        assert_eq!(v.as_str(), Some("x"));
    }

    // ─── Direct ValueVisitor tests ───────────────────────────────────
    // The YAML deserializer routes nodes through Node→Value bridging
    // and never hits visit_bool / visit_i64 / visit_u64 / visit_f64 etc.
    // These tests drive the Visitor directly to exercise every branch.

    use serde::de::value::{BoolDeserializer, F64Deserializer, MapDeserializer, SeqDeserializer};
    use serde::de::{Deserialize, Visitor};

    /// Shortcut for tests: any de::Error type works for the visitor's E param.
    type DeErr = serde::de::value::Error;

    fn scalar_value(node: Node<'static>) -> String {
        if let Node::Scalar(s) = node {
            s.value.into_owned()
        } else {
            panic!("expected Node::Scalar, got {node:?}");
        }
    }

    #[test]
    fn visit_bool_true_yields_plain_true() {
        let n = ValueVisitor.visit_bool::<DeErr>(true).unwrap();
        assert_eq!(scalar_value(n), "true");
    }

    #[test]
    fn visit_bool_false_yields_plain_false() {
        let n = ValueVisitor.visit_bool::<DeErr>(false).unwrap();
        assert_eq!(scalar_value(n), "false");
    }

    #[test]
    fn visit_i64_negative() {
        let n = ValueVisitor.visit_i64::<DeErr>(-42).unwrap();
        assert_eq!(scalar_value(n), "-42");
    }

    #[test]
    fn visit_u64_positive() {
        let n = ValueVisitor.visit_u64::<DeErr>(42).unwrap();
        assert_eq!(scalar_value(n), "42");
    }

    #[test]
    fn visit_f64_finite_owned_string() {
        let n = ValueVisitor.visit_f64::<DeErr>(3.5).unwrap();
        // Finite floats go down the Cow::Owned(v.to_string()) branch.
        assert!(scalar_value(n).starts_with("3.5"));
    }

    #[test]
    fn visit_f64_nan_uses_yaml_special() {
        let n = ValueVisitor.visit_f64::<DeErr>(f64::NAN).unwrap();
        assert_eq!(scalar_value(n), ".nan");
    }

    #[test]
    fn visit_f64_positive_infinity() {
        let n = ValueVisitor.visit_f64::<DeErr>(f64::INFINITY).unwrap();
        assert_eq!(scalar_value(n), ".inf");
    }

    #[test]
    fn visit_f64_negative_infinity() {
        let n = ValueVisitor.visit_f64::<DeErr>(f64::NEG_INFINITY).unwrap();
        assert_eq!(scalar_value(n), "-.inf");
    }

    #[test]
    fn visit_string_yields_double_quoted() {
        let n = ValueVisitor
            .visit_string::<DeErr>("hello".to_string())
            .unwrap();
        if let Node::Scalar(s) = n {
            assert_eq!(s.value, "hello");
            assert_eq!(s.style, ScalarStyle::DoubleQuoted);
        } else {
            panic!("expected scalar");
        }
    }

    #[test]
    fn visit_none_yields_null() {
        let n = ValueVisitor.visit_none::<DeErr>().unwrap();
        assert_eq!(scalar_value(n), "null");
    }

    #[test]
    fn visit_unit_yields_null() {
        let n = ValueVisitor.visit_unit::<DeErr>().unwrap();
        assert_eq!(scalar_value(n), "null");
    }

    #[test]
    fn visit_some_forwards_to_inner_deserializer() {
        // visit_some delegates to deserialize_any on the inner deserializer.
        // A BoolDeserializer's deserialize_any calls visit_bool.
        let inner: BoolDeserializer<DeErr> = BoolDeserializer::new(true);
        let n = ValueVisitor.visit_some::<_>(inner).unwrap();
        assert_eq!(scalar_value(n), "true");
    }

    #[test]
    fn visit_seq_with_three_elements_pushes_each() {
        // SeqDeserializer drives visit_seq, which pushes each next_element.
        let de: SeqDeserializer<_, DeErr> = SeqDeserializer::new(vec![1i32, 2, 3].into_iter());
        let value = Value::deserialize(de).unwrap();
        if let Node::Sequence(seq) = value.0 {
            assert_eq!(seq.items.len(), 3);
            assert_eq!(seq.style, CollectionStyle::Block);
        } else {
            panic!("expected sequence");
        }
    }

    #[test]
    fn visit_map_with_two_entries_pushes_each() {
        // MapDeserializer drives visit_map.
        let de: MapDeserializer<_, DeErr> =
            MapDeserializer::new(vec![("a", 1i32), ("b", 2)].into_iter());
        let value = Value::deserialize(de).unwrap();
        if let Node::Mapping(m) = value.0 {
            assert_eq!(m.entries.len(), 2);
            assert_eq!(m.style, CollectionStyle::Block);
        } else {
            panic!("expected mapping");
        }
    }

    #[test]
    fn deserialize_value_bool_via_real_pipeline() {
        // Routes through the real Deserializer: deserialize_any sees a plain
        // "true"/"false" scalar and calls ValueVisitor::visit_bool, exercising
        // its production body via the public API (not the direct-call test,
        // which tarpaulin ignores under `ignore-tests`). The body builds the
        // returned Scalar from the conditional `if v { "true" } else
        // { "false" }`, so matching every field observes both branches.
        let value: Value = de::from_str("true").unwrap();
        match value.as_node() {
            Node::Scalar(s) => {
                assert_eq!(s.value, "true");
                assert_eq!(s.style, ScalarStyle::Plain);
                assert!(s.tag.is_none());
            }
            other => panic!("expected scalar, got {other:?}"),
        }
        let f: Value = de::from_str("false").unwrap();
        assert_eq!(f.as_str(), Some("false"));
    }

    #[test]
    fn value_visitor_expecting_message() {
        // The `expecting` impl is called by serde when constructing error
        // messages; force a type mismatch and check the message contains
        // the expected wording.
        let de: F64Deserializer<DeErr> = F64Deserializer::new(1.0);
        // Asking for a u8 from a float deserializer will eventually surface
        // ValueVisitor::expecting via serde's error path — but Value's
        // visitor accepts any primitive so this path is hard to trigger
        // via real deserializers. Format the message directly via fmt.
        let mut buf = String::new();
        let _ =
            std::fmt::Write::write_fmt(&mut buf, format_args!("{}", DummyDisplay(ValueVisitor)));
        assert!(buf.contains("a YAML value"));
        // Suppress unused warning on the deserializer we constructed above.
        let _ = de;
    }

    /// Wrapper that calls a Visitor's `expecting` method via Display.
    struct DummyDisplay<V>(V);
    impl<V: Visitor<'static>> fmt::Display for DummyDisplay<V> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.expecting(f)
        }
    }

    // ─── Tag retention tests ─────────────────────────────────────────

    #[test]
    fn value_preserves_custom_tag_via_node_path() {
        // The node path (Composer → Node → Value::new) preserves YAML tags.
        // For `!mytag hello`, the parser delivers handle="!" suffix="mytag",
        // so resolve_tag produces the concatenation "!mytag".
        let node = skald_ast::composer::Composer::new("!mytag hello\n")
            .next()
            .unwrap()
            .unwrap()
            .into_owned();
        let v = Value::new(node);
        // Tag text is preserved verbatim.
        assert_eq!(v.tag().map(|t| t.value.to_string()), Some("!mytag".into()));
        // The scalar value is also intact.
        assert_eq!(v.as_str(), Some("hello"));
    }

    #[test]
    fn typed_serde_path_normalizes_tagged_scalar() {
        // Through the typed serde deserializer, a tagged plain scalar is
        // resolved by its content, not by its tag.  Serde's data model has no
        // tag concept, so `!mytag hello` deserializes to String("hello") —
        // the tag is silently normalized away.  This is expected behavior, not
        // a bug.  Use `from_str_node` + `Value::tag()` when you need the tag.
        let result: String = de::from_str("!mytag hello").unwrap();
        assert_eq!(result, "hello");
    }
}
