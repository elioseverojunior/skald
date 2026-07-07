// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Events produced by the YAML parser.
//!
//! Events represent the SAX-style stream of a YAML document.
//! They are consumed by the composer to build the representation graph,
//! or by the serde deserializer for streaming deserialization.

use crate::types::{CollectionStyle, ScalarStyle, Span};
use std::borrow::Cow;

/// A parser event with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Event<'a> {
    /// The kind of event.
    pub kind: EventKind<'a>,
    /// The source span.
    pub span: Span,
}

/// The kind of a parser event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventKind<'a> {
    /// Start of the YAML stream.
    StreamStart,
    /// End of the YAML stream.
    StreamEnd,

    /// Start of a YAML document.
    DocumentStart {
        /// Whether the document start was explicit (`---`).
        explicit: bool,
    },
    /// End of a YAML document.
    DocumentEnd {
        /// Whether the document end was explicit (`...`).
        explicit: bool,
    },

    /// Start of a mapping.
    MappingStart {
        /// Optional anchor name.
        anchor: Option<Cow<'a, str>>,
        /// Optional tag.
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        /// Block or flow style.
        style: CollectionStyle,
    },
    /// End of a mapping.
    MappingEnd,

    /// Start of a sequence.
    SequenceStart {
        /// Optional anchor name.
        anchor: Option<Cow<'a, str>>,
        /// Optional tag.
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        /// Block or flow style.
        style: CollectionStyle,
    },
    /// End of a sequence.
    SequenceEnd,

    /// A scalar value.
    Scalar {
        /// The scalar value.
        value: Cow<'a, str>,
        /// The presentation style.
        style: ScalarStyle,
        /// Optional anchor name.
        anchor: Option<Cow<'a, str>>,
        /// Optional tag.
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
    },

    /// An alias reference.
    Alias {
        /// The anchor name being referenced.
        name: Cow<'a, str>,
    },
}

impl<'a> EventKind<'a> {
    /// Returns a human-readable name for this event kind.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::StreamStart => "stream-start",
            EventKind::StreamEnd => "stream-end",
            EventKind::DocumentStart { .. } => "document-start",
            EventKind::DocumentEnd { .. } => "document-end",
            EventKind::MappingStart { .. } => "mapping-start",
            EventKind::MappingEnd => "mapping-end",
            EventKind::SequenceStart { .. } => "sequence-start",
            EventKind::SequenceEnd => "sequence-end",
            EventKind::Scalar { .. } => "scalar",
            EventKind::Alias { .. } => "alias",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_names() {
        assert_eq!(EventKind::StreamStart.name(), "stream-start");
        assert_eq!(
            EventKind::DocumentStart { explicit: true }.name(),
            "document-start"
        );
        assert_eq!(
            EventKind::Scalar {
                value: Cow::Borrowed("test"),
                style: ScalarStyle::Plain,
                anchor: None,
                tag: None,
            }
            .name(),
            "scalar"
        );
        assert_eq!(
            EventKind::Alias {
                name: Cow::Borrowed("a")
            }
            .name(),
            "alias"
        );
    }
}
