// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error types for YAML processing.
//!
//! Provides unified error handling across all pipeline stages (scanner, parser, composer)
//! with source span tracking and contextual error frames.
//!
//! # Error Display
//!
//! Errors include source location and contextual information for high-quality diagnostics:
//!
//! ```text
//! error: duplicate key in mapping
//!   --> 14:3
//!    |
//!    = while parsing mapping at 12:1
//!    = note: key "port" first defined at 10:3
//! ```

use crate::limits::ResourceLimits;
use crate::types::Span;
use std::borrow::Cow;
use std::fmt;

/// A YAML processing error with source location and context.
#[derive(Debug, Clone)]
pub struct Error {
    /// The kind of error.
    pub kind: ErrorKind,
    /// The source span where the error occurred, if available.
    pub span: Option<Span>,
    /// Contextual frames describing what the parser was doing when the error occurred.
    pub context: Vec<ContextFrame>,
}

/// A frame of context describing what the parser was doing.
///
/// For example: "while parsing a block mapping" at line 5, column 1.
#[derive(Debug, Clone)]
pub struct ContextFrame {
    /// Human-readable description of what was being parsed.
    pub description: Cow<'static, str>,
    /// Source span of the context.
    pub span: Span,
}

/// The specific kind of error that occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    // ─── Scanner errors ─────────────────────────────────────────────
    /// Input is not valid UTF-8.
    InvalidUtf8,
    /// Encountered an unexpected byte.
    UnexpectedByte(u8),
    /// Invalid escape sequence in a double-quoted scalar.
    InvalidEscape(char),
    /// Unexpected end of input.
    UnexpectedEof,
    /// Invalid character in the current context.
    InvalidCharacter(char),
    /// Tab character used for indentation (not allowed in YAML).
    TabInIndentation,

    // ─── Parser errors ──────────────────────────────────────────────
    /// Expected a different token.
    UnexpectedToken {
        /// What was expected.
        expected: Cow<'static, str>,
        /// What was found.
        found: Cow<'static, str>,
    },
    /// Indentation is invalid in the current context.
    InvalidIndentation {
        /// The expected indentation level.
        expected: u32,
        /// The actual indentation level found.
        found: u32,
    },
    /// Invalid tag directive or tag handle.
    InvalidTag(String),
    /// Invalid anchor name.
    InvalidAnchor(String),

    // ─── Composer errors ────────────────────────────────────────────
    /// Reference to an undefined alias.
    UndefinedAlias(String),
    /// Duplicate anchor name in the same document.
    DuplicateAnchor(String),
    /// Duplicate key in a mapping.
    DuplicateKey {
        /// The duplicate key as a string.
        key: String,
        /// Span of the first occurrence.
        first: Span,
    },

    // ─── Resource limit errors ──────────────────────────────────────
    /// Maximum nesting depth exceeded.
    DepthLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Maximum alias expansion count exceeded.
    AliasExpansionLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Maximum document size exceeded.
    DocumentSizeLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Maximum key length exceeded.
    KeyLengthLimitExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// Maximum node count exceeded.
    NodeCountLimitExceeded {
        /// The configured limit.
        limit: usize,
    },

    // ─── Policy errors ──────────────────────────────────────────────
    /// An anchor or alias was present but the `deny_anchors` policy is active.
    AnchorsDenied,
    /// An explicit tag was present but the `deny_tags` policy is active.
    TagsDenied,
    /// A scalar exceeded the configured `max_scalar_length`.
    ScalarLengthLimitExceeded {
        /// The configured limit, in bytes.
        limit: usize,
        /// The offending scalar's actual length, in bytes.
        actual: usize,
    },
}

impl Error {
    /// Creates a new error with the given kind and span.
    #[must_use]
    pub fn new(kind: ErrorKind, span: Span) -> Self {
        Self {
            kind,
            span: Some(span),
            context: Vec::new(),
        }
    }

    /// Creates a new error without a span.
    #[must_use]
    pub fn spanless(kind: ErrorKind) -> Self {
        Self {
            kind,
            span: None,
            context: Vec::new(),
        }
    }

    /// Adds a context frame to this error.
    #[must_use]
    pub fn with_context(mut self, description: impl Into<Cow<'static, str>>, span: Span) -> Self {
        self.context.push(ContextFrame {
            description: description.into(),
            span,
        });
        self
    }

