// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The lossless YAML document: parse -> edit -> serialize with comments preserved.
//!
//! # Strategy (flat)
//!
//! `Document::parse` builds the tree directly from the **trivia tape**
//! ([`Scanner::with_trivia`]). With trivia on, the token spans tile the entire
//! input byte-for-byte (guaranteed by Phase 1.1). We open a single
//! [`Root`](SyntaxKind::Root) node and emit every byte-bearing tape token, in
//! **source-offset order**, as a typed leaf under it. Because the spans tile
//! the input without overlap, emitting them sorted by start offset reproduces
//! the source byte-for-byte — `Document::parse(input).to_string() == input`
//! holds by construction for all inputs.
//!
//! Note the tape is *not* emitted in source order: the scanner buffers simple
//! keys, so a separating whitespace / `:` can be yielded before the key scalar
//! it follows (e.g. `"k" :`, reproduced on `skald-yaml-test-suite/data/26DV`).
//! We therefore sort byte-bearing tokens by start offset (stable) before
//! emitting; the sort is the load-bearing step for losslessness.
//!
//! ## Why flat, not structural?
//!
//! The original plan preferred a structural tree (real `Mapping`/`Sequence`/
//! `Scalar` nodes derived by correlating parser events with the tape). That
//! approach fails losslessness on real corpus inputs: parser event spans do not
//! visit byte offsets monotonically (anchors-before-keys, indented mapping
//! keys, explicit-key forms), so flushing tape tokens at event boundaries
//! emits them out of source order and reorders/duplicates bytes
//! (reproduced on `skald-yaml-test-suite/data/26DV`). The plan explicitly
//! authorizes the flat fallback in that case. The flat tree still carries the
//! full typed-token tape, so Tasks 5–6 (`get`/`set`) can scan tokens by kind
//! and offset; they will adapt to the flat layout.
//!
//! # Error handling
//!
//! The scanner is not trusted to succeed on malformed input. If it errors
//! mid-input, we stop consuming the tape and emit the entire un-consumed
//! remainder of the source as a single [`Error`](SyntaxKind::Error) leaf, so
//! every byte is still present. `parse` never panics.

use crate::builder::GreenNodeBuilder;
use crate::green::GreenNode;
use crate::kind::SyntaxKind;
use crate::red::{SyntaxElement, SyntaxNode};
use skald_core::limits::ResourceLimits;
use skald_core::parser::{Event, EventKind, Parser};
use skald_core::scanner::Scanner;
use skald_core::scanner::token::{Token, TokenKind};
use std::iter::Peekable;
use std::rc::Rc;

/// A lossless YAML document: a concrete syntax tree that round-trips its source
/// byte-for-byte while exposing structure for edits.
pub struct Document {
    green: Rc<GreenNode>,
    src: String,
}

/// Error returned by [`Document::set`] and [`Document::insert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetError {
    /// The path did not resolve to an existing value.
    PathNotFound,
    /// The document root is not a block mapping (it is a scalar or sequence),
    /// so a top-level key cannot be inserted.
    NotAMapping,
}

impl std::fmt::Display for SetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetError::PathNotFound => write!(f, "path not found"),
            SetError::NotAMapping => write!(f, "root is not a block mapping"),
        }
    }
}

impl std::error::Error for SetError {}

/// Classification of the document root, used by [`Document::insert`].
enum RootKind {
    /// The root is a block or flow mapping.
    Mapping,
    /// The document is empty (only stream/document framing, no node).
    Empty,
    /// The root is a scalar, sequence, alias, or an error.
    Other,
}

/// Classifies the root node by walking the parser event stream up to the first
/// content event. The parser API is an `Iterator<Item = Result<Event>>`.
fn root_kind(src: &str) -> RootKind {
    classify_root_events(Parser::new(src))
}

/// Classifies a document root from its parser event stream. Split out from
/// [`root_kind`] so the exhausted-stream (`None`) arm — which a real parser
/// never produces, since it always terminates with `StreamEnd` — is reachable
/// from a unit test with a synthetic empty stream.
fn classify_root_events<'a, I>(mut events: I) -> RootKind
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    loop {
        match events.next() {
            None => return RootKind::Empty,
            Some(Err(_)) => return RootKind::Other,
            Some(Ok(ev)) => match ev.kind {
                EventKind::StreamStart | EventKind::DocumentStart { .. } => continue,
                EventKind::MappingStart { .. } => return RootKind::Mapping,
                EventKind::StreamEnd | EventKind::DocumentEnd { .. } => {
                    return RootKind::Empty;
                }
                _ => return RootKind::Other,
            },
        }
    }
}

/// Maps a scanner tape token to its CST leaf kind.
fn leaf_kind(kind: &TokenKind<'_>) -> SyntaxKind {
    match kind {
        TokenKind::Comment(_) => SyntaxKind::Comment,
        TokenKind::Whitespace(_) => SyntaxKind::Whitespace,
        TokenKind::LineBreak(_) => SyntaxKind::Newline,
        TokenKind::Scalar { .. } => SyntaxKind::ScalarToken,
        TokenKind::Anchor(_) | TokenKind::Alias(_) | TokenKind::Tag { .. } => SyntaxKind::Property,
        // Everything else with bytes is structural punctuation (`:`, `-`, `[`,
        // `]`, `{`, `}`, `,`, `?`, `---`, `...`, directives).
        _ => SyntaxKind::Punct,
    }
}

