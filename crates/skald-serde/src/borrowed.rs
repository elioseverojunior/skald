// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A zero-copy YAML value borrowing scalar text directly from the input.
//!
//! [`BorrowedValue`] wraps a [`Node<'a>`] whose scalar slices point directly
//! into the source string — no heap allocation for plain, unescaped scalars.
//! This is the borrowing parallel to the owned [`Value`](crate::Value), which
//! wraps a `Node<'static>`.

use skald_ast::node::Node;
use skald_core::error::{Error, ErrorKind, Result};
use std::ops::Deref;

/// A YAML value that borrows scalar slices directly from the source string.
///
/// Plain (unescaped) scalars are zero-copy: the `Cow<'a, str>` inside each
/// `Scalar` node is `Cow::Borrowed`, pointing into `input` without any heap
/// allocation.  Only scalars that require transformation (escape sequences,
/// block-scalar folding, etc.) allocate an owned `String`.
///
/// Use [`BorrowedValue::parse`] to construct one from a `&str`.
///
/// # Comparison with [`Value`](crate::Value)
///
/// | | [`Value`](crate::Value) | `BorrowedValue<'a>` |
/// |---|---|---|
/// | Lifetime | `'static` (owns all data) | borrows from input |
/// | Plain scalars | heap-allocated copy | zero-copy borrow |
/// | Use case | long-lived, send across threads | short-lived parsing |
#[derive(Debug, Clone, PartialEq)]
pub struct BorrowedValue<'a>(Node<'a>);

impl<'a> BorrowedValue<'a> {
    /// Parses the first document of `input`, borrowing scalars from it.
    ///
    /// Returns [`ErrorKind::UnexpectedEof`] when `input` contains no document.
    ///
    /// # Errors
    ///
    /// Returns an error if the input is not valid YAML or is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use skald_serde::BorrowedValue;
    ///
    /// let input = String::from("hello");
    /// let bv = BorrowedValue::parse(&input).unwrap();
    /// assert_eq!(bv.as_str(), Some("hello"));
    /// ```
    pub fn parse(input: &'a str) -> Result<Self> {
        match skald_ast::composer::Composer::new(input).next() {
            Some(node) => Ok(BorrowedValue(node?)),
            None => Err(Error::spanless(ErrorKind::UnexpectedEof)),
        }
    }

    /// Wraps an existing borrowed node.
    #[must_use]
    pub fn new(node: Node<'a>) -> Self {
        BorrowedValue(node)
    }

    /// Returns a reference to the inner node.
    #[must_use]
    pub fn as_node(&self) -> &Node<'a> {
        &self.0
    }

    /// Consumes self, returning the inner node.
    #[must_use]
    pub fn into_node(self) -> Node<'a> {
        self.0
    }
}

impl<'a> Deref for BorrowedValue<'a> {
    type Target = Node<'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> From<Node<'a>> for BorrowedValue<'a> {
    fn from(n: Node<'a>) -> Self {
        BorrowedValue(n)
    }
}

impl<'a> From<BorrowedValue<'a>> for Node<'a> {
    fn from(bv: BorrowedValue<'a>) -> Self {
        bv.0
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn borrowed_value_is_zero_copy_for_plain_scalars() {
        let input = String::from("hello world");
        let bv = BorrowedValue::parse(&input).unwrap();
        assert_eq!(bv.as_str(), Some("hello world"));
        if let skald_ast::node::Node::Scalar(s) = bv.as_node() {
            assert!(
                matches!(s.value, Cow::Borrowed(_)),
                "plain scalar must borrow, not own"
            );
        } else {
            panic!("expected scalar");
        }
    }

    #[test]
    fn borrowed_value_traverses_mapping() {
        let input = String::from("a: 1\nb: two\n");
        let bv = BorrowedValue::parse(&input).unwrap();
        let m = bv.as_node().as_mapping().unwrap();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn borrowed_value_empty_input_returns_unexpected_eof() {
        let result = BorrowedValue::parse("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, ErrorKind::UnexpectedEof),
            "expected UnexpectedEof, got {:?}",
            err.kind
        );
    }

    #[test]
    fn borrowed_value_new_wraps_node() {
        let input = String::from("42");
        let node = skald_ast::composer::Composer::new(&input)
            .next()
            .unwrap()
            .unwrap();
        let bv = BorrowedValue::new(node);
        assert_eq!(bv.as_str(), Some("42"));
    }

    #[test]
    fn borrowed_value_into_node_round_trips() {
        let input = String::from("round: trip");
        let bv = BorrowedValue::parse(&input).unwrap();
        let node = bv.into_node();
        assert!(node.as_mapping().is_some());
    }

    #[test]
    fn borrowed_value_from_node_conversion() {
        let input = String::from("from: conversion");
        let node: Node<'_> = skald_ast::composer::Composer::new(&input)
            .next()
            .unwrap()
            .unwrap();
        let bv: BorrowedValue<'_> = BorrowedValue::from(node);
        assert!(bv.is_mapping());
    }

    #[test]
    fn borrowed_value_into_node_via_from_impl() {
        let input = String::from("key: val");
        let bv = BorrowedValue::parse(&input).unwrap();
        let node: Node<'_> = Node::from(bv);
        assert!(node.as_mapping().is_some());
    }

    #[test]
    fn borrowed_value_deref_exposes_node_methods() {
        let input = String::from("- a\n- b\n");
        let bv = BorrowedValue::parse(&input).unwrap();
        // Deref lets us call Node methods directly.
        assert!(bv.is_sequence());
        assert_eq!(bv.as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn borrowed_value_sequence_items_borrow_via_flow_sequence() {
        // Block-sequence items (`- val\n`) are Cow::Owned because the scanner
        // must look past the trailing newline to determine scalar boundaries,
        // setting all_borrowed = false.  Flow sequences on a single line have
        // no cross-line lookahead, so items remain zero-copy.
        let input = String::from("[alpha, beta]");
        let bv = BorrowedValue::parse(&input).unwrap();
        let items = bv.as_sequence().unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            if let skald_ast::node::Node::Scalar(s) = item {
                assert!(
                    matches!(s.value, Cow::Borrowed(_)),
                    "flow-sequence scalar items must borrow: got {:?}",
                    s.value
                );
            } else {
                panic!("expected scalar item");
            }
        }
    }
}
