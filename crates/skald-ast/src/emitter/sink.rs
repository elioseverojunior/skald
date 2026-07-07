// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Push-based emitter sink: owns all YAML layout behind an imperative API.
//! See docs/plans/2026-06-04-streaming-serializer-design.md.
use std::fmt;
use std::fmt::Write as _;

use skald_core::types::{CollectionStyle, ScalarStyle};

use crate::emitter::{
    EmitterConfig, TrackingWriter, emit_block_scalar, emit_double_quoted, emit_plain,
    emit_single_quoted, write_indent,
};

/// Deferred separator state for a frame. After `before_value` writes the
/// map's `':'`, the separator that follows the colon depends on the child's
/// shape, which is only known on the NEXT sink call: a scalar or empty
/// collection wants `' '`; a non-empty block collection wants `'\n'` plus an
/// indented child. We record `MapValue` and resolve it in the next call.
///
/// `SeqElem` is the analogous marker for block sequences: after `before_elem`
/// writes the `- ` marker, the next `begin_map`/`begin_seq` checks for it. A
/// non-empty block child becomes the inline first child of the `- ` (its first
/// entry/element shares the marker line via `suppress_first_indent`); empty,
/// flow, or scalar items render normally after the `- `.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    MapValue,
    SeqElem,
}

/// Resolved layout role of a scalar, computed under a short borrow of the
/// frame stack so the subsequent rendering borrow of `self.w` does not
/// overlap it.
enum Action {
    TopLevel,
    Key,
    Value,
    Plain,
    /// Scalar inside a flow collection — emit bare text, no newline. The
    /// `", "`/`": "` punctuation is written by `before_elem`/`before_key`/
    /// `before_value`, so this just renders the scalar value.
    FlowItem,
}

// All fields in use: `style`/`level`/`count`/`pending`/`expect_key` thread
// layout state; `suppress_first_indent` lets a block collection that is the
// inline first child of a `- ` skip the indent on its first entry/element.
//
// `open_deferred` marks a frame opened with unknown length (`len == None`): its
// emptiness, and therefore its parent's pending separator, are unknown at
// `begin_*`. The parent separator is resolved on the first `before_elem`/
// `before_key` (non-empty) or at `end_*` (empty), once emptiness is known.
struct Frame {
    style: CollectionStyle,
    level: usize,
    count: usize,
    suppress_first_indent: bool,
    pending: Pending,
    expect_key: bool,
    open_deferred: bool,
}

/// Push-based YAML emitter sink: owns all YAML layout (indentation,
/// block/flow rendering, the deferred-separator state machine, empty-collection
/// `[]`/`{}`, punctuation) behind an imperative API over an explicit `Frame`
/// stack — no recursion, no `Node`. Two drivers feed it: the `Node` walk in
/// `emit_to_string` and the streaming serde `Serializer` in `skald-serde`.
///
/// It **trusts the caller's `ScalarStyle`** — the sink never re-quotes.
pub struct Emitter<'c, W: fmt::Write> {
    w: TrackingWriter<W>,
    cfg: &'c EmitterConfig,
    stack: Vec<Frame>,
    /// A node property (tag) recorded by `tag()` and consumed by the next
    /// `scalar`/`begin_seq`/`begin_map`. Mirrors the old `emit_node`, which
    /// emitted the tag before the node value: for scalars `tag ' '`, for
    /// collections `tag '\n'` then the node's own indent.
    pending_tag: Option<String>,
}

impl<'c, W: fmt::Write> Emitter<'c, W> {
    /// Creates a new push emitter writing into `writer` with the given config.
    pub fn new(writer: W, cfg: &'c EmitterConfig) -> Self {
        Self {
            w: TrackingWriter::new(writer),
            cfg,
            stack: Vec::with_capacity(8),
            pending_tag: None,
        }
    }

    /// Record a node tag to be emitted before the next node value. The raw tag
    /// text (e.g. `!!str` or a full URI) is stored; rendering applies the same
    /// shorthand/`!<...>` rule as the old `emit_tag`.
    pub fn tag(&mut self, value: &str) {
        self.pending_tag = Some(value.to_string());
    }

    /// Write the pending scalar tag (`tag ' '`) if any. Called after the
    /// scalar's leading separator/indent and before the scalar text, matching
    /// the old `emit_node` scalar-tag placement.
    fn write_pending_scalar_tag(&mut self) -> fmt::Result {
        if let Some(tag) = self.pending_tag.take() {
            write_tag(&tag, &mut self.w)?;
            self.w.write_char(' ')?;
        }
        Ok(())
    }

