// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Skald YAML AST — the `Node` representation tree, composer, and emitter.
//!
//! Built on the `skald-core` front-end (scanner → parser → events).

pub mod composer;
pub mod emitter;
pub mod node;

pub use composer::{Composer, compose_all};
pub use emitter::{EmitterConfig, emit, emit_to_string};
pub use node::{Mapping, Node, Scalar, Sequence};
