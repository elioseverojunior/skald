// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML composer.
//!
//! Converts an [`Event`] stream into a [`Node`] representation graph.
//! Resolves anchors and aliases, enforces expansion limits, and detects duplicate keys.
//!
//! # Usage
//!
//! ```
//! use skald_ast::composer;
//!
//! let docs = composer::compose_all("hello").unwrap();
//! assert_eq!(docs.len(), 1);
//! assert_eq!(docs[0].as_str(), Some("hello"));
//! ```

use std::borrow::Cow;
use std::collections::HashMap;

use skald_core::error::{Error, ErrorKind, ParserConfig, Result, Strictness};
use skald_core::parser::Parser;
use skald_core::parser::event::{Event, EventKind};
use skald_core::types::{CollectionStyle, ScalarStyle, Span, Tag};

use crate::node::{Mapping, Node, Scalar, Sequence};

/// Threshold below which duplicate-key detection uses a linear scan instead
/// of a `HashSet`. For mappings with ≤ `DUP_LINEAR_THRESHOLD` keys the O(n²)
/// scan avoids allocating a `HashSet` entirely. Above the threshold a `HashSet`
/// is still used for O(1) per-key detection.
const DUP_LINEAR_THRESHOLD: usize = 16;

/// Composes all documents from a YAML input string.
///
/// Returns one [`Node`] per YAML document. An empty stream produces an empty `Vec`.
pub fn compose_all(input: &str) -> Result<Vec<Node<'_>>> {
    Composer::new(input).collect()
}

/// Returns `true` if `node` is the plain merge key scalar `<<`.
///
/// A tag does not disqualify it — but in practice merge keys are plain
/// untagged scalars; the value-form check is what enforces correctness.
fn is_merge_key(node: &Node<'_>) -> bool {
    node.as_str() == Some("<<")
}

/// A short, human-readable kind name for a node, used in merge-key errors.
fn node_kind(node: &Node<'_>) -> &'static str {
    match node {
        Node::Scalar(_) => "scalar",
        Node::Sequence(_) => "sequence",
        Node::Mapping(_) => "mapping",
    }
}

/// The YAML composer.
///
/// Converts a parser event stream into a [`Node`] representation graph.
/// Each call to [`Iterator::next`] returns the root node of one YAML document.
///
/// # Anchor / Alias Handling
///
/// Anchors (`&name`) register a node in the per-document anchor table.
/// Aliases (`*name`) clone the anchored node into the tree. The
/// [`ResourceLimits::max_alias_expansions`](skald_core::limits::ResourceLimits::max_alias_expansions)
/// limit prevents billion-laughs–style attacks.
///
/// # Duplicate Key Detection
///
/// In [`Strictness::Strict`] mode (the default), duplicate scalar keys in a
/// mapping produce an error.
pub struct Composer<'a> {
    parser: Parser<'a>,
    peeked: Option<Event<'a>>,
    anchors: HashMap<String, Node<'a>>,
    config: ParserConfig,
    node_count: usize,
    alias_expansions: usize,
    errored: bool,
    started: bool,
}

impl<'a> Composer<'a> {
    /// Creates a new composer for the given input.
    #[must_use]
    pub fn new(input: &'a str) -> Self {
        Self::with_config(input, ParserConfig::default())
    }

    /// Creates a new composer with custom configuration.
    #[must_use]
    pub fn with_config(input: &'a str, config: ParserConfig) -> Self {
        Self {
            parser: Parser::with_config(input, config.clone()),
            peeked: None,
            anchors: HashMap::with_capacity(4),
            config,
            node_count: 0,
            alias_expansions: 0,
            errored: false,
            started: false,
        }
    }

    /// Composes the next document, or `None` if the stream is exhausted.
    pub fn compose_document(&mut self) -> Result<Option<Node<'a>>> {
        if !self.started {
            let event = self.next_event()?;
            debug_assert!(matches!(event.kind, EventKind::StreamStart));
            self.started = true;
        }

        // StreamEnd or exhausted → no more documents
        if self.at_stream_end()? {
            return Ok(None);
        }

        // Per-document state reset
        self.anchors.clear();
        self.node_count = 0;
        self.alias_expansions = 0;

        // Consume DocumentStart
        let _doc_start = self.next_event()?;

        // Compose root node
        let root = self.compose_node()?;

        // Consume DocumentEnd
        let _doc_end = self.next_event()?;