    fn emit_scalar_text(&mut self, value: &str, style: ScalarStyle, level: usize) -> fmt::Result {
        match style {
            ScalarStyle::Plain => emit_plain(value, &mut self.w),
            ScalarStyle::SingleQuoted => emit_single_quoted(value, &mut self.w),
            ScalarStyle::DoubleQuoted => emit_double_quoted(value, &mut self.w),
            ScalarStyle::Literal => emit_block_scalar(value, '|', level, self.cfg, &mut self.w),
            ScalarStyle::Folded => emit_block_scalar(value, '>', level, self.cfg, &mut self.w),
        }
    }

    fn cur_level(&self) -> usize {
        self.stack.last().map_or(0, |f| f.level)
    }

    /// Resolve a parent frame's deferred map-value separator against the
    /// child's emptiness. Called from `begin_seq`/`begin_map` BEFORE pushing
    /// the child frame so it operates on the parent (the map). An empty child
    /// renders inline (`' '` then `[]`/`{}`); a non-empty block child starts on
    /// its own indented line (`'\n'`).
    fn take_map_value_sep(&mut self, child_empty: bool) -> fmt::Result {
        if let Some(f) = self.stack.last_mut() {
            if f.pending == Pending::MapValue {
                f.pending = Pending::None;
                if child_empty {
                    self.w.write_char(' ')?;
                } else {
                    writeln!(self.w)?;
                }
            }
        }
        Ok(())
    }

    /// Resolve a parent block-seq frame's deferred `SeqElem` marker against the
    /// child collection being opened. Called from `begin_seq`/`begin_map` BEFORE
    /// pushing the child. Returns whether the child should suppress the indent on
    /// its first entry/element — true only for a non-empty BLOCK child, which
    /// becomes the inline first child of the `- ` already written by
    /// `before_elem`. An empty/flow child clears the marker and renders normally
    /// after the `- ` (no inline first child).
    fn take_seq_elem(&mut self, child_non_empty_block: bool) -> bool {
        if let Some(f) = self.stack.last_mut() {
            if f.pending == Pending::SeqElem {
                f.pending = Pending::None;
                return child_non_empty_block;
            }
        }
        false
    }

    /// Resolve the PARENT's deferred map-value separator when the parent is one
    /// frame below the stack top. Used by deferred (`open_deferred`) collections:
    /// the child frame is already pushed, so the parent sits at `stack[len - 2]`.
    /// Same semantics as `take_map_value_sep` but indexed at depth-1.
    fn take_parent_map_value_sep(&mut self, child_empty: bool) -> fmt::Result {
        let n = self.stack.len();
        if n >= 2 {
            let parent = &mut self.stack[n - 2];
            if parent.pending == Pending::MapValue {
                parent.pending = Pending::None;
                if child_empty {
                    self.w.write_char(' ')?;
                } else {
                    writeln!(self.w)?;
                }
            }
        }
        Ok(())
    }

    /// Resolve the PARENT block-seq frame's deferred `SeqElem` marker when the
    /// parent is one frame below the stack top. Used by deferred collections:
    /// the child frame is already pushed, so the parent sits at `stack[len - 2]`.
    /// Same semantics as `take_seq_elem` but indexed at depth-1.
    fn take_parent_seq_elem(&mut self, child_non_empty_block: bool) -> bool {
        let n = self.stack.len();
        if n >= 2 {
            let parent = &mut self.stack[n - 2];
            if parent.pending == Pending::SeqElem {
                parent.pending = Pending::None;
                return child_non_empty_block;
            }
        }
        false
    }

