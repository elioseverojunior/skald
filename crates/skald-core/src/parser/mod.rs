// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML parser.
//!
//! Converts a [`Token`] stream into a sequence of [`Event`]s (SAX-style).
//! Implements a state machine with an explicit state stack for bounded stack usage
//! and depth limit enforcement.
//!
//! # Architecture
//!
//! The parser does not recurse — it uses a `Vec<State>` stack that is checked
//! against [`ResourceLimits::max_depth`](crate::limits::ResourceLimits::max_depth) on every collection entry. This guarantees
//! that arbitrarily nested YAML documents cannot overflow the call stack.

pub mod event;

pub use event::{Event, EventKind};

use crate::error::{Error, ErrorKind, ParserConfig, Result};
use crate::scanner::Scanner;
use crate::scanner::token::{Token, TokenKind};
use crate::types::{CollectionStyle, Position, ScalarStyle, Span};
use std::borrow::Cow;
use std::collections::HashMap;

/// Parsed tag: (handle, suffix). E.g., `("!!", "str")` for `!!str`.
type TagPair<'a> = (Cow<'a, str>, Cow<'a, str>);

/// Internal parser states. Each state represents what the parser expects next.
#[derive(Debug, Clone, PartialEq)]
enum State {
    StreamStart,
    DocumentStart,
    DocumentContent,
    DocumentEnd,
    /// Block node context that also allows indentless sequences (block entry
    /// at the same indent as the parent mapping).
    /// `in_sequence`: true when this is parsing a value inside a block sequence entry,
    /// false when parsing a mapping value (where BlockEntry starts a new sequence).
    IndentlessBlockNode {
        in_sequence: bool,
    },
    FlowNode,
    BlockSequenceEntry {
        first: bool,
        /// True when this sequence was started by an indentless `BlockEntry`
        /// (no matching `BlockEnd` — the parent scope owns that token).
        indentless: bool,
    },
    BlockMappingKey {
        first: bool,
    },
    BlockMappingValue,
    FlowSequenceEntry {
        first: bool,
    },
    FlowMappingKey {
        first: bool,
        /// True for single-pair implicit mappings inside flow sequences.
        /// These auto-close after one key-value pair.
        implicit: bool,
    },
    FlowMappingValue {
        implicit: bool,
    },
    FlowMappingEmptyValue,
    End,
}

/// The YAML parser.
///
/// Converts a token stream into events. Use as an iterator or call
/// [`next_event`](Parser::next_event) explicitly.
pub struct Parser<'a> {
    scanner: Scanner<'a>,
    /// Buffered token from peeking.
    peeked: Option<Token<'a>>,
    /// State stack (explicit, not recursive).
    states: Vec<State>,
    /// Current state.
    state: State,
    /// Depth tracking for limit enforcement.
    depth: usize,
    /// Configuration.
    config: ParserConfig,
    /// Whether an error has been produced.
    errored: bool,
    /// Tag handle → prefix mappings from `%TAG` directives (per document).
    tag_directives: HashMap<String, String>,
    /// Whether we've seen at least one document end that was implicit (no `...`).
    /// Directives are forbidden after an implicit document end (YAML 1.2.2 §9.2).
    implicit_doc_end: bool,
}

impl<'a> Parser<'a> {
    /// Creates a new parser for the given input.
    #[must_use]
    pub fn new(input: &'a str) -> Self {
        Self::with_config(input, ParserConfig::default())
    }

    /// Creates a new parser with custom configuration.
    #[must_use]
    pub fn with_config(input: &'a str, config: ParserConfig) -> Self {
        let scanner = Scanner::with_limits(input, config.limits.clone());
        Self {
            scanner,
            peeked: None,
            states: Vec::with_capacity(8),
            state: State::StreamStart,
            depth: 0,
            config,
            errored: false,
            tag_directives: HashMap::with_capacity(4),
            implicit_doc_end: false,
        }
    }

