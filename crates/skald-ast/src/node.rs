// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The YAML representation tree (`Node`) and its constituent nodes.

use std::borrow::Cow;

use skald_core::types::{CollectionStyle, ScalarStyle, Span, Tag};

// ─── Representation Graph ───────────────────────────────────────────

/// A YAML node in the representation graph.
///
/// The lifetime `'a` ties borrowed scalar values to the input source,
/// enabling zero-copy parsing for plain scalars.
#[derive(Debug, Clone, PartialEq)]
pub enum Node<'a> {
    /// A scalar (leaf) value.
    Scalar(Scalar<'a>),
    /// An ordered sequence of nodes.
    Sequence(Sequence<'a>),
    /// An ordered mapping of key-value pairs.
    Mapping(Mapping<'a>),
}

impl<'a> Node<'a> {
    /// Returns the span of this node in the source input.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Node::Scalar(s) => s.span,
            Node::Sequence(s) => s.span,
            Node::Mapping(m) => m.span,
        }
    }

    /// Returns the tag of this node, if any.
    #[must_use]
    pub fn tag(&self) -> Option<&Tag<'a>> {
        match self {
            Node::Scalar(s) => s.tag.as_ref(),
            Node::Sequence(s) => s.tag.as_ref(),
            Node::Mapping(m) => m.tag.as_ref(),
        }
    }

    /// Returns `true` if this node is a scalar.
    #[must_use]
    pub fn is_scalar(&self) -> bool {
        matches!(self, Node::Scalar(_))
    }

    /// Returns `true` if this node is a sequence.
    #[must_use]
    pub fn is_sequence(&self) -> bool {
        matches!(self, Node::Sequence(_))
    }

    /// Returns `true` if this node is a mapping.
    #[must_use]
    pub fn is_mapping(&self) -> bool {
        matches!(self, Node::Mapping(_))
    }

    /// Returns the scalar value as a string slice, if this is a scalar node.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar(s) => Some(&s.value),
            _ => None,
        }
    }

    /// Returns a reference to the sequence items, if this is a sequence node.
    #[must_use]
    pub fn as_sequence(&self) -> Option<&[Node<'a>]> {
        match self {
            Node::Sequence(s) => Some(&s.items),
            _ => None,
        }
    }

    /// Returns a reference to the mapping entries, if this is a mapping node.
    #[must_use]
    pub fn as_mapping(&self) -> Option<&[(Node<'a>, Node<'a>)]> {
        match self {
            Node::Mapping(m) => Some(&m.entries),
            _ => None,
        }
    }

    /// Converts this node into a `'static` lifetime by taking ownership of all borrowed data.
    #[must_use]
    pub fn into_owned(self) -> Node<'static> {
        match self {
            Node::Scalar(s) => Node::Scalar(s.into_owned()),
            Node::Sequence(s) => Node::Sequence(s.into_owned()),
            Node::Mapping(m) => Node::Mapping(m.into_owned()),
        }
    }
}

/// A YAML scalar (leaf) value.
#[derive(Debug, Clone, PartialEq)]
pub struct Scalar<'a> {
    /// The scalar value, borrowing from the input when no transformation is needed.
    pub value: Cow<'a, str>,
    /// Optional YAML tag.
    pub tag: Option<Tag<'a>>,
    /// The presentation style used in the source.
    pub style: ScalarStyle,
    /// Source span.
    pub span: Span,
}

impl<'a> Scalar<'a> {
    /// Converts this scalar into a `'static` lifetime by taking ownership of borrowed data.
    #[must_use]
    pub fn into_owned(self) -> Scalar<'static> {
        Scalar {
            value: Cow::Owned(self.value.into_owned()),
            tag: self.tag.map(Tag::into_owned),
            style: self.style,
            span: self.span,
        }
    }
}

/// A YAML sequence (ordered list).
#[derive(Debug, Clone, PartialEq)]
pub struct Sequence<'a> {
    /// The items in the sequence.
    pub items: Vec<Node<'a>>,
    /// Optional YAML tag.
    pub tag: Option<Tag<'a>>,
    /// The presentation style used in the source.
    pub style: CollectionStyle,
    /// Source span.
    pub span: Span,
}

impl<'a> Sequence<'a> {
    /// Converts this sequence into a `'static` lifetime by taking ownership of borrowed data.
    #[must_use]
    pub fn into_owned(self) -> Sequence<'static> {
        Sequence {
            items: self.items.into_iter().map(Node::into_owned).collect(),
            tag: self.tag.map(Tag::into_owned),
            style: self.style,
            span: self.span,
        }
    }
}

/// A YAML mapping (ordered key-value pairs).
///
/// Uses `Vec<(Node, Node)>` instead of `HashMap` to preserve insertion order
/// (required by the YAML spec) and to support non-string keys.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping<'a> {
    /// The key-value entries in insertion order.
    pub entries: Vec<(Node<'a>, Node<'a>)>,
    /// Optional YAML tag.
    pub tag: Option<Tag<'a>>,
    /// The presentation style used in the source.
    pub style: CollectionStyle,
    /// Source span.
    pub span: Span,
}