        Ok(Some(root))
    }

    // ─── Event access ───────────────────────────────────────────────

    fn peek_event(&mut self) -> Result<Option<&Event<'a>>> {
        if self.peeked.is_none() {
            // `transpose` folds the `None` (stream exhausted) and `Some(Err)`
            // (upstream error) cases into the same expression, so neither is a
            // separate branch: errors propagate via `?`, exhaustion yields `None`.
            self.peeked = self.parser.next_event().transpose()?;
        }
        Ok(self.peeked.as_ref())
    }

    fn next_event(&mut self) -> Result<Event<'a>> {
        if let Some(e) = self.peeked.take() {
            return Ok(e);
        }
        // `ok_or` (eager) rather than `ok_or_else`: `Error::spanless` is a
        // cheap unit construction, so building it unconditionally keeps the
        // defensive path on the always-executed expression instead of an
        // unreachable closure.
        self.parser
            .next_event()
            .transpose()?
            .ok_or(Error::spanless(ErrorKind::UnexpectedEof))
    }

    // ─── Peek helpers (return bool to avoid borrow conflicts) ───────

    fn at_stream_end(&mut self) -> Result<bool> {
        Ok(self
            .peek_event()?
            .is_none_or(|e| matches!(e.kind, EventKind::StreamEnd)))
    }

    fn at_sequence_end(&mut self) -> Result<bool> {
        Ok(self
            .peek_event()?
            .is_some_and(|e| matches!(e.kind, EventKind::SequenceEnd)))
    }

    fn at_mapping_end(&mut self) -> Result<bool> {
        Ok(self
            .peek_event()?
            .is_some_and(|e| matches!(e.kind, EventKind::MappingEnd)))
    }

    // ─── Node composition (recursive, bounded by parser depth limit) ─

    fn compose_node(&mut self) -> Result<Node<'a>> {
        let event = self.next_event()?;
        self.compose_node_from_event(event)
    }

    /// Dispatches an already-fetched event to the matching node builder.
    ///
    /// Split out from [`Self::compose_node`] so the defensive `other =>` arm can be
    /// exercised with a synthetic event. A well-formed parser never produces a
    /// non-node event in this position, but the guard is retained against future
    /// parser changes — deleting such "unreachable" guards previously turned a
    /// clean error into an infinite loop.
    fn compose_node_from_event(&mut self, event: Event<'a>) -> Result<Node<'a>> {
        match event.kind {
            // Kept on one line (rustfmt would explode this 4-field pattern across
            // lines, and the coverage instrumentation does not credit the opening
            // line of a multi-line match pattern even though the arm executes).
            #[rustfmt::skip]
            EventKind::Scalar { value, style, anchor, tag } => {
                self.compose_scalar(event.span, value, style, anchor, tag)
            },
            EventKind::Alias { name } => {
                self.alias_expansions += 1;
                self.check_alias_limit(event.span)?;
                self.resolve_alias(&name, event.span)
            }
            EventKind::SequenceStart { anchor, tag, style } => {
                self.compose_sequence(event.span, anchor, tag, style)
            }
            EventKind::MappingStart { anchor, tag, style } => {
                self.compose_mapping(event.span, anchor, tag, style)
            }
            other => Err(Error::new(
                ErrorKind::UnexpectedToken {
                    expected: "node content".into(),
                    found: other.name().to_string().into(),
                },
                event.span,
            )),
        }
    }

    /// Builds a scalar node: resolves its tag and registers any anchor.
    ///
    /// Split out of [`Self::compose_node_from_event`] so the match arm is a single
    /// dispatch expression — a multi-line `EventKind::Scalar { .. }` destructure
    /// in the arm pattern is not credited by the coverage instrumentation even
    /// though it executes on every scalar.
    fn compose_scalar(
        &mut self,
        span: Span,
        value: Cow<'a, str>,
        style: ScalarStyle,
        anchor: Option<Cow<'a, str>>,
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
    ) -> Result<Node<'a>> {
        self.count_node(span)?;
        if let Some(max) = self.config.policies.max_scalar_length
            && value.len() > max
        {
            return Err(Error::new(
                ErrorKind::ScalarLengthLimitExceeded {
                    limit: max,
                    actual: value.len(),
                },
                span,
            ));
        }
        let resolved_tag = self.apply_tag(tag, span)?;
        let node = Node::Scalar(Scalar {
            value,
            tag: resolved_tag,
            style,
            span,
        });
        self.maybe_register_anchor(anchor, &node)?;
        Ok(node)
    }

    fn compose_sequence(
        &mut self,
        start_span: Span,
        anchor: Option<Cow<'a, str>>,
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        style: CollectionStyle,
    ) -> Result<Node<'a>> {
        let mut items = Vec::with_capacity(4);
        loop {
            if self.at_sequence_end()? {
                break;
            }
            items.push(self.compose_node()?);
        }
        let end_event = self.next_event()?; // SequenceEnd
        self.count_node(start_span)?;
        let span = start_span.merge(end_event.span);
        let resolved_tag = self.apply_tag(tag, start_span)?;
        let node = Node::Sequence(Sequence {
            items,
            tag: resolved_tag,
            style,
            span,
        });
        self.maybe_register_anchor(anchor, &node)?;
        Ok(node)
    }

    fn compose_mapping(
        &mut self,
        start_span: Span,
        anchor: Option<Cow<'a, str>>,
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        style: CollectionStyle,
    ) -> Result<Node<'a>> {
        let mut entries: Vec<(Node<'a>, Node<'a>)> = Vec::with_capacity(4);
        let strict = self.config.strictness == Strictness::Strict;
        // Duplicate-key detection strategy:
        //   ≤ DUP_LINEAR_THRESHOLD keys → linear scan over `entries` (zero alloc).
        //   > DUP_LINEAR_THRESHOLD keys → HashSet promoted on first overflow.
        // Both paths produce identical errors (same key string, same first_span).
        // Cow<'static, str> because we only insert Cow::Owned values above the
        // linear-scan threshold — avoids lifetime conflict with `entries` borrow.
        let mut seen_keys: Option<std::collections::HashSet<Cow<'static, str>>> = None;
        // Merge-key sources, in document order. Each element is one source
        // mapping's entry list; precedence is explicit-wins then earlier-source.
        // Only populated when `self.config.merge_keys` is enabled.
        let mut merges: Vec<Vec<(Node<'a>, Node<'a>)>> = Vec::new();
        loop {
            if self.at_mapping_end()? {
                break;
            }
            let key = self.compose_node()?;
            self.check_key_length(&key)?;

            // Merge key (`<<`): its value supplies DEFAULTS rather than a literal
            // entry. Collected here and folded in after the loop so precedence
            // (explicit-wins, earlier-source-wins) stays trivial. Not subject to
            // strict duplicate detection — merge keys never collide as data.
            if self.config.merge_keys && is_merge_key(&key) {
                let value = self.compose_node()?;
                self.collect_merge_sources(value, &mut merges)?;
                continue;
            }

            if strict {
                if let Node::Scalar(ref s) = key {
                    let is_dup = if entries.len() < DUP_LINEAR_THRESHOLD {
                        // Linear scan — no allocation for small mappings.
                        entries.iter().any(|(k, _)| k.as_str() == Some(&*s.value))
                    } else {
                        // Promote to HashSet on first overflow, then O(1) per key.
                        // Use Cow::Owned to avoid lifetime conflicts (keys are
                        // borrowed from `entries` but the closure can't borrow
                        // `entries` for `'a`; owned strings sidestep the issue).
                        let set = seen_keys.get_or_insert_with(|| {
                            let mut h = std::collections::HashSet::with_capacity(entries.len() + 8);
                            for (k, _) in &entries {
                                if let Some(ks) = k.as_str() {
                                    h.insert(Cow::Owned(ks.to_owned()));
                                }
                            }
                            h
                        });
                        !set.insert(Cow::Owned(s.value.to_string()))
                    };
                    if is_dup {
                        let key_str = key.as_str();
                        let first_span = entries
                            .iter()
                            .find_map(|(k, _)| {
                                if k.as_str() == key_str {
                                    Some(k.span())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(key.span());
                        return Err(Error::new(
                            ErrorKind::DuplicateKey {
                                key: s.value.to_string(),
                                first: first_span,
                            },
                            key.span(),
                        ));
                    }
                    // Note: the HashSet is updated inside the `else` branch above
                    // (non-dup path: `set.insert` is called and returns true).
                    // No additional tracking needed here.
                }
            }
            let value = self.compose_node()?;
            entries.push((key, value));
        }

        // Fold merge sources into the explicit entries. Explicit keys win; among
        // sources, earlier wins over later. Non-scalar merged keys can't collide
        // by text, so they are appended unconditionally.
        if !merges.is_empty() {
            let mut present: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|(k, _)| k.as_str().map(str::to_string))
                .collect();
            for source in merges {
                for (k, v) in source {
                    match k.as_str() {
                        Some(text) => {
                            if present.insert(text.to_string()) {
                                entries.push((k, v));
                            }
                        }
                        None => entries.push((k, v)),
                    }
                }
            }
        }

        let end_event = self.next_event()?; // MappingEnd
        self.count_node(start_span)?;
        let span = start_span.merge(end_event.span);
        let resolved_tag = self.apply_tag(tag, start_span)?;
        let node = Node::Mapping(Mapping {
            entries,
            tag: resolved_tag,
            style,
            span,
        });
        self.maybe_register_anchor(anchor, &node)?;
        Ok(node)
    }

    /// Collects the source(s) for a `<<` merge key into `merges`.
    ///
    /// Accepts a single mapping or a sequence of mappings (per YAML 1.1 merge
    /// semantics). Any other shape — or a non-mapping sequence item — is a
    /// composition error.
    fn collect_merge_sources(
        &self,
        value: Node<'a>,
        merges: &mut Vec<Vec<(Node<'a>, Node<'a>)>>,
    ) -> Result<()> {
        match value {
            Node::Mapping(m) => {
                merges.push(m.entries);
                Ok(())
            }
            Node::Sequence(s) => {
                for item in s.items {
                    match item {
                        Node::Mapping(m) => merges.push(m.entries),
                        other => return Err(Self::merge_value_error(&other)),
                    }
                }
                Ok(())
            }
            other => Err(Self::merge_value_error(&other)),
        }
    }

    /// Builds the error for an invalid `<<` merge value, anchored at the value's span.
    fn merge_value_error(node: &Node<'a>) -> Error {
        Error::new(
            ErrorKind::UnexpectedToken {
                expected: "mapping or sequence of mappings for '<<' merge key".into(),
                found: node_kind(node).into(),
            },
            node.span(),
        )
    }

    // ─── Tag resolution ─────────────────────────────────────────────

    /// Applies the `deny_tags` policy, then delegates to `resolve_tag`.
    ///
    /// Returns `Err(TagsDenied)` when the policy is active and a tag is present.
    fn apply_tag(
        &self,
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        span: Span,
    ) -> Result<Option<Tag<'a>>> {
        if self.config.policies.deny_tags && tag.is_some() {
            return Err(Error::new(ErrorKind::TagsDenied, span));
        }
        Ok(self.resolve_tag(tag, span))
    }

    fn resolve_tag(
        &self,
        tag: Option<(Cow<'a, str>, Cow<'a, str>)>,
        span: Span,
    ) -> Option<Tag<'a>> {
        tag.map(|(handle, suffix)| {
            // Tags are pre-resolved by the parser using %TAG directives.
            // Handle is already the resolved prefix.
            let value = if handle.is_empty() && suffix.is_empty() {
                Cow::Borrowed("!")
            } else {
                Cow::Owned(format!("{handle}{suffix}"))
            };
            Tag { value, span }
        })
    }

    // ─── Anchor management ──────────────────────────────────────────

    fn maybe_register_anchor(
        &mut self,
        anchor: Option<Cow<'a, str>>,
        node: &Node<'a>,
    ) -> Result<()> {
        if self.config.policies.deny_anchors && anchor.is_some() {
            return Err(Error::new(ErrorKind::AnchorsDenied, node.span()));
        }
        if let Some(name) = anchor {
            self.anchors.insert(name.into_owned(), node.clone());
        }
        Ok(())
    }

    fn resolve_alias(&self, name: &str, span: Span) -> Result<Node<'a>> {
        if self.config.policies.deny_anchors {
            return Err(Error::new(ErrorKind::AnchorsDenied, span));
        }
        self.anchors
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(ErrorKind::UndefinedAlias(name.to_string()), span))
    }

    // ─── Resource limit checks ──────────────────────────────────────

    fn count_node(&mut self, span: Span) -> Result<()> {
        self.node_count += 1;
        if self.node_count > self.config.limits.max_node_count {
            return Err(Error::node_count_exceeded(&self.config.limits, span));
        }
        Ok(())
    }

    fn check_alias_limit(&self, span: Span) -> Result<()> {
        if self.alias_expansions > self.config.limits.max_alias_expansions {
            return Err(Error::alias_expansion_exceeded(&self.config.limits, span));
        }
        Ok(())
    }

    fn check_key_length(&self, key: &Node<'_>) -> Result<()> {
        if let Some(s) = key.as_str()
            && s.len() > self.config.limits.max_key_length
        {
            return Err(Error::key_length_exceeded(&self.config.limits, key.span()));
        }
        Ok(())
    }
}

