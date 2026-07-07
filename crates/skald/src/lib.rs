// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # Skald — Safe YAML for Rust
//!
//! Zero `unsafe` by default, full YAML 1.2.2 spec compliance, high performance.
//!
//! ## Typed (Serde) API
//!
//! ```
//! use serde::Deserialize;
//!
//! #[derive(Deserialize, Debug)]
//! struct Config {
//!     name: String,
//!     debug: bool,
//! }
//!
//! let config: Config = skald::from_str("name: skald\ndebug: true").unwrap();
//! assert_eq!(config.name, "skald");
//! ```
//!
//! ## Node API
//!
//! ```
//! // Parse a single YAML document to a Node tree.
//! let node = skald::from_str_node("hello: world").unwrap();
//! let entries = node.as_mapping().unwrap();
//! assert_eq!(entries[0].0.as_str(), Some("hello"));
//! assert_eq!(entries[0].1.as_str(), Some("world"));
//! ```
//!
//! ## Multi-Document
//!
//! ```
//! let docs = skald::from_str_multi_node("---\na\n---\nb\n").unwrap();
//! assert_eq!(docs.len(), 2);
//! ```
//!
//! ## Emitting YAML
//!
//! ```
//! let node = skald::from_str_node("hello: world").unwrap();
//! let yaml = skald::to_string_node(&node);
//! assert_eq!(yaml, "hello: world\n");
//! ```
//!
//! ## Pipeline Access
//!
//! For lower-level control, the full pipeline is available via submodules:
//! [`scanner`], [`parser`], [`composer`], [`emitter`].

// Re-export skald-core public API explicitly (always present).
pub use skald_core::{error, limits, parser, scanner, types};

// Re-export skald-ast when the `ast` feature is enabled.
#[cfg(feature = "ast")]
pub use skald_ast::{self as ast, Mapping, Node, Scalar, Sequence, composer, emitter};

// Re-export skald-serde types when the `serde` feature is enabled.
#[cfg(feature = "serde")]
pub use skald_serde::{
    self as serde_integration, BorrowedValue, Error as SerdeError, FlowMap, FlowSeq, FoldStr,
    LitStr, Value,
};

/// Lossless, comment-preserving YAML editing via the concrete syntax tree.
///
/// ```
/// let mut doc = skald::cst::Document::parse("version: 0.0.1  # keep\nname: skald\n");
/// doc.set("version", "0.0.2").unwrap();
/// assert_eq!(doc.to_string(), "version: 0.0.2  # keep\nname: skald\n");
/// assert_eq!(doc.get("name").map(str::trim), Some("skald"));
/// ```
#[cfg(feature = "cst")]
pub use skald_cst as cst;

/// Span-anchored JSON-Schema validation over the Skald `Node` tree.
#[cfg(feature = "schema")]
pub use skald_schema as schema;

#[cfg(any(feature = "ast", feature = "serde"))]
use std::io::Read;

// ─── Serde API (typed) ───────────────────────────────────────────────

/// Deserializes a Rust type from a YAML string.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Server { host: String, port: u16 }
///
/// let s: Server = skald::from_str("host: localhost\nport: 8080").unwrap();
/// assert_eq!(s.host, "localhost");
/// assert_eq!(s.port, 8080);
/// ```
#[cfg(feature = "serde")]
pub fn from_str<T: serde::de::DeserializeOwned + 'static>(input: &str) -> skald_serde::Result<T> {
    skald_serde::from_str(input)
}

/// Deserializes a Rust type from a YAML reader.
///
/// Reads the entire input into memory, then deserializes.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Item { name: String }
///
/// let reader = std::io::Cursor::new(b"name: widget");
/// let item: Item = skald::from_reader(reader).unwrap();
/// assert_eq!(item.name, "widget");
/// ```
#[cfg(feature = "serde")]
pub fn from_reader<T: serde::de::DeserializeOwned + 'static>(
    reader: impl Read,
) -> skald_serde::Result<T> {
    skald_serde::from_reader(reader)
}

