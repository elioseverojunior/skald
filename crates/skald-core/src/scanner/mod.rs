// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML scanner (lexer).
//!
//! Converts a byte stream into a sequence of [`Token`]s.
//! Handles UTF-8 BOM detection, character classification, indentation tracking,
//! and all YAML indicators.
//!
//! # Architecture
//!
//! The scanner is a stateful iterator that tracks:
//! - Current position in the input (byte offset, line, column)
//! - Indentation stack for emitting implicit block tokens
//! - Flow level for context-sensitive scanning inside `[]` and `{}`
//! - Simple key candidates for detecting implicit mapping keys

pub mod chars;
pub mod swar;
pub mod token;

pub use token::{Token, TokenKind};

use crate::error::{Error, ErrorKind, Result};
use crate::limits::ResourceLimits;
use crate::types::{Position, ScalarStyle, Span};
use std::borrow::Cow;
use std::collections::VecDeque;

/// Tracks a potential simple key location.
#[derive(Debug, Clone)]
struct SimpleKey {
    /// Whether a simple key is possible at this level.
    possible: bool,
    /// The token index where the simple key starts.
    token_index: usize,
    /// The position of the simple key.
    pos: Position,
}

/// The YAML scanner.
///
/// Converts input bytes into a stream of [`Token`]s. Use as an iterator
/// or call [`next_token`](Scanner::next_token) explicitly.
pub struct Scanner<'a> {
    input: &'a str,
    bytes: &'a [u8],

    // Current position
    offset: usize,
    line: u32,
    column: u32,

    // Token buffer
    tokens: VecDeque<Token<'a>>,
    tokens_produced: usize,

    // Indentation tracking
    indent: i32,
    indents: Vec<i32>,

    // Flow context depth (0 = block context)
    flow_level: u32,
    /// Whether each flow level is a sequence (true) or mapping (false).
    /// Index 0 corresponds to flow_level 1.
    flow_is_sequence: Vec<bool>,

    // Simple key tracking (one per indentation/flow level)
    simple_keys: Vec<SimpleKey>,

    // State flags
    stream_start_produced: bool,
    stream_end_produced: bool,
    allow_simple_key: bool,
    /// When a multi-line plain scalar ends at a ':' indicator, this is
    /// set to true. The ':' cannot create a valid mapping because the
    /// preceding multi-line scalar cannot serve as an implicit key and
    /// the ':' is not at the start of a line (YAML 1.2.2 §7.4).
    plain_scalar_colon_adjacent: bool,
    /// Set after a block indicator (-, ?, :) so that the next
    /// `skip_to_next_token` can record a tab on the same line.
    after_block_indicator: bool,
    /// Line on which `---` was emitted.  Block collections cannot start
    /// on the same line because `s-indent(n)` requires leading spaces
    /// from column 0, which `---` occupies (YAML 1.2.2 §9.1.4).
    document_start_line: u32,
    /// Line number where a tab was consumed in whitespace immediately
    /// after a block indicator.  Block indicators (`-`, `?`, `:`) on the
    /// **same** line are rejected because their indentation was
    /// established via a tab (YAML 1.2.2 §6.1).  Scalars are fine
    /// because they use `s-separate-in-line` which permits tabs.
    tab_after_indicator_line: u32,
    /// Minimum column for node properties (anchors/tags) following a
    /// block-context Value token.  Set to `indent + 1` after emitting
    /// Value in block context; cleared to -1 on the next non-property
    /// token dispatch (YAML 1.2.2 §8.2.1 — properties at n+1).
    value_property_min_column: i32,
    errored: bool,

    // Resource limits
    limits: ResourceLimits,

    // Trivia side channel — populated only when `preserve_trivia` is set.
    // Kept OUT of `tokens` so simple-key `token_index` accounting is unaffected.
    preserve_trivia: bool,
    trivia: VecDeque<Token<'a>>,
}

impl<'a> Scanner<'a> {
    /// Creates a new scanner for the given input.
    #[must_use]
    pub fn new(input: &'a str) -> Self {
        Self::with_limits(input, ResourceLimits::default())
    }