    /// Emit a scalar node in the current context (top level, map key, map
    /// value, sequence element, or flow item). Trusts the caller's `style`.
    pub fn scalar(&mut self, value: &str, style: ScalarStyle) -> fmt::Result {
        let lvl = self.cur_level();
        // Compute the layout action in a scope that ends before we borrow
        // `self.w`/`emit_scalar_text` (which also borrow `self`), avoiding an
        // overlapping mutable borrow of `self.stack`.
        let action = match self.stack.last_mut() {
            None => Action::TopLevel,
            // Flow keys and values are both bare text; the `", "`/`": "`
            // punctuation is written by `before_key`/`before_value`/
            // `before_elem`, and `expect_key`/`pending` were already cleared
            // there, so any scalar inside a flow frame just renders inline.
            Some(f) if f.style == CollectionStyle::Flow => Action::FlowItem,
            Some(f) if f.expect_key => {
                f.expect_key = false;
                Action::Key
            }
            Some(f) if f.pending == Pending::MapValue => {
                f.pending = Pending::None;
                Action::Value
            }
            Some(_) => Action::Plain,
        };
        match action {
            Action::TopLevel => {
                self.write_pending_scalar_tag()?;
                self.emit_scalar_text(value, style, 0)?;
                // The old emitter appended a trailing newline only when the
                // body did not already end in one. Block scalars emit their own
                // terminating newline, so a top-level block scalar must not get
                // a second.
                if !self.w.ends_with_newline() {
                    writeln!(self.w)?;
                }
                Ok(())
            }
            // Key: no trailing newline; the value separator follows.
            Action::Key => {
                self.write_pending_scalar_tag()?;
                self.emit_scalar_text(value, style, lvl)
            }
            Action::Value => {
                self.w.write_char(' ')?;
                self.write_pending_scalar_tag()?;
                self.emit_scalar_text(value, style, lvl)?;
                writeln!(self.w)
            }
            // Sequence-element scalar (marker already written by `before_elem`).
            Action::Plain => {
                self.write_pending_scalar_tag()?;
                self.emit_scalar_text(value, style, lvl)?;
                writeln!(self.w)
            }
            // Flow item: bare text, no trailing newline.
            Action::FlowItem => {
                self.write_pending_scalar_tag()?;
                self.emit_scalar_text(value, style, lvl)
            }
        }
    }

    /// Open a sequence in the given style. `len` may be `None` (unknown length,
    /// e.g. from a streaming serde `serialize_seq`), in which case parent-layout
    /// resolution is deferred until the first element or `end_seq`.
    pub fn begin_seq(&mut self, style: CollectionStyle, len: Option<usize>) -> fmt::Result {
        // Unknown length: defer all parent-separator/emptiness layout to the
        // first element (`before_elem`) or to `end_seq` (zero elements). Push the
        // frame with `open_deferred`; write nothing parent-related yet.
        if len.is_none() {
            let level = self.stack.last().map_or(0, |f| f.level + 1);
            self.stack.push(Frame {
                style,
                level,
                count: 0,
                suppress_first_indent: false,
                pending: Pending::None,
                expect_key: false,
                open_deferred: true,
            });
            return Ok(());
        }
        // A flow child renders inline after `key: ` regardless of emptiness, so
        // it resolves the parent's deferred separator as inline (`' '`).
        let inline_child = style == CollectionStyle::Flow || matches!(len, Some(0));
        let non_empty_block = style == CollectionStyle::Block && !matches!(len, Some(0));
        // A `SeqElem` parent already wrote `- `; suppress the first element's
        // indent for a non-empty block child. Otherwise resolve a `MapValue`.
        let suppress_first_indent = self.take_seq_elem(non_empty_block);
        self.take_map_value_sep(inline_child)?;
        let level = self.stack.last().map_or(0, |f| f.level + 1);
        self.write_pending_collection_tag(level, suppress_first_indent)?;
        self.stack.push(Frame {
            style,
            level,
            count: 0,
            suppress_first_indent,
            pending: Pending::None,
            expect_key: false,
            open_deferred: false,
        });
        if style == CollectionStyle::Flow {
            self.w.write_char('[')?;
        }
        Ok(())
    }

    /// Resolve a deferred (`open_deferred`) collection's parent layout now that
    /// the first element/entry has arrived (the collection is non-empty). The
    /// child frame is on top; the parent sits at depth-1. Mirrors the `begin_*`
    /// resolution for a non-empty child: resolve the parent's `SeqElem`/
    /// `MapValue`, emit any deferred collection tag, write the deferred flow
    /// opener (`flow_open` = `'['` for a seq, `'{'` for a map), and record
    /// `suppress_first_indent` for a block child of a `- `.
    fn resolve_deferred_open_nonempty(&mut self, flow_open: char) -> fmt::Result {
        let n = self.stack.len();
        let (style, level) = {
            let f = &self.stack[n - 1];
            (f.style, f.level)
        };
        let non_empty_block = style == CollectionStyle::Block;
        let suppress_first_indent = self.take_parent_seq_elem(non_empty_block);
        // Non-empty deferred child: parent map value goes on its own line.
        self.take_parent_map_value_sep(false)?;
        self.write_pending_collection_tag(level, suppress_first_indent)?;
        {
            let f = &mut self.stack[n - 1];
            f.open_deferred = false;
            f.suppress_first_indent = suppress_first_indent;
        }
        if style == CollectionStyle::Flow {
            self.w.write_char(flow_open)?;
        }
        Ok(())
    }

