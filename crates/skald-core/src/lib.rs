// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![doc = "Core YAML 1.2.2 scanner and parser front-end."]
#![doc = ""]
#![doc = "This crate provides the foundational YAML processing pipeline:"]
#![doc = "Scanner (bytes → tokens) → Parser (tokens → events)."]
#![doc = ""]
#![doc = "All types in this crate are built with `#![forbid(unsafe_code)]`."]

pub mod error;
pub mod limits;
pub mod parser;
pub mod scanner;
pub mod types;