impl Document {
    /// Parses YAML source into a lossless CST. Never panics on malformed input
    /// (scanner/parser errors are absorbed into `Error`/`Punct` leaves; every
    /// byte of `input` is preserved).
    #[must_use]
    pub fn parse(input: &str) -> Document {
        let green = build(input);
        Document {
            green,
            src: input.to_string(),
        }
    }

    /// The root syntax node.
    #[must_use]
    pub fn root(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// Looks up the value at a dotted/indexed `path` and returns its exact source
    /// text (a slice of the original input), or `None` if the path does not
    /// resolve.
    ///
    /// Path syntax: `.`-separated segments. A segment is either a mapping key
    /// (matched against the key scalar's text) or a non-negative integer index
    /// into a sequence. An empty path returns the whole root node's value text.
    /// Examples: `settings.debug`, `items.1`, `matrix.0.0`.
    ///
    /// Resolution runs over the [`skald_core::parser::Parser`] event stream of
    /// the **first** document only (the CST tree itself is flat). Malformed input
    /// (any parser error) yields `None`.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&str> {
        let (start, end) = self.resolve_span(path)?;
        self.src.get(start..end)
    }

    /// Replaces the value at `path` with `value`, preserving all surrounding
    /// comments, indentation, and quoting.
    ///
    /// Resolves `path` to the value's exact source byte span (the same resolver
    /// [`get`](Document::get) uses), splices `value` over those bytes, and
    /// re-parses so the green tree stays consistent. Because only the value
    /// token's bytes change, trailing comments and the gap before them survive.
    ///
    /// # Errors
    ///
    /// Returns [`SetError::PathNotFound`] if `path` does not resolve to an
    /// existing value; in that case the document is left unchanged.
    pub fn set(&mut self, path: &str, value: &str) -> Result<(), SetError> {
        let (start, end) = self.resolve_span(path).ok_or(SetError::PathNotFound)?;
        let mut new_src = String::with_capacity(self.src.len() - (end - start) + value.len());
        new_src.push_str(&self.src[..start]);
        new_src.push_str(value);
        new_src.push_str(&self.src[end..]);
        *self = Document::parse(&new_src);
        Ok(())
    }

    /// Inserts a new top-level `key: value` entry into a block-mapping (or empty)
    /// root, preserving existing entries and comments, then reparses.
    ///
    /// The new entry is appended after all existing content. If the source does
    /// not end with a newline, one is added before the new entry so the result is
    /// always well-formed YAML.
    ///
    /// # Errors
    ///
    /// Returns [`SetError::NotAMapping`] if the root is a scalar or sequence.
    /// Top-level insertion only; nested insertion is not yet supported.
    pub fn insert(&mut self, key: &str, value: &str) -> Result<(), SetError> {
        match root_kind(&self.src) {
            RootKind::Mapping | RootKind::Empty => {
                let mut s = self.src.clone();
                if !s.is_empty() && !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push_str(key);
                s.push_str(": ");
                s.push_str(value);
                s.push('\n');
                *self = Document::parse(&s);
                Ok(())
            }
            RootKind::Other => Err(SetError::NotAMapping),
        }
    }

    /// Inserts a new `key: value` entry into the block mapping at `parent_path`,
    /// preserving siblings, comments, and indentation, then reparses.
    ///
    /// Pass an empty `parent_path` to delegate to the existing top-level
    /// [`insert`](Document::insert). Otherwise the path must resolve to a
    /// **block** mapping; flow mappings and sequences are rejected with
    /// [`SetError::NotAMapping`].
    ///
    /// The new entry is appended after all existing entries in the target
    /// mapping.  The child indent is inferred from the line that contains the
    /// first entry's key (the line holding `MappingStart.span.start`).
    ///
    /// # Errors
    ///
    /// - [`SetError::PathNotFound`] if `parent_path` does not resolve.
    /// - [`SetError::NotAMapping`] if the resolved node is not a block mapping
    ///   (it is a flow mapping, a sequence, a scalar, or a parse error).
    pub fn insert_at(&mut self, parent_path: &str, key: &str, value: &str) -> Result<(), SetError> {
        if parent_path.is_empty() {
            return self.insert(key, value);
        }

        let (vstart, vend) = self
            .resolve_span(parent_path)
            .ok_or(SetError::PathNotFound)?;

        // A block-mapping resolve returns vstart = MappingStart.span.start.offset,
        // which coincides with the ':' Value token of the first child key (the
        // scanner's zero-width BlockMappingStart/Key events place MappingStart
        // there).  Flow mappings start with '{'; sequences start with '-'; scalars
        // start with anything else.  Only the ':' case is a block mapping.
        if !self.src[vstart..].starts_with(':') {
            return Err(SetError::NotAMapping);
        }

        // Infer child indent: find the last newline strictly before vstart, then
        // collect the whitespace run at the start of that line.
        let line_start = self.src[..vstart].rfind('\n').map_or(0, |n| n + 1);
        let indent: String = self.src[line_start..]
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .map(|b| b as char)
            .collect();

        // Build the new source: everything up to vend, then a newline, the child
        // indent, the new key-value pair, then the rest of the original source.
        let extra = 1 + indent.len() + key.len() + 2 + value.len(); // '\n' + indent + key + ": " + value
        let mut s = String::with_capacity(self.src.len() + extra);
        s.push_str(&self.src[..vend]);
        s.push('\n');
        s.push_str(&indent);
        s.push_str(key);
        s.push_str(": ");
        s.push_str(value);
        s.push_str(&self.src[vend..]);

        *self = Document::parse(&s);
        Ok(())
    }