    /// Resolve a deferred (`open_deferred`) collection that turned out EMPTY at
    /// `end_*`. The frame is still on top; the parent sits at depth-1. Mirrors
    /// the `begin_*` resolution for an empty child (inline `' '`) and emits any
    /// deferred collection tag. For a flow child, the deferred `flow_open`
    /// (`'['`/`'{'`) is written here so the matching close yields `[]`/`{}`;
    /// for a block child the empty `[]`/`{}` is left to the zero-count path.
    fn resolve_deferred_open_empty(&mut self, flow_open: char) -> fmt::Result {
        let (style, level) = {
            let f = &self.stack[self.stack.len() - 1];
            (f.style, f.level)
        };
        // An empty deferred child clears the parent `SeqElem` (renders normally
        // after the `- `, no inline first child) and takes the inline map sep.
        self.take_parent_seq_elem(false);
        self.take_parent_map_value_sep(true)?;
        self.write_pending_collection_tag(level, false)?;
        if style == CollectionStyle::Flow {
            self.w.write_char(flow_open)?;
        }
        Ok(())
    }

    /// Begin a sequence element. Block: indent at the sequence's own level +
    /// "- ". Flow: a `", "` separator before every item after the first.
    pub fn before_elem(&mut self) -> fmt::Result {
        // First element of a deferred (unknown-length) seq: the collection is
        // now known non-empty, so resolve the deferred parent layout (and write
        // the deferred flow `[`) before laying out this element.
        if self.stack.last().is_some_and(|f| f.open_deferred) {
            self.resolve_deferred_open_nonempty('[')?;
        }
        let f = self.stack.last().expect("before_elem outside seq");
        let level = f.level;
        let flow = f.style == CollectionStyle::Flow;
        let count = f.count;
        let suppress = f.suppress_first_indent;
        if let Some(f) = self.stack.last_mut() {
            f.count += 1;
        }
        if flow {
            if count > 0 {
                self.w.write_str(", ")?;
            }
            return Ok(());
        }
        if suppress {
            // Inline first element of a `- `: the marker already positioned the
            // cursor; emit no indent for this element, then resume normal
            // indentation for the rest.
            if let Some(f) = self.stack.last_mut() {
                f.suppress_first_indent = false;
            }
        } else {
            write_indent(level, self.cfg, &mut self.w)?;
        }
        self.w.write_str("- ")?;
        // Flag the seq frame so a non-empty block child collection becomes the
        // inline first child of this `- ` (its first entry shares the line).
        if let Some(f) = self.stack.last_mut() {
            f.pending = Pending::SeqElem;
        }
        Ok(())
    }

    /// Close the current sequence. An empty block sequence renders as `[]`.
    pub fn end_seq(&mut self) -> fmt::Result {
        // A deferred seq closed with zero elements: resolve the parent layout as
        // for an empty child (inline `' '`) before the frame is popped, so the
        // empty `[]` renders inline after `: `/`- ` like the `Some(0)` path.
        if self.stack.last().is_some_and(|f| f.open_deferred) {
            self.resolve_deferred_open_empty('[')?;
        }
        let f = self.stack.pop().expect("end_seq without begin_seq");
        if f.style == CollectionStyle::Flow {
            self.w.write_char(']')?;
            self.finish_flow_collection_line()?;
            return Ok(());
        }
        if f.count == 0 {
            self.w.write_str("[]")?;
            self.finish_empty_collection_line()?;
        }
        Ok(())
    }

