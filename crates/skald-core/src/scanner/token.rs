// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Token types produced by the YAML scanner.

use crate::types::{ScalarStyle, Span};
use std::borrow::Cow;
use std::fmt;

/// A token produced by the scanner, with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token<'a> {
    /// The kind of token.
    pub kind: TokenKind<'a>,
    /// The source span of this token.
    pub span: Span,
}

/// The kind of a YAML token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind<'a> {
    /// Start of the YAML stream.
    StreamStart,
    /// End of the YAML stream.
    StreamEnd,

    /// Explicit document start: `---`.
    DocumentStart,
    /// Explicit document end: `...`.
    DocumentEnd,

    /// Start of a block sequence (implicit, emitted on indentation increase).
    BlockSequenceStart,
    /// Start of a block mapping (implicit, emitted on indentation increase).
    BlockMappingStart,
    /// End of a block collection (implicit, emitted on indentation decrease).
    BlockEnd,

    /// Start of a flow sequence: `[`.
    FlowSequenceStart,
    /// End of a flow sequence: `]`.
    FlowSequenceEnd,
    /// Start of a flow mapping: `{`.
    FlowMappingStart,
    /// End of a flow mapping: `}`.
    FlowMappingEnd,

    /// Entry in a block sequence: `-`.
    BlockEntry,
    /// Entry separator in a flow collection: `,`.
    FlowEntry,

    /// Explicit key indicator: `?`.
    Key,
    /// Value indicator: `:`.
    Value,

    /// An anchor: `&name`.
    Anchor(Cow<'a, str>),
    /// An alias: `*name`.
    Alias(Cow<'a, str>),

    /// A tag: `!handle!suffix` or `!suffix` or `!!suffix`.
    Tag {
        /// The tag handle (e.g. `!`, `!!`, `!handle!`).
        handle: Cow<'a, str>,
        /// The tag suffix.
        suffix: Cow<'a, str>,
    },

    /// A scalar value.
    Scalar {
        /// The scalar value.
        value: Cow<'a, str>,
        /// The presentation style.
        style: ScalarStyle,
    },

    /// A comment, including the leading `#`, excluding the trailing line break.
    /// Only emitted in `preserve_trivia` mode.
    Comment(Cow<'a, str>),
    /// A run of inter-token spaces/tabs (indentation or separation).
    /// Only emitted in `preserve_trivia` mode.
    Whitespace(Cow<'a, str>),
    /// A single line break (`\n`, `\r\n`, or `\r`).
    /// Only emitted in `preserve_trivia` mode.
    LineBreak(Cow<'a, str>),

    /// A `%YAML` directive.
    VersionDirective {
        /// Major version number.
        major: u8,
        /// Minor version number.
        minor: u8,
    },
    /// A `%TAG` directive.
    TagDirective {
        /// The tag handle.
        handle: Cow<'a, str>,
        /// The tag prefix.
        prefix: Cow<'a, str>,
    },
}

impl<'a> TokenKind<'a> {
    /// Returns a human-readable name for this token kind.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            TokenKind::StreamStart => "stream-start",
            TokenKind::StreamEnd => "stream-end",
            TokenKind::DocumentStart => "document-start",
            TokenKind::DocumentEnd => "document-end",
            TokenKind::BlockSequenceStart => "block-sequence-start",
            TokenKind::BlockMappingStart => "block-mapping-start",
            TokenKind::BlockEnd => "block-end",
            TokenKind::FlowSequenceStart => "flow-sequence-start",
            TokenKind::FlowSequenceEnd => "flow-sequence-end",
            TokenKind::FlowMappingStart => "flow-mapping-start",
            TokenKind::FlowMappingEnd => "flow-mapping-end",
            TokenKind::BlockEntry => "block-entry",
            TokenKind::FlowEntry => "flow-entry",
            TokenKind::Key => "key",
            TokenKind::Value => "value",
            TokenKind::Anchor(_) => "anchor",
            TokenKind::Alias(_) => "alias",
            TokenKind::Tag { .. } => "tag",
            TokenKind::Scalar { .. } => "scalar",
            TokenKind::VersionDirective { .. } => "version-directive",
            TokenKind::TagDirective { .. } => "tag-directive",
            TokenKind::Comment(_) => "comment",
            TokenKind::Whitespace(_) => "whitespace",
            TokenKind::LineBreak(_) => "line-break",
        }
    }

    /// Returns `true` for trivia tokens (comments, whitespace, line breaks).
    #[must_use]
    #[rustfmt::skip]
    pub fn is_trivia(&self) -> bool {
        // Single-line `matches!` so the llvm coverage engine credits the body
        // (a multi-line match-pattern arm is not attributed even when executed).
        matches!(self, TokenKind::Comment(_) | TokenKind::Whitespace(_) | TokenKind::LineBreak(_))
    }
}

impl<'a> fmt::Display for TokenKind<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kind_names() {
        assert_eq!(TokenKind::StreamStart.name(), "stream-start");
        assert_eq!(TokenKind::StreamEnd.name(), "stream-end");
        assert_eq!(TokenKind::DocumentStart.name(), "document-start");
        assert_eq!(TokenKind::BlockEntry.name(), "block-entry");
        assert_eq!(
            TokenKind::Scalar {
                value: Cow::Borrowed("test"),
                style: ScalarStyle::Plain,
            }
            .name(),
            "scalar"
        );
    }

    #[test]
    fn token_kind_display() {
        assert_eq!(
            TokenKind::FlowMappingStart.to_string(),
            "flow-mapping-start"
        );
    }

    /// Exercise the four `name()` arms that the YAML test suite rarely
    /// triggers through end-to-end parsing (anchor/alias/tag/directive
    /// classifications).
    #[test]
    fn trivia_variants_report_as_trivia() {
        use std::borrow::Cow;
        assert!(TokenKind::Comment(Cow::Borrowed("# hi")).is_trivia());
        assert!(TokenKind::Whitespace(Cow::Borrowed("  ")).is_trivia());
        assert!(TokenKind::LineBreak(Cow::Borrowed("\n")).is_trivia());
        assert!(!TokenKind::StreamStart.is_trivia());
        assert!(!TokenKind::Value.is_trivia());
    }

    #[test]
    fn token_kind_name_covers_node_decorations_and_directives() {
        assert_eq!(TokenKind::Anchor(Cow::Borrowed("a")).name(), "anchor");
        assert_eq!(TokenKind::Alias(Cow::Borrowed("a")).name(), "alias");
        assert_eq!(
            TokenKind::Tag {
                handle: Cow::Borrowed("!"),
                suffix: Cow::Borrowed("foo"),
            }
            .name(),
            "tag"
        );
        assert_eq!(
            TokenKind::VersionDirective { major: 1, minor: 2 }.name(),
            "version-directive"
        );
        assert_eq!(
            TokenKind::TagDirective {
                handle: Cow::Borrowed("!!"),
                prefix: Cow::Borrowed("tag:yaml.org,2002:"),
            }
            .name(),
            "tag-directive"
        );
    }

    /// The trivia `name()` arms are not produced by default (non-trivia)
    /// scanning, so name them directly.
    #[test]
    fn token_kind_name_covers_trivia_variants() {
        assert_eq!(TokenKind::Comment(Cow::Borrowed("# c")).name(), "comment");
        assert_eq!(
            TokenKind::Whitespace(Cow::Borrowed("  ")).name(),
            "whitespace"
        );
        assert_eq!(
            TokenKind::LineBreak(Cow::Borrowed("\n")).name(),
            "line-break"
        );
    }
}