    /// Returns `true` if this error is a resource limit violation.
    #[must_use]
    pub fn is_limit_error(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::DepthLimitExceeded { .. }
                | ErrorKind::AliasExpansionLimitExceeded { .. }
                | ErrorKind::DocumentSizeLimitExceeded { .. }
                | ErrorKind::KeyLengthLimitExceeded { .. }
                | ErrorKind::NodeCountLimitExceeded { .. }
        )
    }

    /// Creates a depth limit exceeded error.
    #[must_use]
    pub fn depth_exceeded(limits: &ResourceLimits, span: Span) -> Self {
        Self::new(
            ErrorKind::DepthLimitExceeded {
                limit: limits.max_depth,
            },
            span,
        )
    }

    /// Creates an alias expansion limit exceeded error.
    #[must_use]
    pub fn alias_expansion_exceeded(limits: &ResourceLimits, span: Span) -> Self {
        Self::new(
            ErrorKind::AliasExpansionLimitExceeded {
                limit: limits.max_alias_expansions,
            },
            span,
        )
    }

    /// Creates a document size limit exceeded error.
    #[must_use]
    pub fn document_size_exceeded(limits: &ResourceLimits, span: Span) -> Self {
        Self::new(
            ErrorKind::DocumentSizeLimitExceeded {
                limit: limits.max_document_size,
            },
            span,
        )
    }

    /// Creates a key length limit exceeded error.
    #[must_use]
    pub fn key_length_exceeded(limits: &ResourceLimits, span: Span) -> Self {
        Self::new(
            ErrorKind::KeyLengthLimitExceeded {
                limit: limits.max_key_length,
            },
            span,
        )
    }

    /// Creates a node count limit exceeded error.
    #[must_use]
    pub fn node_count_exceeded(limits: &ResourceLimits, span: Span) -> Self {
        Self::new(
            ErrorKind::NodeCountLimitExceeded {
                limit: limits.max_node_count,
            },
            span,
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(span) = &self.span {
            write!(f, " at {}", span.start)?;
        }
        for ctx in &self.context {
            write!(f, "\n  = {} at {}", ctx.description, ctx.span.start)?;
        }
        Ok(())
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Scanner
            ErrorKind::InvalidUtf8 => write!(f, "invalid UTF-8"),
            ErrorKind::UnexpectedByte(b) => write!(f, "unexpected byte: 0x{b:02X}"),
            ErrorKind::InvalidEscape(c) => write!(f, "invalid escape sequence: \\{c}"),
            ErrorKind::UnexpectedEof => write!(f, "unexpected end of input"),
            ErrorKind::InvalidCharacter(c) => write!(f, "invalid character: {c:?}"),
            ErrorKind::TabInIndentation => {
                write!(f, "tab character used for indentation")
            }

            // Parser
            ErrorKind::UnexpectedToken { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            ErrorKind::InvalidIndentation { expected, found } => {
                write!(f, "invalid indentation: expected {expected}, found {found}")
            }
            ErrorKind::InvalidTag(tag) => write!(f, "invalid tag: {tag}"),
            ErrorKind::InvalidAnchor(anchor) => write!(f, "invalid anchor: {anchor}"),

            // Composer
            ErrorKind::UndefinedAlias(name) => write!(f, "undefined alias: *{name}"),
            ErrorKind::DuplicateAnchor(name) => write!(f, "duplicate anchor: &{name}"),
            ErrorKind::DuplicateKey { key, first } => {
                write!(f, "duplicate key: {key:?} (first defined at {first})")
            }

            // Limits
            ErrorKind::DepthLimitExceeded { limit } => {
                write!(f, "maximum nesting depth exceeded (limit: {limit})")
            }
            ErrorKind::AliasExpansionLimitExceeded { limit } => {
                write!(f, "maximum alias expansions exceeded (limit: {limit})")
            }
            ErrorKind::DocumentSizeLimitExceeded { limit } => {
                write!(f, "maximum document size exceeded (limit: {limit} bytes)")
            }
            ErrorKind::KeyLengthLimitExceeded { limit } => {
                write!(f, "maximum key length exceeded (limit: {limit} bytes)")
            }
            ErrorKind::NodeCountLimitExceeded { limit } => {
                write!(f, "maximum node count exceeded (limit: {limit})")
            }

            // Policy
            ErrorKind::AnchorsDenied => {
                write!(f, "anchors and aliases are not allowed by policy")
            }
            ErrorKind::TagsDenied => write!(f, "explicit tags are not allowed by policy"),
            ErrorKind::ScalarLengthLimitExceeded { limit, actual } => {
                write!(
                    f,
                    "scalar length {actual} exceeds the limit of {limit} bytes"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// Result type alias for YAML operations.
pub type Result<T> = std::result::Result<T, Error>;

/// The strictness level for YAML parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Strictness {
    /// Fail on the first error. This is the default and recommended for security.
    #[default]
    Strict,
    /// Collect errors and attempt to continue parsing (best-effort).
    Lenient,
}

/// The YAML schema to use for tag resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SchemaKind {
    /// Failsafe schema — all scalars are strings, all collections are generic.
    Failsafe,
    /// JSON schema — recognizes null, bool, int, float.
    Json,
    /// Core schema — extends JSON schema with additional type recognition. Default.
    #[default]
    Core,
}

/// Composable, opt-in parsing policies that harden against abuse.
///
/// All fields default to permissive (off / unbounded), so default parsing
/// behavior is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParserPolicies {
    /// Reject documents that declare anchors (`&a`) or use aliases (`*a`).
    pub deny_anchors: bool,
    /// Reject documents that carry explicit tags (`!tag`, `!!str`).
    pub deny_tags: bool,
    /// Maximum byte length for any single scalar (`None` = unbounded).
    pub max_scalar_length: Option<usize>,
}

/// Configuration for the YAML parser.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct ParserConfig {
    /// Resource limits.
    pub limits: ResourceLimits,
    /// Strictness level.
    pub strictness: Strictness,
    /// Schema for tag resolution.
    pub schema: SchemaKind,
    /// Resolve YAML 1.1 merge keys (`<<`). Off by default (strict YAML 1.2 has
    /// no merge key); enable for `serde_yaml`-style merge behavior.
    pub merge_keys: bool,
    /// Opt-in policies that harden parsing against abuse.
    pub policies: ParserPolicies,
    /// Enable YAML 1.1 compatibility for scalar bool/null resolution
    /// (the "Norway problem": `yes`/`no`/`on`/`off`/`y`/`n` resolve to booleans).
    /// Off by default — default resolution follows the strict YAML 1.2 core schema.
    /// Note: 1.1 integer/float quirks (underscores, base-60) are NOT covered.
    pub yaml_1_1: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Position;

    fn test_span() -> Span {
        Span {
            start: Position {
                offset: 10,
                line: 2,
                column: 3,
            },
            end: Position {
                offset: 15,
                line: 2,
                column: 8,
            },
        }
    }

    #[test]
    fn error_new_with_span() {
        let err = Error::new(ErrorKind::UnexpectedEof, test_span());
        assert!(err.span.is_some());
        assert!(err.context.is_empty());
    }

    #[test]
    fn error_spanless() {
        let err = Error::spanless(ErrorKind::InvalidUtf8);
        assert!(err.span.is_none());
    }

    #[test]
    fn error_with_context() {
        let err = Error::new(ErrorKind::UnexpectedEof, test_span())
            .with_context("while parsing a block mapping", test_span());
        assert_eq!(err.context.len(), 1);
        assert_eq!(err.context[0].description, "while parsing a block mapping");
    }

    #[test]
    fn error_is_limit_error() {
        let limit_err = Error::depth_exceeded(&ResourceLimits::default(), test_span());
        assert!(limit_err.is_limit_error());

        let other_err = Error::new(ErrorKind::UnexpectedEof, test_span());
        assert!(!other_err.is_limit_error());
    }

    #[test]
    fn error_display() {
        let err = Error::new(ErrorKind::UnexpectedEof, test_span());
        let display = err.to_string();
        assert!(display.contains("unexpected end of input"));
        assert!(display.contains("2:3"));
    }

    #[test]
    fn error_display_with_context() {
        let err = Error::new(
            ErrorKind::DuplicateKey {
                key: "port".to_string(),
                first: test_span(),
            },
            test_span(),
        )
        .with_context("while parsing a block mapping", test_span());
        let display = err.to_string();
        assert!(display.contains("duplicate key"));
        assert!(display.contains("port"));
        assert!(display.contains("while parsing a block mapping"));
    }

    #[test]
    fn error_kind_display_variants() {
        assert_eq!(ErrorKind::InvalidUtf8.to_string(), "invalid UTF-8");
        assert_eq!(
            ErrorKind::UnexpectedByte(0xFF).to_string(),
            "unexpected byte: 0xFF"
        );
        assert_eq!(
            ErrorKind::InvalidEscape('z').to_string(),
            "invalid escape sequence: \\z"
        );
        assert_eq!(
            ErrorKind::TabInIndentation.to_string(),
            "tab character used for indentation"
        );

        let limit = ErrorKind::DepthLimitExceeded { limit: 128 };
        assert!(limit.to_string().contains("128"));
    }

    #[test]
    fn error_kind_display_remaining_variants() {
        assert_eq!(
            ErrorKind::UnexpectedToken {
                expected: "a scalar".into(),
                found: "end of mapping".into(),
            }
            .to_string(),
            "expected a scalar, found end of mapping"
        );
        assert_eq!(
            ErrorKind::InvalidCharacter('x').to_string(),
            "invalid character: 'x'"
        );
        assert_eq!(
            ErrorKind::InvalidIndentation {
                expected: 4,
                found: 2,
            }
            .to_string(),
            "invalid indentation: expected 4, found 2"
        );
        assert_eq!(
            ErrorKind::InvalidTag("!!bogus".to_string()).to_string(),
            "invalid tag: !!bogus"
        );
        assert_eq!(
            ErrorKind::InvalidAnchor("bad anchor".to_string()).to_string(),
            "invalid anchor: bad anchor"
        );
        assert_eq!(
            ErrorKind::UndefinedAlias("ref".to_string()).to_string(),
            "undefined alias: *ref"
        );
        assert_eq!(
            ErrorKind::DuplicateAnchor("a".to_string()).to_string(),
            "duplicate anchor: &a"
        );

        let alias = ErrorKind::AliasExpansionLimitExceeded { limit: 1024 }.to_string();
        assert!(alias.contains("alias expansions"));
        assert!(alias.contains("1024"));

        let doc = ErrorKind::DocumentSizeLimitExceeded { limit: 4096 }.to_string();
        assert!(doc.contains("document size"));
        assert!(doc.contains("4096"));

        let key = ErrorKind::KeyLengthLimitExceeded { limit: 256 }.to_string();
        assert!(key.contains("key length"));
        assert!(key.contains("256"));

        let nodes = ErrorKind::NodeCountLimitExceeded { limit: 999 }.to_string();
        assert!(nodes.contains("node count"));
        assert!(nodes.contains("999"));
    }

    #[test]
    fn limit_error_constructors() {
        let limits = ResourceLimits::default();
        let span = test_span();

        let err = Error::depth_exceeded(&limits, span);
        assert!(matches!(
            err.kind,
            ErrorKind::DepthLimitExceeded { limit: 128 }
        ));

        let err = Error::alias_expansion_exceeded(&limits, span);
        assert!(matches!(
            err.kind,
            ErrorKind::AliasExpansionLimitExceeded { limit: 1024 }
        ));

        let err = Error::document_size_exceeded(&limits, span);
        assert!(matches!(
            err.kind,
            ErrorKind::DocumentSizeLimitExceeded { .. }
        ));

        let err = Error::key_length_exceeded(&limits, span);
        assert!(matches!(
            err.kind,
            ErrorKind::KeyLengthLimitExceeded { limit: 1024 }
        ));

        let err = Error::node_count_exceeded(&limits, span);
        assert!(matches!(
            err.kind,
            ErrorKind::NodeCountLimitExceeded { limit: 1_000_000 }
        ));
    }

    #[test]
    fn default_config() {
        let config = ParserConfig::default();
        assert_eq!(config.strictness, Strictness::Strict);
        assert_eq!(config.schema, SchemaKind::Core);
        assert_eq!(config.limits.max_depth, 128);
    }

    #[test]
    fn parser_config_merge_keys_defaults_off() {
        assert!(!ParserConfig::default().merge_keys);
    }

    #[test]
    fn parser_config_yaml_1_1_defaults_off() {
        assert!(!ParserConfig::default().yaml_1_1);
    }

    #[test]
    fn parser_policies_default_permissive() {
        let p = ParserConfig::default().policies;
        assert!(!p.deny_anchors);
        assert!(!p.deny_tags);
        assert_eq!(p.max_scalar_length, None);
    }

    #[test]
    fn policy_error_kinds_display() {
        assert!(!ErrorKind::AnchorsDenied.to_string().is_empty());
        assert!(!ErrorKind::TagsDenied.to_string().is_empty());
        assert!(
            !ErrorKind::ScalarLengthLimitExceeded {
                limit: 4,
                actual: 9
            }
            .to_string()
            .is_empty()
        );
    }
}
