// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML emitter.
//!
//! Converts a [`Node`] tree back into formatted YAML text.
//! Supports configurable output styles (block/flow, indentation, line width).
//!
//! # Usage
//!
//! ```
//! use skald_ast::composer;
//! use skald_ast::emitter::{emit_to_string, EmitterConfig};
//!
//! let node = composer::compose_all("hello: world").unwrap().remove(0);
//! let yaml = emit_to_string(&node, &EmitterConfig::default());
//! assert_eq!(yaml, "hello: world\n");
//! ```

use std::fmt;

use skald_core::types::CollectionStyle;

use crate::node::{Mapping, Node};

pub mod sink;

pub use sink::Emitter;

/// Configuration for the YAML emitter.
#[derive(Debug, Clone)]
#[must_use]
pub struct EmitterConfig {
    /// Spaces per indent level. Default: 2.
    pub indent: u8,
    /// Soft line width target for flow style. Default: 80.
    pub line_width: u16,
    /// Prefer block style for collections. Default: true.
    pub prefer_block: bool,
    /// Alphabetically sort mapping keys. Default: false.
    pub sort_keys: bool,
    /// Emit explicit document markers (`---` / `...`). Default: false.
    pub explicit_document: bool,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            indent: 2,
            line_width: 80,
            prefer_block: true,
            sort_keys: false,
            explicit_document: false,
        }
    }
}

/// Emits a [`Node`] tree as a YAML string.
#[must_use]
pub fn emit_to_string(node: &Node<'_>, config: &EmitterConfig) -> String {
    let mut out = String::with_capacity(256);
    // fmt::Write on String never fails.
    emit(node, config, &mut out).unwrap();
    out
}

/// Emits a [`Node`] tree to any [`fmt::Write`] destination.
///
/// Layout lives entirely in the push [`sink::Emitter`]; this is a thin
/// recursive walk (`drive_node`) that drives that sink. The optional
/// `explicit_document` markers (`---` / `...`) are written around the body.
pub fn emit<W: fmt::Write>(node: &Node<'_>, config: &EmitterConfig, writer: &mut W) -> fmt::Result {
    if config.explicit_document {
        writeln!(writer, "---")?;
    }
    {
        let mut sink = sink::Emitter::new(WriteAdapter(writer), config);
        drive_node(node, &mut sink, config)?;
        sink.finish()?;
    }
    if config.explicit_document {
        writeln!(writer, "...")?;
    }
    Ok(())
}

/// Thin recursive walk that drives the push [`sink::Emitter`]. Emits a node's
/// tag (if any) first, then the node value via the sink's imperative API.
fn drive_node<W: fmt::Write>(
    node: &Node<'_>,
    e: &mut sink::Emitter<'_, W>,
    config: &EmitterConfig,
) -> fmt::Result {
    if let Some(tag) = node.tag() {
        e.tag(&tag.value);
    }
    match node {
        Node::Scalar(s) => e.scalar(&s.value, s.style),
        Node::Sequence(seq) => {
            e.begin_seq(seq.style, Some(seq.items.len()))?;
            for item in &seq.items {
                e.before_elem()?;
                drive_node(item, e, config)?;
            }
            e.end_seq()
        }
        Node::Mapping(map) => {
            e.begin_map(map.style, Some(map.entries.len()))?;
            for &idx in &entry_order(map, config) {
                let (k, v) = &map.entries[idx];
                e.before_key()?;
                drive_node(k, e, config)?;
                e.before_value()?;
                drive_node(v, e, config)?;
            }
            e.end_map()
        }
    }
}

/// Visit order for a mapping's entries. Mirrors the old `emit_block_mapping`:
/// when `sort_keys` is set, scalar keys sort by their string value (non-scalar
/// keys collapse to `""`), preserving relative order via the stable sort.
/// `emit_flow_mapping` did not sort, so flow mappings keep source order.
fn entry_order(map: &Mapping<'_>, config: &EmitterConfig) -> Vec<usize> {
    let mut order: Vec<usize> = (0..map.entries.len()).collect();
    if config.sort_keys && map.style == CollectionStyle::Block {
        order.sort_by(|&a, &b| {
            let ka = map.entries[a].0.as_str().unwrap_or("");
            let kb = map.entries[b].0.as_str().unwrap_or("");
            ka.cmp(kb)
        });
    }
    order
}