    /// Returns the next event, or `None` if the stream is exhausted.
    pub fn next_event(&mut self) -> Option<Result<Event<'a>>> {
        if self.errored {
            return None;
        }
        match self.parse() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(e) => {
                self.errored = true;
                Some(Err(e))
            }
        }
    }

    // ─── Token access ───────────────────────────────────────────────

    fn peek_token(&mut self) -> Result<Option<&Token<'a>>> {
        if self.peeked.is_none() {
            self.peeked = self.scanner.next().transpose()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn next_token(&mut self) -> Result<Option<Token<'a>>> {
        if let Some(t) = self.peeked.take() {
            return Ok(Some(t));
        }
        self.scanner.next().transpose()
    }

    fn require_token(&mut self) -> Result<Token<'a>> {
        self.next_token()?
            .ok_or_else(|| Error::spanless(ErrorKind::UnexpectedEof))
    }

    fn peek_token_kind(&mut self) -> Result<Option<&TokenKind<'a>>> {
        Ok(self.peek_token()?.map(|t| &t.kind))
    }

    /// Builds an [`ErrorKind::UnexpectedToken`] anchored at the current token,
    /// reporting what was `expected` and naming the token actually found.
    ///
    /// Used for the flow-collection separator errors (`',' or ']'` / `'}'`),
    /// where the next token is neither a separator nor the closing delimiter.
    fn unexpected_token(&mut self, expected: &'static str) -> Result<Error> {
        let span = self
            .peek_token()?
            .map(|t| t.span)
            .unwrap_or(Span::point(Position::start()));
        let found = self
            .peek_token_kind()?
            .map(|k| k.name())
            .unwrap_or("end-of-input")
            .to_string()
            .into();
        Ok(Error::new(
            ErrorKind::UnexpectedToken {
                expected: expected.into(),
                found,
            },
            span,
        ))
    }

    // ─── State management ────────────────────────────────────────────

    /// Pops the previous state from the stack, restoring it as the current state.
    fn pop_state(&mut self) {
        self.state = self.states.pop().unwrap_or(State::End);
    }

    /// Enters a new collection, setting the state and checking the depth limit.
    /// The caller's continuation state must already be on the `states` stack.
    fn enter_collection(&mut self, state: State, span: Span) -> Result<()> {
        self.depth += 1;
        if self.depth > self.config.limits.max_depth {
            return Err(Error::depth_exceeded(&self.config.limits, span));
        }
        self.state = state;
        Ok(())
    }

    /// Leaves a collection, decrementing depth and popping the continuation state.
    fn leave_collection(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.pop_state();
    }

    // ─── Anchor/Tag collection ──────────────────────────────────────

    fn parse_anchor_and_tag(&mut self) -> Result<(Option<Cow<'a, str>>, Option<TagPair<'a>>)> {
        let mut anchor = None;
        let mut tag = None;

        loop {
            match self.peek_token_kind()? {
                Some(TokenKind::Anchor(_)) => {
                    // A node can have at most one anchor (YAML 1.2.2 §6.9.2).
                    if anchor.is_some() {
                        let span = self
                            .peek_token()?
                            .map(|t| t.span)
                            .unwrap_or(Span::point(Position::start()));
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "node content".into(),
                                found: "duplicate anchor".into(),
                            },
                            span,
                        ));
                    }
                    let token = self.require_token()?;
                    if let TokenKind::Anchor(name) = token.kind {
                        anchor = Some(name);
                    }
                }
                Some(TokenKind::Tag { .. }) => {
                    let token = self.require_token()?;
                    if let TokenKind::Tag { handle, suffix } = token.kind {
                        tag = Some(self.resolve_tag(handle, suffix, token.span)?);
                    }
                }
                _ => break,
            }
        }

        Ok((anchor, tag))
    }

    /// Resolves a tag handle + suffix using the current `%TAG` directives.
    ///
    /// - Verbatim tags (empty handle from `!<uri>`) pass through as-is.
    /// - Known handles (`!`, `!!`, custom `!e!`) are expanded to their declared prefix.
    /// - Unknown handles produce an error (YAML 1.2.2 §6.8.2).
    fn resolve_tag(
        &self,
        handle: Cow<'a, str>,
        suffix: Cow<'a, str>,
        span: Span,
    ) -> Result<TagPair<'a>> {
        if handle.is_empty() {
            // Verbatim tag: !<uri> — suffix is the full URI
            return Ok((Cow::Owned(String::new()), suffix));
        }
        if let Some(prefix) = self.tag_directives.get(handle.as_ref()) {
            Ok((Cow::Owned(prefix.clone()), suffix))
        } else {
            Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "declared tag handle".into(),
                    found: format!("undeclared tag handle '{handle}'").into(),
                },
                span,
            ))
        }
    }

    // ─── Main parse dispatch ────────────────────────────────────────

    fn parse(&mut self) -> Result<Option<Event<'a>>> {
        match self.state {
            State::StreamStart => self.parse_stream_start(),
            State::DocumentStart => self.parse_document_start(),
            State::DocumentContent => self.parse_document_content(),
            State::DocumentEnd => self.parse_document_end(),
            State::IndentlessBlockNode { in_sequence } => self.parse_node_indentless(in_sequence),
            State::FlowNode => self.parse_node(false, false),
            State::BlockSequenceEntry { first, indentless } => {
                self.parse_block_sequence_entry(first, indentless)
            }
            State::BlockMappingKey { first } => self.parse_block_mapping_key(first),
            State::BlockMappingValue => self.parse_block_mapping_value(),
            State::FlowSequenceEntry { first } => self.parse_flow_sequence_entry(first),
            State::FlowMappingKey { first, implicit } => {
                self.parse_flow_mapping_key(first, implicit)
            }
            State::FlowMappingValue { implicit } => self.parse_flow_mapping_value(implicit),
            State::FlowMappingEmptyValue => self.parse_flow_mapping_empty_value(),
            State::End => Ok(None),
        }
    }

    // ─── Stream ─────────────────────────────────────────────────────

    fn parse_stream_start(&mut self) -> Result<Option<Event<'a>>> {
        let token = self.require_token()?;
        self.state = State::DocumentStart;
        Ok(Some(Event {
            kind: EventKind::StreamStart,
            span: token.span,
        }))
    }

    // ─── Document ───────────────────────────────────────────────────

    fn parse_document_start(&mut self) -> Result<Option<Event<'a>>> {
        // Reset tag directives for this document and register defaults
        self.tag_directives.clear();
        self.tag_directives
            .insert("!!".to_string(), "tag:yaml.org,2002:".to_string());
        self.tag_directives.insert("!".to_string(), "!".to_string());

        // Collect version/tag directives and skip bare document-end markers.
        // A `...` without a preceding document is valid YAML and should be skipped.
        let mut has_version_directive = false;
        let mut has_any_directive = false;
        loop {
            match self.peek_token_kind()? {
                Some(TokenKind::VersionDirective { .. }) => {
                    // Directives require an explicit document-end '...' first
                    // if a previous document was implicitly closed (YAML 1.2.2 §9.2).
                    if self.implicit_doc_end {
                        let span = self
                            .peek_token()?
                            .map(|t| t.span)
                            .unwrap_or(Span::point(Position::start()));
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "document end '...' before directive".into(),
                                found: "%YAML directive".into(),
                            },
                            span,
                        ));
                    }
                    if has_version_directive {
                        let span = self
                            .peek_token()?
                            .map(|t| t.span)
                            .unwrap_or(Span::point(Position::start()));
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "document start '---'".into(),
                                found: "duplicate %YAML directive".into(),
                            },
                            span,
                        ));
                    }
                    has_version_directive = true;
                    has_any_directive = true;
                    self.next_token()?;
                }
                Some(TokenKind::TagDirective { .. }) => {
                    // Directives require an explicit document-end '...' first
                    if self.implicit_doc_end {
                        let span = self
                            .peek_token()?
                            .map(|t| t.span)
                            .unwrap_or(Span::point(Position::start()));
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "document end '...' before directive".into(),
                                found: "%TAG directive".into(),
                            },
                            span,
                        ));
                    }
                    has_any_directive = true;
                    let token = self.require_token()?;
                    if let TokenKind::TagDirective { handle, prefix } = token.kind {
                        self.tag_directives
                            .insert(handle.into_owned(), prefix.into_owned());
                    }
                }
                Some(TokenKind::DocumentEnd) => {
                    self.next_token()?;
                }
                _ => break,
            }
        }

        match self.peek_token_kind()? {
            Some(TokenKind::DocumentStart) => {
                let token = self.require_token()?;
                self.state = State::DocumentContent;
                Ok(Some(Event {
                    kind: EventKind::DocumentStart { explicit: true },
                    span: token.span,
                }))
            }
            Some(TokenKind::StreamEnd) => {
                // Directives must be followed by a document (9MMA, B63P)
                if has_any_directive {
                    let span = self
                        .peek_token()?
                        .map(|t| t.span)
                        .unwrap_or(Span::point(Position::start()));
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: "document start '---' after directive".into(),
                            found: "end of stream".into(),
                        },
                        span,
                    ));
                }
                let token = self.require_token()?;
                self.state = State::End;
                Ok(Some(Event {
                    kind: EventKind::StreamEnd,
                    span: token.span,
                }))
            }
            _ => {
                // Bare (implicit) document start.
                // Per YAML §9.2, bare documents are only valid as the first
                // document in the stream or after an explicit document-end
                // marker '...'.  After an implicit document end, only '---'
                // may start a new document.
                if self.implicit_doc_end {
                    let span = self
                        .peek_token()?
                        .map(|t| t.span)
                        .unwrap_or(Span::point(Position::start()));
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: "document start '---' or document end '...'".into(),
                            found: "content without document marker".into(),
                        },
                        span,
                    ));
                }
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                self.state = State::DocumentContent;
                Ok(Some(Event {
                    kind: EventKind::DocumentStart { explicit: false },
                    span,
                }))
            }
        }
    }

    fn parse_document_content(&mut self) -> Result<Option<Event<'a>>> {
        match self.peek_token_kind()? {
            Some(TokenKind::DocumentEnd)
            | Some(TokenKind::DocumentStart)
            | Some(TokenKind::StreamEnd) => {
                // Empty document
                self.state = State::DocumentEnd;
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                Ok(Some(Event {
                    kind: EventKind::Scalar {
                        value: Cow::Borrowed(""),
                        style: ScalarStyle::Plain,
                        anchor: None,
                        tag: None,
                    },
                    span,
                }))
            }
            _ => {
                // Push continuation: after the root node, go to DocumentEnd.
                self.states.push(State::DocumentEnd);
                self.parse_node(true, false)
            }
        }
    }

    fn parse_document_end(&mut self) -> Result<Option<Event<'a>>> {
        let (explicit, span) = match self.peek_token_kind()? {
            Some(TokenKind::DocumentEnd) => {
                let token = self.require_token()?;
                (true, token.span)
            }
            _ => {
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                (false, span)
            }
        };

        self.implicit_doc_end = !explicit;
        self.state = State::DocumentStart;
        Ok(Some(Event {
            kind: EventKind::DocumentEnd { explicit },
            span,
        }))
    }

    // ─── Node ───────────────────────────────────────────────────────
    //
    // Convention: before calling parse_node, the caller pushes its
    // continuation state onto `self.states`. For a scalar / alias,
    // parse_node pops that continuation immediately. For a collection,
    // the continuation stays on the stack and is popped when the
    // collection ends via `leave_collection`.

    fn parse_node(&mut self, block: bool, indentless: bool) -> Result<Option<Event<'a>>> {
        let props = self.parse_anchor_and_tag()?;
        self.parse_node_inner(block, indentless, props)
    }

    /// Core node dispatch given already-parsed anchor/tag properties.
    ///
    /// Callers either let [`Self::parse_node`] collect the properties first, or pass
    /// properties they have already consumed (indentless block-sequence values).
    fn parse_node_inner(
        &mut self,
        block: bool,
        indentless: bool,
        props: (Option<Cow<'a, str>>, Option<TagPair<'a>>),
    ) -> Result<Option<Event<'a>>> {
        let (anchor, tag) = props;

        match self.peek_token_kind()? {
            Some(TokenKind::Alias(_)) => {
                // Aliases cannot have node properties (anchors or tags)
                if anchor.is_some() || tag.is_some() {
                    let span = self
                        .peek_token()?
                        .map(|t| t.span)
                        .unwrap_or(Span::point(Position::start()));
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: "node content".into(),
                            found: "alias (cannot have anchor or tag)".into(),
                        },
                        span,
                    ));
                }
                let token = self.require_token()?;
                self.pop_state();
                if let TokenKind::Alias(name) = token.kind {
                    Ok(Some(Event {
                        kind: EventKind::Alias { name },
                        span: token.span,
                    }))
                } else {
                    unreachable!()
                }
            }
            Some(TokenKind::Scalar { .. }) => {
                let token = self.require_token()?;
                self.pop_state();
                if let TokenKind::Scalar { value, style } = token.kind {
                    Ok(Some(Event {
                        kind: EventKind::Scalar {
                            value,
                            style,
                            anchor,
                            tag,
                        },
                        span: token.span,
                    }))
                } else {
                    unreachable!()
                }
            }
            Some(TokenKind::FlowSequenceStart) => {
                let token = self.require_token()?;
                let span = token.span;
                self.enter_collection(State::FlowSequenceEntry { first: true }, span)?;
                Ok(Some(Event {
                    kind: EventKind::SequenceStart {
                        anchor,
                        tag,
                        style: CollectionStyle::Flow,
                    },
                    span,
                }))
            }
            Some(TokenKind::FlowMappingStart) => {
                let token = self.require_token()?;
                let span = token.span;
                self.enter_collection(
                    State::FlowMappingKey {
                        first: true,
                        implicit: false,
                    },
                    span,
                )?;
                Ok(Some(Event {
                    kind: EventKind::MappingStart {
                        anchor,
                        tag,
                        style: CollectionStyle::Flow,
                    },
                    span,
                }))
            }
            Some(TokenKind::BlockSequenceStart) if block => {
                let token = self.require_token()?;
                let span = token.span;
                self.enter_collection(
                    State::BlockSequenceEntry {
                        first: true,
                        indentless: false,
                    },
                    span,
                )?;
                Ok(Some(Event {
                    kind: EventKind::SequenceStart {
                        anchor,
                        tag,
                        style: CollectionStyle::Block,
                    },
                    span,
                }))
            }
            Some(TokenKind::BlockMappingStart) if block => {
                let token = self.require_token()?;
                let span = token.span;
                self.enter_collection(State::BlockMappingKey { first: true }, span)?;
                Ok(Some(Event {
                    kind: EventKind::MappingStart {
                        anchor,
                        tag,
                        style: CollectionStyle::Block,
                    },
                    span,
                }))
            }
            // Indentless block sequence: `-` at the same indent as the parent
            // mapping, without a preceding BlockSequenceStart token.
            Some(TokenKind::BlockEntry) if indentless => {
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                self.enter_collection(
                    State::BlockSequenceEntry {
                        first: true,
                        indentless: true,
                    },
                    span,
                )?;
                Ok(Some(Event {
                    kind: EventKind::SequenceStart {
                        anchor,
                        tag,
                        style: CollectionStyle::Block,
                    },
                    span,
                }))
            }
            _ => {
                if anchor.is_some() || tag.is_some() {
                    // Empty scalar with anchor/tag
                    self.pop_state();
                    let span = self
                        .peek_token()?
                        .map(|t| t.span)
                        .unwrap_or(Span::point(Position::start()));
                    Ok(Some(Event {
                        kind: EventKind::Scalar {
                            value: Cow::Borrowed(""),
                            style: ScalarStyle::Plain,
                            anchor,
                            tag,
                        },
                        span,
                    }))
                } else {
                    let span = self
                        .peek_token()?
                        .map(|t| t.span)
                        .unwrap_or(Span::point(Position::start()));
                    let found = self
                        .peek_token_kind()?
                        .map(|k| k.name())
                        .unwrap_or("end-of-input");
                    Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: "node content".into(),
                            found: found.to_string().into(),
                        },
                        span,
                    ))
                }
            }
        }
    }

    /// Parse a node in indentless context.
    ///
    /// When `in_sequence` is true (i.e., inside a block sequence entry value),
    /// and we see an anchor/tag followed by a `BlockEntry`, the anchor/tag
    /// belongs to an empty scalar — the `BlockEntry` is the next entry in the
    /// *parent* sequence, not a new indentless sequence for this value.
    ///
    /// When `in_sequence` is false (mapping value), `BlockEntry` starts an
    /// indentless sequence as the mapping value — the standard behavior.
    fn parse_node_indentless(&mut self, in_sequence: bool) -> Result<Option<Event<'a>>> {
        if in_sequence {
            // Peek ahead: if there's an anchor/tag and the *next* token after
            // that is BlockEntry, emit an empty scalar (the anchor/tag decorates
            // the empty value, and BlockEntry belongs to the parent sequence).
            let has_properties = self
                .peek_token_kind()?
                .is_some_and(|k| matches!(k, TokenKind::Anchor(_) | TokenKind::Tag { .. }));
            if has_properties {
                let (anchor, tag) = self.parse_anchor_and_tag()?;
                match self.peek_token_kind()? {
                    Some(TokenKind::BlockEntry)
                    | Some(TokenKind::BlockEnd)
                    | Some(TokenKind::Key)
                    | Some(TokenKind::Value)
                    | Some(TokenKind::DocumentStart)
                    | Some(TokenKind::DocumentEnd)
                    | Some(TokenKind::StreamEnd) => {
                        // Anchor/tag on an empty scalar — the next token belongs
                        // to the parent context.
                        self.pop_state();
                        let span = self
                            .peek_token()?
                            .map(|t| t.span)
                            .unwrap_or(Span::point(Position::start()));
                        return Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor,
                                tag,
                            },
                            span,
                        }));
                    }
                    _ => {
                        // There's actual content after the anchor/tag — delegate
                        // to the shared dispatch with the properties we already
                        // consumed (block context, indentless allowed).
                        return self.parse_node_inner(true, true, (anchor, tag));
                    }
                }
            }
        }
        // Default: standard indentless node parsing
        self.parse_node(true, true)
    }

    // ─── Block Sequence ─────────────────────────────────────────────

    fn parse_block_sequence_entry(
        &mut self,
        _first: bool,
        indentless: bool,
    ) -> Result<Option<Event<'a>>> {
        // The scanner guarantees a `BlockEntry` token leads every entry — both
        // the first (immediately after `BlockSequenceStart`, or the `-` that
        // begins an indentless sequence) and every subsequent one. Any other
        // token therefore terminates the sequence.
        match self.peek_token_kind()? {
            Some(TokenKind::BlockEntry) => {
                let token = self.require_token()?;
                let span = token.span;

                match self.peek_token_kind()? {
                    Some(TokenKind::BlockEntry) | Some(TokenKind::BlockEnd) => {
                        // Empty entry
                        self.state = State::BlockSequenceEntry {
                            first: false,
                            indentless,
                        };
                        Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor: None,
                                tag: None,
                            },
                            span,
                        }))
                    }
                    _ => {
                        self.states.push(State::BlockSequenceEntry {
                            first: false,
                            indentless,
                        });
                        self.state = State::IndentlessBlockNode { in_sequence: true };
                        self.parse()
                    }
                }
            }
            Some(TokenKind::BlockEnd) if !indentless => {
                // Normal block sequence end — consume the BlockEnd token. Only
                // sequences that started with `BlockSequenceStart` receive one.
                let token = self.require_token()?;
                self.leave_collection();
                Ok(Some(Event {
                    kind: EventKind::SequenceEnd,
                    span: token.span,
                }))
            }
            _ => {
                // Indentless sequence end (or `BlockEnd` for an indentless
                // sequence): the next token belongs to the parent context, so
                // do NOT consume it.
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                self.leave_collection();
                Ok(Some(Event {
                    kind: EventKind::SequenceEnd,
                    span,
                }))
            }
        }
    }

    // ─── Block Mapping ──────────────────────────────────────────────

    fn parse_block_mapping_key(&mut self, _first: bool) -> Result<Option<Event<'a>>> {
        // The scanner guarantees every mapping entry — the first (right after
        // `BlockMappingStart`) and every subsequent one — is led by a `Key` or
        // `Value` token. Any other token (notably `BlockEnd`) closes the
        // mapping; it is consumed and a `MappingEnd` is emitted.
        match self.peek_token_kind()? {
            Some(TokenKind::Key) => {
                let token = self.require_token()?;

                match self.peek_token_kind()? {
                    Some(TokenKind::Key) | Some(TokenKind::Value) | Some(TokenKind::BlockEnd) => {
                        // Empty key
                        self.state = State::BlockMappingValue;
                        Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor: None,
                                tag: None,
                            },
                            span: token.span,
                        }))
                    }
                    _ => {
                        self.states.push(State::BlockMappingValue);
                        // Allow indentless sequences as explicit key values
                        // (e.g., "?\n- a\n- b" where the sequence is at column 0)
                        self.state = State::IndentlessBlockNode { in_sequence: false };
                        self.parse()
                    }
                }
            }
            Some(TokenKind::Value) => {
                // Implicit empty key (`:` without a preceding `?` key)
                self.state = State::BlockMappingValue;
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                Ok(Some(Event {
                    kind: EventKind::Scalar {
                        value: Cow::Borrowed(""),
                        style: ScalarStyle::Plain,
                        anchor: None,
                        tag: None,
                    },
                    span,
                }))
            }
            _ => {
                let token = self.require_token()?; // consume BlockEnd
                self.leave_collection();
                Ok(Some(Event {
                    kind: EventKind::MappingEnd,
                    span: token.span,
                }))
            }
        }
    }

    fn parse_block_mapping_value(&mut self) -> Result<Option<Event<'a>>> {
        match self.peek_token_kind()? {
            Some(TokenKind::Value) => {
                let token = self.require_token()?;

                match self.peek_token_kind()? {
                    Some(TokenKind::Key) | Some(TokenKind::Value) | Some(TokenKind::BlockEnd) => {
                        // Empty value
                        self.state = State::BlockMappingKey { first: false };
                        Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor: None,
                                tag: None,
                            },
                            span: token.span,
                        }))
                    }
                    _ => {
                        self.states.push(State::BlockMappingKey { first: false });
                        self.state = State::IndentlessBlockNode { in_sequence: false };
                        self.parse()
                    }
                }
            }
            _ => {
                // Missing value — emit empty scalar
                self.state = State::BlockMappingKey { first: false };
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                Ok(Some(Event {
                    kind: EventKind::Scalar {
                        value: Cow::Borrowed(""),
                        style: ScalarStyle::Plain,
                        anchor: None,
                        tag: None,
                    },
                    span,
                }))
            }
        }
    }

    // ─── Flow Sequence ──────────────────────────────────────────────

    fn parse_flow_sequence_entry(&mut self, first: bool) -> Result<Option<Event<'a>>> {
        if !first {
            match self.peek_token_kind()? {
                Some(TokenKind::FlowEntry) => {
                    self.next_token()?;
                }
                Some(TokenKind::FlowSequenceEnd) => {}
                _ => return Err(self.unexpected_token("',' or ']'")?),
            }
        }

        match self.peek_token_kind()? {
            Some(TokenKind::FlowSequenceEnd) => {
                let token = self.require_token()?;
                self.leave_collection();
                Ok(Some(Event {
                    kind: EventKind::SequenceEnd,
                    span: token.span,
                }))
            }
            Some(TokenKind::Key) | Some(TokenKind::Value) => {
                // Implicit mapping inside flow sequence: [a: 1, b: 2] or [: value]
                // Don't consume the token — let parse_flow_mapping_key handle it.
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                self.states.push(State::FlowSequenceEntry { first: false });
                self.enter_collection(
                    State::FlowMappingKey {
                        first: true,
                        implicit: true,
                    },
                    span,
                )?;
                Ok(Some(Event {
                    kind: EventKind::MappingStart {
                        anchor: None,
                        tag: None,
                        style: CollectionStyle::Flow,
                    },
                    span,
                }))
            }
            _ => {
                self.states.push(State::FlowSequenceEntry { first: false });
                self.state = State::FlowNode;
                self.parse()
            }
        }
    }

    // ─── Flow Mapping ───────────────────────────────────────────────

    fn parse_flow_mapping_key(&mut self, first: bool, implicit: bool) -> Result<Option<Event<'a>>> {
        if !first {
            if implicit {
                // Single-pair implicit mapping — auto-close after one entry.
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                self.leave_collection();
                return Ok(Some(Event {
                    kind: EventKind::MappingEnd,
                    span,
                }));
            }
            match self.peek_token_kind()? {
                Some(TokenKind::FlowEntry) => {
                    self.next_token()?;
                }
                Some(TokenKind::FlowMappingEnd) => {}
                _ => return Err(self.unexpected_token("',' or '}'")?),
            }
        }

        match self.peek_token_kind()? {
            Some(TokenKind::FlowMappingEnd) => {
                let token = self.require_token()?;
                self.leave_collection();
                Ok(Some(Event {
                    kind: EventKind::MappingEnd,
                    span: token.span,
                }))
            }
            Some(TokenKind::Key) => {
                let token = self.require_token()?;

                match self.peek_token_kind()? {
                    Some(TokenKind::Value)
                    | Some(TokenKind::FlowEntry)
                    | Some(TokenKind::FlowMappingEnd) => {
                        // Empty key
                        self.state = State::FlowMappingValue { implicit };
                        Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor: None,
                                tag: None,
                            },
                            span: token.span,
                        }))
                    }
                    _ => {
                        self.states.push(State::FlowMappingValue { implicit });
                        self.state = State::FlowNode;
                        self.parse()
                    }
                }
            }
            Some(TokenKind::Value) => {
                // Empty key — bare `:` without preceding key: { : bar } or [ : val ]
                self.state = State::FlowMappingValue { implicit };
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                Ok(Some(Event {
                    kind: EventKind::Scalar {
                        value: Cow::Borrowed(""),
                        style: ScalarStyle::Plain,
                        anchor: None,
                        tag: None,
                    },
                    span,
                }))
            }
            _ => {
                // Implicit key
                self.states.push(State::FlowMappingEmptyValue);
                self.state = State::FlowNode;
                self.parse()
            }
        }
    }

    fn parse_flow_mapping_value(&mut self, implicit: bool) -> Result<Option<Event<'a>>> {
        match self.peek_token_kind()? {
            Some(TokenKind::Value) => {
                let token = self.require_token()?;

                match self.peek_token_kind()? {
                    Some(TokenKind::FlowEntry) | Some(TokenKind::FlowMappingEnd) => {
                        // Empty value
                        self.state = State::FlowMappingKey {
                            first: false,
                            implicit,
                        };
                        Ok(Some(Event {
                            kind: EventKind::Scalar {
                                value: Cow::Borrowed(""),
                                style: ScalarStyle::Plain,
                                anchor: None,
                                tag: None,
                            },
                            span: token.span,
                        }))
                    }
                    _ => {
                        self.states.push(State::FlowMappingKey {
                            first: false,
                            implicit,
                        });
                        self.state = State::FlowNode;
                        self.parse()
                    }
                }
            }
            _ => {
                // Missing value
                self.state = State::FlowMappingKey {
                    first: false,
                    implicit,
                };
                let span = self
                    .peek_token()?
                    .map(|t| t.span)
                    .unwrap_or(Span::point(Position::start()));
                Ok(Some(Event {
                    kind: EventKind::Scalar {
                        value: Cow::Borrowed(""),
                        style: ScalarStyle::Plain,
                        anchor: None,
                        tag: None,
                    },
                    span,
                }))
            }
        }
    }

    fn parse_flow_mapping_empty_value(&mut self) -> Result<Option<Event<'a>>> {
        self.state = State::FlowMappingKey {
            first: false,
            implicit: false,
        };
        let span = self
            .peek_token()?
            .map(|t| t.span)
            .unwrap_or(Span::point(Position::start()));
        Ok(Some(Event {
            kind: EventKind::Scalar {
                value: Cow::Borrowed(""),
                style: ScalarStyle::Plain,
                anchor: None,
                tag: None,
            },
            span,
        }))
    }
}