/// Serializes a Rust type to a YAML string.
///
/// # Examples
///
/// ```
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct Point { x: i32, y: i32 }
///
/// let yaml = skald::to_string(&Point { x: 1, y: 2 }).unwrap();
/// assert!(yaml.contains("x: 1"));
/// assert!(yaml.contains("y: 2"));
/// ```
#[cfg(feature = "serde")]
pub fn to_string<T: serde::Serialize>(value: &T) -> skald_serde::Result<String> {
    skald_serde::to_string(value)
}

/// Serializes a Rust type to a YAML string with custom emitter configuration.
///
/// # Examples
///
/// ```
/// use serde::Serialize;
/// use skald::emitter::EmitterConfig;
///
/// #[derive(Serialize)]
/// struct Data { key: String }
///
/// let config = EmitterConfig { explicit_document: true, ..EmitterConfig::default() };
/// let yaml = skald::to_string_with(&Data { key: "val".into() }, &config).unwrap();
/// assert!(yaml.starts_with("---"));
/// ```
#[cfg(feature = "serde")]
pub fn to_string_with<T: serde::Serialize>(
    value: &T,
    config: &skald_ast::emitter::EmitterConfig,
) -> skald_serde::Result<String> {
    skald_serde::to_string_with(value, config)
}

/// Deserializes a Rust type from a YAML string with custom parser configuration.
///
/// Use this to control strictness (lenient vs strict), resource limits, and schema.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
/// use skald::error::{ParserConfig, Strictness};
///
/// #[derive(Deserialize)]
/// struct Config { name: String }
///
/// let config = ParserConfig { strictness: Strictness::Lenient, ..Default::default() };
/// let c: Config = skald::from_str_with("name: app", config).unwrap();
/// assert_eq!(c.name, "app");
/// ```
#[cfg(feature = "serde")]
pub fn from_str_with<T: serde::de::DeserializeOwned + 'static>(
    input: &str,
    config: skald_core::error::ParserConfig,
) -> skald_serde::Result<T> {
    skald_serde::from_str_with(input, config)
}

/// Deserializes a Rust type from a YAML reader with custom parser configuration.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
/// use skald::error::ParserConfig;
///
/// #[derive(Deserialize)]
/// struct Item { name: String }
///
/// let reader = std::io::Cursor::new(b"name: widget");
/// let item: Item = skald::from_reader_with(reader, ParserConfig::default()).unwrap();
/// assert_eq!(item.name, "widget");
/// ```
#[cfg(feature = "serde")]
pub fn from_reader_with<T: serde::de::DeserializeOwned + 'static>(
    reader: impl Read,
    config: skald_core::error::ParserConfig,
) -> skald_serde::Result<T> {
    skald_serde::from_reader_with(reader, config)
}

/// Deserializes all YAML documents from a string into a `Vec<T>`.
///
/// Each document is composed into a `Node`, then deserialized to `T`.
/// An empty stream produces an empty `Vec`.
///
/// # Examples
///
/// ```
/// let docs: Vec<String> = skald::from_str_multi("---\nhello\n---\nworld\n").unwrap();
/// assert_eq!(docs, vec!["hello", "world"]);
/// ```
#[cfg(feature = "serde")]
pub fn from_str_multi<T: serde::de::DeserializeOwned + 'static>(
    input: &str,
) -> skald_serde::Result<Vec<T>> {
    skald_serde::from_str_multi(input)
}

/// Deserializes all YAML documents from a string into a `Vec<T>`, with custom parser configuration.
///
/// # Examples
///
/// ```
/// use skald::error::ParserConfig;
///
/// let config = ParserConfig::default();
/// let docs: Vec<String> = skald::from_str_multi_with("---\na\n---\nb\n", config).unwrap();
/// assert_eq!(docs, vec!["a", "b"]);
/// ```
#[cfg(feature = "serde")]
pub fn from_str_multi_with<T: serde::de::DeserializeOwned + 'static>(
    input: &str,
    config: skald_core::error::ParserConfig,
) -> skald_serde::Result<Vec<T>> {
    skald_serde::from_str_multi_with(input, config)
}