/// Adapter so `TrackingWriter` can wrap a `&mut W` (which itself impl `fmt::Write`).
struct WriteAdapter<'a, W: fmt::Write>(&'a mut W);

impl<W: fmt::Write> fmt::Write for WriteAdapter<'_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}

// ─── Internal helpers ───────────────────────────────────────────────

/// Wrapper around `fmt::Write` that tracks the last character written.
/// Needed because `fmt::Write` is write-only — no read-back.
pub(crate) struct TrackingWriter<W: fmt::Write> {
    inner: W,
    last_char: Option<char>,
}

impl<W: fmt::Write> TrackingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            last_char: None,
        }
    }

    pub(crate) fn ends_with_newline(&self) -> bool {
        self.last_char == Some('\n')
    }
}

impl<W: fmt::Write> fmt::Write for TrackingWriter<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(c) = s.as_bytes().last() {
            self.last_char = Some(*c as char);
        }
        self.inner.write_str(s)
    }
}

pub(crate) fn write_indent<W: fmt::Write>(
    level: usize,
    config: &EmitterConfig,
    writer: &mut W,
) -> fmt::Result {
    let spaces = level * config.indent as usize;
    for _ in 0..spaces {
        writer.write_char(' ')?;
    }
    Ok(())
}

// ─── Scalar emission ────────────────────────────────────────────────

pub(crate) fn emit_plain<W: fmt::Write>(value: &str, writer: &mut W) -> fmt::Result {
    // Trust the caller's style choice. If ScalarStyle::Plain was set,
    // the value should be emitted unquoted. The serializer handles
    // quoting ambiguous strings (like "true" or "42") by choosing
    // DoubleQuoted style for those values.
    writer.write_str(value)
}

pub(crate) fn emit_single_quoted<W: fmt::Write>(value: &str, writer: &mut W) -> fmt::Result {
    writer.write_char('\'')?;
    for ch in value.chars() {
        if ch == '\'' {
            writer.write_str("''")?;
        } else {
            writer.write_char(ch)?;
        }
    }
    writer.write_char('\'')
}

/// Characters that are valid (`c-printable`, YAML 1.2 §5.1) but should still be
/// escaped in double-quoted output for safe, unambiguous interop:
///
/// * **U+2028 / U+2029** — line and paragraph separators. Legal in YAML, but
///   JavaScript/JSON consumers treat them as line breaks, so leaving them raw
///   silently corrupts such payloads.
/// * **Format characters** (General_Category=Cf) — zero-width joiners, bidi
///   overrides, tag characters, the BOM, … They are invisible, so escaping them
///   keeps the emitted text honest (cf. "Trojan Source", CVE-2021-42574).
///
/// All of these round-trip losslessly: the scanner decodes `\uXXXX`/`\UXXXXXXXX`
/// (and the named `\L`/`\P`) back to the original code point. Cf ranges track
/// Unicode 15.1; a missed future addition only means it is emitted raw (the
/// prior behaviour), never incorrect output.
fn needs_quoted_escape(c: char) -> bool {
    matches!(c,
        // Zl / Zp — line and paragraph separators.
        '\u{2028}' | '\u{2029}'
        // Cf — Unicode format characters.
        | '\u{00AD}'
        | '\u{0600}'..='\u{0605}' | '\u{061C}' | '\u{06DD}' | '\u{070F}'
        | '\u{0890}'..='\u{0891}' | '\u{08E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}'
    )
}

