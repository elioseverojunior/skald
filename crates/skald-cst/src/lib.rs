// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Skald lossless concrete syntax tree (CST).
//!
//! Placeholder crate scaffolded in Phase 1.0. The red/green tree, the
//! trivia-preserving builder, and the `Document` editing API land in Phase 1.

pub mod builder;
pub mod document;
pub mod green;
pub mod kind;
pub mod red;

pub use builder::GreenNodeBuilder;
pub use document::{Document, SetError};
pub use kind::SyntaxKind;
pub use red::{SyntaxElement, SyntaxNode};
