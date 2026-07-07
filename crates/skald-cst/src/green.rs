// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Immutable, structurally-shared green tree.

use crate::kind::SyntaxKind;
use std::rc::Rc;

/// A leaf token holding its exact source text.
#[derive(Debug, PartialEq, Eq)]
pub struct GreenToken {
    kind: SyntaxKind,
    text: Box<str>,
}

impl GreenToken {
    /// The token kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// The token's exact source text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte length of the token text.
    #[must_use]
    pub fn text_len(&self) -> usize {
        self.text.len()
    }
}

/// A child of a green node: a subtree or a token.
#[derive(Debug, Clone)]
pub enum GreenChild {
    /// A subtree.
    Node(Rc<GreenNode>),
    /// A leaf token.
    Token(Rc<GreenToken>),
}

impl GreenChild {
    /// Builds a token child from kind + text.
    #[must_use]
    pub fn token(kind: SyntaxKind, text: &str) -> Self {
        GreenChild::Token(Rc::new(GreenToken {
            kind,
            text: text.into(),
        }))
    }

    /// Byte length of this child's text.
    #[must_use]
    pub fn text_len(&self) -> usize {
        match self {
            GreenChild::Node(n) => n.text_len(),
            GreenChild::Token(t) => t.text_len(),
        }
    }

    /// Appends this child's source text to `out`.
    pub fn write_text(&self, out: &mut String) {
        match self {
            GreenChild::Node(n) => n.write_text(out),
            GreenChild::Token(t) => out.push_str(t.text()),
        }
    }
}

/// An interior green node (immutable, `Rc`-shared).
#[derive(Debug)]
pub struct GreenNode {
    kind: SyntaxKind,
    children: Vec<GreenChild>,
    text_len: usize,
}

impl GreenNode {
    /// Builds a node, caching its total text length. Returns an `Rc` for sharing.
    #[must_use]
    pub fn new(kind: SyntaxKind, children: Vec<GreenChild>) -> Rc<Self> {
        let text_len = children.iter().map(GreenChild::text_len).sum();
        Rc::new(GreenNode {
            kind,
            children,
            text_len,
        })
    }

    /// The node kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// Total byte length of the node's text.
    #[must_use]
    pub fn text_len(&self) -> usize {
        self.text_len
    }

    /// The node's children.
    #[must_use]
    pub fn children(&self) -> &[GreenChild] {
        &self.children
    }

    /// Appends the node's full source text to `out`.
    pub fn write_text(&self, out: &mut String) {
        for c in &self.children {
            c.write_text(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::SyntaxKind;

    #[test]
    fn green_text_len_and_text() {
        let t1 = GreenChild::token(SyntaxKind::ScalarToken, "hello");
        let t2 = GreenChild::token(SyntaxKind::Whitespace, " ");
        let node = GreenNode::new(SyntaxKind::Scalar, vec![t1, t2]);
        assert_eq!(node.text_len(), 6);
        let mut s = String::new();
        node.write_text(&mut s);
        assert_eq!(s, "hello ");
    }
}