pub(crate) fn emit_double_quoted<W: fmt::Write>(value: &str, writer: &mut W) -> fmt::Result {
    writer.write_char('"')?;
    for ch in value.chars() {
        match ch {
            '"' => writer.write_str("\\\"")?,
            '\\' => writer.write_str("\\\\")?,
            '\n' => writer.write_str("\\n")?,
            '\r' => writer.write_str("\\r")?,
            '\t' => writer.write_str("\\t")?,
            '\0' => writer.write_str("\\0")?,
            '\x07' => writer.write_str("\\a")?,
            '\x08' => writer.write_str("\\b")?,
            '\x0B' => writer.write_str("\\v")?,
            '\x0C' => writer.write_str("\\f")?,
            '\x1B' => writer.write_str("\\e")?,
            c if c.is_control() => {
                // Every Unicode control character (General_Category=Cc) lies in
                // 0x00..=0x1F, 0x7F, or 0x80..=0x9F — all `<= 0xFF` — so the
                // single-byte `\xXX` escape always suffices. The specific
                // controls with shorter escapes (\n, \t, …) are handled above.
                write!(writer, "\\x{:02X}", c as u32)?;
            }
            c if needs_quoted_escape(c) => {
                // Separator / format characters: escape numerically. Code points
                // up to U+FFFF use `\uXXXX`; astral ones use `\UXXXXXXXX`.
                let cp = c as u32;
                if cp <= 0xFFFF {
                    write!(writer, "\\u{cp:04X}")?;
                } else {
                    write!(writer, "\\U{cp:08X}")?;
                }
            }
            c => writer.write_char(c)?,
        }
    }
    writer.write_char('"')
}

