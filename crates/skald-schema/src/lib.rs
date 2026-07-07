// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Span-anchored JSON-Schema validation over the Skald `Node` tree (subset of draft 2020-12).

/// Schema-driven coercion of defaults and scalar types over a CST `Document`.
pub mod coerce;
/// The `Schema` model and the supported JSON-Schema draft 2020-12 subset.
pub mod schema;
/// Span-anchored validation of a `Node` tree against a `Schema`.
pub mod validate;

pub use coerce::{Coercion, Insertion, apply_defaults, coerce_to_schema};
pub use schema::{JsonType, Schema};
pub use validate::{SchemaError, validate};