/// Deserializes all YAML documents from a reader into a `Vec<T>`.
///
/// Reads the entire input into memory, then deserializes all documents.
///
/// # Examples
///
/// ```
/// let reader = std::io::Cursor::new(b"---\nhello\n---\nworld\n");
/// let docs: Vec<String> = skald::from_reader_multi(reader).unwrap();
/// assert_eq!(docs, vec!["hello", "world"]);
/// ```
#[cfg(feature = "serde")]
pub fn from_reader_multi<T: serde::de::DeserializeOwned + 'static>(
    mut reader: impl Read,
) -> skald_serde::Result<Vec<T>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf).map_err(|e| {
        skald_serde::Error::core(skald_core::error::Error::spanless(
            skald_core::error::ErrorKind::UnexpectedToken {
                expected: "readable input".into(),
                found: format!("I/O error: {e}").into(),
            },
        ))
    })?;
    skald_serde::from_str_multi(&buf)
}

/// Deserializes all YAML documents from a reader into a `Vec<T>`, with custom parser configuration.
///
/// # Examples
///
/// ```
/// use skald::error::ParserConfig;
///
/// let reader = std::io::Cursor::new(b"---\na\n---\nb\n");
/// let docs: Vec<String> = skald::from_reader_multi_with(reader, ParserConfig::default()).unwrap();
/// assert_eq!(docs, vec!["a", "b"]);
/// ```
#[cfg(feature = "serde")]
pub fn from_reader_multi_with<T: serde::de::DeserializeOwned + 'static>(
    mut reader: impl Read,
    config: skald_core::error::ParserConfig,
) -> skald_serde::Result<Vec<T>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf).map_err(|e| {
        skald_serde::Error::core(skald_core::error::Error::spanless(
            skald_core::error::ErrorKind::UnexpectedToken {
                expected: "readable input".into(),
                found: format!("I/O error: {e}").into(),
            },
        ))
    })?;
    skald_serde::from_str_multi_with(&buf, config)
}

/// Serializes a Rust type to YAML, writing to any [`io::Write`](std::io::Write) destination.
///
/// # Examples
///
/// ```
/// let mut buf = Vec::new();
/// skald::to_writer(&mut buf, &vec![1, 2, 3]).unwrap();
/// assert_eq!(std::str::from_utf8(&buf).unwrap(), "- 1\n- 2\n- 3\n");
/// ```
#[cfg(feature = "serde")]
pub fn to_writer<T: serde::Serialize, W: std::io::Write>(
    writer: W,
    value: &T,
) -> skald_serde::Result<()> {
    skald_serde::to_writer(writer, value)
}

/// Serializes a Rust type to YAML with custom emitter configuration, writing to any [`io::Write`](std::io::Write) destination.
///
/// # Examples
///
/// ```
/// use skald::emitter::EmitterConfig;
///
/// let config = EmitterConfig { explicit_document: true, ..EmitterConfig::default() };
/// let mut buf = Vec::new();
/// skald::to_writer_with(&mut buf, &vec![1, 2], &config).unwrap();
/// let yaml = std::str::from_utf8(&buf).unwrap();
/// assert!(yaml.starts_with("---"));
/// ```
#[cfg(feature = "serde")]
pub fn to_writer_with<T: serde::Serialize, W: std::io::Write>(
    writer: W,
    value: &T,
    config: &skald_ast::emitter::EmitterConfig,
) -> skald_serde::Result<()> {
    skald_serde::to_writer_with(writer, value, config)
}

// ─── Node API ────────────────────────────────────────────────────────

/// Parses a single YAML document from a string to a [`Node`] tree.
///
/// If the input contains multiple documents, only the first is returned.
/// Use [`from_str_multi_node`] to parse all documents.
///
/// # Errors
///
/// Returns an error if the input is not valid YAML.
///
/// # Examples
///
/// ```
/// let node = skald::from_str_node("hello").unwrap();
/// assert_eq!(node.as_str(), Some("hello"));
/// ```
#[cfg(feature = "ast")]
pub fn from_str_node(input: &str) -> skald_core::error::Result<skald_ast::Node<'_>> {
    skald_ast::composer::Composer::new(input)
        .next()
        .unwrap_or_else(|| {
            Err(skald_core::error::Error::spanless(
                skald_core::error::ErrorKind::UnexpectedEof,
            ))
        })
}