impl<'a> Mapping<'a> {
    /// Converts this mapping into a `'static` lifetime by taking ownership of borrowed data.
    #[must_use]
    pub fn into_owned(self) -> Mapping<'static> {
        Mapping {
            entries: self
                .entries
                .into_iter()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
            tag: self.tag.map(Tag::into_owned),
            style: self.style,
            span: self.span,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use skald_core::types::{Position, Span};

    use super::*;

    #[test]
    fn node_span_collections() {
        let span = Span {
            start: Position {
                offset: 1,
                line: 1,
                column: 2,
            },
            end: Position {
                offset: 4,
                line: 1,
                column: 5,
            },
        };
        let seq = Node::Sequence(Sequence {
            items: vec![],
            tag: None,
            style: CollectionStyle::Block,
            span,
        });
        assert_eq!(seq.span(), span);

        let map = Node::Mapping(Mapping {
            entries: vec![],
            tag: None,
            style: CollectionStyle::Block,
            span,
        });
        assert_eq!(map.span(), span);
    }

    #[test]
    fn as_str_on_non_scalar() {
        let seq = Node::Sequence(Sequence {
            items: vec![],
            tag: None,
            style: CollectionStyle::Block,
            span: Span::point(Position::start()),
        });
        assert!(seq.as_str().is_none());
    }

    #[test]
    fn node_accessors() {
        let scalar = Node::Scalar(Scalar {
            value: Cow::Borrowed("hello"),
            tag: None,
            style: ScalarStyle::Plain,
            span: Span::point(Position::start()),
        });
        assert!(scalar.is_scalar());
        assert!(!scalar.is_sequence());
        assert!(!scalar.is_mapping());
        assert_eq!(scalar.as_str(), Some("hello"));
        assert!(scalar.as_sequence().is_none());
        assert!(scalar.as_mapping().is_none());
    }

    #[test]
    fn node_sequence_accessor() {
        let seq = Node::Sequence(Sequence {
            items: vec![Node::Scalar(Scalar {
                value: Cow::Borrowed("item"),
                tag: None,
                style: ScalarStyle::Plain,
                span: Span::point(Position::start()),
            })],
            tag: None,
            style: CollectionStyle::Block,
            span: Span::point(Position::start()),
        });
        assert!(seq.is_sequence());
        assert_eq!(seq.as_sequence().unwrap().len(), 1);
    }

    #[test]
    fn scalar_into_owned() {
        let scalar = Scalar {
            value: Cow::Borrowed("hello"),
            tag: Some(Tag {
                value: Cow::Borrowed("!!str"),
                span: Span::point(Position::start()),
            }),
            style: ScalarStyle::Plain,
            span: Span::point(Position::start()),
        };
        let owned: Scalar<'static> = scalar.into_owned();
        assert_eq!(&*owned.value, "hello");
        assert_eq!(&*owned.tag.unwrap().value, "!!str");
    }

    #[test]
    fn node_into_owned_scalar() {
        let node = Node::Scalar(Scalar {
            value: Cow::Borrowed("test"),
            tag: None,
            style: ScalarStyle::Plain,
            span: Span::point(Position::start()),
        });
        let owned: Node<'static> = node.into_owned();
        assert_eq!(owned.as_str(), Some("test"));
    }

    #[test]
    fn node_into_owned_sequence() {
        let node = Node::Sequence(Sequence {
            items: vec![Node::Scalar(Scalar {
                value: Cow::Borrowed("item"),
                tag: None,
                style: ScalarStyle::Plain,
                span: Span::point(Position::start()),
            })],
            tag: None,
            style: CollectionStyle::Block,
            span: Span::point(Position::start()),
        });
        let owned: Node<'static> = node.into_owned();
        assert_eq!(owned.as_sequence().unwrap().len(), 1);
        assert_eq!(owned.as_sequence().unwrap()[0].as_str(), Some("item"));
    }

    #[test]
    fn node_into_owned_mapping() {
        let node = Node::Mapping(Mapping {
            entries: vec![(
                Node::Scalar(Scalar {
                    value: Cow::Borrowed("key"),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: Span::point(Position::start()),
                }),
                Node::Scalar(Scalar {
                    value: Cow::Borrowed("val"),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: Span::point(Position::start()),
                }),
            )],
            tag: None,
            style: CollectionStyle::Block,
            span: Span::point(Position::start()),
        });
        let owned: Node<'static> = node.into_owned();
        let entries = owned.as_mapping().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.as_str(), Some("key"));
        assert_eq!(entries[0].1.as_str(), Some("val"));
    }

    #[test]
    fn node_mapping_accessor() {
        let map = Node::Mapping(Mapping {
            entries: vec![(
                Node::Scalar(Scalar {
                    value: Cow::Borrowed("key"),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: Span::point(Position::start()),
                }),
                Node::Scalar(Scalar {
                    value: Cow::Borrowed("value"),
                    tag: None,
                    style: ScalarStyle::Plain,
                    span: Span::point(Position::start()),
                }),
            )],
            tag: None,
            style: CollectionStyle::Block,
            span: Span::point(Position::start()),
        });
        assert!(map.is_mapping());
        assert_eq!(map.as_mapping().unwrap().len(), 1);
    }
}