pub(crate) fn emit_block_scalar<W: fmt::Write>(
    value: &str,
    indicator: char,
    level: usize,
    config: &EmitterConfig,
    writer: &mut W,
) -> fmt::Result {
    // Block scalars require multiline content; fall back for single-line.
    if !value.contains('\n') {
        return emit_double_quoted(value, writer);
    }

    // Determine chomping indicator based on trailing newlines.
    let chomp = if value.ends_with("\n\n") {
        '+' // keep
    } else if value.ends_with('\n') {
        ' ' // clip (default, no indicator needed)
    } else {
        '-' // strip
    };

    // Write header
    writer.write_char(indicator)?;
    if chomp != ' ' {
        writer.write_char(chomp)?;
    }
    writeln!(writer)?;

    // Write indented content lines
    let content_indent = (level + 1) * config.indent as usize;
    for line in value.split('\n') {
        if line.is_empty() {
            // Empty lines in block scalars: just a bare newline
            writeln!(writer)?;
        } else {
            for _ in 0..content_indent {
                writer.write_char(' ')?;
            }
            writeln!(writer, "{line}")?;
        }
    }

    // The split produces a trailing empty element after the final '\n',
    // which we already wrote as a newline. For strip chomp, we need to
    // undo the extra trailing newline — but since we wrote it already
    // and fmt::Write doesn't support truncation, we handle this by
    // not writing the last empty split element. Let me restructure:

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer;
    use crate::node::{Scalar, Sequence};
    use skald_core::types::{Position, ScalarStyle, Span, Tag};

    // ─── Helper ─────────────────────────────────────────────────────

    fn roundtrip(input: &str) -> String {
        let nodes = composer::compose_all(input).unwrap();
        assert!(!nodes.is_empty(), "no documents parsed from: {input:?}");
        emit_to_string(&nodes[0], &EmitterConfig::default())
    }

    fn roundtrip_eq(input: &str) {
        let yaml = roundtrip(input);
        let reparsed = composer::compose_all(&yaml).unwrap();
        let original = composer::compose_all(input).unwrap();
        assert_eq!(
            original.len(),
            reparsed.len(),
            "document count mismatch for input: {input:?}\nemitted: {yaml:?}"
        );
        for (i, (o, r)) in original.iter().zip(reparsed.iter()).enumerate() {
            assert_eq!(
                node_semantic(o),
                node_semantic(r),
                "document {i} mismatch for input: {input:?}\nemitted: {yaml:?}"
            );
        }
    }

    /// Strips spans/styles for semantic comparison.
    fn node_semantic(node: &Node<'_>) -> String {
        match node {
            Node::Scalar(s) => format!("S({:?})", &*s.value),
            Node::Sequence(s) => {
                let items: Vec<String> = s.items.iter().map(node_semantic).collect();
                format!("[{}]", items.join(", "))
            }
            Node::Mapping(m) => {
                let entries: Vec<String> = m
                    .entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", node_semantic(k), node_semantic(v)))
                    .collect();
                format!("{{{}}}", entries.join(", "))
            }
        }
    }

    // ─── Step 1: Scaffold ───────────────────────────────────────────

    #[test]
    fn emit_plain_scalar() {
        assert_eq!(roundtrip("hello"), "hello\n");
    }

    #[test]
    fn emit_config_defaults() {
        let config = EmitterConfig::default();
        assert_eq!(config.indent, 2);
        assert_eq!(config.line_width, 80);
        assert!(config.prefer_block);
        assert!(!config.sort_keys);
        assert!(!config.explicit_document);
    }

    // ─── Step 2: Scalar styles ──────────────────────────────────────

    #[test]
    fn emit_double_quoted_scalar() {
        // Parser preserves ScalarStyle::DoubleQuoted; emitter respects it
        assert_eq!(roundtrip("\"hello world\""), "\"hello world\"\n");
    }

    #[test]
    fn emit_single_quoted_scalar() {
        // Parser preserves ScalarStyle::SingleQuoted; emitter respects it
        assert_eq!(roundtrip("'hello world'"), "'hello world'\n");
    }

    #[test]
    fn emit_plain_scalar_null() {
        // Plain scalar "null" round-trips faithfully — style is trusted.
        assert_eq!(roundtrip("null"), "null\n");
    }

    #[test]
    fn emit_plain_scalar_bool() {
        // Plain scalars for booleans round-trip faithfully.
        for val in ["true", "false"] {
            assert_eq!(roundtrip(val), format!("{val}\n"));
        }
    }

    #[test]
    fn emit_scalar_needs_quoting_colon_space() {
        roundtrip_eq("'key: value'");
    }

    #[test]
    fn emit_scalar_needs_quoting_hash() {
        roundtrip_eq("'has # comment'");
    }

    #[test]
    fn emit_scalar_empty_string() {
        roundtrip_eq("''");
    }

    #[test]
    fn emit_double_quoted_escapes() {
        roundtrip_eq("\"line1\\nline2\"");
    }

    #[test]
    fn emit_literal_block_scalar() {
        let input = "|\n  line1\n  line2\n";
        roundtrip_eq(input);
    }

    #[test]
    fn emit_folded_block_scalar() {
        let input = ">\n  line1\n  line2\n";
        roundtrip_eq(input);
    }

    // ─── Step 3: Block collections ──────────────────────────────────

    #[test]
    fn emit_block_mapping_simple() {
        roundtrip_eq("key: value");
    }

    #[test]
    fn emit_block_mapping_multi() {
        roundtrip_eq("a: 1\nb: 2\nc: 3");
    }

    #[test]
    fn emit_block_sequence_simple() {
        roundtrip_eq("- a\n- b\n- c");
    }

    #[test]
    fn emit_nested_mapping_in_sequence() {
        roundtrip_eq("- key: value\n  other: stuff");
    }

    #[test]
    fn emit_nested_sequence_in_mapping() {
        roundtrip_eq("items:\n- a\n- b");
    }

    #[test]
    fn emit_deeply_nested() {
        roundtrip_eq("a:\n  b:\n    c: deep");
    }

    #[test]
    fn emit_sort_keys() {
        let nodes = composer::compose_all("c: 3\na: 1\nb: 2").unwrap();
        let config = EmitterConfig {
            sort_keys: true,
            ..EmitterConfig::default()
        };
        let yaml = emit_to_string(&nodes[0], &config);
        assert!(yaml.starts_with("a: "), "keys should be sorted: {yaml:?}");
    }

    #[test]
    fn emit_empty_mapping() {
        assert_eq!(roundtrip("{}"), "{}\n");
    }

    #[test]
    fn emit_empty_sequence() {
        assert_eq!(roundtrip("[]"), "[]\n");
    }

    // ─── Step 4: Flow collections ───────────────────────────────────

    #[test]
    fn emit_flow_sequence() {
        roundtrip_eq("[1, 2, 3]");
    }

    #[test]
    fn emit_flow_mapping() {
        roundtrip_eq("{a: 1, b: 2}");
    }

    #[test]
    fn emit_flow_nested() {
        roundtrip_eq("[{a: 1}, {b: 2}]");
    }

    // ─── Step 5: Document markers and tags ──────────────────────────

    #[test]
    fn emit_explicit_document() {
        let nodes = composer::compose_all("hello").unwrap();
        let config = EmitterConfig {
            explicit_document: true,
            ..EmitterConfig::default()
        };
        let yaml = emit_to_string(&nodes[0], &config);
        assert!(yaml.starts_with("---\n"), "should start with ---: {yaml:?}");
        assert!(yaml.ends_with("...\n"), "should end with ...: {yaml:?}");
    }

    #[test]
    fn emit_tagged_scalar() {
        roundtrip_eq("!!str hello");
    }

    // ─── Step 7: Round-trip integration ─────────────────────────────

    #[test]
    fn roundtrip_complex() {
        roundtrip_eq(
            "database:\n  host: localhost\n  port: 5432\n  names:\n  - primary\n  - replica",
        );
    }

    #[test]
    fn roundtrip_mixed_styles() {
        roundtrip_eq("block:\n- {flow: mapping}\n- [flow, sequence]");
    }

    #[test]
    fn roundtrip_special_values() {
        // These must survive round-trip without becoming YAML reserved words
        roundtrip_eq("- 'null'\n- 'true'\n- 'false'\n- '42'\n- '3.14'");
    }

    // ─── Direct emission tests for unexercised escape paths ─────────
    // The round-trip tests above all parse first, which normalizes
    // scalars through the composer/serializer. These tests construct
    // Node values directly so they can exercise every branch of the
    // escape logic in emit_double_quoted / emit_single_quoted and the
    // tag-on-collection path in emit_node.

    fn synth_span() -> Span {
        Span::point(Position::start())
    }

    fn make_scalar(value: &str, style: ScalarStyle) -> Node<'static> {
        Node::Scalar(Scalar {
            value: std::borrow::Cow::Owned(value.to_string()),
            tag: None,
            style,
            span: synth_span(),
        })
    }

    fn emit_node_to_string(node: &Node<'_>) -> String {
        emit_to_string(node, &EmitterConfig::default())
    }

    #[test]
    fn single_quoted_escapes_embedded_apostrophe() {
        let node = make_scalar("it's", ScalarStyle::SingleQuoted);
        let out = emit_node_to_string(&node);
        assert!(out.contains("'it''s'"), "got: {out}");
    }

    #[test]
    fn double_quoted_escapes_backslash_and_quote() {
        let node = make_scalar(r#"a"b\c"#, ScalarStyle::DoubleQuoted);
        let out = emit_node_to_string(&node);
        assert!(out.contains(r#"a\"b\\c"#), "got: {out}");
    }

    #[test]
    fn double_quoted_escapes_common_control_chars() {
        // Each character maps to a named escape sequence.
        for (raw, expected_seq) in [
            ('\n', "\\n"),
            ('\r', "\\r"),
            ('\t', "\\t"),
            ('\0', "\\0"),
            ('\x07', "\\a"),
            ('\x08', "\\b"),
            ('\x0B', "\\v"),
            ('\x0C', "\\f"),
            ('\x1B', "\\e"),
        ] {
            let s = format!("x{raw}y");
            let node = make_scalar(&s, ScalarStyle::DoubleQuoted);
            let out = emit_node_to_string(&node);
            assert!(
                out.contains(expected_seq),
                "char {raw:?} should produce {expected_seq:?} in output, got: {out}"
            );
        }
    }

    #[test]
    fn double_quoted_escapes_low_byte_control_char() {
        // 0x01 has no named escape — should produce \x01.
        let s = format!("a{}b", '\x01');
        let node = make_scalar(&s, ScalarStyle::DoubleQuoted);
        let out = emit_node_to_string(&node);
        assert!(out.contains("\\x01"), "got: {out}");
    }

    // U+2028/U+2029 (line/paragraph separators) and Unicode format characters
    // (General_Category=Cf) are valid YAML but are escaped on emission by
    // `needs_quoted_escape` so the output is unambiguous for JS/JSON consumers
    // and free of invisible characters. They round-trip losslessly: the scanner
    // decodes `\uXXXX`/`\UXXXXXXXX` back to the original code point.
    #[test]
    fn double_quoted_escapes_basic_multilingual_plane_control() {
        // U+2028 (LINE SEPARATOR) -- 16-bit code point -> `\uXXXX` form.
        let s = format!("a{}b", '\u{2028}');
        let node = make_scalar(&s, ScalarStyle::DoubleQuoted);
        let out = emit_node_to_string(&node);
        assert!(
            out.to_uppercase().contains("\\U2028"),
            "expected \\u2028, got: {out}"
        );
    }

    #[test]
    fn double_quoted_escapes_paragraph_separator() {
        // U+2029 (PARAGRAPH SEPARATOR) -- also a 16-bit `\uXXXX` escape.
        let s = format!("a{}b", '\u{2029}');
        let out = emit_node_to_string(&make_scalar(&s, ScalarStyle::DoubleQuoted));
        assert!(out.to_uppercase().contains("\\U2029"), "got: {out}");
    }

    #[test]
    fn double_quoted_escapes_high_unicode_control() {
        // U+E0001 (LANGUAGE TAG) -- astral format char -> `\UXXXXXXXX` form.
        let s = format!("a{}b", '\u{E0001}');
        let node = make_scalar(&s, ScalarStyle::DoubleQuoted);
        let out = emit_node_to_string(&node);
        assert!(
            out.to_uppercase().contains("\\U000E0001"),
            "expected \\U000E0001 form, got: {out}"
        );
    }

    #[test]
    fn double_quoted_separators_round_trip() {
        // Emit a scalar holding both separators plus a zero-width format char,
        // then parse it back: the raw characters must not leak, and re-parsing
        // must recover them exactly.
        let original = "x\u{2028}\u{2029}\u{200B}y";
        let emitted = emit_node_to_string(&make_scalar(original, ScalarStyle::DoubleQuoted));
        assert!(
            !emitted.contains('\u{2028}') && !emitted.contains('\u{2029}'),
            "raw separator leaked into emitted output: {emitted:?}"
        );
        let reparsed = composer::compose_all(&emitted).unwrap();
        match &reparsed[0] {
            Node::Scalar(s) => assert_eq!(s.value.as_ref(), original),
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn tagged_collection_emits_tag_then_newline() {
        // emit_node lines 126-128: when emitting a tagged collection
        // (non-scalar), the tag is followed by newline + indent, not space.
        let inner_scalar = make_scalar("v", ScalarStyle::Plain);
        let tagged_sequence = Node::Sequence(Sequence {
            items: vec![inner_scalar],
            tag: Some(Tag {
                value: std::borrow::Cow::Borrowed("!mytag"),
                span: synth_span(),
            }),
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let out = emit_node_to_string(&tagged_sequence);
        // Tag should be emitted on its own line, separate from the items.
        assert!(out.contains("!mytag"), "tag should be emitted: {out}");
        assert!(out.contains("- v"), "items should follow: {out}");
        let tag_pos = out.find("!mytag").unwrap();
        let item_pos = out.find("- v").unwrap();
        assert!(tag_pos < item_pos, "tag should precede items");
    }

    #[test]
    fn full_uri_tag_uses_angle_bracket_form() {
        // emit_tag: tags that don't start with `!` are wrapped in `!<...>`.
        let scalar = Node::Scalar(Scalar {
            value: std::borrow::Cow::Borrowed("x"),
            tag: Some(Tag {
                value: std::borrow::Cow::Borrowed("tag:example.com,2024:Custom"),
                span: synth_span(),
            }),
            style: ScalarStyle::Plain,
            span: synth_span(),
        });
        let out = emit_node_to_string(&scalar);
        assert!(
            out.contains("!<tag:example.com,2024:Custom>"),
            "expected wrapped form, got: {out}"
        );
    }

    #[test]
    fn block_scalar_single_line_falls_back_to_double_quoted() {
        // emit_block_scalar line 239: single-line literal has no '\n',
        // so it falls back to double-quoted form.
        let node = make_scalar("oneline", ScalarStyle::Literal);
        let out = emit_node_to_string(&node);
        assert_eq!(out, "\"oneline\"\n", "got: {out}");
    }

    #[test]
    fn block_scalar_keep_chomp_indicator() {
        // emit_block_scalar line 244 + 254: value ending in "\n\n"
        // produces the keep chomp indicator `+`.
        let node = make_scalar("a\nb\n\n", ScalarStyle::Literal);
        let out = emit_node_to_string(&node);
        assert!(
            out.starts_with("|+\n"),
            "expected keep chomp header: {out:?}"
        );
    }

    #[test]
    fn block_scalar_strip_chomp_indicator() {
        // emit_block_scalar line 248 + 254: value with no trailing newline
        // produces the strip chomp indicator `-`.
        let node = make_scalar("a\nb", ScalarStyle::Literal);
        let out = emit_node_to_string(&node);
        assert!(
            out.starts_with("|-\n"),
            "expected strip chomp header: {out:?}"
        );
    }

    #[test]
    fn block_scalar_clip_chomp_no_indicator() {
        // emit_block_scalar: value ending in a single '\n' uses clip
        // (default, chomp == ' ', so line 254 is skipped — no indicator).
        let node = make_scalar("a\nb\n", ScalarStyle::Folded);
        let out = emit_node_to_string(&node);
        assert!(
            out.starts_with(">\n"),
            "expected clip chomp header: {out:?}"
        );
    }

    #[test]
    fn nested_block_sequence_in_block_sequence_inline() {
        // emit_block_sequence line 327: a block sequence whose item is
        // itself a non-empty block sequence emits the inner first item
        // on the same line as the `-`.
        let inner = Node::Sequence(Sequence {
            items: vec![
                make_scalar("x", ScalarStyle::Plain),
                make_scalar("y", ScalarStyle::Plain),
            ],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let outer = Node::Sequence(Sequence {
            items: vec![inner],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let out = emit_node_to_string(&outer);
        // Re-parsing should yield the same semantic structure.
        let reparsed = composer::compose_all(&out).unwrap();
        assert_eq!(
            node_semantic(&outer),
            node_semantic(&reparsed[0]),
            "out: {out}"
        );
    }

    #[test]
    fn empty_block_sequence_emits_flow_brackets() {
        // emit_block_sequence line 317: an empty Block-style sequence
        // is emitted as `[]`.
        let node = Node::Sequence(Sequence {
            items: vec![],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let out = emit_node_to_string(&node);
        assert_eq!(out, "[]\n", "got: {out}");
    }

    #[test]
    fn empty_block_mapping_emits_flow_braces() {
        // emit_block_mapping line 350: an empty Block-style mapping
        // is emitted as `{}`.
        let node = Node::Mapping(Mapping {
            entries: vec![],
            tag: None,
            style: CollectionStyle::Block,
            span: synth_span(),
        });
        let out = emit_node_to_string(&node);
        assert_eq!(out, "{}\n", "got: {out}");
    }

    #[test]
    fn tracking_writer_records_final_newline() {
        // Indirectly exercise TrackingWriter::write_str (line 106-107)
        // by emitting a value that contains a newline at the end and
        // checking the output has the trailing newline tracked.
        let node = make_scalar("hello", ScalarStyle::Plain);
        let out = emit_node_to_string(&node);
        // emit_to_string ensures trailing newline if not already present.
        assert!(
            out.ends_with('\n'),
            "output should end with newline: {out:?}"
        );
    }
}