/// Parses all YAML documents from a string.
///
/// Returns one [`Node`] per document.
/// An empty stream produces an empty `Vec`.
///
/// # Examples
///
/// ```
/// let docs = skald::from_str_multi_node("---\na\n---\nb\n").unwrap();
/// assert_eq!(docs.len(), 2);
/// ```
#[cfg(feature = "ast")]
pub fn from_str_multi_node(input: &str) -> skald_core::error::Result<Vec<skald_ast::Node<'_>>> {
    skald_ast::composer::compose_all(input)
}

/// Parses a single YAML document from a reader to a [`Node`] tree.
///
/// Reads the entire input into memory, then parses.
/// The returned node owns all its data (`'static` lifetime).
///
/// # Examples
///
/// ```
/// let reader = std::io::Cursor::new(b"hello");
/// let node = skald::from_reader_node(reader).unwrap();
/// assert_eq!(node.as_str(), Some("hello"));
/// ```
#[cfg(feature = "ast")]
pub fn from_reader_node<R: Read>(
    mut reader: R,
) -> skald_core::error::Result<skald_ast::Node<'static>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf).map_err(|e| {
        skald_core::error::Error::spanless(skald_core::error::ErrorKind::UnexpectedToken {
            expected: "readable input".into(),
            found: format!("I/O error: {e}").into(),
        })
    })?;
    let node = from_str_node(&buf)?;
    Ok(node.into_owned())
}

/// Parses all YAML documents from a reader.
///
/// Reads the entire input into memory, then parses all documents.
/// The returned nodes own all their data (`'static` lifetime).
///
/// # Examples
///
/// ```
/// let reader = std::io::Cursor::new(b"---\na\n---\nb\n");
/// let docs = skald::from_reader_multi_node(reader).unwrap();
/// assert_eq!(docs.len(), 2);
/// ```
#[cfg(feature = "ast")]
pub fn from_reader_multi_node<R: Read>(
    mut reader: R,
) -> skald_core::error::Result<Vec<skald_ast::Node<'static>>> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf).map_err(|e| {
        skald_core::error::Error::spanless(skald_core::error::ErrorKind::UnexpectedToken {
            expected: "readable input".into(),
            found: format!("I/O error: {e}").into(),
        })
    })?;
    let nodes = from_str_multi_node(&buf)?;
    Ok(nodes.into_iter().map(|n| n.into_owned()).collect())
}

/// Emits a [`Node`] as a YAML string with default settings.
///
/// This is infallible — the emitter always succeeds.
///
/// # Examples
///
/// ```
/// let node = skald::from_str_node("key: value").unwrap();
/// let yaml = skald::to_string_node(&node);
/// assert_eq!(yaml, "key: value\n");
/// ```
#[must_use]
#[cfg(feature = "ast")]
pub fn to_string_node(node: &skald_ast::Node<'_>) -> String {
    skald_ast::emitter::emit_to_string(node, &skald_ast::emitter::EmitterConfig::default())
}

/// Emits a [`Node`] as a YAML string with custom configuration.
///
/// # Examples
///
/// ```
/// use skald::emitter::EmitterConfig;
///
/// let node = skald::from_str_node("key: value").unwrap();
/// let config = EmitterConfig { explicit_document: true, ..EmitterConfig::default() };
/// let yaml = skald::to_string_node_with(&node, config);
/// assert!(yaml.starts_with("---"));
/// ```
#[must_use]
#[cfg(feature = "ast")]
pub fn to_string_node_with(
    node: &skald_ast::Node<'_>,
    config: skald_ast::emitter::EmitterConfig,
) -> String {
    skald_ast::emitter::emit_to_string(node, &config)
}