impl<'a> Iterator for Parser<'a> {
    type Item = Result<Event<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_event()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Vec<EventKind<'_>> {
        Parser::new(input)
            .map(|r| r.expect("parse error").kind)
            .collect()
    }

    fn event_names(input: &str) -> Vec<&'static str> {
        parse(input).iter().map(|e| e.name()).collect()
    }

    fn parse_err(input: &str) -> Error {
        Parser::new(input)
            .find_map(|r| r.err())
            .expect("expected parse error")
    }

    // ─── Anchors/tags on block-sequence entry values (parse_node_with_properties) ─

    #[test]
    fn anchored_scalar_in_block_sequence_entry() {
        // `- &a foo`: anchor + scalar in a block sequence entry value.
        let events = parse("- &a foo");
        assert!(matches!(
            events
                .iter()
                .find(|e| matches!(e, EventKind::Scalar { .. })),
            Some(EventKind::Scalar {
                anchor: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn anchored_flow_sequence_in_block_sequence_entry() {
        // `- &a [1, 2]`: anchor + flow sequence start (line 777).
        let names = event_names("- &a [1, 2]");
        assert_eq!(names.iter().filter(|&&n| n == "sequence-start").count(), 2);
    }

    #[test]
    fn anchored_flow_mapping_in_block_sequence_entry() {
        // `- &a {x: 1}`: anchor + flow mapping start (line 790).
        let names = event_names("- &a {x: 1}");
        assert!(names.contains(&"mapping-start"));
    }

    #[test]
    fn anchored_block_sequence_in_block_sequence_entry() {
        // `- &a\n  - x`: anchor + nested block sequence (line 809).
        let names = event_names("- &a\n  - x");
        assert_eq!(names.iter().filter(|&&n| n == "sequence-start").count(), 2);
    }

    #[test]
    fn anchored_block_mapping_in_block_sequence_entry() {
        // `- &a\n  x: 1`: anchor + nested block mapping (line 828).
        let names = event_names("- &a\n  x: 1");
        assert!(names.contains(&"mapping-start"));
    }

    #[test]
    fn tagged_scalar_in_block_sequence_then_next_entry() {
        // `- !!str x\n- y`: tag + scalar, then a second entry.
        let names = event_names("- !!str x\n- y");
        assert_eq!(names.iter().filter(|&&n| n == "scalar").count(), 2);
    }

    #[test]
    fn anchor_on_empty_block_sequence_entry_value() {
        // `- &a\n- b`: the anchor decorates an empty scalar; the next
        // BlockEntry belongs to the parent sequence.
        let events = parse("- &a\n- b");
        let anchored = events.iter().any(
            |e| matches!(e, EventKind::Scalar { anchor: Some(_), value, .. } if value.is_empty()),
        );
        assert!(anchored, "expected empty anchored scalar: {events:?}");
    }

    #[test]
    fn alias_with_anchor_in_block_sequence_entry_errors() {
        // `- &a *b`: an alias cannot carry node properties (anchor/tag).
        // This drives parse_node_with_properties' Alias arm error
        // (parser lines 736-746).
        let err = parse_err("- &a *b");
        match err.kind {
            ErrorKind::UnexpectedToken { found, .. } => {
                assert!(found.contains("alias"), "got {found}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn alias_without_properties_in_block_sequence_entry() {
        // `- &a x\n- *a`: a plain alias entry (no properties) is consumed
        // via parse_node_with_properties' alias success path (749-755)
        // is not taken here, but a bare alias entry exercises alias
        // handling end-to-end.
        let names = event_names("- &a x\n- *a");
        assert!(names.contains(&"alias"));
    }

    // ─── Explicit keys / indentless values (1033-1048, 1071) ────────

    #[test]
    fn explicit_empty_key_then_value() {
        // `?\n: v`: explicit key with empty content, then a value.
        let names = event_names("?\n: v");
        assert!(names.contains(&"mapping-start"));
        assert!(names.contains(&"mapping-end"));
    }

    #[test]
    fn explicit_key_with_indentless_sequence_value() {
        // `?\n- a\n- b`: explicit key whose value is an indentless seq.
        let names = event_names("?\n- a\n- b");
        assert!(names.contains(&"sequence-start"));
    }

    #[test]
    fn two_explicit_keys_block_end() {
        // `? k\n? k2`: two explicit keys exercises the BlockEnd path
        // in the block-mapping-key state (line 1071).
        let names = event_names("? k\n? k2");
        assert!(names.contains(&"mapping-end"));
    }

    // ─── Flow collection separator errors (1160, 1243) ──────────────

    #[test]
    fn flow_sequence_missing_separator_errors() {
        // `[[1] [2]]`: two items without a comma — line 1160.
        let err = parse_err("[[1] [2]]");
        match err.kind {
            ErrorKind::UnexpectedToken { expected, .. } => {
                assert!(expected.contains(']'), "got {expected}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn flow_mapping_missing_separator_errors() {
        // `{a: 1 b: 2}` / `{a: b\nc: d}`: missing comma — line 1243.
        let err = parse_err("{a: b\nc: d}");
        match err.kind {
            ErrorKind::UnexpectedToken { expected, .. } => {
                assert!(expected.contains('}'), "got {expected}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ─── Property indentation error in indentless block value ───────

    #[test]
    fn property_at_column_zero_in_mapping_value_errors() {
        // `key:\n&a\n- x`: anchor at column 0 under a mapping value
        // violates the property-indentation rule.
        let err = parse_err("key:\n&a\n- x");
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    // ─── Unterminated flow collections (error propagation) ──────────

    #[test]
    fn unterminated_flow_collections_error() {
        assert!(
            parse_err("[").kind
                == ErrorKind::UnexpectedToken {
                    expected: "node content".into(),
                    found: "stream-end".into(),
                }
        );
        let _ = parse_err("{");
    }

    #[test]
    fn empty_stream() {
        let names = event_names("");
        assert_eq!(names, vec!["stream-start", "stream-end"]);
    }

    #[test]
    fn plain_scalar() {
        let events = parse("hello");
        let names: Vec<&str> = events.iter().map(|e| e.name()).collect();
        assert!(names.contains(&"scalar"));
        assert!(matches!(
            &events[2],
            EventKind::Scalar { value, style: ScalarStyle::Plain, .. } if value == "hello"
        ));
    }

    #[test]
    fn block_mapping() {
        let names = event_names("a: 1\nb: 2");
        assert!(names.contains(&"mapping-start"));
        assert!(names.contains(&"mapping-end"));
        // Should have scalars for keys and values
        let scalar_count = names.iter().filter(|&&n| n == "scalar").count();
        assert!(
            scalar_count >= 4,
            "expected at least 4 scalars, got {scalar_count}"
        );
    }

    #[test]
    fn block_sequence() {
        let names = event_names("- one\n- two");
        assert!(names.contains(&"sequence-start"));
        assert!(names.contains(&"sequence-end"));
    }

    #[test]
    fn nested_mapping_in_sequence() {
        let input = "- a: 1\n- b: 2";
        let names = event_names(input);
        assert!(names.contains(&"sequence-start"));
        assert!(names.contains(&"mapping-start"));
    }

    #[test]
    fn flow_sequence() {
        let names = event_names("[1, 2, 3]");
        assert!(names.contains(&"sequence-start"));
        assert!(names.contains(&"sequence-end"));
    }

    #[test]
    fn flow_mapping() {
        let names = event_names("{a: 1, b: 2}");
        assert!(names.contains(&"mapping-start"));
        assert!(names.contains(&"mapping-end"));
    }

    #[test]
    fn explicit_document() {
        let events = parse("---\nhello\n...");
        assert!(matches!(
            &events[1],
            EventKind::DocumentStart { explicit: true }
        ));
        assert!(matches!(
            &events[3],
            EventKind::DocumentEnd { explicit: true }
        ));
    }

    #[test]
    fn multi_document() {
        let names = event_names("---\na\n---\nb");
        let doc_starts = names.iter().filter(|&&n| n == "document-start").count();
        assert!(
            doc_starts >= 2,
            "expected at least 2 document starts, got {doc_starts}"
        );
    }

    #[test]
    fn anchor_on_scalar() {
        let events = parse("&anchor hello");
        let scalar = events
            .iter()
            .find(|e| matches!(e, EventKind::Scalar { .. }));
        assert!(matches!(
            scalar,
            Some(EventKind::Scalar { anchor: Some(name), .. }) if name == "anchor"
        ));
    }

    #[test]
    fn alias_event() {
        let events = parse("*ref");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EventKind::Alias { name } if name == "ref"))
        );
    }

    #[test]
    fn tag_on_scalar() {
        let events = parse("!!str hello");
        let scalar = events
            .iter()
            .find(|e| matches!(e, EventKind::Scalar { .. }));
        assert!(matches!(
            scalar,
            Some(EventKind::Scalar { tag: Some(_), .. })
        ));
    }

    #[test]
    fn depth_limit() {
        let config = ParserConfig {
            limits: crate::limits::ResourceLimits {
                max_depth: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        // Deeply nested flow sequence
        let input = "[[[1]]]";
        let mut parser = Parser::with_config(input, config);
        let results: Vec<_> = parser.by_ref().collect();
        assert!(
            results.iter().any(|r| r.is_err()),
            "expected depth limit error"
        );
    }

    #[test]
    fn parser_stops_after_error() {
        let config = ParserConfig {
            limits: crate::limits::ResourceLimits {
                max_depth: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let input = "[[1]]";
        let mut parser = Parser::with_config(input, config);
        let results: Vec<_> = parser.by_ref().collect();
        let errors: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one error, got {}",
            errors.len()
        );
    }

    #[test]
    fn stream_start_and_end_always_present() {
        for input in ["hello", "a: 1", "- x", "[1]", "{a: 1}"] {
            let names = event_names(input);
            assert_eq!(
                names.first(),
                Some(&"stream-start"),
                "missing stream-start for: {input}"
            );
            assert_eq!(
                names.last(),
                Some(&"stream-end"),
                "missing stream-end for: {input}"
            );
        }
    }

    // ─── Tag (not anchor) on indentless block-sequence entry value (686) ─

    #[test]
    fn tagged_scalar_in_block_sequence_entry() {
        // `- !!str x`: a *tag* (not an anchor) as the first property token of a
        // block-sequence entry value drives the `Tag { .. }` arm of the
        // `has_properties` matches! in parse_node_indentless (line 686).
        let events = parse("- !!str x");
        assert!(matches!(
            events
                .iter()
                .find(|e| matches!(e, EventKind::Scalar { .. })),
            Some(EventKind::Scalar { tag: Some(_), .. })
        ));
    }

    #[test]
    fn tag_on_empty_block_sequence_entry_value() {
        // `- !!str\n- b`: a tag decorating an empty entry value, with the next
        // BlockEntry belonging to the parent sequence (Tag arm of 686 + empty
        // scalar branch).
        let events = parse("- !!str\n- b");
        assert!(events.iter().any(
            |e| matches!(e, EventKind::Scalar { tag: Some(_), value, .. } if value.is_empty())
        ));
    }

    // ─── Flow separator errors (990, 1073) ──────────────────────────

    #[test]
    fn flow_sequence_missing_comma_errors() {
        // `[[a] [b]]`: two nested flow sequences with no `,` separator. After
        // the first `[a]` closes, the next token is `[` rather than `,`/`]`,
        // driving the flow-sequence entry separator error ("',' or ']'").
        let err = parse_err("[[a] [b]]");
        match err.kind {
            ErrorKind::UnexpectedToken { expected, .. } => {
                assert!(expected.contains(']'), "got {expected}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn flow_mapping_missing_comma_errors() {
        // `{a: [1] b: 2}`: after the first pair's flow-sequence value closes,
        // the next token is a key rather than `,`/`}`, driving the flow-mapping
        // separator error ("',' or '}'").
        let err = parse_err("{a: [1] b: 2}");
        match err.kind {
            ErrorKind::UnexpectedToken { expected, .. } => {
                assert!(expected.contains('}'), "got {expected}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