impl<'a> Iterator for Composer<'a> {
    type Item = Result<Node<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.errored {
            return None;
        }
        match self.compose_document() {
            Ok(Some(node)) => Some(Ok(node)),
            Ok(None) => None,
            Err(e) => {
                self.errored = true;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skald_core::error::ParserPolicies;
    use skald_core::limits::ResourceLimits;
    use skald_core::types::{CollectionStyle, ScalarStyle};

    fn compose_with(input: &str, policies: ParserPolicies) -> Result<Node<'_>> {
        let cfg = ParserConfig {
            policies,
            ..Default::default()
        };
        Composer::with_config(input, cfg).next().unwrap()
    }

    #[test]
    fn deny_anchors_rejects_anchor_and_alias() {
        let p = ParserPolicies {
            deny_anchors: true,
            ..Default::default()
        };
        assert!(matches!(
            compose_with("a: &x 1\nb: *x\n", p.clone())
                .unwrap_err()
                .kind,
            ErrorKind::AnchorsDenied
        ));
        assert!(matches!(
            compose_with("x: &anc 1\n", p).unwrap_err().kind,
            ErrorKind::AnchorsDenied
        ));
    }

    #[test]
    fn deny_tags_rejects_explicit_tag() {
        let p = ParserPolicies {
            deny_tags: true,
            ..Default::default()
        };
        assert!(matches!(
            compose_with("x: !!str 5\n", p.clone()).unwrap_err().kind,
            ErrorKind::TagsDenied
        ));
        assert!(matches!(
            compose_with("x: !custom v\n", p).unwrap_err().kind,
            ErrorKind::TagsDenied
        ));
    }

    #[test]
    fn max_scalar_length_rejects_long_scalar() {
        let p = ParserPolicies {
            max_scalar_length: Some(4),
            ..Default::default()
        };
        assert!(matches!(
            compose_with("k: abcdefg\n", p.clone()).unwrap_err().kind,
            ErrorKind::ScalarLengthLimitExceeded {
                limit: 4,
                actual: 7
            }
        ));
        assert!(compose_with("k: abcd\n", p).is_ok());
    }

    #[test]
    fn policies_off_by_default_unchanged() {
        assert!(
            compose_with(
                "a: &x 1\nb: *x\nc: !!str hello\n",
                ParserPolicies::default()
            )
            .is_ok()
        );
    }

    fn compose(input: &str) -> Vec<Node<'_>> {
        Composer::new(input)
            .map(|r| r.expect("compose error"))
            .collect()
    }