    /// Reformats the source: trims trailing whitespace from each line and
    /// ensures exactly one final newline. Comments, indentation, semantics, and
    /// block-scalar content are preserved. Idempotent.
    ///
    /// # Tokenisation realities
    ///
    /// The scanner does **not** always emit a separate `Whitespace` trivia token
    /// for end-of-line positions:
    ///
    /// * A plain scalar such as `value   \n` is scanned as one `ScalarToken`
    ///   whose text is `"value   \n"`.  The trailing spaces live inside the
    ///   scalar text and must be stripped.
    /// * A `Comment` token such as `# note  ` is followed by a standalone
    ///   `Newline` token; the trailing spaces are inside the comment text.
    /// * A standalone `Whitespace` token that is immediately followed by a
    ///   `Newline` (or is the last token) is structural trivia and is dropped.
    /// * A `ScalarToken` whose text starts with `|` or `>` is a block scalar
    ///   (literal or folded).  Its body, including all internal trailing spaces,
    ///   **must not** be modified.
    #[must_use]
    pub fn reformatted(&self) -> String {
        let mut toks: Vec<(SyntaxKind, String)> = Vec::new();
        collect_tokens(&self.root(), &mut toks);

        let mut out = String::with_capacity(self.src.len());
        for i in 0..toks.len() {
            let (kind, text) = &toks[i];
            let next_kind = toks.get(i + 1).map(|(k, _)| *k);
            let is_last = i + 1 == toks.len();

            match kind {
                // Structural whitespace trivia: drop when trailing before a newline
                // or at the very end of the stream.
                SyntaxKind::Whitespace => {
                    let trailing = next_kind == Some(SyntaxKind::Newline) || is_last;
                    if !trailing {
                        out.push_str(text);
                    }
                }
                // Comment: strip trailing horizontal whitespace from the comment
                // text itself (the scanner bakes it into the Comment token, not a
                // separate Whitespace token).
                SyntaxKind::Comment => {
                    out.push_str(strip_trailing_horizontal_ws(text));
                }
                // Plain scalar tokens: the scanner includes trailing spaces and the
                // line-ending `\n` inside the scalar text.  Strip trailing horizontal
                // whitespace immediately before any embedded `\n` (or `\r\n`).
                // Block scalars (`|`/`>`) AND quoted scalars (`"`/`'`) are left
                // untouched: their trailing spaces can be significant content, so
                // only truly-plain scalars are stripped (YAML discards a plain
                // scalar's trailing whitespace, making the strip a no-op-safe).
                SyntaxKind::ScalarToken => {
                    if preserves_trailing_ws(text) {
                        out.push_str(text);
                    } else {
                        out.push_str(&strip_trailing_ws_before_newline(text));
                    }
                }
                // Everything else (Punct, Property, Newline, Error, …) is emitted
                // verbatim.
                _ => out.push_str(text),
            }
        }

        // Normalise to exactly one trailing newline.
        let trimmed = out.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("{trimmed}\n")
        }
    }

    /// Resolves a dotted/indexed `path` to the value's byte span `(start, end)`
    /// in [`src`](Document::src). Shared by [`get`](Document::get) and
    /// [`set`](Document::set). Returns `None` if the path does not resolve or the
    /// input is malformed.
    fn resolve_span(&self, path: &str) -> Option<(usize, usize)> {
        let segments: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            path.split('.').collect()
        };

        let mut events = Parser::new(&self.src).peekable();
        // Skip stream/document framing until the first node-bearing event.
        loop {
            match events.peek() {
                Some(Ok(ev)) => match ev.kind {
                    EventKind::StreamStart | EventKind::DocumentStart { .. } => {
                        events.next();
                    }
                    EventKind::StreamEnd
                    | EventKind::DocumentEnd { .. }
                    | EventKind::MappingEnd
                    | EventKind::SequenceEnd => return None,
                    _ => break,
                },
                _ => return None,
            }
        }

        let (start, end) = resolve(&mut events, &segments)?;
        Some(self.trim_span_trailing_trivia(start, end))
    }

    /// Trims trailing ASCII whitespace from a resolved value span.
    ///
    /// The scanner's plain-scalar token span tiles the input and so extends past
    /// the scalar's content to include the separating whitespace before a
    /// trailing comment or the line break (e.g. the span for `0.0.1  # c` value
    /// covers `"0.0.1  "`). For both [`get`](Document::get) (exact value text)
    /// and [`set`](Document::set) (splice only the value, preserving the gap and
    /// comment), the span must cover only the content bytes. Leading whitespace
    /// is already excluded by the scanner, so trimming the trailing end yields
    /// exactly the scalar content.
    fn trim_span_trailing_trivia(&self, start: usize, end: usize) -> (usize, usize) {
        let slice = &self.src.as_bytes()[start..end];
        let trimmed_len = slice
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(0, |i| i + 1);
        (start, start + trimmed_len)
    }
}

/// A peekable iterator over the parser's `Result<Event>` stream.
type Events<'a, I> = Peekable<I>;

