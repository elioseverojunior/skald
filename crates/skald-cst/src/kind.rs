// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Syntax kinds for the YAML concrete syntax tree.

/// The kind of a CST node or token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxKind {
    /// The whole parsed stream (root).
    Root,
    /// A single document.
    Document,
    /// A block or flow mapping.
    Mapping,
    /// One `key: value` entry.
    MappingEntry,
    /// A block or flow sequence.
    Sequence,
    /// One sequence item.
    SequenceItem,
    /// A scalar value node.
    Scalar,
    /// A scalar token's raw source text.
    ScalarToken,
    /// Structural punctuation (`:`, `-`, `?`, `[`, `]`, `{`, `}`, `,`, `---`, `...`).
    Punct,
    /// An anchor/alias/tag token.
    Property,
    /// A comment.
    Comment,
    /// Inter-token spaces/tabs.
    Whitespace,
    /// A line break.
    Newline,
    /// Unclassified bytes (defensive; preserves losslessness).
    Error,
}

impl SyntaxKind {
    /// Returns true for token (leaf) kinds.
    #[must_use]
    pub fn is_token(self) -> bool {
        matches!(
            self,
            SyntaxKind::ScalarToken
                | SyntaxKind::Punct
                | SyntaxKind::Property
                | SyntaxKind::Comment
                | SyntaxKind::Whitespace
                | SyntaxKind::Newline
                | SyntaxKind::Error
        )
    }

    /// Returns true for trivia token kinds.
    #[must_use]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::Comment | SyntaxKind::Whitespace | SyntaxKind::Newline
        )
    }
}

#[cfg(test)]
mod tests {
    use super::SyntaxKind;

    #[test]
    fn is_token_classifies_leaves_and_inner_nodes() {
        for k in [
            SyntaxKind::ScalarToken,
            SyntaxKind::Punct,
            SyntaxKind::Property,
            SyntaxKind::Comment,
            SyntaxKind::Whitespace,
            SyntaxKind::Newline,
            SyntaxKind::Error,
        ] {
            assert!(k.is_token(), "{k:?} must be a token");
        }
        for k in [
            SyntaxKind::Root,
            SyntaxKind::Document,
            SyntaxKind::Mapping,
            SyntaxKind::MappingEntry,
            SyntaxKind::Sequence,
            SyntaxKind::SequenceItem,
            SyntaxKind::Scalar,
        ] {
            assert!(!k.is_token(), "{k:?} must not be a token");
        }
    }

    #[test]
    fn is_trivia_classifies_only_comment_whitespace_newline() {
        for k in [
            SyntaxKind::Comment,
            SyntaxKind::Whitespace,
            SyntaxKind::Newline,
        ] {
            assert!(k.is_trivia(), "{k:?} must be trivia");
        }
        for k in [
            SyntaxKind::ScalarToken,
            SyntaxKind::Punct,
            SyntaxKind::Property,
            SyntaxKind::Error,
            SyntaxKind::Scalar,
        ] {
            assert!(!k.is_trivia(), "{k:?} must not be trivia");
        }
    }
}