    /// Open a mapping in the given style. `len` may be `None` (unknown length),
    /// in which case parent-layout resolution is deferred until the first entry
    /// or `end_map`.
    pub fn begin_map(&mut self, style: CollectionStyle, len: Option<usize>) -> fmt::Result {
        // Unknown length: defer all parent-separator/emptiness layout to the
        // first entry (`before_key`) or to `end_map` (zero entries).
        if len.is_none() {
            let level = self.stack.last().map_or(0, |f| f.level + 1);
            self.stack.push(Frame {
                style,
                level,
                count: 0,
                suppress_first_indent: false,
                pending: Pending::None,
                expect_key: false,
                open_deferred: true,
            });
            return Ok(());
        }
        // A flow child renders inline after `key: ` regardless of emptiness.
        let inline_child = style == CollectionStyle::Flow || matches!(len, Some(0));
        let non_empty_block = style == CollectionStyle::Block && !matches!(len, Some(0));
        // A `SeqElem` parent already wrote `- `; suppress the first entry's
        // indent for a non-empty block child. Otherwise resolve a `MapValue`.
        let suppress_first_indent = self.take_seq_elem(non_empty_block);
        self.take_map_value_sep(inline_child)?;
        let level = self.stack.last().map_or(0, |f| f.level + 1);
        self.write_pending_collection_tag(level, suppress_first_indent)?;
        self.stack.push(Frame {
            style,
            level,
            count: 0,
            suppress_first_indent,
            pending: Pending::None,
            expect_key: false,
            open_deferred: false,
        });
        if style == CollectionStyle::Flow {
            self.w.write_char('{')?;
        }
        Ok(())
    }

    /// Begin a mapping entry: lay out indentation (block) or a separator (flow),
    /// then the caller emits the key via [`scalar`](Self::scalar) or a nested
    /// collection.
    pub fn before_key(&mut self) -> fmt::Result {
        // First entry of a deferred (unknown-length) map: the collection is now
        // known non-empty, so resolve the deferred parent layout (and write the
        // deferred flow `{`) before laying out this entry.
        if self.stack.last().is_some_and(|f| f.open_deferred) {
            self.resolve_deferred_open_nonempty('{')?;
        }
        let f = self.stack.last().expect("before_key outside map");
        let level = f.level;
        let flow = f.style == CollectionStyle::Flow;
        let count = f.count;
        let suppress = f.suppress_first_indent;
        if let Some(f) = self.stack.last_mut() {
            f.expect_key = true;
            f.count += 1;
        }
        if flow {
            if count > 0 {
                self.w.write_str(", ")?;
            }
            return Ok(());
        }
        if suppress {
            // Inline first entry of a `- `: the marker already positioned the
            // cursor; skip this entry's indent, then resume normal indentation.
            if let Some(f) = self.stack.last_mut() {
                f.suppress_first_indent = false;
            }
            return Ok(());
        }
        write_indent(level, self.cfg, &mut self.w)
    }

    /// Write the mapping `:` separator and arm the deferred value separator;
    /// the caller then emits the value via [`scalar`](Self::scalar) or a nested
    /// collection.
    pub fn before_value(&mut self) -> fmt::Result {
        let flow = self
            .stack
            .last()
            .is_some_and(|f| f.style == CollectionStyle::Flow);
        if let Some(f) = self.stack.last_mut() {
            f.expect_key = false;
            // Flow values are always inline; no deferred separator to resolve.
            f.pending = if flow {
                Pending::None
            } else {
                Pending::MapValue
            };
        }
        if flow {
            // emit_flow_mapping oracle: key, ": ", value.
            self.w.write_str(": ")
        } else {
            self.w.write_char(':')
        }
    }

    /// Close the current mapping. An empty block mapping renders as `{}`.
    pub fn end_map(&mut self) -> fmt::Result {
        // A deferred map closed with zero entries: resolve the parent layout as
        // for an empty child (inline `' '`) before the frame is popped.
        if self.stack.last().is_some_and(|f| f.open_deferred) {
            self.resolve_deferred_open_empty('{')?;
        }
        let f = self.stack.pop().expect("end_map without begin_map");
        if f.style == CollectionStyle::Flow {
            self.w.write_char('}')?;
            self.finish_flow_collection_line()?;
            return Ok(());
        }
        if f.count == 0 {
            self.w.write_str("{}")?;
            self.finish_empty_collection_line()?;
        }
        Ok(())
    }

    /// Terminate the line of an empty collection (`[]`/`{}`). At the top level
    /// the collection owns its trailing newline. As a block-map value or a
    /// block-seq element the entry/item line terminates here (the parent's
    /// `':'`+`' '`+`[]`/`{}` or `- `+`[]`/`{}` are already written; only the
    /// newline remains). A nested flow parent never reaches here (empty flow
    /// collections close via `finish_flow_collection_line`).
    fn finish_empty_collection_line(&mut self) -> fmt::Result {
        match self.stack.last() {
            None => writeln!(self.w),
            // A nested flow parent separates with `", "`, not a newline.
            Some(f) if f.style == CollectionStyle::Flow => Ok(()),
            Some(_) => writeln!(self.w),
        }
    }

