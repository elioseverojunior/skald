// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Core YAML data types.
//!
//! Defines source-location, style, and shared types used throughout
//! the pipeline. The tree types (`Node`, `Scalar`, `Sequence`, `Mapping`)
//! are defined in and re-exported from `skald-ast`.

use std::fmt;

// ─── Source Location ────────────────────────────────────────────────

/// A byte offset and line/column position within a YAML source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Byte offset from the start of the input.
    pub offset: usize,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (in bytes, not characters).
    pub column: u32,
}

impl Position {
    /// Creates a new position at the start of the input.
    #[must_use]
    pub fn start() -> Self {
        Self {
            offset: 0,
            line: 1,
            column: 1,
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A span covering a range in the source input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Start position (inclusive).
    pub start: Position,
    /// End position (exclusive).
    pub end: Position,
}

impl Span {
    /// Creates a zero-width span at the given position.
    #[must_use]
    pub fn point(pos: Position) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    /// Merges two spans into one that covers both.
    #[must_use]
    pub fn merge(self, other: Span) -> Span {
        let start = if self.start.offset <= other.start.offset {
            self.start
        } else {
            other.start
        };
        let end = if self.end.offset >= other.end.offset {
            self.end
        } else {
            other.end
        };
        Span { start, end }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

// ─── Tags ───────────────────────────────────────────────────────────

/// A YAML tag (e.g. `!!str`, `!!int`, or a custom tag URI).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag<'a> {
    /// The resolved tag URI.
    pub value: std::borrow::Cow<'a, str>,
    /// Source span of the tag in the input.
    pub span: Span,
}

impl<'a> Tag<'a> {
    /// Converts this tag into a `'static` lifetime by taking ownership of borrowed data.
    #[must_use]
    pub fn into_owned(self) -> Tag<'static> {
        Tag {
            value: std::borrow::Cow::Owned(self.value.into_owned()),
            span: self.span,
        }
    }
}

// ─── Styles ─────────────────────────────────────────────────────────

/// The presentation style of a scalar value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarStyle {
    /// No quotes — plain scalar.
    Plain,
    /// Single-quoted scalar (`'...'`).
    SingleQuoted,
    /// Double-quoted scalar (`"..."`).
    DoubleQuoted,
    /// Literal block scalar (`|`).
    Literal,
    /// Folded block scalar (`>`).
    Folded,
}

/// The presentation style of a collection (mapping or sequence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionStyle {
    /// Block style (indentation-based).
    Block,
    /// Flow style (inline, JSON-like with `{}` or `[]`).
    Flow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_start() {
        let pos = Position::start();
        assert_eq!(pos.offset, 0);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.column, 1);
    }

    #[test]
    fn position_display() {
        let pos = Position {
            offset: 42,
            line: 3,
            column: 7,
        };
        assert_eq!(pos.to_string(), "3:7");
    }

    #[test]
    fn span_point() {
        let pos = Position::start();
        let span = Span::point(pos);
        assert_eq!(span.start, span.end);
    }

    #[test]
    fn span_merge() {
        let a = Span {
            start: Position {
                offset: 0,
                line: 1,
                column: 1,
            },
            end: Position {
                offset: 5,
                line: 1,
                column: 6,
            },
        };
        let b = Span {
            start: Position {
                offset: 3,
                line: 1,
                column: 4,
            },
            end: Position {
                offset: 10,
                line: 2,
                column: 3,
            },
        };
        let merged = a.merge(b);
        assert_eq!(merged.start.offset, 0);
        assert_eq!(merged.end.offset, 10);
    }

    #[test]
    fn span_merge_reversed() {
        // other starts before self, and self ends after other:
        // exercises the `other.start` and `self.end` branches of merge.
        let a = Span {
            start: Position {
                offset: 8,
                line: 2,
                column: 1,
            },
            end: Position {
                offset: 20,
                line: 3,
                column: 5,
            },
        };
        let b = Span {
            start: Position {
                offset: 2,
                line: 1,
                column: 3,
            },
            end: Position {
                offset: 12,
                line: 2,
                column: 5,
            },
        };
        let merged = a.merge(b);
        assert_eq!(merged.start.offset, 2);
        assert_eq!(merged.end.offset, 20);
    }

    #[test]
    fn span_display() {
        let span = Span {
            start: Position {
                offset: 0,
                line: 1,
                column: 1,
            },
            end: Position {
                offset: 5,
                line: 1,
                column: 6,
            },
        };
        assert_eq!(span.to_string(), "1:1..1:6");
    }

    #[test]
    fn tag_into_owned() {
        use std::borrow::Cow;
        let tag = Tag {
            value: Cow::Borrowed("!!str"),
            span: Span::point(Position::start()),
        };
        let owned: Tag<'static> = tag.into_owned();
        assert!(matches!(owned.value, Cow::Owned(_)));
        assert_eq!(&*owned.value, "!!str");
    }
}