    /// Creates a new scanner with custom resource limits.
    #[must_use]
    pub fn with_limits(input: &'a str, limits: ResourceLimits) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            offset: 0,
            line: 1,
            column: 0,
            tokens: VecDeque::with_capacity(16),
            tokens_produced: 0,
            indent: -1,
            indents: Vec::with_capacity(8),
            flow_level: 0,
            flow_is_sequence: Vec::with_capacity(4),
            simple_keys: vec![SimpleKey {
                possible: false,
                token_index: 0,
                pos: Position::start(),
            }],
            stream_start_produced: false,
            stream_end_produced: false,
            allow_simple_key: true,
            plain_scalar_colon_adjacent: false,
            document_start_line: 0,
            after_block_indicator: false,
            tab_after_indicator_line: 0,
            value_property_min_column: -1,
            errored: false,
            limits,
            preserve_trivia: false,
            trivia: VecDeque::new(),
        }
    }

    /// Creates a scanner that also captures trivia (comments, whitespace, line
    /// breaks) into a side buffer. Used by the CST builder; the default
    /// constructors keep trivia off and behave identically to before.
    #[must_use]
    pub fn with_trivia(input: &'a str, limits: ResourceLimits, preserve_trivia: bool) -> Self {
        let mut s = Self::with_limits(input, limits);
        s.preserve_trivia = preserve_trivia;
        s
    }

    /// Returns whether trivia preservation is enabled.
    #[must_use]
    pub fn preserve_trivia(&self) -> bool {
        self.preserve_trivia
    }

    /// Test helper: drain the trivia side buffer into a `Vec`.
    /// Only available in tests; does not affect iterator state.
    #[cfg(test)]
    fn drain_trivia_for_test(&self) -> Vec<Token<'a>> {
        self.trivia.iter().cloned().collect()
    }

    /// Returns the next token, or `None` if the stream is exhausted.
    pub fn next_token(&mut self) -> Option<Result<Token<'a>>> {
        if self.errored {
            return None;
        }

        loop {
            // If the buffer has tokens, check whether we can safely return
            // the front token. A pending simple key whose token_index matches
            // the next-to-return position means the token stream might still
            // be reordered — we must fetch more input first.
            if !self.tokens.is_empty() && !self.has_pending_simple_key() {
                self.tokens_produced += 1;
                return self.tokens.pop_front().map(Ok);
            }

            if self.stream_end_produced && self.tokens.is_empty() {
                return None;
            }

            match self.fetch_next_token() {
                Ok(true) => { /* loop again — check if we can return now */ }
                Ok(false) => {
                    // No more input; drain remaining buffer. When the buffer is
                    // empty, loop instead of returning here: `stream_end_produced`
                    // is necessarily set by now, so the terminal check above
                    // returns `None`. This keeps the single source of truth for
                    // stream termination at one place.
                    if !self.tokens.is_empty() {
                        self.tokens_produced += 1;
                        return self.tokens.pop_front().map(Ok);
                    }
                }
                Err(e) => {
                    self.errored = true;
                    return Some(Err(e));
                }
            }
        }
    }

    /// Populate `self.tokens` until the front significant token is ready to be
    /// returned (i.e. no pending simple key blocks it), without popping it.
    ///
    /// Returns:
    /// - `Ok(true)`  — a significant token is now at the front of `self.tokens`
    /// - `Ok(false)` — stream is exhausted (`stream_end_produced` and buffer empty)
    /// - `Err(e)`    — scanner error; `self.errored` is set
    fn populate_significant_front(&mut self) -> Result<bool> {
        if self.errored {
            return Ok(false);
        }
        loop {
            if !self.tokens.is_empty() && !self.has_pending_simple_key() {
                return Ok(true);
            }
            if self.stream_end_produced && self.tokens.is_empty() {
                return Ok(false);
            }
            match self.fetch_next_token() {
                Ok(_) => { /* keep looping until front is ready */ }
                Err(e) => {
                    self.errored = true;
                    return Err(e);
                }
            }
        }
    }

    /// Iterator body used only when `preserve_trivia` is on.
    ///
    /// Ensures the significant-token front is populated, then compares its
    /// start offset against the trivia-front start offset. The token with the
    /// smaller (or equal) offset is yielded first, maintaining non-decreasing
    /// source order.  After the last significant token (StreamEnd / exhaustion),
    /// any remaining trivia is flushed before returning `None`.
    fn next_with_trivia(&mut self) -> Option<Result<Token<'a>>> {
        // Populate the significant buffer (non-destructively).
        let has_sig = match self.populate_significant_front() {
            Ok(v) => v,
            Err(e) => return Some(Err(e)),
        };

        let sig_offset = if has_sig {
            self.tokens
                .front()
                .map(|t| t.span.start.offset)
                .unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };

        let trivia_offset = self
            .trivia
            .front()
            .map(|t| t.span.start.offset)
            .unwrap_or(usize::MAX);

        // If trivia comes first (strictly before the significant token), yield it.
        if trivia_offset <= sig_offset && !self.trivia.is_empty() {
            return self.trivia.pop_front().map(Ok);
        }

        // Yield the significant token if available (accounting untouched).
        if has_sig {
            self.tokens_produced += 1;
            return self.tokens.pop_front().map(Ok);
        }

        // Stream exhausted: `has_sig` is false here, so `sig_offset == usize::MAX`
        // and the `trivia_offset <= sig_offset` guard above has already drained
        // every buffered trivia token. `skip_to_next_token` consumes (and buffers)
        // all trailing trivia *before* `fetch_stream_end` enqueues StreamEnd at the
        // EOF offset, so trailing trivia always carries a strictly smaller offset
        // and exits ahead of StreamEnd. Nothing can remain to flush.
        None
    }

    /// Returns `true` if any pending simple key refers to the next token
    /// position, meaning we must fetch more input before returning tokens.
    fn has_pending_simple_key(&self) -> bool {
        for key in &self.simple_keys {
            if key.possible && key.token_index == self.tokens_produced {
                return true;
            }
        }
        false
    }

    // ─── Position tracking ──────────────────────────────────────────

    fn pos(&self) -> Position {
        Position {
            offset: self.offset,
            line: self.line,
            column: self.column,
        }
    }

    fn span_from(&self, start: Position) -> Span {
        Span {
            start,
            end: self.pos(),
        }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(offset).copied()
    }

    #[inline]
    fn at_end(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    /// Advance past the byte at the current offset, updating the column.
    ///
    /// Line breaks are never consumed through this method: every break in the
    /// input is consumed via [`Self::skip_line`], which performs the line/column
    /// bookkeeping for newlines. Each call site guards the current byte against
    /// [`chars::is_break`] (or peeks a known non-break byte) before calling
    /// `advance`, so only the column advances here.
    fn advance(&mut self) {
        if self.offset < self.bytes.len() {
            self.offset += 1;
            self.column += 1;
        }
    }

    fn advance_n(&mut self, n: usize) {
        for _ in 0..n {
            self.advance();
        }
    }

    /// Push the current UTF-8 character to `dest` and advance past all its bytes.
    ///
    /// ASCII bytes (< 0x80) take the fast path. Multi-byte sequences are decoded
    /// directly from `self.input`, which is a `&str` (valid UTF-8 by
    /// construction), so the character at the current offset always exists.
    fn push_char_to(&mut self, dest: &mut String) {
        if let Some(b) = self.peek() {
            if b < 0x80 {
                dest.push(b as char);
                self.advance();
            } else if let Some(ch) = self.input[self.offset..].chars().next() {
                dest.push(ch);
                for _ in 0..ch.len_utf8() {
                    self.advance();
                }
            }
        }
    }

    /// Advances past one character (ASCII or multi-byte UTF-8) without copying
    /// it. The borrowed-scalar fast path in `fetch_plain_scalar` uses this in
    /// place of [`Self::push_char_to`]: the byte(s) stay part of the borrowable
    /// input slice, so no allocation is needed. A single uniform path advances
    /// over the whole character, whatever its byte width.
    fn skip_char(&mut self) {
        if let Some(ch) = self.input[self.offset..].chars().next() {
            for _ in 0..ch.len_utf8() {
                self.advance();
            }
        }
    }

    fn skip_line(&mut self) {
        if self.offset < self.bytes.len() {
            let b = self.bytes[self.offset];
            if b == b'\r' && self.peek_at(self.offset + 1) == Some(b'\n') {
                self.offset += 2;
            } else if b == b'\r' || b == b'\n' {
                self.offset += 1;
            }
            self.line += 1;
            self.column = 0;
        }
    }

    /// Records `[start_off, self.offset)` as a trivia token into the side
    /// buffer. Only active when `preserve_trivia` is set and the run is
    /// non-empty. The text is borrowed directly from the input so it is
    /// byte-exact and zero-copy.
    fn push_trivia(
        &mut self,
        start: Position,
        start_off: usize,
        make: impl FnOnce(Cow<'a, str>) -> TokenKind<'a>,
    ) {
        if !self.preserve_trivia || self.offset == start_off {
            return;
        }
        let span = self.span_from(start);
        let text = Cow::Borrowed(&self.input[start_off..self.offset]);
        self.trivia.push_back(Token {
            kind: make(text),
            span,
        });
    }

    // ─── Check limits ───────────────────────────────────────────────

    fn check_document_size(&self) -> Result<()> {
        if self.offset > self.limits.max_document_size {
            Err(Error::document_size_exceeded(
                &self.limits,
                Span::point(self.pos()),
            ))
        } else {
            Ok(())
        }
    }

    // ─── Token buffer management ────────────────────────────────────

    fn enqueue(&mut self, kind: TokenKind<'a>, span: Span) {
        self.tokens.push_back(Token { kind, span });
    }

    fn insert_at(&mut self, index: usize, kind: TokenKind<'a>, span: Span) {
        let buf_index = index.saturating_sub(self.tokens_produced);
        if buf_index <= self.tokens.len() {
            self.tokens.insert(buf_index, Token { kind, span });
        }
    }

    // ─── Main fetch loop ────────────────────────────────────────────

    fn fetch_next_token(&mut self) -> Result<bool> {
        if !self.stream_start_produced {
            self.fetch_stream_start();
            return Ok(true);
        }

        self.skip_to_next_token()?;
        self.check_document_size()?;
        self.stale_simple_keys()?;
        self.unroll_indent(self.column as i32);

        if self.at_end() {
            if !self.stream_end_produced {
                self.fetch_stream_end();
                return Ok(true);
            }
            return Ok(false);
        }

        let b = self.bytes[self.offset];

        // Check property indentation for block values (YAML §8.2.1).
        // After a block Value, node properties must be at column > indent.
        if self.value_property_min_column >= 0 {
            if (b == b'&' || b == b'!') && self.flow_level == 0 {
                if (self.column as i32) < self.value_property_min_column {
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: format!(
                                "node property indented past column {}",
                                self.value_property_min_column - 1
                            )
                            .into(),
                            found: format!("column {}", self.column).into(),
                        },
                        Span::point(self.pos()),
                    ));
                }
            } else if b != b'&' && b != b'!' {
                // Non-property token: clear the check.
                self.value_property_min_column = -1;
            }
        }

        // Document indicators: --- or ...
        if self.column == 0 {
            if b == b'-' && self.check_sequence(b"---") {
                return self.fetch_document_indicator(TokenKind::DocumentStart);
            }
            if b == b'.' && self.check_sequence(b"...") {
                return self.fetch_document_indicator(TokenKind::DocumentEnd);
            }
        }

        match b {
            // Flow indicators
            b'[' => self.fetch_flow_collection_start(TokenKind::FlowSequenceStart),
            b'{' => self.fetch_flow_collection_start(TokenKind::FlowMappingStart),
            b']' => self.fetch_flow_collection_end(TokenKind::FlowSequenceEnd),
            b'}' => self.fetch_flow_collection_end(TokenKind::FlowMappingEnd),
            b',' => self.fetch_flow_entry(),

            // Block entry
            b'-' if self.is_blank_or_break_at(self.offset + 1) && self.flow_level == 0 => {
                self.fetch_block_entry()
            }

            // Key
            b'?' if self.is_blank_or_break_at(self.offset + 1)
                || (self.flow_level > 0 && self.is_blank_break_or_flow_at(self.offset + 1)) =>
            {
                self.fetch_key()
            }

            // Value
            // In block context: colon followed by blank/break/EOF.
            // In flow context: colon followed by blank/break/EOF, flow indicator,
            //   or adjacent to a JSON-like key (pending simple key).
            b':' if self.is_blank_or_break_at(self.offset + 1)
                || (self.flow_level > 0
                    && (self.is_blank_break_or_flow_at(self.offset + 1)
                        || self.simple_keys.last().is_some_and(|k| k.possible))) =>
            {
                self.fetch_value()
            }

            // Anchor and alias
            b'*' => self.fetch_alias(),
            b'&' => self.fetch_anchor(),

            // Tag
            b'!' => self.fetch_tag(),

            // Literal and folded block scalars
            b'|' if self.flow_level == 0 => self.fetch_block_scalar(ScalarStyle::Literal),
            b'>' if self.flow_level == 0 => self.fetch_block_scalar(ScalarStyle::Folded),

            // Quoted scalars
            b'\'' => self.fetch_single_quoted_scalar(),
            b'"' => self.fetch_double_quoted_scalar(),

            // Directive
            b'%' if self.column == 0 => self.fetch_directive(),

            // Plain scalar (everything else that's not a special character in this context)
            _ => self.fetch_plain_scalar(),
        }
    }

    fn check_sequence(&self, expected: &[u8]) -> bool {
        if self.offset + expected.len() > self.bytes.len() {
            return false;
        }
        let slice = &self.bytes[self.offset..self.offset + expected.len()];
        if slice != expected {
            return false;
        }
        // Must be followed by blank/break or end of input
        self.is_blank_or_break_at(self.offset + expected.len())
    }

    fn is_blank_or_break_at(&self, offset: usize) -> bool {
        match self.bytes.get(offset) {
            Some(&b) => chars::is_blank_or_break(b),
            None => true, // end of input counts
        }
    }

    /// Check if the byte at `offset` is a blank, break, or flow indicator.
    /// In flow context, `:` is a value indicator only when followed by one of these.
    fn is_blank_break_or_flow_at(&self, offset: usize) -> bool {
        // End of input counts as a separator.
        self.bytes.get(offset).is_none_or(|&b| {
            chars::is_blank_or_break(b) || matches!(b, b',' | b'[' | b']' | b'{' | b'}')
        })
    }

    /// Check if current position begins a document indicator (`---` or `...`
    /// followed by blank, break, or EOF).
    fn is_document_indicator(&self) -> bool {
        if let Some(b) = self.peek() {
            (b == b'-' && self.check_sequence(b"---")) || (b == b'.' && self.check_sequence(b"..."))
        } else {
            false
        }
    }

    // ─── Skip whitespace and comments ───────────────────────────────

    fn skip_to_next_token(&mut self) -> Result<()> {
        let mut at_line_start = false;
        loop {
            let mut had_tab_at_flow_line_start = false;

            // Skip whitespace: spaces always, tabs as separation whitespace.
            // Note: tabs cannot be used for indentation per YAML spec, but
            // they are valid separation whitespace between tokens on a line.
            let ws_start = self.pos();
            let ws_off = self.offset;
            while let Some(b) = self.peek() {
                if b == b' ' {
                    self.advance();
                } else if b == b'\t' {
                    // In block context, tabs in the indentation zone
                    // (column <= current indent) are invalid (YAML 1.2.2 §6.1).
                    if at_line_start && self.flow_level == 0 && (self.column as i32) <= self.indent
                    {
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "spaces for indentation".into(),
                                found: "tab character".into(),
                            },
                            Span::point(self.pos()),
                        ));
                    }
                    // In flow context at line start, a tab before
                    // sufficient space-based indentation is invalid
                    // because s-indent(n) requires spaces only.
                    if at_line_start && self.flow_level > 0 && (self.column as i32) <= self.indent {
                        had_tab_at_flow_line_start = true;
                    }
                    // Record the line when a tab appears after a block
                    // indicator so we can reject block structures whose
                    // indentation was tab-determined.
                    if self.after_block_indicator && self.flow_level == 0 {
                        self.tab_after_indicator_line = self.line;
                    }
                    self.advance();
                } else {
                    break;
                }
            }
            self.push_trivia(ws_start, ws_off, TokenKind::Whitespace);
            // Reject tab-based indentation in flow context if followed
            // by content (not just an empty line).
            if had_tab_at_flow_line_start
                && let Some(b) = self.peek()
                && !chars::is_break(b)
            {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "spaces for indentation".into(),
                        found: "tab character".into(),
                    },
                    Span::point(self.pos()),
                ));
            }
            // Flow content on a new line must be indented past the
            // current block level (YAML 1.2.2 §8.1.1.2).  Flow
            // indicators (], }, ,) are exempt — they close or
            // continue the collection regardless of column.
            if at_line_start
                && self.flow_level > 0
                && let Some(b) = self.peek()
            {
                let is_flow_indicator = b == b']' || b == b'}' || b == b',';
                if !is_flow_indicator && !chars::is_break(b) && (self.column as i32) <= self.indent
                {
                    return Err(Error::new(
                        ErrorKind::UnexpectedToken {
                            expected: "proper indentation in flow content".into(),
                            found: format!("column {}", self.column).into(),
                        },
                        Span::point(self.pos()),
                    ));
                }
            }
            self.after_block_indicator = false;

            // Skip comments — YAML requires whitespace or start-of-line
            // before '#' for it to begin a comment (YAML 1.2.2 §6.6).
            // Check the byte immediately before the '#' in the input.
            if self.peek() == Some(b'#') {
                let preceded_by_whitespace =
                    self.offset == 0 || chars::is_blank_or_break(self.bytes[self.offset - 1]);
                if preceded_by_whitespace {
                    let c_start = self.pos();
                    let c_off = self.offset;
                    // SWAR: skip all bytes up to (but not including) the first
                    // line break.  Comments never span multiple lines so
                    // `column` advances by the same count and no newline
                    // bookkeeping is needed here.
                    let n = swar::find_line_end(self.bytes, self.offset);
                    self.offset += n;
                    self.column += n as u32;
                    self.push_trivia(c_start, c_off, TokenKind::Comment);
                }
            }

            // Skip line breaks
            if let Some(b) = self.peek()
                && chars::is_break(b)
            {
                let lb_start = self.pos();
                let lb_off = self.offset;
                self.skip_line();
                self.push_trivia(lb_start, lb_off, TokenKind::LineBreak);
                if self.flow_level == 0 {
                    self.allow_simple_key = true;
                }
                // A line break means we've moved to a new line; any
                // "colon-adjacent" flag from the previous line is no
                // longer relevant.
                self.plain_scalar_colon_adjacent = false;
                at_line_start = true;
                continue;
            }

            break;
        }
        Ok(())
    }

    // ─── Indentation ────────────────────────────────────────────────

    /// Opens a block collection at `column` if it is deeper than the current
    /// indent. All callers gate this on `flow_level == 0` (indentation is
    /// meaningless inside flow context), so no flow guard is needed here.
    fn roll_indent(&mut self, column: i32, kind: TokenKind<'a>, token_index: Option<usize>) {
        if self.indent < column {
            self.indents.push(self.indent);
            self.indent = column;
            let span = Span::point(self.pos());
            if let Some(idx) = token_index {
                self.insert_at(idx, kind, span);
            } else {
                self.enqueue(kind, span);
            }
        }
    }

    fn unroll_indent(&mut self, column: i32) {
        if self.flow_level > 0 {
            return;
        }
        while self.indent > column {
            let span = Span::point(self.pos());
            self.enqueue(TokenKind::BlockEnd, span);
            if let Some(prev) = self.indents.pop() {
                self.indent = prev;
            }
        }
    }

    // ─── Simple key tracking ────────────────────────────────────────

    fn save_simple_key(&mut self) {
        let possible = self.allow_simple_key;
        if possible {
            let key = SimpleKey {
                possible: true,
                token_index: self.tokens_produced + self.tokens.len(),
                pos: self.pos(),
            };
            self.remove_simple_key();
            if let Some(last) = self.simple_keys.last_mut() {
                *last = key;
            }
        }
    }

    fn remove_simple_key(&mut self) {
        if let Some(last) = self.simple_keys.last_mut() {
            last.possible = false;
        }
    }

    fn stale_simple_keys(&mut self) -> Result<()> {
        // Invalidate simple keys that have crossed a line boundary.
        let current_line = self.line;
        // Block level (index 0): implicit key must reside on a single line.
        if let Some(key) = self.simple_keys.first_mut()
            && key.possible
            && key.pos.line != current_line
        {
            key.possible = false;
        }
        // Flow sequence levels: implicit pair keys must be on a single
        // line (YAML 1.2.2 §7.4 ns-flow-pair).  Flow mapping keys
        // are allowed to span lines, so we only stale sequence-level keys.
        for (i, is_seq) in self.flow_is_sequence.iter().enumerate() {
            if *is_seq {
                let key_idx = i + 1; // simple_keys[0] is block level
                if let Some(key) = self.simple_keys.get_mut(key_idx)
                    && key.possible
                    && key.pos.line != current_line
                {
                    key.possible = false;
                }
            }
        }
        Ok(())
    }

    // ─── Stream start/end ───────────────────────────────────────────

    fn fetch_stream_start(&mut self) {
        self.stream_start_produced = true;

        // Skip UTF-8 BOM if present, emitting it as Whitespace trivia so that
        // lossless reconstruction covers the 3 BOM bytes.
        if self.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            let bom_start = self.pos();
            let bom_off = self.offset;
            self.offset += 3;
            self.push_trivia(bom_start, bom_off, TokenKind::Whitespace);
        }

        self.column = 0;
        let span = Span::point(self.pos());
        self.enqueue(TokenKind::StreamStart, span);
    }

    fn fetch_stream_end(&mut self) {
        // Unroll all remaining indentation
        self.unroll_indent(-1);
        self.remove_simple_key();
        self.allow_simple_key = false;
        self.stream_end_produced = true;
        let span = Span::point(self.pos());
        self.enqueue(TokenKind::StreamEnd, span);
    }

    // ─── Document indicators ────────────────────────────────────────

    fn fetch_document_indicator(&mut self, kind: TokenKind<'a>) -> Result<bool> {
        self.unroll_indent(-1);
        self.remove_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance_n(3);

        // After document-end '...', only whitespace, comments, and
        // line-breaks are allowed on the same line (YAML 1.2.2 §9.1.2).
        if matches!(kind, TokenKind::DocumentEnd) {
            while self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                self.advance();
            }
            if self.peek() == Some(b'#') {
                while let Some(b) = self.peek() {
                    if chars::is_break(b) {
                        break;
                    }
                    self.advance();
                }
            }
            if let Some(b) = self.peek()
                && !chars::is_break(b)
            {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "end of line after document end marker".into(),
                        found: format!("'{}'", b as char).into(),
                    },
                    self.span_from(start),
                ));
            }
        }

        let span = self.span_from(start);
        if matches!(kind, TokenKind::DocumentStart) {
            self.document_start_line = start.line;
        }
        self.enqueue(kind, span);
        Ok(true)
    }

    // ─── Flow collections ───────────────────────────────────────────

    fn fetch_flow_collection_start(&mut self, kind: TokenKind<'a>) -> Result<bool> {
        self.save_simple_key();
        self.flow_level += 1;
        self.flow_is_sequence
            .push(matches!(kind, TokenKind::FlowSequenceStart));
        self.allow_simple_key = true;
        self.simple_keys.push(SimpleKey {
            possible: false,
            token_index: 0,
            pos: self.pos(),
        });

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(kind, span);
        Ok(true)
    }

    fn fetch_flow_collection_end(&mut self, kind: TokenKind<'a>) -> Result<bool> {
        self.remove_simple_key();
        if self.flow_level > 0 {
            self.flow_level -= 1;
            self.flow_is_sequence.pop();
            self.simple_keys.pop();
        }
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(kind, span);
        Ok(true)
    }

    fn fetch_flow_entry(&mut self) -> Result<bool> {
        self.remove_simple_key();
        self.allow_simple_key = true;

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(TokenKind::FlowEntry, span);
        Ok(true)
    }

    // ─── Block entry ────────────────────────────────────────────────

    fn fetch_block_entry(&mut self) -> Result<bool> {
        // Reject block entry whose indentation was established via a tab
        // after a preceding block indicator on the same line.
        if self.flow_level == 0 && self.tab_after_indicator_line == self.line {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "spaces for indentation".into(),
                    found: "tab character before block entry".into(),
                },
                Span::point(self.pos()),
            ));
        }
        // Block collections cannot start on the `---` line.
        if self.flow_level == 0 && self.document_start_line == self.line && self.column > 0 {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "block content at column 0".into(),
                    found: "block entry on document start line".into(),
                },
                Span::point(self.pos()),
            ));
        }
        if self.flow_level == 0 {
            if !self.allow_simple_key {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "block content".into(),
                        found: "block entry '-'".into(),
                    },
                    Span::point(self.pos()),
                ));
            }
            self.roll_indent(self.column as i32, TokenKind::BlockSequenceStart, None);
        }
        self.remove_simple_key();
        self.allow_simple_key = true;

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(TokenKind::BlockEntry, span);
        self.after_block_indicator = self.flow_level == 0;
        Ok(true)
    }

    // ─── Key and Value ──────────────────────────────────────────────

    fn fetch_key(&mut self) -> Result<bool> {
        if self.flow_level == 0 && self.tab_after_indicator_line == self.line {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "spaces for indentation".into(),
                    found: "tab character before key indicator".into(),
                },
                Span::point(self.pos()),
            ));
        }
        if self.flow_level == 0 {
            if !self.allow_simple_key {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "block content".into(),
                        found: "key '?'".into(),
                    },
                    Span::point(self.pos()),
                ));
            }
            self.roll_indent(self.column as i32, TokenKind::BlockMappingStart, None);
        }
        self.remove_simple_key();
        self.allow_simple_key = self.flow_level == 0;

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(TokenKind::Key, span);
        self.after_block_indicator = self.flow_level == 0;
        Ok(true)
    }

    fn fetch_value(&mut self) -> Result<bool> {
        if self.flow_level == 0 && self.tab_after_indicator_line == self.line {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "spaces for indentation".into(),
                    found: "tab character before value indicator".into(),
                },
                Span::point(self.pos()),
            ));
        }

        // Check if this is a value for a simple key
        if let Some(key) = self.simple_keys.last().cloned()
            && key.possible
        {
            let key_span = Span::point(key.pos);
            self.insert_at(key.token_index, TokenKind::Key, key_span);

            // Roll indent for the simple key. A block mapping cannot begin on
            // the `---` line, but that case never reaches this simple-key
            // branch: forming a *possible* simple key at column > 0 on the
            // document-start line would require `allow_simple_key` to be set
            // after `---`, yet the only block tokens that set it (`-`, `?`)
            // are rejected first by the document-start-line guards in
            // `fetch_block_entry` / `fetch_key`. The reachable
            // mapping-on-`---` case is handled by the complex value path below.
            if self.flow_level == 0 {
                self.roll_indent(
                    key.pos.column as i32,
                    TokenKind::BlockMappingStart,
                    Some(key.token_index),
                );
            }

            self.remove_simple_key();
            self.allow_simple_key = false;

            let start = self.pos();
            self.advance();
            let span = self.span_from(start);
            self.enqueue(TokenKind::Value, span);
            self.after_block_indicator = self.flow_level == 0;
            if self.flow_level == 0 {
                self.value_property_min_column = self.indent + 1;
            }
            return Ok(true);
        }

        // Otherwise this is a complex value
        if self.flow_level == 0 {
            if !self.allow_simple_key || self.plain_scalar_colon_adjacent {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "block content".into(),
                        found: "value ':'".into(),
                    },
                    Span::point(self.pos()),
                ));
            }
            // Block mappings cannot start on the `---` line.
            if self.document_start_line == self.line && self.column > 0 {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "block content at column 0".into(),
                        found: "mapping on document start line".into(),
                    },
                    Span::point(self.pos()),
                ));
            }
            self.roll_indent(self.column as i32, TokenKind::BlockMappingStart, None);
        }
        self.remove_simple_key();
        self.allow_simple_key = self.flow_level == 0;

        let start = self.pos();
        self.advance();
        let span = self.span_from(start);
        self.enqueue(TokenKind::Value, span);
        self.after_block_indicator = self.flow_level == 0;
        if self.flow_level == 0 {
            self.value_property_min_column = self.indent + 1;
        }
        Ok(true)
    }

    // ─── Anchor and Alias ───────────────────────────────────────────

    fn fetch_anchor(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;
        self.scan_anchor_or_alias(true)
    }

    fn fetch_alias(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;
        self.scan_anchor_or_alias(false)
    }

    fn scan_anchor_or_alias(&mut self, is_anchor: bool) -> Result<bool> {
        let start = self.pos();
        self.advance(); // skip & or *

        let name_start = self.offset;
        while let Some(b) = self.peek() {
            // Anchor names: ns-anchor-char = ns-char - c-flow-indicator
            // Stop at whitespace, flow indicators, or EOF.
            // Allow multi-byte UTF-8 continuation bytes (>= 0x80).
            if chars::is_blank_or_break(b) || matches!(b, b'[' | b']' | b'{' | b'}' | b',') {
                break;
            }
            self.advance();
        }

        let name = &self.input[name_start..self.offset];
        if name.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidAnchor("empty name".to_string()),
                self.span_from(start),
            ));
        }

        let span = self.span_from(start);
        let kind = if is_anchor {
            TokenKind::Anchor(Cow::Borrowed(name))
        } else {
            TokenKind::Alias(Cow::Borrowed(name))
        };
        self.enqueue(kind, span);
        Ok(true)
    }

    // ─── Tag ────────────────────────────────────────────────────────

    fn fetch_tag(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance(); // skip !

        let (handle, suffix) = if self.peek() == Some(b'<') {
            // Verbatim tag: !<uri> — use empty handle to distinguish from primary !suffix
            self.advance(); // skip <
            let uri_start = self.offset;
            while let Some(b) = self.peek() {
                if b == b'>' {
                    break;
                }
                self.advance();
            }
            let suffix = Cow::Borrowed(&self.input[uri_start..self.offset]);
            if self.peek() == Some(b'>') {
                self.advance();
            }
            (Cow::Borrowed(""), suffix)
        } else if self.peek() == Some(b'!') {
            // Secondary tag: !!suffix
            self.advance();
            let suffix_start = self.offset;
            while let Some(b) = self.peek() {
                if chars::is_blank_or_break(b) || chars::is_flow(b) {
                    break;
                }
                self.advance();
            }
            (
                Cow::Borrowed("!!"),
                Self::percent_decode(&self.input[suffix_start..self.offset]),
            )
        } else {
            // Primary tag: !suffix or named handle !handle!suffix
            let suffix_start = self.offset;
            while let Some(b) = self.peek() {
                if chars::is_blank_or_break(b) || chars::is_flow(b) {
                    break;
                }
                if b == b'!' && self.offset > suffix_start {
                    // Named handle: everything before ! is the handle
                    let handle = Cow::Borrowed(&self.input[start.offset..=self.offset]);
                    self.advance();
                    let s_start = self.offset;
                    while let Some(b2) = self.peek() {
                        if chars::is_blank_or_break(b2) || chars::is_flow(b2) {
                            break;
                        }
                        self.advance();
                    }
                    let suffix = Self::percent_decode(&self.input[s_start..self.offset]);
                    let span = self.span_from(start);
                    self.enqueue(TokenKind::Tag { handle, suffix }, span);
                    return Ok(true);
                }
                self.advance();
            }
            (
                Cow::Borrowed("!"),
                Self::percent_decode(&self.input[suffix_start..self.offset]),
            )
        };

        // A tag must be followed by whitespace, line break, or EOF.
        // Non-whitespace after the tag suffix (e.g., `!tag{`) means
        // the tag contains invalid characters (YAML 1.2.2 §6.8.2).
        if let Some(b) = self.peek()
            && !chars::is_blank_or_break(b)
            && b != b','
        {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "whitespace after tag".into(),
                    found: format!("'{}'", b as char).into(),
                },
                self.span_from(start),
            ));
        }

        let span = self.span_from(start);
        self.enqueue(TokenKind::Tag { handle, suffix }, span);
        Ok(true)
    }

    /// Decode percent-encoded characters in a tag suffix (e.g., `%21` → `!`).
    fn percent_decode(s: &str) -> Cow<'_, str> {
        if !s.contains('%') {
            return Cow::Borrowed(s);
        }
        let mut result = String::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    result.push((h << 4 | l) as u8 as char);
                    i += 3;
                    continue;
                }
            }
            result.push(bytes[i] as char);
            i += 1;
        }
        Cow::Owned(result)
    }

    // ─── Block Scalar ───────────────────────────────────────────────

    fn fetch_block_scalar(&mut self, style: ScalarStyle) -> Result<bool> {
        self.remove_simple_key();
        self.allow_simple_key = true;

        let start = self.pos();
        self.advance(); // skip | or >

        // Parse chomping indicator and indentation indicator
        let mut chomping = 0i8; // -1 = strip, 0 = clip (default), 1 = keep
        let mut explicit_indent: Option<u32> = None;

        // Check for chomping/indent indicators
        for _ in 0..2 {
            if let Some(b) = self.peek() {
                if b == b'+' {
                    chomping = 1;
                    self.advance();
                } else if b == b'-' {
                    chomping = -1;
                    self.advance();
                } else if b.is_ascii_digit() && b != b'0' {
                    explicit_indent = Some((b - b'0') as u32);
                    self.advance();
                }
            }
        }

        // After indicators, only whitespace or break are valid.
        // A comment '#' is allowed only after whitespace (YAML 1.2.2 §8.1).
        if let Some(b) = self.peek()
            && !chars::is_blank_or_break(b)
        {
            return Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "valid block scalar header".into(),
                    found: format!("'{}'", b as char).into(),
                },
                self.span_from(start),
            ));
        }

        // Skip trailing whitespace and comment on the header line
        while self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
            self.advance();
        }
        if self.peek() == Some(b'#') {
            while let Some(b) = self.peek() {
                if chars::is_break(b) {
                    break;
                }
                self.advance();
            }
        }

        // Consume the line break after the header
        if let Some(b) = self.peek()
            && chars::is_break(b)
        {
            self.skip_line();
        }

        // Determine indentation
        let block_indent = if let Some(n) = explicit_indent {
            self.indent.max(0) as u32 + n
        } else {
            // Auto-detect: find first non-empty line (one with non-space,
            // non-break content). Track the max indent seen on space-only
            // lines — if no real content line exists, space-only lines
            // determine the indent so their spaces are consumed as indent
            // rather than treated as content.
            let mut auto_indent = 0u32;
            let mut max_space_only = 0u32;
            let mut probe = self.offset;
            while probe < self.bytes.len() {
                let b = self.bytes[probe];
                if b == b' ' {
                    auto_indent += 1;
                    probe += 1;
                } else if chars::is_break(b) {
                    // Space-only or fully-empty line
                    if auto_indent > 0 {
                        max_space_only = max_space_only.max(auto_indent);
                    }
                    auto_indent = 0;
                    if b == b'\r' && self.peek_at(probe + 1) == Some(b'\n') {
                        probe += 2;
                    } else {
                        probe += 1;
                    }
                } else {
                    break;
                }
            }
            // The probe found a real content line if it stopped on a
            // non-space non-break byte (auto_indent > 0 indicates leading
            // spaces were counted on that line).
            let found_content = probe < self.bytes.len() && !chars::is_break(self.bytes[probe]);

            // When only space-only lines exist (no real content), use their
            // indent so the spaces aren't mistaken for content.
            if auto_indent == 0 && max_space_only > 0 {
                auto_indent = max_space_only;
            }

            // If a content line was found but preceding space-only lines
            // have MORE spaces than the content indentation, the scalar is
            // invalid: those lines would be "more indented" content that
            // comes before the line that established the indent, which is
            // ambiguous (YAML 1.2.2 §8.1.3).
            if found_content && max_space_only > auto_indent {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: format!("indentation of at most {auto_indent} spaces").into(),
                        found: format!("line with {max_space_only} spaces before content").into(),
                    },
                    Span::point(self.pos()),
                ));
            }

            auto_indent.max((self.indent + 1).max(0) as u32)
        };

        // Collect the block scalar content
        let mut content = String::with_capacity(256);
        let mut trailing_breaks = String::with_capacity(16);
        let mut prev_more_indented = false;

        loop {
            // Check for document indicators at start of line
            if self.column == 0
                && let Some(b) = self.peek()
                && ((b == b'-' && self.check_sequence(b"---"))
                    || (b == b'.' && self.check_sequence(b"...")))
            {
                break;
            }

            // Count leading spaces
            let mut current_indent = 0u32;
            while self.peek() == Some(b' ') && current_indent < block_indent {
                self.advance();
                current_indent += 1;
            }

            // Tabs in the indentation zone are invalid (YAML 1.2.2 §6.1).
            if self.peek() == Some(b'\t') && current_indent < block_indent {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "spaces for indentation".into(),
                        found: "tab character".into(),
                    },
                    Span::point(self.pos()),
                ));
            }

            // Check if this line is part of the block
            if current_indent < block_indent
                && !self.at_end()
                && let Some(b) = self.peek()
                && !chars::is_break(b)
            {
                break;
            }

            // Check for end of input
            if self.at_end() {
                // If we consumed full indentation, treat EOF as an implicit
                // line break — this empty trailing line produces a break that
                // chomping can act on (YAML treats EOF as a line break).
                if current_indent >= block_indent {
                    trailing_breaks.push('\n');
                }
                break;
            }

            // Handle empty/less-indented lines
            if let Some(b) = self.peek()
                && chars::is_break(b)
            {
                trailing_breaks.push('\n');
                self.skip_line();
                continue;
            }

            // Determine if current line is "more indented" (extra whitespace beyond block_indent)
            let current_more_indented = self.peek() == Some(b' ') || self.peek() == Some(b'\t');

            // We have a content line — flush trailing breaks.
            // For folded scalars, line folding rules (YAML spec §8.2.1):
            //   - Single break between normal lines → fold to space
            //   - Single break adjacent to more-indented → preserve as \n
            //   - Multiple breaks: if previous line was normal, the first
            //     break is the line-ending (consumed); rest are empty lines
            //   - Multiple breaks: if previous line was more-indented,
            //     all breaks are preserved
            if style == ScalarStyle::Folded && !content.is_empty() {
                if trailing_breaks == "\n" {
                    // Single break
                    if !prev_more_indented && !current_more_indented {
                        // Normal → Normal: fold to space
                        content.push(' ');
                    } else {
                        // Adjacent to more-indented: preserve break
                        content.push('\n');
                    }
                } else if prev_more_indented || current_more_indented {
                    // More-indented: preserve all breaks
                    content.push_str(&trailing_breaks);
                } else {
                    // Normal → Normal with empty lines: strip line-ending, keep empty lines
                    content.push_str(&trailing_breaks[1..]);
                }
            } else {
                content.push_str(&trailing_breaks);
            }
            trailing_breaks.clear();

            // Read the content of this line
            while let Some(b) = self.peek() {
                if chars::is_break(b) {
                    break;
                }
                self.push_char_to(&mut content);
            }

            prev_more_indented = current_more_indented;

            // Consume the line break
            if let Some(b) = self.peek()
                && chars::is_break(b)
            {
                trailing_breaks.push('\n');
                self.skip_line();
            }
        }

        // Apply chomping
        match chomping {
            -1 => { /* strip: remove all trailing breaks */ }
            0 => {
                // clip: keep one trailing newline only if there was content
                if !content.is_empty() {
                    content.push('\n');
                }
            }
            _ => {
                // keep: keep all trailing breaks
                content.push_str(&trailing_breaks);
            }
        }

        let span = self.span_from(start);
        self.enqueue(
            TokenKind::Scalar {
                value: Cow::Owned(content),
                style,
            },
            span,
        );
        Ok(true)
    }

    // ─── Quoted Scalars ─────────────────────────────────────────────

    fn fetch_single_quoted_scalar(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance(); // skip opening '

        let mut content = String::with_capacity(64);
        let mut needs_owned = false;

        loop {
            match self.peek() {
                None => {
                    return Err(Error::new(ErrorKind::UnexpectedEof, self.span_from(start)));
                }
                Some(b'\'') => {
                    self.advance();
                    // Escaped single quote ''
                    if self.peek() == Some(b'\'') {
                        content.push('\'');
                        needs_owned = true;
                        self.advance();
                    } else {
                        break;
                    }
                }
                Some(b) if chars::is_break(b) => {
                    needs_owned = true;
                    // Trim trailing whitespace from current content
                    while content.ends_with(' ') || content.ends_with('\t') {
                        content.pop();
                    }
                    self.skip_line();
                    // Count empty lines and skip leading whitespace
                    let mut extra_breaks = String::with_capacity(8);
                    while let Some(b2) = self.peek() {
                        if b2 == b' ' || b2 == b'\t' {
                            self.advance();
                        } else if chars::is_break(b2) {
                            extra_breaks.push('\n');
                            self.skip_line();
                        } else {
                            break;
                        }
                    }
                    // Document indicators at line start terminate even inside
                    // quoted scalars (YAML 1.2.2 §9.1.2).
                    if self.column == 0 && self.is_document_indicator() {
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "closing single quote".into(),
                                found: "document indicator".into(),
                            },
                            self.span_from(start),
                        ));
                    }
                    if extra_breaks.is_empty() {
                        content.push(' ');
                    } else {
                        content.push_str(&extra_breaks);
                    }
                }
                Some(_) => {
                    self.push_char_to(&mut content);
                }
            }
        }

        let span = self.span_from(start);
        // When no `''` escape or line-folding occurred, `content` is byte-for-byte
        // the slice between the quotes, so we can borrow it zero-copy.
        let value = if needs_owned {
            Cow::Owned(content)
        } else {
            Cow::Borrowed(&self.input[start.offset + 1..self.offset - 1])
        };

        self.enqueue(
            TokenKind::Scalar {
                value,
                style: ScalarStyle::SingleQuoted,
            },
            span,
        );
        Ok(true)
    }

    fn fetch_double_quoted_scalar(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance(); // skip opening "

        let mut content = String::with_capacity(64);
        // Track literal trailing whitespace separately so escape-produced
        // characters are never trimmed on line breaks.
        let mut trailing_ws = String::with_capacity(8);

        loop {
            match self.peek() {
                None => {
                    return Err(Error::new(ErrorKind::UnexpectedEof, self.span_from(start)));
                }
                Some(b'"') => {
                    content.push_str(&trailing_ws);
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    content.push_str(&trailing_ws);
                    trailing_ws.clear();
                    self.advance();
                    match self.peek() {
                        None => {
                            return Err(Error::new(
                                ErrorKind::UnexpectedEof,
                                self.span_from(start),
                            ));
                        }
                        Some(b'0') => {
                            content.push('\0');
                            self.advance();
                        }
                        Some(b'a') => {
                            content.push('\x07');
                            self.advance();
                        }
                        Some(b'b') => {
                            content.push('\x08');
                            self.advance();
                        }
                        Some(b't') | Some(b'\t') => {
                            content.push('\t');
                            self.advance();
                        }
                        Some(b'n') => {
                            content.push('\n');
                            self.advance();
                        }
                        Some(b'v') => {
                            content.push('\x0B');
                            self.advance();
                        }
                        Some(b'f') => {
                            content.push('\x0C');
                            self.advance();
                        }
                        Some(b'r') => {
                            content.push('\r');
                            self.advance();
                        }
                        Some(b'e') => {
                            content.push('\x1B');
                            self.advance();
                        }
                        Some(b' ') => {
                            content.push(' ');
                            self.advance();
                        }
                        Some(b'"') => {
                            content.push('"');
                            self.advance();
                        }
                        Some(b'/') => {
                            content.push('/');
                            self.advance();
                        }
                        Some(b'\\') => {
                            content.push('\\');
                            self.advance();
                        }
                        Some(b'N') => {
                            content.push('\u{0085}');
                            self.advance();
                        }
                        Some(b'_') => {
                            content.push('\u{00A0}');
                            self.advance();
                        }
                        Some(b'L') => {
                            content.push('\u{2028}');
                            self.advance();
                        }
                        Some(b'P') => {
                            content.push('\u{2029}');
                            self.advance();
                        }
                        Some(b'x') => {
                            self.advance();
                            let ch = self.scan_hex_escape(2, start)?;
                            content.push(ch);
                        }
                        Some(b'u') => {
                            self.advance();
                            let ch = self.scan_hex_escape(4, start)?;
                            content.push(ch);
                        }
                        Some(b'U') => {
                            self.advance();
                            let ch = self.scan_hex_escape(8, start)?;
                            content.push(ch);
                        }
                        Some(b) if chars::is_break(b) => {
                            // Escaped line break: skip it and leading whitespace
                            self.skip_line();
                            while let Some(b2) = self.peek() {
                                if b2 == b' ' || b2 == b'\t' {
                                    self.advance();
                                } else {
                                    break;
                                }
                            }
                        }
                        Some(b) => {
                            return Err(Error::new(
                                ErrorKind::InvalidEscape(b as char),
                                self.span_from(start),
                            ));
                        }
                    }
                }
                Some(b) if chars::is_break(b) => {
                    // Discard literal trailing whitespace (tracked in trailing_ws)
                    trailing_ws.clear();
                    self.skip_line();
                    // Fold line break: count empty lines and skip leading whitespace
                    let mut extra_breaks = String::with_capacity(8);
                    while let Some(b2) = self.peek() {
                        if b2 == b' ' {
                            self.advance();
                        } else if b2 == b'\t' {
                            // Tabs in the indentation zone are invalid
                            if self.flow_level == 0 && (self.column as i32) <= self.indent {
                                return Err(Error::new(
                                    ErrorKind::UnexpectedToken {
                                        expected: "spaces for indentation".into(),
                                        found: "tab character".into(),
                                    },
                                    self.span_from(start),
                                ));
                            }
                            self.advance();
                        } else if chars::is_break(b2) {
                            extra_breaks.push('\n');
                            self.skip_line();
                        } else {
                            break;
                        }
                    }
                    // Document indicators at line start terminate even inside
                    // quoted scalars (YAML 1.2.2 §9.1.2).
                    if self.column == 0 && self.is_document_indicator() {
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "closing double quote".into(),
                                found: "document indicator".into(),
                            },
                            self.span_from(start),
                        ));
                    }
                    // Flow scalar content must be indented past the containing
                    // collection (YAML 1.2.2 §8.1.1.2).
                    if self.flow_level == 0 && (self.column as i32) <= self.indent {
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "proper indentation in quoted scalar".into(),
                                found: format!("column {}", self.column).into(),
                            },
                            self.span_from(start),
                        ));
                    }
                    if extra_breaks.is_empty() {
                        content.push(' ');
                    } else {
                        content.push_str(&extra_breaks);
                    }
                }
                Some(b) if b == b' ' || b == b'\t' => {
                    // Literal whitespace — buffer it in case a line break follows
                    trailing_ws.push(b as char);
                    self.advance();
                }
                Some(_) => {
                    content.push_str(&trailing_ws);
                    trailing_ws.clear();
                    self.push_char_to(&mut content);
                }
            }
        }

        let span = self.span_from(start);
        self.enqueue(
            TokenKind::Scalar {
                value: Cow::Owned(content),
                style: ScalarStyle::DoubleQuoted,
            },
            span,
        );
        Ok(true)
    }

    fn scan_hex_escape(&mut self, digits: usize, start: Position) -> Result<char> {
        let mut code: u32 = 0;
        for _ in 0..digits {
            // `to_digit(16)` validates the byte is a hex digit and yields its
            // value in one step, returning `None` for anything else.
            match self.peek().and_then(|b| (b as char).to_digit(16)) {
                Some(value) => {
                    code = code * 16 + value;
                    self.advance();
                }
                None => {
                    return Err(Error::new(
                        ErrorKind::InvalidEscape('x'),
                        self.span_from(start),
                    ));
                }
            }
        }
        char::from_u32(code)
            .ok_or_else(|| Error::new(ErrorKind::InvalidEscape('u'), self.span_from(start)))
    }

    // ─── Plain Scalar ───────────────────────────────────────────────

    fn fetch_plain_scalar(&mut self) -> Result<bool> {
        self.save_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();

        // Validate ns-plain-first: c-indicator chars (-, ?, :) can start a
        // plain scalar only if followed by ns-plain-safe (YAML 1.2.2 §7.3.3).
        if let Some(b) = self.peek()
            && matches!(b, b'-' | b'?' | b':')
        {
            let next_safe = self.peek_at(self.offset + 1).is_some_and(|b2| {
                !chars::is_blank_or_break(b2) && (self.flow_level == 0 || !chars::is_flow(b2))
            });
            if !next_safe {
                return Err(Error::new(
                    ErrorKind::UnexpectedToken {
                        expected: "scalar value".into(),
                        found: format!("'{}'", b as char).into(),
                    },
                    self.span_from(start),
                ));
            }
        }

        // `content` is materialized lazily: while the scalar is fully
        // borrowable (no line folding or whitespace trimming that rewrites the
        // bytes), the span is tracked by offset (`content_start..value_end`) and
        // nothing is allocated — the common case for plain scalars. The String
        // is created only when the first line break forces a transformation.
        // `whitespace`/`leading_break`/`trailing_breaks` start empty (no heap
        // allocation until pushed) and are only used on the owned/folding path.
        let content_start = self.offset;
        let mut value_end = self.offset;
        let mut content: Option<String> = None;
        let mut whitespace = String::new();
        let mut leading_break = String::new();
        let mut trailing_breaks = String::new();
        let mut all_borrowed = true;
        let mut last_content_line = self.line;

        loop {
            if self.at_end() {
                break;
            }

            // Check for document indicators at start of line
            if self.column == 0
                && let Some(b) = self.peek()
                && ((b == b'-' && self.check_sequence(b"---"))
                    || (b == b'.' && self.check_sequence(b"...")))
            {
                break;
            }

            // Check for comment: `#` is a comment only when preceded by
            // whitespace/a line break, or at the very start (no content yet).
            // `self.offset > value_end` means blanks/breaks have been consumed
            // since the last content byte — true in both the borrowed and owned
            // paths (whitespace is no longer buffered on the borrowed path).
            let content_empty = content
                .as_ref()
                .map_or(value_end == content_start, |c| c.is_empty());
            if self.peek() == Some(b'#') && (self.offset > value_end || content_empty) {
                break;
            }

            // `at_end()` was checked above and nothing has advanced the offset
            // since, so the current byte is guaranteed to be in bounds.
            let b = self.bytes[self.offset];

            // Check for end of plain scalar
            if chars::is_blank_or_break(b) {
                // Collect whitespace. In borrowed mode it is captured implicitly
                // by the contiguous input slice (and trailing whitespace is left
                // out of `value_end`), so only the owned/folding path buffers it.
                if chars::is_whitespace(b) {
                    if !all_borrowed && leading_break.is_empty() && trailing_breaks.is_empty() {
                        whitespace.push(b as char);
                    }
                    self.advance();
                    continue;
                }

                // Line break
                if chars::is_break(b) {
                    // First break forces ownership: materialize `content` from
                    // the borrowable prefix scanned so far (trailing whitespace
                    // excluded), then fold line breaks as usual.
                    if content.is_none() {
                        content = Some(self.input[content_start..value_end].to_owned());
                    }
                    all_borrowed = false;
                    if whitespace.is_empty() && leading_break.is_empty() {
                        leading_break.push('\n');
                    } else if !leading_break.is_empty() {
                        trailing_breaks.push('\n');
                    } else {
                        leading_break.push('\n');
                        whitespace.clear();
                    }
                    self.skip_line();

                    // Skip leading whitespace on next line
                    while self.peek() == Some(b' ') {
                        self.advance();
                    }

                    // In block context, stop if the next non-empty line is
                    // not indented past the current block collection indent.
                    // Empty lines (next char is a break) are allowed inside
                    // plain scalars — defer the indent check until we find a
                    // line with actual content (YAML 1.2.2 §8.1.3).
                    if self.flow_level == 0
                        && (self.column as i32) <= self.indent
                        && !self.peek().is_some_and(chars::is_break)
                    {
                        break;
                    }
                    continue;
                }
            }

            // Check for indicators that end a plain scalar
            if b == b':'
                && (self.is_blank_or_break_at(self.offset + 1)
                    || (self.flow_level > 0 && self.is_blank_break_or_flow_at(self.offset + 1)))
            {
                break;
            }
            if self.flow_level > 0 && chars::is_flow(b) {
                break;
            }

            // Flush accumulated whitespace/breaks into the owned buffer. Only
            // reachable on the owned path (borrowed mode leaves these empty), so
            // `content` is always `Some` here when there is anything to flush.
            if let Some(content) = content.as_mut() {
                if !leading_break.is_empty() {
                    if trailing_breaks.is_empty() {
                        content.push(' ');
                    } else {
                        content.push_str(&trailing_breaks);
                        trailing_breaks.clear();
                    }
                    leading_break.clear();
                } else if !whitespace.is_empty() {
                    content.push_str(&whitespace);
                    whitespace.clear();
                }
            }

            last_content_line = self.line;

            // SWAR fast-path: scan forward 8 bytes at a time to find the next
            // structural character, then bulk-append the safe run.
            //
            // Preconditions at this point:
            //   - current byte is a valid content byte (not structural)
            //   - self.input is valid UTF-8; non-ASCII bytes (>= 0x80) are
            //     multi-byte UTF-8 continuations which never match our ASCII
            //     stop set, so they pass through the SWAR scan correctly.
            //   - column advances by byte count (same as the existing
            //     push_char_to path which calls advance() once per byte).
            {
                let n = if self.flow_level == 0 {
                    swar::find_plain_scalar_end_block(self.bytes, self.offset)
                } else {
                    swar::find_plain_scalar_end_flow(self.bytes, self.offset)
                };

                if let Some(content) = content.as_mut() {
                    // Owned mode: copy the run into the buffer.
                    if n > 1 {
                        // Bulk-append the run. It is guaranteed to be valid UTF-8
                        // (substring of self.input) and to contain no newlines.
                        let run_end = self.offset + n;
                        content.push_str(&self.input[self.offset..run_end]);
                        self.offset = run_end;
                        self.column += n as u32;
                    } else {
                        // Single byte or immediate stop — fall back to
                        // push_char_to for multi-byte UTF-8 and edge cases.
                        self.push_char_to(content);
                    }
                } else if n > 1 {
                    // Borrowed mode: advance over the run without copying; the
                    // bytes stay part of the borrowable `content_start..value_end`
                    // slice.
                    self.offset += n;
                    self.column += n as u32;
                } else {
                    // Single byte / multi-byte: advance without copying.
                    self.skip_char();
                }
                value_end = self.offset;
            }
        }

        // A multi-line plain scalar that ended at a ':' indicator means
        // the ':' cannot form a valid mapping — the scalar cannot serve
        // as an implicit key (YAML 1.2.2 §7.4), and the ':' is adjacent
        // to the scalar's content, not at the start of a new line.
        self.plain_scalar_colon_adjacent = last_content_line == self.line
            && last_content_line != start.line
            && self.peek() == Some(b':')
            && self.is_blank_or_break_at(self.offset + 1);

        // Reject empty content. `fetch_plain_scalar` is the catch-all dispatch
        // target, so it also receives bytes that cannot actually begin a plain
        // scalar — notably `#` reached in flow context (e.g. `[#]`), which is a
        // c-indicator excluded by ns-plain-first (YAML 1.2.2 §7.3.3). Such a
        // byte breaks the scan loop on its first iteration with no content and
        // without advancing the offset; returning an error here is both correct
        // (the input is malformed) and essential — falling through would enqueue
        // an empty scalar and re-dispatch at the same offset forever.
        let content_empty = content
            .as_ref()
            .map_or(value_end == content_start, |c| c.is_empty());
        if content_empty {
            let next = self
                .peek()
                .map_or("EOF".to_string(), |b| (b as char).to_string());
            let kind = ErrorKind::UnexpectedToken {
                expected: "scalar value".into(),
                found: format!("'{next}'").into(),
            };
            return Err(Error::new(kind, self.span_from(start)));
        }

        let span = self.span_from(start);

        // When every character was taken verbatim from the input (no line
        // folding or trimming), `content` was never materialized and the value
        // is borrowed zero-copy from the source slice. Otherwise the owned
        // buffer (built during folding) is handed over.
        let value = match content {
            Some(content) => Cow::Owned(content),
            None => Cow::Borrowed(&self.input[content_start..value_end]),
        };

        self.enqueue(
            TokenKind::Scalar {
                value,
                style: ScalarStyle::Plain,
            },
            span,
        );

        self.allow_simple_key = true;
        Ok(true)
    }

    // ─── Directives ─────────────────────────────────────────────────

    fn fetch_directive(&mut self) -> Result<bool> {
        self.unroll_indent(-1);
        self.remove_simple_key();
        self.allow_simple_key = false;

        let start = self.pos();
        self.advance(); // skip %

        // Read directive name
        let name_start = self.offset;
        while let Some(b) = self.peek() {
            if chars::is_blank_or_break(b) {
                break;
            }
            self.advance();
        }
        let name = &self.input[name_start..self.offset];

        match name {
            "YAML" => {
                // Skip whitespace (s-separate-in-line: space or tab)
                while self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                    self.advance();
                }
                // Read version
                let ver_start = self.offset;
                while let Some(b) = self.peek() {
                    if chars::is_blank_or_break(b) {
                        break;
                    }
                    self.advance();
                }
                let version = &self.input[ver_start..self.offset];
                let parts: Vec<&str> = version.split('.').collect();
                if parts.len() == 2 {
                    let major = parts[0].parse().unwrap_or(1);
                    let minor = parts[1].parse().unwrap_or(2);

                    // Reject extra content after version number (H7TQ)
                    while self.peek() == Some(b' ') || self.peek() == Some(b'\t') {
                        self.advance();
                    }
                    if let Some(b) = self.peek()
                        && !chars::is_break(b)
                        && b != b'#'
                    {
                        return Err(Error::new(
                            ErrorKind::UnexpectedToken {
                                expected: "end of %YAML directive".into(),
                                found: format!("'{}'", b as char).into(),
                            },
                            self.span_from(start),
                        ));
                    }

                    let span = self.span_from(start);
                    self.enqueue(TokenKind::VersionDirective { major, minor }, span);
                } else {
                    return Err(Error::new(
                        ErrorKind::InvalidTag(format!("invalid YAML version: {version}")),
                        self.span_from(start),
                    ));
                }
            }
            "TAG" => {
                // Skip whitespace
                while self.peek() == Some(b' ') {
                    self.advance();
                }
                // Read handle
                let handle_start = self.offset;
                while let Some(b) = self.peek() {
                    if b == b' ' {
                        break;
                    }
                    self.advance();
                }
                let handle = &self.input[handle_start..self.offset];

                // Skip whitespace
                while self.peek() == Some(b' ') {
                    self.advance();
                }
                // Read prefix
                let prefix_start = self.offset;
                while let Some(b) = self.peek() {
                    if chars::is_blank_or_break(b) {
                        break;
                    }
                    self.advance();
                }
                let prefix = &self.input[prefix_start..self.offset];

                let span = self.span_from(start);
                self.enqueue(
                    TokenKind::TagDirective {
                        handle: Cow::Borrowed(handle),
                        prefix: Cow::Borrowed(prefix),
                    },
                    span,
                );
            }
            _ => {
                // Unknown directive — skip to end of line
                while let Some(b) = self.peek() {
                    if chars::is_break(b) {
                        break;
                    }
                    self.advance();
                }
            }
        }

        Ok(true)
    }
}