    /// Terminate the line of a just-closed flow collection (`[...]`/`{...}`).
    /// At the top level the collection owns its trailing newline. As a block-map
    /// value (`key: [..]`) or a block-seq element (`- [..]`) the entry/item line
    /// terminates here with `'\n'`. When nested inside another flow collection,
    /// the parent's `", "` handles separation, so nothing is written.
    fn finish_flow_collection_line(&mut self) -> fmt::Result {
        match self.stack.last() {
            None => writeln!(self.w),
            Some(f) if f.style == CollectionStyle::Flow => Ok(()),
            // Block parent (map value or seq element): the entry/item line ends.
            Some(_) => writeln!(self.w),
        }
    }

    /// Write the pending collection tag (`tag '\n'` then the node's own indent
    /// when not the inline first child of a `- `). Called from `begin_seq`/
    /// `begin_map` after the parent separator is resolved and before the child
    /// frame is pushed, mirroring the old `emit_node` collection-tag placement.
    fn write_pending_collection_tag(&mut self, level: usize, inline: bool) -> fmt::Result {
        if let Some(tag) = self.pending_tag.take() {
            write_tag(&tag, &mut self.w)?;
            writeln!(self.w)?;
            if !inline {
                write_indent(level, self.cfg, &mut self.w)?;
            }
        }
        Ok(())
    }

    /// Finish emitting, consuming the sink. All output has already been written
    /// to the underlying writer; this exists for symmetry and future flushing.
    pub fn finish(self) -> fmt::Result {
        Ok(())
    }
}