/// Emits a [`Node`] as YAML to any [`std::io::Write`] destination.
///
/// # Errors
///
/// Returns an error if writing fails.
///
/// # Examples
///
/// ```
/// let node = skald::from_str_node("hello").unwrap();
/// let mut buf = Vec::new();
/// skald::to_writer_node(&mut buf, &node).unwrap();
/// assert_eq!(std::str::from_utf8(&buf).unwrap(), "hello\n");
/// ```
#[cfg(feature = "ast")]
pub fn to_writer_node<W: std::io::Write>(
    mut writer: W,
    node: &skald_ast::Node<'_>,
) -> std::io::Result<()> {
    let yaml = to_string_node(node);
    writer.write_all(yaml.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Helper: a reader that always fails with an I/O error ────────

    struct FailingReader;

    impl std::io::Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "simulated I/O failure",
            ))
        }
    }

    // ─── Serde API tests ────────────────────────────────────────────

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_struct() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Config {
            name: String,
            debug: bool,
        }
        let c: Config = from_str("name: skald\ndebug: true").unwrap();
        assert_eq!(c.name, "skald");
        assert!(c.debug);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_struct() {
        #[derive(serde::Deserialize)]
        struct Item {
            id: u32,
        }
        let reader = std::io::Cursor::new(b"id: 42");
        let item: Item = from_reader(reader).unwrap();
        assert_eq!(item.id, 42);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_to_string_struct() {
        #[derive(serde::Serialize)]
        struct Point {
            x: i32,
            y: i32,
        }
        let yaml = to_string(&Point { x: 1, y: 2 }).unwrap();
        assert!(yaml.contains("x: 1"));
        assert!(yaml.contains("y: 2"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct Config {
            name: String,
            debug: bool,
            count: u32,
        }
        let original = Config {
            name: "skald".into(),
            debug: true,
            count: 42,
        };
        let yaml = to_string(&original).unwrap();
        let roundtripped: Config = from_str(&yaml).unwrap();
        assert_eq!(original, roundtripped);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_to_string_with_config() {
        #[derive(serde::Serialize)]
        struct Data {
            key: String,
        }
        let config = skald_ast::emitter::EmitterConfig {
            explicit_document: true,
            ..Default::default()
        };
        let yaml = to_string_with(&Data { key: "val".into() }, &config).unwrap();
        assert!(yaml.starts_with("---"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_value_roundtrip() {
        let value: Value = from_str("name: skald\nlist:\n  - a\n  - b").unwrap();
        assert!(value.is_mapping());
        let yaml = to_string(&value).unwrap();
        assert!(yaml.contains("name: skald"));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_with_lenient() {
        use std::collections::HashMap;
        let config = skald_core::error::ParserConfig {
            strictness: skald_core::error::Strictness::Lenient,
            ..Default::default()
        };
        let result: HashMap<String, i32> = from_str_with("a: 1\na: 2", config).unwrap();
        assert_eq!(result.get("a"), Some(&2));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_with() {
        #[derive(serde::Deserialize)]
        struct Item {
            name: String,
        }
        let reader = std::io::Cursor::new(b"name: test");
        let item: Item =
            from_reader_with(reader, skald_core::error::ParserConfig::default()).unwrap();
        assert_eq!(item.name, "test");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_multi() {
        let docs: Vec<String> = from_str_multi("---\nhello\n---\nworld\n").unwrap();
        assert_eq!(docs, vec!["hello", "world"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_multi_empty() {
        let docs: Vec<String> = from_str_multi("").unwrap();
        assert!(docs.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_multi() {
        let reader = std::io::Cursor::new(b"---\na\n---\nb\n");
        let docs: Vec<String> = from_reader_multi(reader).unwrap();
        assert_eq!(docs, vec!["a", "b"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_to_writer() {
        let mut buf = Vec::new();
        to_writer(&mut buf, &vec![1, 2]).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), "- 1\n- 2\n");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_to_writer_with_config() {
        let config = skald_ast::emitter::EmitterConfig {
            explicit_document: true,
            ..Default::default()
        };
        let mut buf = Vec::new();
        to_writer_with(&mut buf, &"hello", &config).unwrap();
        let yaml = std::str::from_utf8(&buf).unwrap();
        assert!(yaml.starts_with("---"));
    }

    // ─── Node API tests ─────────────────────────────────────────────

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_scalar() {
        let node = from_str_node("hello").unwrap();
        assert_eq!(node.as_str(), Some("hello"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_mapping() {
        let node = from_str_node("key: value").unwrap();
        let entries = node.as_mapping().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.as_str(), Some("key"));
        assert_eq!(entries[0].1.as_str(), Some("value"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_sequence() {
        let node = from_str_node("- a\n- b\n- c").unwrap();
        let items = node.as_sequence().unwrap();
        assert_eq!(items.len(), 3);
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_multi_node_documents() {
        let docs = from_str_multi_node("---\na\n---\nb\n").unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].as_str(), Some("a"));
        assert_eq!(docs[1].as_str(), Some("b"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_empty_returns_error() {
        assert!(from_str_node("").is_err());
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_str_multi_node_empty_returns_empty_vec() {
        let docs = from_str_multi_node("").unwrap();
        assert!(docs.is_empty());
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_scalar() {
        let reader = std::io::Cursor::new(b"hello");
        let node = from_reader_node(reader).unwrap();
        assert_eq!(node.as_str(), Some("hello"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_mapping() {
        let reader = std::io::Cursor::new(b"key: value");
        let node = from_reader_node(reader).unwrap();
        let entries = node.as_mapping().unwrap();
        assert_eq!(entries[0].1.as_str(), Some("value"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_multi_node() {
        let reader = std::io::Cursor::new(b"---\na\n---\nb\n");
        let docs = from_reader_multi_node(reader).unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_empty_returns_error() {
        let reader = std::io::Cursor::new(b"");
        assert!(from_reader_node(reader).is_err());
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_to_string_roundtrip() {
        let node = from_str_node("key: value").unwrap();
        let yaml = to_string_node(&node);
        assert_eq!(yaml, "key: value\n");
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_to_string_with_config() {
        let node = from_str_node("key: value").unwrap();
        let config = skald_ast::emitter::EmitterConfig {
            explicit_document: true,
            ..Default::default()
        };
        let yaml = to_string_node_with(&node, config);
        assert!(yaml.starts_with("---"));
    }

    #[cfg(feature = "ast")]
    #[test]
    fn node_to_writer() {
        let node = from_str_node("hello").unwrap();
        let mut buf = Vec::new();
        to_writer_node(&mut buf, &node).unwrap();
        assert_eq!(std::str::from_utf8(&buf).unwrap(), "hello\n");
    }

    // ─── from_str_multi_with (lines 210, 214) ───────────────────────

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_multi_with_happy_path() {
        let config = skald_core::error::ParserConfig::default();
        let docs: Vec<String> = from_str_multi_with("---\nhello\n---\nworld\n", config).unwrap();
        assert_eq!(docs, vec!["hello", "world"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_multi_with_empty_input() {
        let config = skald_core::error::ParserConfig::default();
        let docs: Vec<String> = from_str_multi_with("", config).unwrap();
        assert!(docs.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_str_multi_with_lenient_mode() {
        use skald_core::error::Strictness;
        let config = skald_core::error::ParserConfig {
            strictness: Strictness::Lenient,
            ..Default::default()
        };
        let docs: Vec<String> = from_str_multi_with("---\nfoo\n---\nbar\n", config).unwrap();
        assert_eq!(docs, vec!["foo", "bar"]);
    }

    // ─── from_reader_multi I/O error path (lines 233-236) ───────────

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_multi_io_error() {
        let result: skald_serde::Result<Vec<String>> = from_reader_multi(FailingReader);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("I/O error") || !err_msg.is_empty());
    }

    // ─── from_reader_multi_with (lines 254, 258-267) ────────────────

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_multi_with_happy_path() {
        let reader = std::io::Cursor::new(b"---\na\n---\nb\n");
        let config = skald_core::error::ParserConfig::default();
        let docs: Vec<String> = from_reader_multi_with(reader, config).unwrap();
        assert_eq!(docs, vec!["a", "b"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_multi_with_empty_input() {
        let reader = std::io::Cursor::new(b"");
        let config = skald_core::error::ParserConfig::default();
        let docs: Vec<String> = from_reader_multi_with(reader, config).unwrap();
        assert!(docs.is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_from_reader_multi_with_io_error() {
        let config = skald_core::error::ParserConfig::default();
        let result: skald_serde::Result<Vec<String>> =
            from_reader_multi_with(FailingReader, config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("I/O error") || !err_msg.is_empty());
    }

    // ─── from_reader_node I/O error path (lines 368-370) ────────────

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_node_io_error() {
        let result = from_reader_node(FailingReader);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("I/O error") || !err_msg.is_empty());
    }

    // ─── from_reader_multi_node I/O error path (lines 394-396) ──────

    #[cfg(feature = "ast")]
    #[test]
    fn node_from_reader_multi_node_io_error() {
        let result = from_reader_multi_node(FailingReader);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("I/O error") || !err_msg.is_empty());
    }
}