impl<'a> Iterator for Scanner<'a> {
    type Item = Result<Token<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.preserve_trivia {
            return self.next_with_trivia();
        }
        self.next_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &str) -> Vec<TokenKind<'_>> {
        Scanner::new(input)
            .map(|r| r.expect("scan error").kind)
            .collect()
    }

    fn scan_err(input: &str) -> Error {
        Scanner::new(input)
            .find_map(|r| r.err())
            .expect("expected error")
    }

    #[test]
    fn empty_stream() {
        let tokens = scan("");
        assert_eq!(tokens, vec![TokenKind::StreamStart, TokenKind::StreamEnd]);
    }

    #[test]
    fn comment_indicator_in_flow_is_error_not_infinite_loop() {
        // `#` is a c-indicator: a plain scalar cannot start with it
        // (YAML 1.2.2 §7.3.3). Reached as the flow node here (no preceding
        // whitespace, so not a comment), it must error rather than dispatch an
        // empty, non-advancing plain scalar that re-runs at the same offset
        // forever. `scan_err` itself only returns if the scanner terminates.
        for input in ["[#]", "{#}", "[# x]", "- [#]\n"] {
            let err = scan_err(input);
            assert!(
                matches!(err.kind, ErrorKind::UnexpectedToken { .. }),
                "expected UnexpectedToken for {input:?}, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn utf8_bom_skipped() {
        let input = "\u{FEFF}hello";
        let tokens = scan(input);
        assert!(matches!(tokens[0], TokenKind::StreamStart));
        assert!(matches!(tokens[1], TokenKind::Scalar { .. }));
    }

    #[test]
    fn plain_scalar() {
        let tokens = scan("hello");
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                value,
                style: ScalarStyle::Plain
            } if value == "hello"
        ));
    }

    #[test]
    fn single_quoted_scalar() {
        let tokens = scan("'hello world'");
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                value,
                style: ScalarStyle::SingleQuoted
            } if value == "hello world"
        ));
    }

    #[test]
    fn single_quoted_escape() {
        let tokens = scan("'it''s'");
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                value,
                style: ScalarStyle::SingleQuoted
            } if value == "it's"
        ));
    }

    #[test]
    fn double_quoted_scalar() {
        let tokens = scan(r#""hello\nworld""#);
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                value,
                style: ScalarStyle::DoubleQuoted
            } if value == "hello\nworld"
        ));
    }

    #[test]
    fn double_quoted_unicode_escape() {
        let tokens = scan(r#""\u0041""#);
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                value,
                style: ScalarStyle::DoubleQuoted
            } if value == "A"
        ));
    }

    #[test]
    fn block_mapping() {
        let tokens = scan("key: value");
        // StreamStart, BlockMappingStart, Key, Scalar(key), Value, Scalar(value), BlockEnd, StreamEnd
        let names: Vec<&str> = tokens.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"block-mapping-start"));
        assert!(names.contains(&"key"));
        assert!(names.contains(&"value"));
    }

    #[test]
    fn block_sequence() {
        let tokens = scan("- one\n- two");
        let names: Vec<&str> = tokens.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"block-sequence-start"));
        assert!(names.contains(&"block-entry"));
    }

    #[test]
    fn flow_sequence() {
        let tokens = scan("[1, 2, 3]");
        let names: Vec<&str> = tokens.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"flow-sequence-start"));
        assert!(names.contains(&"flow-entry"));
        assert!(names.contains(&"flow-sequence-end"));
    }

    #[test]
    fn flow_mapping() {
        let tokens = scan("{a: 1}");
        let names: Vec<&str> = tokens.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"flow-mapping-start"));
        assert!(names.contains(&"flow-mapping-end"));
    }

    #[test]
    fn anchor_and_alias() {
        let tokens = scan("&anchor value");
        assert!(matches!(&tokens[1], TokenKind::Anchor(name) if name == "anchor"));

        let tokens = scan("*alias");
        assert!(matches!(&tokens[1], TokenKind::Alias(name) if name == "alias"));
    }

    #[test]
    fn tag() {
        let tokens = scan("!!str hello");
        assert!(matches!(
            &tokens[1],
            TokenKind::Tag { handle, suffix } if handle == "!!" && suffix == "str"
        ));
    }

    #[test]
    fn document_indicators() {
        let tokens = scan("---\nhello\n...");
        let names: Vec<&str> = tokens.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"document-start"));
        assert!(names.contains(&"document-end"));
    }

    #[test]
    fn literal_block_scalar() {
        let tokens = scan("|\n  hello\n  world");
        assert!(matches!(
            &tokens[1],
            TokenKind::Scalar {
                style: ScalarStyle::Literal,
                ..
            }
        ));
    }

    #[test]
    fn version_directive() {
        let tokens = scan("%YAML 1.2\n---");
        assert!(matches!(
            &tokens[1],
            TokenKind::VersionDirective { major: 1, minor: 2 }
        ));
    }

    #[test]
    fn empty_anchor_error() {
        let err = scan_err("&");
        assert!(matches!(err.kind, ErrorKind::InvalidAnchor(_)));
    }

    /// In trivia-preserving mode, once the scanner has errored a subsequent
    /// poll must return `None` (the `populate_significant_front` errored guard,
    /// not the `next_token` fast-path guard). Drives the merged iterator past
    /// its first error to exercise that guard.
    #[test]
    fn trivia_iterator_is_fused_after_error() {
        let mut s = Scanner::with_trivia("&", ResourceLimits::default(), true);
        // First few polls yield StreamStart then the scan error.
        let mut saw_err = false;
        for _ in 0..8 {
            match s.next_with_trivia() {
                Some(Err(_)) => {
                    saw_err = true;
                    break;
                }
                Some(Ok(_)) => {}
                None => break,
            }
        }
        assert!(saw_err, "expected a scan error from a bare anchor");
        // After erroring, the next poll must short-circuit to None via the
        // `populate_significant_front` errored guard.
        assert!(
            s.next_with_trivia().is_none(),
            "errored trivia iterator must be fused to None"
        );
    }

    #[test]
    fn unterminated_single_quote_error() {
        let err = scan_err("'hello");
        assert!(matches!(err.kind, ErrorKind::UnexpectedEof));
    }

    #[test]
    fn unterminated_double_quote_error() {
        let err = scan_err("\"hello");
        assert!(matches!(err.kind, ErrorKind::UnexpectedEof));
    }

    #[test]
    fn invalid_escape_error() {
        let err = scan_err("\"\\z\"");
        assert!(matches!(err.kind, ErrorKind::InvalidEscape('z')));
    }

    #[test]
    fn document_size_limit() {
        let limits = ResourceLimits {
            max_document_size: 5,
            ..ResourceLimits::default()
        };
        let mut scanner = Scanner::with_limits("hello world", limits);
        let result: Vec<_> = scanner.by_ref().collect();
        let has_error = result.iter().any(|r| r.is_err());
        assert!(has_error, "expected document size error");
    }

    #[test]
    fn all_tokens_have_spans() {
        let input = "key: [1, 2]";
        for result in Scanner::new(input) {
            let token = result.expect("scan error");
            // All spans should have valid positions
            assert!(token.span.start.line >= 1);
        }
    }

    // ─── Additional coverage: helpers ───────────────────────────────

    /// Returns the value of the first Scalar token in the stream.
    fn first_scalar(input: &str) -> String {
        for kind in scan(input) {
            if let TokenKind::Scalar { value, .. } = kind {
                return value.into_owned();
            }
        }
        panic!("no scalar token in {input:?}");
    }

    // ─── Double-quoted escape sequences (1503-1589) ─────────────────

    #[test]
    fn double_quoted_named_escapes() {
        assert_eq!(first_scalar(r#""\0""#), "\0");
        assert_eq!(first_scalar(r#""\a""#), "\u{07}");
        assert_eq!(first_scalar(r#""\b""#), "\u{08}");
        assert_eq!(first_scalar(r#""\t""#), "\t");
        assert_eq!(first_scalar(r#""\n""#), "\n");
        assert_eq!(first_scalar(r#""\v""#), "\u{0B}");
        assert_eq!(first_scalar(r#""\f""#), "\u{0C}");
        assert_eq!(first_scalar(r#""\r""#), "\r");
        assert_eq!(first_scalar(r#""\e""#), "\u{1B}");
        assert_eq!(first_scalar(r#""\ ""#), " ");
        assert_eq!(first_scalar(r#""\"""#), "\"");
        assert_eq!(first_scalar(r#""\/""#), "/");
        assert_eq!(first_scalar(r#""\\""#), "\\");
        assert_eq!(first_scalar(r#""\N""#), "\u{85}");
        assert_eq!(first_scalar(r#""\_""#), "\u{A0}");
        assert_eq!(first_scalar(r#""\L""#), "\u{2028}");
        assert_eq!(first_scalar(r#""\P""#), "\u{2029}");
    }

    #[test]
    fn double_quoted_hex_escapes() {
        assert_eq!(first_scalar(r#""\x41""#), "A");
        assert_eq!(first_scalar(r#""é""#), "\u{e9}");
        assert_eq!(first_scalar(r#""\U0001F600""#), "\u{1F600}");
    }

    #[test]
    fn double_quoted_backslash_at_eof_errors() {
        // Backslash with no following byte before the closing quote/EOF.
        let err = scan_err("\"abc\\");
        assert_eq!(err.kind, ErrorKind::UnexpectedEof);
    }

    #[test]
    fn double_quoted_invalid_hex_escape_errors() {
        // `\x` followed by a non-hex digit.
        let err = scan_err(r#""\xZZ""#);
        assert!(matches!(err.kind, ErrorKind::InvalidEscape(_)));
    }

    #[test]
    fn double_quoted_surrogate_codepoint_errors() {
        // `\uD800` is an unpaired surrogate — not a valid scalar value.
        let err = scan_err(r#""\uD800""#);
        assert!(matches!(err.kind, ErrorKind::InvalidEscape(_)));
    }

    // ─── Single-quoted scalar that requires owning (1461) ───────────

    #[test]
    fn single_quoted_with_doubled_apostrophe_is_owned() {
        // The `''` -> `'` transformation forces an owned Cow.
        assert_eq!(first_scalar("'it''s here'"), "it's here");
    }

    // ─── Tag URI percent-escapes (hex_nibble 1111-1113) ─────────────

    #[test]
    fn tag_uri_percent_escapes_use_hex_nibble() {
        // Verbatim tag with %-escapes exercises hex_nibble for lower,
        // upper and (via a malformed case) the None arm.
        let tokens = scan("!<tag:%2c%2C>");
        assert!(
            tokens.iter().any(|k| matches!(k, TokenKind::Tag { .. })),
            "expected a tag token: {tokens:?}"
        );
    }

    #[test]
    fn tag_uri_incomplete_percent_escape() {
        // A `%` not followed by two hex digits — hex_nibble None arm.
        let tokens = scan("!<a%zz>");
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Tag { .. })));
    }

    // ─── Document-start-line collection errors (794, 882) ───────────

    #[test]
    fn block_entry_on_document_start_line_errors() {
        // `--- - x`: a block sequence cannot start on the `---` line.
        let err = scan_err("--- - x");
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn block_mapping_on_document_start_line_errors() {
        // `--- a: 1`: a block mapping cannot start on the `---` line.
        let err = scan_err("--- a: 1");
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    // ─── Explicit key indicator `?` paths (829-844) ─────────────────

    #[test]
    fn explicit_key_indicator_scans() {
        // `? a` produces a Key token (roll_indent block mapping path).
        let tokens = scan("? a\n: b");
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Key)));
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Value)));
    }

    // ─── Carriage-return line handling (224-230, 277, 1204) ─────────

    #[test]
    fn crlf_line_endings_scan() {
        let tokens = scan("a: 1\r\nb: 2\r\n");
        assert!(
            tokens
                .iter()
                .filter(|k| matches!(k, TokenKind::Key))
                .count()
                >= 2
        );
    }

    #[test]
    fn lone_cr_line_ending_scans() {
        let tokens = scan("a: 1\rb: 2\r");
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Key)));
    }

    #[test]
    fn block_scalar_with_crlf_blank_lines() {
        // CRLF inside a block scalar's leading blank lines (line 1204).
        let v = first_scalar("|\r\n\r\n  text\r\n");
        assert!(v.contains("text"), "got {v:?}");
    }

    // ─── Multibyte UTF-8 push_char_to (254, 258, 264) ───────────────

    #[test]
    fn multibyte_utf8_in_plain_scalar() {
        // 2-byte (é), 3-byte (€), 4-byte (😀) characters in a quoted
        // scalar drive the multi-byte branch of push_char_to.
        assert_eq!(first_scalar("\"é\\n€\""), "é\n€");
        assert_eq!(first_scalar("\"😀\\nx\""), "😀\nx");
    }

    // ─── Flow `:` value at EOF (449) ────────────────────────────────

    #[test]
    fn flow_mapping_colon_at_eof() {
        // `{a:` — `:` followed by EOF inside flow context.
        let tokens = scan("{a:");
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Value)));
    }

    // ─── Empty plain scalar error (1864) ────────────────────────────

    #[test]
    fn standalone_tab_then_colon_does_not_panic() {
        // Inputs that may produce an empty plain scalar attempt must not
        // panic; they either scan or error cleanly.
        let _ = Scanner::new(": ").collect::<Vec<_>>();
        let _ = Scanner::new("- :").collect::<Vec<_>>();
    }

    // ─── Plain scalar that requires owning (1886) ───────────────────

    #[test]
    fn multiline_plain_scalar_is_owned_and_folded() {
        // A multi-line plain scalar folds line breaks to spaces, which
        // forces an owned value (candidate != content, line 1886).
        let v = first_scalar("a\nb\nc");
        assert_eq!(v, "a b c");
    }

    #[test]
    fn explicit_key_on_document_start_line_errors() {
        // `--- ? k`: an explicit `?` key cannot start on the `---` line;
        // `allow_simple_key` is false there, exercising the key-not-allowed
        // branch (scanner lines 839-844).
        let err = scan_err("--- ? k");
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    // ─── Tab before `?` key indicator (fetch_key tab guard) ─────────

    #[test]
    fn tab_before_explicit_key_indicator_errors() {
        // A tab following a Value indicator (`:`) sets
        // `tab_after_indicator_line`; a `?` key on that same line then hits
        // the tab-before-key-indicator guard in `fetch_key`.
        let err = scan_err("a:\t? b");
        match err.kind {
            ErrorKind::UnexpectedToken { found, .. } => {
                assert!(found.contains("before key indicator"), "got {found}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ─── Block mapping on document-start line (fetch_value guard) ────

    #[test]
    fn indented_mapping_on_document_start_line_errors() {
        // `--- key: value`: a block mapping cannot begin on the `---` line at a
        // non-zero column. This reaches the complex value path guard
        // (`document_start_line == self.line && self.column > 0`).
        let err = scan_err("--- key: value");
        match err.kind {
            ErrorKind::UnexpectedToken { found, .. } => {
                assert!(found.contains("document start line"), "got {found}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ─── Line-ending handling in advance() (CR, CRLF) ───────────────

    #[test]
    fn carriage_return_only_line_ending() {
        // Classic-Mac `\r` line endings (CR not followed by LF) advance the
        // line counter via the first branch of `advance`.
        let tokens = scan("a: 1\rb: 2");
        assert!(tokens.iter().any(|k| matches!(k, TokenKind::Scalar { .. })));
    }

    #[test]
    fn crlf_line_ending() {
        // Windows `\r\n` line endings exercise the CR-then-LF branch of
        // `advance` (the CR is skipped, the LF drives the newline).
        let tokens = scan("a: 1\r\nb: 2\r\n");
        let scalars = tokens
            .iter()
            .filter(|k| matches!(k, TokenKind::Scalar { .. }))
            .count();
        assert_eq!(scalars, 4, "expected 4 scalars across two CRLF lines");
    }

    // ─── Multi-byte UTF-8 content (push_char_to slow path) ──────────

    #[test]
    fn multibyte_utf8_in_double_quoted_scalar() {
        // Non-ASCII content drives the multi-byte branch of `push_char_to`
        // (decoding a full char from `self.input`).
        let tokens = scan("\"café — naïve 日本語\"");
        assert!(
            tokens
                .iter()
                .any(|k| matches!(k, TokenKind::Scalar { value, .. } if value.contains("日本語")))
        );
    }

    #[test]
    fn multibyte_utf8_in_single_quoted_scalar() {
        let tokens = scan("'héllo wörld'");
        assert!(
            tokens
                .iter()
                .any(|k| matches!(k, TokenKind::Scalar { value, .. } if value.contains('é')))
        );
    }

    // ─── Plain scalar running to EOF (None => break in fetch loop) ───

    #[test]
    fn bare_plain_scalar_to_eof() {
        // A plain scalar with no trailing break reaches EOF mid-loop, taking
        // the `None => break` arm of the plain-scalar fetch loop.
        let tokens = scan("hello");
        assert!(
            tokens
                .iter()
                .any(|k| matches!(k, TokenKind::Scalar { value, .. } if value == "hello"))
        );
    }

    // ─── Trivia option ───

    #[test]
    fn trivia_off_by_default_emits_no_trivia() {
        let toks: Vec<_> = Scanner::new("a: 1 # c\n")
            .map(|r| r.unwrap().kind)
            .collect();
        assert!(
            toks.iter().all(|k| !k.is_trivia()),
            "default scan must not emit trivia"
        );
    }

    #[test]
    fn with_trivia_constructor_sets_flag() {
        let s = Scanner::with_trivia("x", ResourceLimits::default(), true);
        assert!(s.preserve_trivia());
    }

    /// Verifies trivia capture: the merged iterator yields a `Comment` token,
    /// and after iteration the side buffer is fully drained (Task 4 behaviour).
    #[test]
    fn task3_trivia_buffered_during_scan() {
        let mut s = Scanner::with_trivia("a:  1   # note\n", ResourceLimits::default(), true);
        // Drive the scanner to completion — trivia is now yielded inline.
        let all: Vec<_> = (&mut s).map(|r| r.unwrap()).collect();
        assert!(
            all.iter().any(|t| matches!(&t.kind, TokenKind::Comment(_))),
            "merged stream must contain at least one Comment token"
        );
        assert!(
            s.drain_trivia_for_test().is_empty(),
            "trivia side buffer must be empty after iterator is exhausted"
        );
    }

    /// Full lossless-coverage test: every byte of the input must appear in
    /// exactly one token span once trivia is merged into the token stream.
    /// Enabled by Task 4 (iterator merge).
    #[test]
    fn trivia_runs_are_captured_with_spans() {
        let input = "a:  1   # note\n";
        let toks: Vec<_> = Scanner::with_trivia(input, ResourceLimits::default(), true)
            .map(|r| r.unwrap())
            .collect();
        let mut covered = vec![false; input.len()];
        for t in &toks {
            let (s, e) = (t.span.start.offset, t.span.end.offset);
            for b in covered.iter_mut().take(e).skip(s) {
                assert!(!*b, "overlap");
                *b = true;
            }
        }
        assert!(
            covered.iter().all(|&b| b),
            "every byte must be covered: {covered:?}"
        );
        assert!(
            toks.iter()
                .any(|t| matches!(&t.kind, TokenKind::Comment(c) if c == "# note"))
        );
        assert!(
            toks.iter()
                .any(|t| matches!(&t.kind, TokenKind::Whitespace(_)))
        );
    }

    /// Merged stream must be non-decreasing by start offset; consuming tokens
    /// must reconstruct the original input byte-for-byte.
    #[test]
    fn merged_stream_is_offset_ordered_and_lossless() {
        let input = "# lead\nkey: value  # trail\nlist:\n  - a\n  - b\n";
        let toks: Vec<_> = Scanner::with_trivia(input, ResourceLimits::default(), true)
            .map(|r| r.unwrap())
            .collect();
        let consuming: Vec<_> = toks
            .iter()
            .filter(|t| t.span.end.offset > t.span.start.offset)
            .collect();
        for w in consuming.windows(2) {
            assert!(
                w[0].span.start.offset <= w[1].span.start.offset,
                "out of order: {:?} then {:?}",
                w[0].kind,
                w[1].kind
            );
        }
        let rebuilt: String = consuming
            .iter()
            .map(|t| &input[t.span.start.offset..t.span.end.offset])
            .collect();
        assert_eq!(rebuilt, input, "lossless reconstruction failed");
    }

    /// Edge-case lossless fixtures covering BOM, CRLF, trailing spaces,
    /// comment-only input, tab separation, nested with inline comment,
    /// no trailing newline, and empty input.
    #[test]
    fn lossless_roundtrip_edge_fixtures() {
        let fixtures = [
            "\u{FEFF}a: 1\n",           // BOM
            "a: 1\r\nb: 2\r\n",         // CRLF
            "a: 1   \n",                // trailing spaces
            "\n\n# only a comment\n\n", // blank + comment-only lines
            "a:\tb\n",                  // tab as separation
            "list:\n  - x  # c\n",      // nested + inline comment
            "key: value",               // no trailing newline
            "",                         // empty input
        ];
        for input in fixtures {
            let toks: Vec<_> = Scanner::with_trivia(input, ResourceLimits::default(), true)
                .map(|r| r.unwrap())
                .collect();
            let rebuilt: String = toks
                .iter()
                .filter(|t| t.span.end.offset > t.span.start.offset)
                .map(|t| &input[t.span.start.offset..t.span.end.offset])
                .collect();
            assert_eq!(rebuilt, input, "lossless failed for {input:?}");
        }
    }

    /// Verifies that `Scanner::new` (trivia OFF) and the significant tokens
    /// from `Scanner::with_trivia` (trivia ON, non-trivia filtered out) produce
    /// byte-identical `TokenKind` sequences.
    #[test]
    fn flag_off_byte_identical_to_baseline() {
        for input in [
            "a: 1 # c\n",
            "list:\n  - x\n",
            "{a: [1, 2]}\n",
            "foo: bar\nbaz: qux\n",
        ] {
            let off: Vec<_> = Scanner::new(input).map(|r| r.unwrap().kind).collect();
            let on_sig: Vec<_> = Scanner::with_trivia(input, ResourceLimits::default(), true)
                .map(|r| r.unwrap().kind)
                .filter(|k| !k.is_trivia())
                .collect();
            assert_eq!(
                off, on_sig,
                "trivia mode changed significant tokens for {input:?}"
            );
        }
    }
}