/// Render a tag, mirroring the old `emit_tag`: shorthand tags (starting with
/// `!`) pass through verbatim; anything else is wrapped in `!<...>`.
fn write_tag<W: fmt::Write>(tag: &str, writer: &mut W) -> fmt::Result {
    if tag.starts_with('!') {
        write!(writer, "{tag}")
    } else {
        write!(writer, "!<{tag}>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::EmitterConfig;
    use skald_core::types::ScalarStyle;

    fn emit_with<F: FnOnce(&mut Emitter<'_, &mut String>)>(f: F) -> String {
        let cfg = EmitterConfig::default();
        let mut out = String::new();
        {
            let mut e = Emitter::new(&mut out, &cfg);
            f(&mut e);
            e.finish().unwrap();
        }
        out
    }

    #[test]
    fn top_level_plain_scalar() {
        let s = emit_with(|e| e.scalar("hello", ScalarStyle::Plain).unwrap());
        assert_eq!(s, "hello\n");
    }

    #[test]
    fn block_seq_of_scalars() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, Some(2)).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "- a\n- b\n");
    }

    #[test]
    fn empty_block_seq_is_flow_brackets() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, Some(0)).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "[]\n");
    }

    #[test]
    fn block_map_scalar_values() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(2)).unwrap();
            e.before_key().unwrap();
            e.scalar("k1", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("v1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("k2", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("v2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "k1: v1\nk2: v2\n");
    }

    #[test]
    fn block_map_nested_map_value() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("outer", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("inner", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("x", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "outer:\n  inner: x\n");
    }

    #[test]
    fn block_map_seq_value() {
        // value is a non-empty block sequence -> newline then indented block at level+1
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("items", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_seq(CollectionStyle::Block, Some(2)).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "items:\n  - a\n  - b\n");
    }

    #[test]
    fn flow_seq() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Flow, Some(2)).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "[a, b]\n");
    }

    #[test]
    fn flow_map() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Flow, Some(2)).unwrap();
            e.before_key().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
        });
        // matches emit_flow_mapping oracle: '{' key ": " value ", " ... '}'
        assert_eq!(s, "{a: 1, b: 2}\n");
    }

    #[test]
    fn block_map_flow_seq_value() {
        // a flow sequence as a block-map value renders inline: "k: [a, b]\n"
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("k", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_seq(CollectionStyle::Flow, Some(2)).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "k: [a, b]\n");
    }

    #[test]
    fn block_seq_of_maps_inline_first() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, Some(1)).unwrap();
            e.before_elem().unwrap();
            e.begin_map(CollectionStyle::Block, Some(2)).unwrap();
            e.before_key().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "- a: 1\n  b: 2\n");
    }

    #[test]
    fn block_seq_of_seqs_inline_first() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, Some(1)).unwrap();
            e.before_elem().unwrap();
            e.begin_seq(CollectionStyle::Block, Some(2)).unwrap();
            e.before_elem().unwrap();
            e.scalar("x", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("y", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "- - x\n  - y\n");
    }

    #[test]
    fn block_map_empty_seq_value() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("k", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_seq(CollectionStyle::Block, Some(0)).unwrap();
            e.end_seq().unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "k: []\n");
    }

    #[test]
    fn unknown_len_empty_seq_as_map_value_renders_brackets() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("k", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_seq(CollectionStyle::Block, None).unwrap(); // unknown len
            e.end_seq().unwrap(); // zero elements
            e.end_map().unwrap();
        });
        assert_eq!(s, "k: []\n");
    }

    #[test]
    fn unknown_len_nonempty_seq_as_map_value_is_block() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("k", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_seq(CollectionStyle::Block, None).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
            e.end_map().unwrap();
        });
        // must match the Some(len) oracle for the same shape: "k:\n  - a\n  - b\n"
        assert_eq!(s, "k:\n  - a\n  - b\n");
    }

    #[test]
    fn unknown_len_top_level_empty_seq() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, None).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "[]\n");
    }

    // ── Unknown-length (`len == None`) MAP coverage ──

    #[test]
    fn unknown_len_top_level_empty_map() {
        // Covers begin_map(Block, None) deferral + end_map empty resolution.
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, None).unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "{}\n");
    }

    #[test]
    fn unknown_len_empty_map_as_map_value_renders_braces() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("k", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_map(CollectionStyle::Block, None).unwrap(); // unknown len
            e.end_map().unwrap(); // zero entries
            e.end_map().unwrap();
        });
        assert_eq!(s, "k: {}\n");
    }

    #[test]
    fn unknown_len_nonempty_map_as_map_value_is_block() {
        // Covers begin_map(Block, None) + before_key deferred resolution.
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Block, Some(1)).unwrap();
            e.before_key().unwrap();
            e.scalar("outer", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.begin_map(CollectionStyle::Block, None).unwrap();
            e.before_key().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
            e.end_map().unwrap();
        });
        // Matches the Some(len) oracle for the same shape.
        assert_eq!(s, "outer:\n  a: 1\n  b: 2\n");
    }

    #[test]
    fn unknown_len_block_map_as_seq_element_inline_first() {
        // The seq element is an unknown-length block map; covers the
        // `take_parent_seq_elem` non-empty-block branch in
        // `resolve_deferred_open_nonempty` (sink lines 184-191).
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Block, Some(1)).unwrap();
            e.before_elem().unwrap();
            e.begin_map(CollectionStyle::Block, None).unwrap();
            e.before_key().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
            e.end_seq().unwrap();
        });
        // Matches the Some(len) `block_seq_of_maps_inline_first` oracle.
        assert_eq!(s, "- a: 1\n  b: 2\n");
    }

    // ── Unknown-length (`len == None`) FLOW coverage ──
    // The flow `[`/`{` opener is deferred to first-element / empty-close
    // resolution (sink lines 327 / 349).

    #[test]
    fn unknown_len_flow_seq_nonempty() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Flow, None).unwrap();
            e.before_elem().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_elem().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "[a, b]\n");
    }

    #[test]
    fn unknown_len_flow_seq_empty() {
        let s = emit_with(|e| {
            e.begin_seq(CollectionStyle::Flow, None).unwrap();
            e.end_seq().unwrap();
        });
        assert_eq!(s, "[]\n");
    }

    #[test]
    fn unknown_len_flow_map_nonempty() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Flow, None).unwrap();
            e.before_key().unwrap();
            e.scalar("a", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("1", ScalarStyle::Plain).unwrap();
            e.before_key().unwrap();
            e.scalar("b", ScalarStyle::Plain).unwrap();
            e.before_value().unwrap();
            e.scalar("2", ScalarStyle::Plain).unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "{a: 1, b: 2}\n");
    }

    #[test]
    fn unknown_len_flow_map_empty() {
        let s = emit_with(|e| {
            e.begin_map(CollectionStyle::Flow, None).unwrap();
            e.end_map().unwrap();
        });
        assert_eq!(s, "{}\n");
    }
}
