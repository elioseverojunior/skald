// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Builds a green tree via start_node/token/finish_node.

use crate::green::{GreenChild, GreenNode};
use crate::kind::SyntaxKind;
use std::rc::Rc;

/// Incrementally builds a [`GreenNode`] tree.
#[derive(Default)]
pub struct GreenNodeBuilder {
    stack: Vec<(SyntaxKind, Vec<GreenChild>)>,
    root: Option<Rc<GreenNode>>,
}

impl GreenNodeBuilder {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a new node; subsequent tokens/nodes attach to it until `finish_node`.
    pub fn start_node(&mut self, kind: SyntaxKind) {
        self.stack.push((kind, Vec::new()));
    }

    /// Adds a leaf token to the currently open node.
    pub fn token(&mut self, kind: SyntaxKind, text: &str) {
        self.stack
            .last_mut()
            .expect("token() called before start_node()")
            .1
            .push(GreenChild::token(kind, text));
    }

    /// Closes the current node, attaching it to its parent, or recording it as the root.
    pub fn finish_node(&mut self) {
        let (kind, children) = self
            .stack
            .pop()
            .expect("finish_node() without start_node()");
        let node = GreenNode::new(kind, children);
        match self.stack.last_mut() {
            Some(parent) => parent.1.push(GreenChild::Node(node)),
            None => self.root = Some(node),
        }
    }

    /// Finishes building and returns the root node. Panics if unbalanced.
    #[must_use]
    pub fn finish(self) -> Rc<GreenNode> {
        assert!(self.stack.is_empty(), "finish() with unclosed nodes");
        self.root.expect("finish() without a completed root node")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kind::SyntaxKind;

    #[test]
    fn builds_nested_tree_losslessly() {
        let mut b = GreenNodeBuilder::new();
        b.start_node(SyntaxKind::Root);
        b.start_node(SyntaxKind::Scalar);
        b.token(SyntaxKind::ScalarToken, "hi");
        b.finish_node();
        b.token(SyntaxKind::Newline, "\n");
        b.finish_node();
        let root = b.finish();
        let mut s = String::new();
        root.write_text(&mut s);
        assert_eq!(s, "hi\n");
        assert_eq!(root.kind(), SyntaxKind::Root);
    }
}