/// Resolves `segments` against the node whose start-event is at the front of
/// `events`. Returns the resolved value's `(start_offset, end_offset)` byte span
/// into the source, or `None`. On entry, the next event is the node's start
/// (Scalar/Alias for leaves, MappingStart/SequenceStart for collections).
fn resolve<'a, I>(events: &mut Events<'a, I>, segments: &[&str]) -> Option<(usize, usize)>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    let ev = match events.next()? {
        Ok(ev) => ev,
        Err(_) => return None,
    };
    let node_span = (ev.span.start.offset, ev.span.end.offset);

    match ev.kind {
        EventKind::Scalar { .. } | EventKind::Alias { .. } => {
            // A leaf: only an empty remaining path can match it. A deeper path
            // (e.g. `leaf.more`) has no child to descend into.
            if segments.is_empty() {
                Some(node_span)
            } else {
                None
            }
        }
        EventKind::MappingStart { .. } => resolve_mapping(events, segments, node_span),
        EventKind::SequenceStart { .. } => resolve_sequence(events, segments, node_span),
        // Framing/End events are never node starts here.
        _ => None,
    }
}

/// Resolves a mapping. `events` is positioned just after `MappingStart`;
/// `start_span` is the mapping node's own span (returned when `segments` is
/// empty). Reads `key`/`value` node pairs until `MappingEnd`.
fn resolve_mapping<'a, I>(
    events: &mut Events<'a, I>,
    segments: &[&str],
    start_span: (usize, usize),
) -> Option<(usize, usize)>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    // Entire mapping requested.
    if segments.is_empty() {
        let end = consume_node_body(events)?;
        return Some((start_span.0, end));
    }

    let target = segments[0];
    let mut found: Option<(usize, usize)> = None;

    loop {
        match events.peek() {
            Some(Ok(ev)) if matches!(ev.kind, EventKind::MappingEnd) => {
                events.next();
                break;
            }
            Some(Ok(_)) => {
                // Read the KEY node, capturing its scalar text if it is a scalar.
                let key_text = read_key(events)?;
                let matched = found.is_none() && key_text.as_deref() == Some(target);
                if matched {
                    found = resolve(events, &segments[1..]);
                } else {
                    skip_node(events)?; // skip the value node
                }
            }
            // Parser error or premature end.
            _ => return None,
        }
    }

    found
}

/// Resolves a sequence. `events` is positioned just after `SequenceStart`;
/// `start_span` is the sequence node's own span. Iterates items counting index.
fn resolve_sequence<'a, I>(
    events: &mut Events<'a, I>,
    segments: &[&str],
    start_span: (usize, usize),
) -> Option<(usize, usize)>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    if segments.is_empty() {
        let end = consume_node_body(events)?;
        return Some((start_span.0, end));
    }

    let target: usize = segments[0].parse().ok()?;
    let mut idx = 0usize;
    let mut found: Option<(usize, usize)> = None;

    loop {
        match events.peek() {
            Some(Ok(ev)) if matches!(ev.kind, EventKind::SequenceEnd) => {
                events.next();
                break;
            }
            Some(Ok(_)) => {
                if idx == target && found.is_none() {
                    found = resolve(events, &segments[1..]);
                } else {
                    skip_node(events)?;
                }
                idx += 1;
            }
            _ => return None,
        }
    }

    found
}

/// Reads one key node. Returns `Some(Some(text))` for a scalar key, `Some(None)`
/// for a non-scalar key (which was skipped), or `None` on error.
fn read_key<'a, I>(events: &mut Events<'a, I>) -> Option<Option<String>>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    match events.peek() {
        Some(Ok(ev)) => match &ev.kind {
            EventKind::Scalar { value, .. } => {
                let text = value.to_string();
                events.next();
                Some(Some(text))
            }
            // Non-scalar key (e.g. complex `? [a]`): consume its whole subtree.
            _ => {
                skip_node(events)?;
                Some(None)
            }
        },
        _ => None,
    }
}

/// Consumes the remainder of a collection body (events already past its Start)
/// up to and including its matching End, accounting for nesting. Returns the
/// end offset of the closing End event. The matching End is found purely by
/// depth counting, so it works for both mappings and sequences.
fn consume_node_body<'a, I>(events: &mut Events<'a, I>) -> Option<usize>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    let mut depth = 1usize;
    loop {
        let ev = match events.next()? {
            Ok(ev) => ev,
            Err(_) => return None,
        };
        match ev.kind {
            EventKind::MappingStart { .. } | EventKind::SequenceStart { .. } => depth += 1,
            EventKind::MappingEnd | EventKind::SequenceEnd => {
                depth -= 1;
                if depth == 0 {
                    return Some(ev.span.end.offset);
                }
            }
            _ => {}
        }
    }
}

/// Skips exactly one full node subtree starting at the front of `events`:
/// a Scalar/Alias consumes one event; a collection consumes through its
/// matching End. Returns `None` on a parser error or unexpected end.
fn skip_node<'a, I>(events: &mut Events<'a, I>) -> Option<()>
where
    I: Iterator<Item = skald_core::error::Result<Event<'a>>>,
{
    let ev = match events.next()? {
        Ok(ev) => ev,
        Err(_) => return None,
    };
    match ev.kind {
        EventKind::Scalar { .. } | EventKind::Alias { .. } => Some(()),
        EventKind::MappingStart { .. } | EventKind::SequenceStart { .. } => {
            consume_node_body(events).map(|_| ())
        }
        _ => None,
    }
}