    fn compose_one(input: &str) -> Node<'_> {
        let mut docs = compose(input);
        assert_eq!(docs.len(), 1, "expected 1 document, got {}", docs.len());
        docs.remove(0)
    }

    // ─── Basic scalars ──────────────────────────────────────────────

    #[test]
    fn plain_scalar() {
        let node = compose_one("hello");
        assert_eq!(node.as_str(), Some("hello"));
        assert!(node.is_scalar());
    }

    #[test]
    fn scalar_preserves_style() {
        if let Node::Scalar(s) = compose_one("hello") {
            assert_eq!(s.style, ScalarStyle::Plain);
        } else {
            panic!("expected scalar");
        }
    }

    // ─── Collections ────────────────────────────────────────────────

    #[test]
    fn block_mapping() {
        let node = compose_one("a: 1\nb: 2");
        let entries = node.as_mapping().expect("expected mapping");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.as_str(), Some("a"));
        assert_eq!(entries[0].1.as_str(), Some("1"));
        assert_eq!(entries[1].0.as_str(), Some("b"));
        assert_eq!(entries[1].1.as_str(), Some("2"));
    }

    #[test]
    fn block_sequence() {
        let node = compose_one("- one\n- two");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("one"));
        assert_eq!(items[1].as_str(), Some("two"));
    }

    #[test]
    fn flow_mapping() {
        let node = compose_one("{a: 1, b: 2}");
        let entries = node.as_mapping().expect("expected mapping");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.as_str(), Some("a"));
        assert_eq!(entries[0].1.as_str(), Some("1"));
    }

    #[test]
    fn flow_sequence() {
        let node = compose_one("[1, 2, 3]");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_str(), Some("1"));
        assert_eq!(items[2].as_str(), Some("3"));
    }

    #[test]
    fn empty_mapping() {
        let node = compose_one("{}");
        let entries = node.as_mapping().expect("expected mapping");
        assert!(entries.is_empty());
    }

    #[test]
    fn empty_sequence() {
        let node = compose_one("[]");
        let items = node.as_sequence().expect("expected sequence");
        assert!(items.is_empty());
    }

    #[test]
    fn mapping_preserves_style() {
        if let Node::Mapping(m) = compose_one("a: 1") {
            assert_eq!(m.style, CollectionStyle::Block);
        }
        if let Node::Mapping(m) = compose_one("{a: 1}") {
            assert_eq!(m.style, CollectionStyle::Flow);
        }
    }

    #[test]
    fn sequence_preserves_style() {
        if let Node::Sequence(s) = compose_one("- 1") {
            assert_eq!(s.style, CollectionStyle::Block);
        }
        if let Node::Sequence(s) = compose_one("[1]") {
            assert_eq!(s.style, CollectionStyle::Flow);
        }
    }

    // ─── Nesting ────────────────────────────────────────────────────

    #[test]
    fn nested_mapping_in_sequence() {
        let node = compose_one("- a: 1\n- b: 2");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        assert!(items[0].is_mapping());
        assert!(items[1].is_mapping());
    }

    #[test]
    fn nested_sequence_in_mapping() {
        let node = compose_one("items:\n  - one\n  - two");
        let entries = node.as_mapping().expect("expected mapping");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0.as_str(), Some("items"));
        let items = entries[0].1.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("one"));
    }

    // ─── Anchors / Aliases ──────────────────────────────────────────

    #[test]
    fn anchor_and_alias_scalar() {
        let node = compose_one("- &first foo\n- *first");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("foo"));
        assert_eq!(items[1].as_str(), Some("foo"));
    }

    #[test]
    fn anchor_on_mapping() {
        let node = compose_one("- &m\n  a: 1\n- *m");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        assert!(items[0].is_mapping());
        assert!(items[1].is_mapping());
        assert_eq!(items[1].as_mapping().unwrap().len(), 1);
    }

    #[test]
    fn anchor_redefinition() {
        // YAML spec: anchor redefinition is allowed, last one wins
        let node = compose_one("- &a one\n- &a two\n- *a");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items[2].as_str(), Some("two"));
    }

    #[test]
    fn undefined_alias_error() {
        let mut composer = Composer::new("*nope");
        let result: Vec<_> = composer.by_ref().collect();
        assert!(result.iter().any(|r| r.is_err()));
    }

    // ─── Documents ──────────────────────────────────────────────────

    #[test]
    fn empty_stream() {
        let docs = compose("");
        assert!(docs.is_empty());
    }

    #[test]
    fn explicit_document() {
        let node = compose_one("---\nhello\n...");
        assert_eq!(node.as_str(), Some("hello"));
    }

    #[test]
    fn multi_document() {
        let docs = compose("---\na\n---\nb");
        assert!(
            docs.len() >= 2,
            "expected at least 2 documents, got {}",
            docs.len()
        );
        assert_eq!(docs[0].as_str(), Some("a"));
        assert_eq!(docs[1].as_str(), Some("b"));
    }

    // ─── Tags ───────────────────────────────────────────────────────

    #[test]
    fn tag_secondary_handle() {
        let node = compose_one("!!str hello");
        if let Node::Scalar(s) = &node {
            let tag = s.tag.as_ref().expect("expected tag");
            assert_eq!(tag.value.as_ref(), "tag:yaml.org,2002:str");
        } else {
            panic!("expected scalar");
        }
    }

    #[test]
    fn tag_primary_handle() {
        let node = compose_one("!local hello");
        if let Node::Scalar(s) = &node {
            let tag = s.tag.as_ref().expect("expected tag");
            assert_eq!(tag.value.as_ref(), "!local");
        } else {
            panic!("expected scalar");
        }
    }

    // ─── Spans ──────────────────────────────────────────────────────

    #[test]
    fn span_tracking() {
        let node = compose_one("hello");
        let span = node.span();
        // Scanner uses 0-based columns; line is also 0-based internally
        assert!(span.start.offset == 0, "scalar starts at offset 0");
    }

    // ─── Duplicate key detection ────────────────────────────────────

    #[test]
    fn duplicate_key_strict() {
        let config = ParserConfig {
            strictness: Strictness::Strict,
            ..Default::default()
        };
        let mut composer = Composer::with_config("a: 1\na: 2", config);
        let result: Vec<_> = composer.by_ref().collect();
        assert!(
            result.iter().any(|r| r.is_err()),
            "expected duplicate key error"
        );
    }

    #[test]
    fn duplicate_key_lenient() {
        let config = ParserConfig {
            strictness: Strictness::Lenient,
            ..Default::default()
        };
        let mut composer = Composer::with_config("a: 1\na: 2", config);
        let node = composer.next().unwrap().unwrap();
        let entries = node.as_mapping().expect("expected mapping");
        assert_eq!(entries.len(), 2, "lenient mode preserves both entries");
    }

    // ─── Resource limits ────────────────────────────────────────────

    #[test]
    fn node_count_limit() {
        let config = ParserConfig {
            limits: ResourceLimits {
                max_node_count: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut composer = Composer::with_config("[1, 2, 3]", config);
        let result: Vec<_> = composer.by_ref().collect();
        assert!(
            result.iter().any(|r| r.is_err()),
            "expected node count error"
        );
    }

    #[test]
    fn key_length_limit() {
        let config = ParserConfig {
            limits: ResourceLimits {
                max_key_length: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut composer = Composer::with_config("long_key: value", config);
        let result: Vec<_> = composer.by_ref().collect();
        assert!(
            result.iter().any(|r| r.is_err()),
            "expected key length error"
        );
    }

    #[test]
    fn alias_expansion_limit() {
        let config = ParserConfig {
            limits: ResourceLimits {
                max_alias_expansions: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut composer = Composer::with_config("- &a foo\n- *a\n- *a", config);
        let result: Vec<_> = composer.by_ref().collect();
        assert!(
            result.iter().any(|r| r.is_err()),
            "expected alias expansion error"
        );
    }

    // ─── Error handling ─────────────────────────────────────────────

    #[test]
    fn composer_stops_after_error() {
        let mut composer = Composer::new("*undefined");
        let results: Vec<_> = composer.by_ref().collect();
        let errors: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
        assert_eq!(errors.len(), 1, "expected exactly one error");
    }

    // ─── Convenience function ───────────────────────────────────────

    #[test]
    fn compose_all_function() {
        let docs = compose_all("hello").unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].as_str(), Some("hello"));
    }

    #[test]
    fn compose_all_empty() {
        let docs = compose_all("").unwrap();
        assert!(docs.is_empty());
    }

    #[test]
    fn compose_node_rejects_non_node_event() {
        use skald_core::types::{Position, Span};
        // A well-formed parser never hands a collection-end event to node
        // composition, but the defensive arm must reject one as malformed
        // rather than mishandle it. Drive the dispatch with a synthetic event.
        let mut composer = Composer::new("x");
        let event = Event {
            kind: EventKind::SequenceEnd,
            span: Span::point(Position::start()),
        };
        let err = composer.compose_node_from_event(event).unwrap_err();
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn compose_all_multi() {
        let docs = compose_all("---\na\n---\nb").unwrap();
        assert!(docs.len() >= 2);
    }

    #[test]
    fn compose_all_propagates_error() {
        let result = compose_all("*undefined");
        assert!(result.is_err());
    }

    // ─── Limit checks (count_node, check_key_length) ────────────────

    #[test]
    fn max_node_count_exceeded() {
        let config = ParserConfig {
            limits: ResourceLimits {
                max_node_count: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        // Three scalars + sequence node = 4 nodes total, exceeds limit of 2.
        let result = compose_all_with_config("- a\n- b\n- c", config);
        assert!(result.is_err(), "expected NodeCountExceeded error");
    }

    #[test]
    fn max_key_length_exceeded() {
        let config = ParserConfig {
            limits: ResourceLimits {
                max_key_length: 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = compose_all_with_config("longkey: 1", config);
        assert!(result.is_err(), "expected KeyLengthExceeded error");
    }

    fn compose_all_with_config<'a>(input: &'a str, config: ParserConfig) -> Result<Vec<Node<'a>>> {
        Composer::with_config(input, config).collect()
    }

    // ─── Duplicate key detection (strict mode) ──────────────────────

    #[test]
    fn duplicate_key_in_strict_mode_errors_with_first_span() {
        // The composer should return an error pointing at the duplicate
        // key, AND the error should reference the first occurrence's span.
        let result = compose_all("a: 1\na: 2");
        let err = result.expect_err("expected DuplicateKey");
        // Use Debug-formatted error to assert the variant kind.
        let msg = format!("{err:?}");
        assert!(
            msg.contains("DuplicateKey") || msg.contains("Duplicate"),
            "expected DuplicateKey error, got: {msg}"
        );
    }

    #[test]
    fn duplicate_key_lenient_mode_accepts() {
        let config = ParserConfig {
            strictness: Strictness::Lenient,
            ..Default::default()
        };
        let result = compose_all_with_config("a: 1\na: 2", config);
        assert!(result.is_ok(), "lenient mode should accept duplicates");
        let docs = result.unwrap();
        // Lenient mode keeps both entries
        let entries = docs[0].as_mapping().expect("expected mapping");
        assert_eq!(entries.len(), 2);
    }

    // ─── Tag resolution edges ───────────────────────────────────────

    #[test]
    fn non_specific_tag_resolves_to_exclamation() {
        // A bare `!` tag with empty handle/suffix should resolve to "!".
        // This is the line 304 Cow::Borrowed("!") branch.
        let node = compose_one("! plain");
        if let Node::Scalar(s) = &node {
            assert!(s.tag.is_some(), "expected tag");
            let tag = s.tag.as_ref().unwrap();
            assert_eq!(tag.value, "!");
        } else {
            panic!("expected scalar with tag, got {node:?}");
        }
    }

    // ─── Multi-document state reset ─────────────────────────────────

    #[test]
    fn anchors_do_not_leak_between_documents() {
        // Anchor `&a` defined in doc 1 must not resolve as alias in doc 2.
        let mut composer = Composer::new("---\n&a foo\n---\n*a");
        let docs: Vec<_> = composer.by_ref().collect();
        assert_eq!(docs.len(), 2);
        // First doc succeeds; second should error because *a is undefined
        // (anchor table was cleared at doc boundary on line 92).
        assert!(docs[0].is_ok(), "first doc should compose");
        assert!(
            docs[1].is_err(),
            "second doc should fail with undefined alias because anchor table reset"
        );
    }

    #[test]
    fn node_count_resets_between_documents() {
        // node_count is per-document. Two separate documents each
        // containing 2 nodes should not exceed max_node_count = 3.
        let config = ParserConfig {
            limits: ResourceLimits {
                max_node_count: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = compose_all_with_config("---\n- a\n- b\n---\n- x\n- y", config);
        assert!(
            result.is_ok(),
            "node_count reset between docs (line 93) should allow this"
        );
    }

    // ─── Iterator behavior after error ──────────────────────────────

    #[test]
    fn iterator_returns_none_after_error() {
        let mut composer = Composer::new("*undefined");
        let first = composer.next();
        assert!(first.is_some() && first.unwrap().is_err());
        // After first error, errored flag (line 363-364) should make
        // subsequent next() return None.
        assert!(composer.next().is_none());
        assert!(composer.next().is_none());
    }

    #[test]
    fn iterator_returns_none_when_stream_exhausted() {
        let mut composer = Composer::new("hello");
        let _first = composer.next().expect("first doc");
        // Stream exhausted; subsequent next() should return None.
        assert!(composer.next().is_none());
    }

    // ─── Sequence/mapping with explicit anchors and tags ────────────

    #[test]
    fn anchor_on_sequence() {
        let node = compose_one("- &s\n  - one\n- *s");
        let items = node.as_sequence().expect("expected sequence");
        assert_eq!(items.len(), 2);
        // items[1] is the aliased sequence
        assert!(items[1].is_sequence());
    }

    #[test]
    fn tag_on_mapping() {
        // Tag on a mapping triggers compose_mapping's resolve_tag branch.
        let docs = compose_all("!!map\na: 1").unwrap();
        if let Node::Mapping(m) = &docs[0] {
            assert!(m.tag.is_some());
        } else {
            panic!("expected mapping");
        }
    }

    #[test]
    fn tag_on_sequence() {
        let docs = compose_all("!!seq\n- one").unwrap();
        if let Node::Sequence(s) = &docs[0] {
            assert!(s.tag.is_some());
        } else {
            panic!("expected sequence");
        }
    }

    // ─── Verbatim empty tag resolves to non-specific "!" (line 304) ──

    #[test]
    fn verbatim_empty_tag_resolves_to_exclamation() {
        // `!<>` produces handle="" and suffix="" — the composer's
        // resolve_tag maps both-empty to the non-specific tag "!".
        let node = compose_one("!<>");
        if let Node::Scalar(s) = &node {
            let tag = s.tag.as_ref().expect("expected tag");
            assert_eq!(tag.value, "!");
        } else {
            panic!("expected scalar, got {node:?}");
        }
    }

    // ─── compose_node `other` arm + error propagation (192-197) ─────

    #[test]
    fn flow_mapping_trailing_comma_then_eof_errors() {
        // `{a: 1, ` — after the trailing comma the parser feeds a
        // stream-end event where node content is expected, exercising
        // compose_node's catch-all `other` arm (Duplicate? no: unexpected).
        let result = compose_all("{a: 1, ");
        let err = result.expect_err("expected error");
        assert!(matches!(err.kind, ErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn unterminated_flow_mapping_propagates_parser_error() {
        // `{a` — parser errors mid-mapping; the error surfaces through
        // the composer's next_event/peek_event error-propagation paths.
        let result = compose_all("{a");
        assert!(result.is_err());
    }

    #[test]
    fn unterminated_flow_sequence_propagates_parser_error() {
        // `[a` — same as above for a flow sequence.
        let result = compose_all("[a");
        assert!(result.is_err());
    }

    #[test]
    fn empty_anchor_name_propagates_parser_error() {
        // `&` — the parser errors at the root node position; the error is
        // returned by compose_node's `next_event` (composer line 127,
        // the `Some(Err(e)) => Err(e)` arm).
        let result = compose_all("&");
        assert!(matches!(
            result.expect_err("err").kind,
            ErrorKind::InvalidAnchor(_)
        ));
    }

    #[test]
    fn directive_without_document_propagates_parser_error() {
        // `%TAG` — the parser errors while consuming DocumentStart; the
        // error flows through the composer's `next_event` error arm.
        let result = compose_all("%TAG");
        assert!(result.is_err());
    }

    #[test]
    fn empty_alias_name_propagates_parser_error() {
        // `*` — empty alias name; parser error propagated via next_event.
        let result = compose_all("*");
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_key_skips_non_matching_prior_entry() {
        // `a: 1\nb: 2\nb: 3` — when the duplicate `b` is detected, the
        // first_span search iterates entries [a, b]; the `a` entry does
        // not match (find_map closure returns None — composer line 262)
        // before the matching `b` entry is found.
        let result = compose_all("a: 1\nb: 2\nb: 3");
        let err = result.expect_err("expected duplicate key error");
        assert!(matches!(err.kind, ErrorKind::DuplicateKey { .. }));
    }

    #[test]
    fn malformed_flow_inputs_error_without_panic() {
        // A spread of malformed flow collections; each must surface an
        // error (propagated through compose_node / peek_event / next_event)
        // rather than panicking. Exercises error-propagation branches.
        for input in ["[", "{", "{,}", "[1 2", "{a: b\nc: d}", "[a, ", "{a: 1, "] {
            let result = compose_all(input);
            assert!(result.is_err(), "expected error for {input:?}");
        }
    }

    #[test]
    fn alias_inside_flow_sequence_undefined_errors() {
        // `[*missing]` — undefined alias detected while composing the
        // flow sequence items (error propagation through compose_node).
        let result = compose_all("[*missing]");
        assert!(matches!(
            result.expect_err("err").kind,
            ErrorKind::UndefinedAlias(_)
        ));
    }

    // ─── Merge keys (<<) ────────────────────────────────────────────

    fn compose_one_merge(input: &str) -> Node<'_> {
        let cfg = ParserConfig {
            merge_keys: true,
            ..Default::default()
        };
        Composer::with_config(input, cfg).next().unwrap().unwrap()
    }

    #[test]
    fn merge_key_merges_anchor_mapping() {
        let doc = compose_one_merge("base: &b\n  a: 1\n  b: 2\nderived:\n  <<: *b\n  b: 3\n");
        // derived = { a: 1 (merged), b: 3 (own wins) }
        let m = doc.as_mapping().unwrap();
        let derived = m
            .iter()
            .find(|(k, _)| k.as_str() == Some("derived"))
            .unwrap()
            .1
            .as_mapping()
            .unwrap();
        let get = |k: &str| {
            derived
                .iter()
                .find(|(kk, _)| kk.as_str() == Some(k))
                .map(|(_, v)| v.as_str().unwrap().to_string())
        };
        assert_eq!(get("a"), Some("1".into()));
        assert_eq!(get("b"), Some("3".into())); // own key wins over merged
        assert!(
            derived.iter().all(|(k, _)| k.as_str() != Some("<<")),
            "<< must not remain as a key"
        );
    }

    #[test]
    fn merge_key_sequence_first_source_wins() {
        let doc =
            compose_one_merge("a: &a { x: 1, y: 1 }\nb: &b { y: 2, z: 2 }\nm:\n  <<: [*a, *b]\n");
        let m = doc.as_mapping().unwrap();
        let mm = m
            .iter()
            .find(|(k, _)| k.as_str() == Some("m"))
            .unwrap()
            .1
            .as_mapping()
            .unwrap();
        let get = |k: &str| {
            mm.iter()
                .find(|(kk, _)| kk.as_str() == Some(k))
                .map(|(_, v)| v.as_str().unwrap().to_string())
        };
        assert_eq!(get("x"), Some("1".into()));
        assert_eq!(get("y"), Some("1".into())); // *a (earlier) wins over *b
        assert_eq!(get("z"), Some("2".into()));
    }

    #[test]
    fn merge_key_disabled_keeps_literal_key() {
        // default config: merge_keys = false → "<<" is a normal key
        let doc2 = Composer::new("anchor: &a {p: 1}\nlit:\n  <<: *a\n")
            .next()
            .unwrap()
            .unwrap();
        let m = doc2.as_mapping().unwrap();
        let lit = m
            .iter()
            .find(|(k, _)| k.as_str() == Some("lit"))
            .unwrap()
            .1
            .as_mapping()
            .unwrap();
        assert!(
            lit.iter().any(|(k, _)| k.as_str() == Some("<<")),
            "merge disabled: << stays literal"
        );
    }

    fn compose_merge_result(input: &str) -> Result<Node<'_>> {
        let cfg = ParserConfig {
            merge_keys: true,
            ..Default::default()
        };
        Composer::with_config(input, cfg).next().unwrap()
    }

    #[test]
    fn merge_value_scalar_is_error() {
        // `<<: scalar` — a plain scalar merge value is not a mapping/sequence.
        let err = compose_merge_result("d:\n  <<: oops\n").unwrap_err();
        assert!(
            matches!(err.kind, ErrorKind::UnexpectedToken { .. }),
            "scalar merge value must be a composition error, got {:?}",
            err.kind
        );
    }

    #[test]
    fn merge_value_sequence_with_non_mapping_item_is_error() {
        // `<<: [*a, scalar]` — a sequence item that is not a mapping.
        let err = compose_merge_result("a: &a { x: 1 }\nd:\n  <<: [*a, plain]\n").unwrap_err();
        assert!(
            matches!(err.kind, ErrorKind::UnexpectedToken { .. }),
            "non-mapping merge sequence item must error, got {:?}",
            err.kind
        );
    }

    #[test]
    fn merge_source_preserves_complex_non_scalar_key() {
        // A merged mapping whose key is itself a collection (a complex key)
        // exercises the `None` arm: such keys can't be deduplicated by text and
        // are pushed through verbatim.
        let doc = compose_merge_result("base: &b\n  ? [1, 2]\n  : v\nd:\n  <<: *b\n").unwrap();
        let m = doc.as_mapping().unwrap();
        let d = m
            .iter()
            .find(|(k, _)| k.as_str() == Some("d"))
            .unwrap()
            .1
            .as_mapping()
            .unwrap();
        assert!(
            d.iter().any(|(k, _)| matches!(k, Node::Sequence(_))),
            "complex sequence key must survive the merge"
        );
    }

    #[test]
    fn node_kind_names_each_variant() {
        // node_kind is reached on every merge-error shape: scalar, sequence,
        // and (via a sequence item) mapping rejection. Scalar + sequence are
        // already covered by the error tests above; assert the helper directly
        // for the mapping arm too.
        use skald_core::types::Position;
        let span = Span::point(Position::start());
        let scalar = Node::Scalar(Scalar {
            value: Cow::Borrowed("x"),
            tag: None,
            style: ScalarStyle::Plain,
            span,
        });
        let seq = Node::Sequence(Sequence {
            items: vec![],
            tag: None,
            style: CollectionStyle::Flow,
            span,
        });
        let map = Node::Mapping(Mapping {
            entries: vec![],
            tag: None,
            style: CollectionStyle::Flow,
            span,
        });
        assert_eq!(node_kind(&scalar), "scalar");
        assert_eq!(node_kind(&seq), "sequence");
        assert_eq!(node_kind(&map), "mapping");
    }

    #[test]
    fn deny_anchors_rejects_bare_alias() {
        // An alias with no preceding (registerable) anchor reaches
        // `resolve_alias`, whose deny_anchors guard fires before lookup.
        let p = ParserPolicies {
            deny_anchors: true,
            ..Default::default()
        };
        assert!(matches!(
            compose_with("b: *undefined\n", p).unwrap_err().kind,
            ErrorKind::AnchorsDenied
        ));
    }
}
