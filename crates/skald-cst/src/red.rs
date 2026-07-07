// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Red tree: a cursor over the green tree providing absolute byte offsets.
//!
//! No parent pointers are stored (keeps the tree acyclic and `unsafe`-free);
//! navigation is downward, with offsets computed on traversal.

use crate::green::{GreenChild, GreenNode, GreenToken};
use crate::kind::SyntaxKind;
use std::rc::Rc;

/// A node cursor: a green node plus its absolute start offset.
#[derive(Clone)]
pub struct SyntaxNode {
    green: Rc<GreenNode>,
    offset: usize,
}

/// A child element: a subtree node or a leaf token, each with its offset.
#[derive(Clone)]
pub enum SyntaxElement {
    /// A subtree.
    Node(SyntaxNode),
    /// A token: its green token + absolute start offset.
    Token(Rc<GreenToken>, usize),
}

impl SyntaxElement {
    /// `(start, end)` absolute byte offsets.
    #[must_use]
    pub fn text_range(&self) -> (usize, usize) {
        match self {
            SyntaxElement::Node(n) => n.text_range(),
            SyntaxElement::Token(t, off) => (*off, off + t.text_len()),
        }
    }

    /// The element's kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        match self {
            SyntaxElement::Node(n) => n.kind(),
            SyntaxElement::Token(t, _) => t.kind(),
        }
    }

    /// The element's source text.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            SyntaxElement::Node(n) => n.text(),
            SyntaxElement::Token(t, _) => t.text().to_string(),
        }
    }

    /// If this element is a node, returns it.
    #[must_use]
    pub fn as_node(&self) -> Option<&SyntaxNode> {
        match self {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(..) => None,
        }
    }
}

impl SyntaxNode {
    /// Wraps a green root at offset 0.
    #[must_use]
    pub fn new_root(green: Rc<GreenNode>) -> Self {
        Self { green, offset: 0 }
    }

    /// The node kind.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        self.green.kind()
    }

    /// `(start, end)` absolute byte offsets.
    #[must_use]
    pub fn text_range(&self) -> (usize, usize) {
        (self.offset, self.offset + self.green.text_len())
    }

    /// This node's full source text.
    #[must_use]
    pub fn text(&self) -> String {
        let mut s = String::new();
        self.green.write_text(&mut s);
        s
    }

    /// Iterates children (nodes and tokens) with computed absolute offsets.
    pub fn children_with_tokens(&self) -> impl Iterator<Item = SyntaxElement> + '_ {
        let mut cur = self.offset;
        self.green.children().iter().map(move |c| {
            let start = cur;
            cur += c.text_len();
            match c {
                GreenChild::Node(n) => SyntaxElement::Node(SyntaxNode {
                    green: n.clone(),
                    offset: start,
                }),
                GreenChild::Token(t) => SyntaxElement::Token(t.clone(), start),
            }
        })
    }

    /// Iterates only child nodes (skipping tokens).
    pub fn child_nodes(&self) -> impl Iterator<Item = SyntaxNode> + '_ {
        self.children_with_tokens().filter_map(|e| match e {
            SyntaxElement::Node(n) => Some(n),
            SyntaxElement::Token(..) => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::GreenNodeBuilder;
    use crate::kind::SyntaxKind;

    #[test]
    fn red_offsets_and_text() {
        let mut b = GreenNodeBuilder::new();
        b.start_node(SyntaxKind::Root);
        b.token(SyntaxKind::ScalarToken, "a");
        b.token(SyntaxKind::Punct, ": ");
        b.token(SyntaxKind::ScalarToken, "b");
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());
        assert_eq!(root.text_range(), (0, 4));
        let kids: Vec<_> = root.children_with_tokens().collect();
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[2].text_range(), (3, 4)); // "b"
        assert_eq!(root.text(), "a: b");
    }

    /// Builds a tree with a nested child node so that `SyntaxElement::Node`
    /// arms of `text_range`/`kind`/`text`/`as_node` and the `child_nodes`
    /// filter are all exercised alongside their token counterparts.
    #[test]
    fn red_element_accessors_cover_node_and_token_arms() {
        let mut b = GreenNodeBuilder::new();
        b.start_node(SyntaxKind::Root);
        b.start_node(SyntaxKind::Scalar); // nested child node
        b.token(SyntaxKind::ScalarToken, "hi");
        b.finish_node();
        b.token(SyntaxKind::Newline, "\n"); // sibling token
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());

        let elems: Vec<SyntaxElement> = root.children_with_tokens().collect();
        assert_eq!(elems.len(), 2);

        // First element is the nested node.
        let node_elem = &elems[0];
        assert_eq!(node_elem.kind(), SyntaxKind::Scalar);
        assert_eq!(node_elem.text(), "hi");
        assert_eq!(node_elem.text_range(), (0, 2));
        let inner = node_elem.as_node().expect("node element yields Some");
        assert_eq!(inner.kind(), SyntaxKind::Scalar);

        // Second element is the trailing token.
        let tok_elem = &elems[1];
        assert_eq!(tok_elem.kind(), SyntaxKind::Newline);
        assert_eq!(tok_elem.text(), "\n");
        assert_eq!(tok_elem.text_range(), (2, 3));
        assert!(tok_elem.as_node().is_none(), "token element yields None");

        // child_nodes skips the trailing token, keeping only the nested node.
        let child_nodes: Vec<SyntaxNode> = root.child_nodes().collect();
        assert_eq!(child_nodes.len(), 1);
        assert_eq!(child_nodes[0].kind(), SyntaxKind::Scalar);
    }
}