/// Returns `true` if `text` is a block scalar token (literal `|` or folded
/// `>`), or a quoted scalar (text starts with `"` or `'`). Such scalars embed
/// their full body (including internal/trailing spaces that may be significant)
/// as a single token; reformatting must leave them untouched. Only truly-plain
/// scalars are trimmed, where trailing whitespace is insignificant per YAML.
fn preserves_trailing_ws(text: &str) -> bool {
    let first = text.as_bytes().first().copied();
    matches!(first, Some(b'|') | Some(b'>') | Some(b'"') | Some(b'\''))
}

/// Strips trailing horizontal whitespace (spaces and tabs) from `text`,
/// without touching the text after the last non-whitespace character.
/// Used for `Comment` tokens where the trailing spaces are baked in.
fn strip_trailing_horizontal_ws(text: &str) -> &str {
    let trimmed_len = text
        .as_bytes()
        .iter()
        .rposition(|b| *b != b' ' && *b != b'\t')
        .map_or(0, |i| i + 1);
    &text[..trimmed_len]
}

/// Strips trailing horizontal whitespace (spaces and tabs) immediately before
/// any embedded line endings (`\n` or `\r\n`) in `text`. Used for plain
/// `ScalarToken`s where the scanner includes the `\n` inside the token text.
/// Interior lines of multi-line plain scalars also get their trailing ws
/// stripped (only relevant when the scalar spans continuation lines).
fn strip_trailing_ws_before_newline(text: &str) -> String {
    // Fast path: no newline in text → nothing to do.
    if !text.contains('\n') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        // Each `line` ends with `\n` (or is the final non-terminated fragment).
        let (content, ending) = if let Some(stripped) = line.strip_suffix('\n') {
            let (c, e) = stripped
                .strip_suffix('\r')
                .map_or((stripped, "\n"), |c2| (c2, "\r\n"));
            (c, e)
        } else {
            (line, "")
        };
        // Strip trailing spaces/tabs from the content portion only.
        let trimmed_len = content
            .as_bytes()
            .iter()
            .rposition(|b| *b != b' ' && *b != b'\t')
            .map_or(0, |i| i + 1);
        out.push_str(&content[..trimmed_len]);
        out.push_str(ending);
    }
    out
}

/// Collects all leaf tokens from `node` (depth-first) into `out` as
/// `(kind, text)` pairs. The flat CST has all tokens as direct Root children,
/// but the helper is written recursively for robustness.
fn collect_tokens(node: &SyntaxNode, out: &mut Vec<(SyntaxKind, String)>) {
    for el in node.children_with_tokens() {
        match el {
            SyntaxElement::Node(n) => collect_tokens(&n, out),
            SyntaxElement::Token(t, _) => out.push((t.kind(), t.text().to_string())),
        }
    }
}

/// Serializes the document back to source, byte-for-byte for unedited docs.
/// `Document::to_string()` (via [`ToString`]) is therefore lossless.
impl std::fmt::Display for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        self.green.write_text(&mut s);
        f.write_str(&s)
    }
}

/// Collects the full trivia tape. On a scanner error, returns the tokens
/// gathered so far plus the byte offset where scanning stopped (so the caller
/// can emit the remainder raw and stay lossless).
fn collect_tape(input: &str) -> (Vec<Token<'_>>, Option<usize>) {
    let mut tape = Vec::new();
    let scanner = Scanner::with_trivia(input, ResourceLimits::default(), true);
    let mut stop_at = None;
    for res in scanner {
        match res {
            // Token spans are guaranteed to stay within `input`; `build` also
            // re-filters by `span.end <= input.len()`, so no per-token bounds
            // guard is needed here.
            Ok(tok) => tape.push(tok),
            Err(_) => {
                // Resume from the furthest byte we have already covered.
                let covered = tape.last().map_or(0, |t| t.span.end.offset);
                stop_at = Some(covered.min(input.len()));
                break;
            }
        }
    }
    (tape, stop_at)
}

/// Builds the green tree for `input` by draining the trivia tape flat under a
/// single `Root` node (see the module-level strategy notes).
fn build(input: &str) -> Rc<GreenNode> {
    let (tape, scan_stop) = collect_tape(input);
    let mut b = GreenNodeBuilder::new();
    b.start_node(SyntaxKind::Root);

    // Keep only byte-bearing tokens (drop zero-width StreamStart,
    // BlockMappingStart, Key, … which carry no source) and sort them by start
    // offset. The scanner buffers simple keys, so emission order is not source
    // order; sorting restores it. Stable sort keeps equal-offset ties (which
    // cannot both be byte-bearing, since spans are disjoint) deterministic.
    let mut leaves: Vec<&Token<'_>> = tape
        .iter()
        .filter(|t| t.span.end.offset > t.span.start.offset && t.span.end.offset <= input.len())
        .collect();
    leaves.sort_by_key(|t| t.span.start.offset);

    let mut covered = 0usize;
    for tok in leaves {
        let (s, e) = (tok.span.start.offset, tok.span.end.offset);
        // Fill any gap the tape left uncovered (e.g. reserved/unknown
        // directive lines such as `%FOO …`, which the scanner swallows without
        // emitting a byte-bearing token). Raw bytes go in as an `Error` leaf so
        // every byte survives the round-trip.
        if s > covered {
            b.token(SyntaxKind::Error, &input[covered..s]);
        }
        b.token(leaf_kind(&tok.kind), &input[s..e]);
        covered = e;
    }

    // Trailing remainder: whatever the tape never reached, including the case
    // where the scanner stopped early on malformed input. One `Error` leaf
    // keeps the round-trip lossless.
    let tail_from = match scan_stop {
        Some(stop) => covered.max(stop).min(input.len()),
        None => covered,
    };
    if tail_from < input.len() {
        b.token(SyntaxKind::Error, &input[tail_from..]);
    }

    b.finish_node(); // Root
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_to_string_is_lossless() {
        for input in [
            "a: 1\n",
            "# c\nkey: value  # trail\n",
            "list:\n  - a\n  - b\n",
            "a: 1\r\nb: 2\r\n",
            "\u{FEFF}x: y\n",
            "nested:\n  k: v # note\n",
            "plain",
            "",
        ] {
            let doc = Document::parse(input);
            assert_eq!(doc.to_string(), input, "lossless failed for {input:?}");
        }
    }

    #[test]
    fn get_by_path() {
        let doc = Document::parse("settings:\n  debug: true\nitems:\n  - a\n  - b\n");
        assert_eq!(doc.get("settings.debug").map(str::trim), Some("true"));
        assert_eq!(doc.get("items.1").map(str::trim), Some("b"));
        assert_eq!(doc.get("nope"), None);
        assert_eq!(doc.get("settings.debug.deeper"), None);
    }

    #[test]
    fn get_flow_mapping() {
        let doc = Document::parse("a: {x: 1, y: 2}\n");
        assert_eq!(doc.get("a.x").map(str::trim), Some("1"));
        assert_eq!(doc.get("a.y").map(str::trim), Some("2"));
        assert_eq!(doc.get("a").map(str::trim), Some("{x: 1, y: 2}"));
    }

    #[test]
    fn get_nested_sequence() {
        let doc = Document::parse("matrix:\n  - [1, 2]\n  - [3, 4]\n");
        assert_eq!(doc.get("matrix.0.0").map(str::trim), Some("1"));
        assert_eq!(doc.get("matrix.1.1").map(str::trim), Some("4"));
        assert_eq!(doc.get("matrix.2"), None);
    }

    #[test]
    fn get_root_scalar() {
        let doc = Document::parse("plain\n");
        assert_eq!(doc.get("").map(str::trim), Some("plain"));
        assert_eq!(doc.get("nope"), None);
    }

    #[test]
    fn get_malformed_is_none() {
        let doc = Document::parse("a: [1, 2");
        assert_eq!(doc.get("b"), None);
    }

    #[test]
    fn root_node_is_root_kind() {
        let doc = Document::parse("a: 1\n");
        assert_eq!(doc.root().kind(), SyntaxKind::Root);
    }

    #[test]
    fn display_matches_to_string() {
        let doc = Document::parse("a: 1\n");
        assert_eq!(format!("{doc}"), doc.to_string());
    }

    #[test]
    fn set_replaces_value_preserving_trivia() {
        let mut doc = Document::parse("version: 0.0.1  # keep me\nname: skald\n");
        doc.set("version", "0.0.2").unwrap();
        assert_eq!(doc.to_string(), "version: 0.0.2  # keep me\nname: skald\n");
    }

    #[test]
    fn set_nested_and_sequence() {
        let mut doc = Document::parse("settings:\n  debug: true  # c\nitems:\n  - a\n  - b\n");
        doc.set("settings.debug", "false").unwrap();
        doc.set("items.1", "z").unwrap();
        assert_eq!(
            doc.to_string(),
            "settings:\n  debug: false  # c\nitems:\n  - a\n  - z\n"
        );
    }

    #[test]
    fn set_unknown_path_errors() {
        let mut doc = Document::parse("a: 1\n");
        assert_eq!(doc.set("b", "2"), Err(SetError::PathNotFound));
        // unchanged after a failed set
        assert_eq!(doc.to_string(), "a: 1\n");
    }

    #[test]
    fn insert_appends_top_level_key_preserving_comments() {
        let mut doc = Document::parse("name: skald  # the name\n");
        doc.insert("version", "1").unwrap();
        assert_eq!(doc.to_string(), "name: skald  # the name\nversion: 1\n");
        assert_eq!(doc.get("version").map(str::trim), Some("1"));
    }

    #[test]
    fn insert_into_empty_doc_creates_mapping() {
        let mut doc = Document::parse("");
        doc.insert("a", "1").unwrap();
        assert_eq!(doc.to_string(), "a: 1\n");
    }

    #[test]
    fn insert_handles_missing_trailing_newline() {
        let mut doc = Document::parse("a: 1");
        doc.insert("b", "2").unwrap();
        assert_eq!(doc.to_string(), "a: 1\nb: 2\n");
    }

    #[test]
    fn insert_rejects_non_mapping_root() {
        let mut doc = Document::parse("- item\n");
        assert!(doc.insert("k", "v").is_err());
        let mut scalar = Document::parse("just a scalar\n");
        assert!(scalar.insert("k", "v").is_err());
    }

    #[test]
    fn insert_at_nested_mapping_preserves_indent_and_comments() {
        let mut doc = Document::parse("server:\n  host: x  # the host\nother: y\n");
        doc.insert_at("server", "port", "8080").unwrap();
        assert_eq!(
            doc.to_string(),
            "server:\n  host: x  # the host\n  port: 8080\nother: y\n"
        );
    }

    #[test]
    fn insert_at_deeper_nesting() {
        let mut doc = Document::parse("a:\n  b:\n    c: 1\n");
        doc.insert_at("a.b", "d", "2").unwrap();
        assert_eq!(doc.to_string(), "a:\n  b:\n    c: 1\n    d: 2\n");
    }

    #[test]
    fn insert_at_root_matches_top_level_insert() {
        let mut doc = Document::parse("x: 1\n");
        doc.insert_at("", "y", "2").unwrap();
        assert_eq!(doc.to_string(), "x: 1\ny: 2\n");
    }

    #[test]
    fn insert_at_rejects_non_block_mapping_parent() {
        let mut doc = Document::parse("server: {host: x}\n");
        assert!(doc.insert_at("server", "port", "1").is_err());
        let mut seq = Document::parse("items:\n  - a\n");
        assert!(seq.insert_at("items", "k", "v").is_err());
    }

    // ── reformatted ──────────────────────────────────────────────────────────

    #[test]
    fn reformat_trims_trailing_whitespace() {
        let doc = Document::parse("a: 1   \nb: 2\t\n");
        assert_eq!(doc.reformatted(), "a: 1\nb: 2\n");
    }

    #[test]
    fn reformat_preserves_indentation_and_comments() {
        let doc = Document::parse("server:\n  host: x  # note  \n");
        assert_eq!(doc.reformatted(), "server:\n  host: x  # note\n");
    }

    #[test]
    fn reformat_ensures_single_final_newline() {
        assert_eq!(Document::parse("a: 1").reformatted(), "a: 1\n");
        assert_eq!(Document::parse("a: 1\n\n\n").reformatted(), "a: 1\n");
    }

    #[test]
    fn reformat_preserves_block_scalar_content_including_trailing_spaces() {
        let src = "lit: |\n  line with trailing   \n  next\n";
        let doc = Document::parse(src);
        assert_eq!(doc.reformatted(), src);
    }

    #[test]
    fn reformat_is_idempotent() {
        let once = Document::parse("a: 1  \n").reformatted();
        let twice = Document::parse(&once).reformatted();
        assert_eq!(once, twice);
    }

    #[test]
    fn reformat_preserves_quoted_scalar_trailing_ws() {
        // A double-quoted multi-line scalar's trailing spaces before a fold can
        // be significant — they must NOT be stripped (only plain scalars are).
        let src = "msg: \"line one   \n  line two\"\n";
        let doc = Document::parse(src);
        assert_eq!(doc.reformatted(), src);
        // Single-quoted likewise preserved.
        let sq = "x: 'a   \n  b'\n";
        assert_eq!(Document::parse(sq).reformatted(), sq);
    }

    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn set_error_display_messages() {
        assert_eq!(SetError::PathNotFound.to_string(), "path not found");
        assert_eq!(
            SetError::NotAMapping.to_string(),
            "root is not a block mapping"
        );
    }

    #[test]
    fn insert_on_malformed_root_is_not_a_mapping() {
        // A bare anchor with no node is a parser error reached before any content
        // event, so root_kind takes its `Some(Err(_)) => Other` arm → NotAMapping.
        let mut doc = Document::parse("&\n");
        assert_eq!(doc.insert("k", "v"), Err(SetError::NotAMapping));
    }

    #[test]
    fn reformat_whitespace_only_yields_empty() {
        // Trims to nothing → the empty-result arm of `reformatted`.
        assert_eq!(Document::parse("\n\n\n").reformatted(), "");
        assert_eq!(Document::parse("   \n  \n").reformatted(), "");
    }

    #[test]
    fn get_into_unterminated_flow_sequence_is_none() {
        // Descending INTO an unterminated sequence makes resolve_sequence hit a
        // parser error / premature end (the `_ => return None` arm).
        let doc = Document::parse("a: [1, 2");
        assert_eq!(doc.get("a.0"), None);
    }

    #[test]
    fn get_into_unterminated_flow_mapping_is_none() {
        // Descending INTO an unterminated mapping makes resolve_mapping hit a
        // parser error / premature end.
        let doc = Document::parse("a: {b: 1");
        assert_eq!(doc.get("a.b"), None);
    }

    #[test]
    fn get_skips_non_scalar_complex_key() {
        // A complex (sequence) key `? [a, b]` is non-scalar: read_key skips its
        // whole subtree, then the real `target` key still resolves.
        let doc = Document::parse("? [a, b]\n: first\ntarget: second\n");
        assert_eq!(doc.get("target").map(str::trim), Some("second"));
    }

    #[test]
    fn get_whole_nested_collection_consumes_body() {
        // Requesting a flow mapping whose value nests another flow collection
        // forces consume_node_body to recurse with depth tracking. Flow form is
        // used so the resolved span is unambiguous and stable.
        let doc = Document::parse("outer: {inner: [1, 2]}\nsibling: z\n");
        let whole = doc.get("outer").map(str::trim).unwrap();
        assert_eq!(whole, "{inner: [1, 2]}");
        // The sibling must NOT be swallowed into outer's span.
        assert!(!whole.contains("sibling"));
    }

    #[test]
    fn get_skips_sibling_with_nested_collection_value() {
        // Skipping a non-target key whose value is a nested collection drives
        // skip_node → consume_node_body across the nested mapping/sequence.
        let doc = Document::parse("first:\n  nested:\n    - x\nsecond: y\n");
        assert_eq!(doc.get("second").map(str::trim), Some("y"));
    }

    #[test]
    fn strip_trailing_ws_before_newline_handles_unterminated_final_fragment() {
        // A multi-line fragment whose final line is NOT newline-terminated drives
        // the else-arm of the stripper directly (no parser/scanner involvement).
        assert_eq!(
            strip_trailing_ws_before_newline("one   \ntwo   "),
            "one\ntwo"
        );
        // CRLF terminator on the first line, bare final fragment.
        assert_eq!(strip_trailing_ws_before_newline("a \t\r\nb \t"), "a\r\nb");
    }

    #[test]
    fn get_on_empty_document_is_none() {
        // resolve_span skips StreamStart then meets StreamEnd → the End-event
        // framing arm returns None.
        let doc = Document::parse("");
        assert_eq!(doc.get("anything"), None);
    }

    #[test]
    fn get_on_immediate_parse_error_is_none() {
        // A bare anchor errors right after framing: resolve_span's peek hits the
        // catch-all `_ => return None` arm.
        let doc = Document::parse("&\n");
        assert_eq!(doc.get("x"), None);
    }

    #[test]
    fn get_value_that_errors_during_resolve_is_none() {
        // Descending to a value that is an invalid bare anchor makes the inner
        // resolve's `events.next()` yield an Err (the resolve Err arm).
        let doc = Document::parse("a: {x: &}\n");
        assert_eq!(doc.get("a.x"), None);
    }

    #[test]
    fn get_skips_value_that_errors_is_none() {
        // Skipping a non-target sibling whose value errors drives skip_node's
        // Err arm (and read_key's error handling).
        let doc = Document::parse("a: &\nb: 1\n");
        assert_eq!(doc.get("b"), None);
    }

    // ── direct helper tests for defensive arms ────────────────────────────────
    //
    // These resolver helpers carry defensive guards against shapes a well-formed
    // parser never emits in that position (a non-node-start where a node is
    // expected, an exhausted/error stream mid-resolution). The guards are kept on
    // purpose — removing such "unreachable" arms previously turned a clean error
    // into an infinite loop. We exercise them with synthetic event streams so the
    // guards stay covered without depending on a parser bug.

    use skald_core::error::Result as CoreResult;
    use skald_core::types::{Position, Span};

    fn ev(kind: EventKind<'static>) -> CoreResult<Event<'static>> {
        Ok(Event {
            kind,
            span: Span::point(Position::start()),
        })
    }

    #[test]
    fn classify_root_events_empty_stream_is_empty() {
        // An exhausted stream (the `None` arm) classifies as Empty.
        let none: Vec<CoreResult<Event<'static>>> = Vec::new();
        assert!(matches!(
            classify_root_events(none.into_iter()),
            RootKind::Empty
        ));
    }

    #[test]
    fn resolve_rejects_non_node_start_event() {
        // A `MappingEnd` where a node start is expected hits resolve's `_ => None`.
        let events = vec![ev(EventKind::MappingEnd)];
        let mut it = events.into_iter().peekable();
        assert_eq!(resolve(&mut it, &[]), None);
    }

    #[test]
    fn skip_node_rejects_non_node_start_event() {
        // A `SequenceEnd` where a node start is expected hits skip_node's `_ => None`.
        let events = vec![ev(EventKind::SequenceEnd)];
        let mut it = events.into_iter().peekable();
        assert_eq!(skip_node(&mut it), None);
    }

    #[test]
    fn read_key_on_exhausted_stream_is_none() {
        // An empty stream in key position hits read_key's `_ => None` arm.
        let events: Vec<CoreResult<Event<'static>>> = Vec::new();
        let mut it = events.into_iter().peekable();
        assert_eq!(read_key(&mut it), None);
    }

    #[test]
    fn collect_tokens_recurses_into_child_nodes() {
        // The flat CST never nests nodes, but collect_tokens is recursive for
        // robustness — drive the Node arm with a hand-built nested tree.
        let mut b = GreenNodeBuilder::new();
        b.start_node(SyntaxKind::Root);
        b.start_node(SyntaxKind::Mapping); // nested child node
        b.token(SyntaxKind::ScalarToken, "k");
        b.token(SyntaxKind::Punct, ": ");
        b.token(SyntaxKind::ScalarToken, "v");
        b.finish_node();
        b.finish_node();
        let root = SyntaxNode::new_root(b.finish());

        let mut toks = Vec::new();
        collect_tokens(&root, &mut toks);
        let kinds: Vec<SyntaxKind> = toks.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                SyntaxKind::ScalarToken,
                SyntaxKind::Punct,
                SyntaxKind::ScalarToken
            ]
        );
    }

    #[test]
    fn corpus_lossless() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../skald-yaml-test-suite/data");
        if !dir.exists() {
            eprintln!("skipping corpus: submodule absent");
            return;
        }
        let mut n = 0;
        let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let f = entry.path().join("in.yaml");
            if let Ok(src) = std::fs::read_to_string(&f) {
                assert_eq!(
                    Document::parse(&src).to_string(),
                    src,
                    "lossless failed for {f:?}"
                );
                n += 1;
            }
        }
        eprintln!("corpus_lossless checked {n} files");
    }
}
