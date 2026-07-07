// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! SWAR (SIMD-Within-A-Register) structural-byte scan helpers.
//!
//! Processes 8 bytes at a time using `u64` bit tricks to locate the first byte
//! matching a set of structural characters.  100% safe Rust — forbid(unsafe_code)
//! is honoured; the only intrinsic used is `u64::from_le_bytes` on a fixed-size array.
//!
//! # Algorithm
//!
//! The classic SWAR "has-zero" test:
//!
//! ```text
//! has_zero(word) = (word - 0x0101_0101_0101_0101)
//!                 & (!word)
//!                 & 0x8080_8080_8080_8080
//! ```
//!
//! This produces a word with a set high bit in each byte position that was zero.
//! To test for an arbitrary value `v`, XOR the word with `broadcast(v)` first —
//! flipping the target byte to zero — then apply `has_zero`.
//!
//! # Position accounting
//!
//! All helpers return a **byte count** (how many bytes the caller may skip),
//! not a transformed value.  The caller advances its offset/column by that count.
//! Newlines are never skipped by these helpers: any helper that might encounter
//! a `\n` or `\r` stops *before* it, preserving the invariant that all
//! line-break bookkeeping is done by `skip_line()`.

/// Broadcasts a single byte `v` into all 8 byte lanes of a `u64`.
#[inline(always)]
const fn broadcast(v: u8) -> u64 {
    v as u64 * 0x0101_0101_0101_0101_u64
}

/// Returns a mask with the high bit set in each byte lane that is zero.
#[inline(always)]
const fn has_zero(word: u64) -> u64 {
    word.wrapping_sub(0x0101_0101_0101_0101_u64) & !word & 0x8080_8080_8080_8080_u64
}

/// Returns a mask with the high bit set in each byte lane equal to `v`.
#[inline(always)]
const fn has_value(word: u64, v: u8) -> u64 {
    has_zero(word ^ broadcast(v))
}

/// Number of leading bytes (from LSB end) in a SWAR match mask with no match.
///
/// Given a mask from `has_zero`/`has_value` (high bit set per matching lane),
/// returns the index of the first matching byte.  Returns 8 if no match.
///
/// On little-endian (x86/aarch64), byte 0 of the original slice lands in bits
/// `[7:0]` of the loaded word.  The first match's high bit is therefore at the
/// *lowest* set bit of the mask.
#[inline(always)]
fn match_offset(mask: u64) -> usize {
    // `0u64.trailing_zeros()` is 64, so this also yields 8 ("no match") for a
    // zero mask without a separate branch — one uniform, fully-covered path.
    (mask.trailing_zeros() / 8) as usize
}

/// Load 8 bytes from `bytes[offset..offset+8]` as a little-endian `u64`.
///
/// Panics (debug-only) if fewer than 8 bytes remain — always guard with
/// `bytes.len() - offset >= 8` before calling.
#[inline(always)]
fn load_u64_le(bytes: &[u8], offset: usize) -> u64 {
    // The slice [offset..offset+8] is bounds-checked by the compiler since we
    // always call this after an `i + 8 <= end` guard.  Assembling the word from
    // eight explicit, individually bounds-checked index reads is infallible —
    // it has no error arm, so coverage credits every executed line and there is
    // no dead defensive branch to test.
    u64::from(bytes[offset])
        | (u64::from(bytes[offset + 1]) << 8)
        | (u64::from(bytes[offset + 2]) << 16)
        | (u64::from(bytes[offset + 3]) << 24)
        | (u64::from(bytes[offset + 4]) << 32)
        | (u64::from(bytes[offset + 5]) << 40)
        | (u64::from(bytes[offset + 6]) << 48)
        | (u64::from(bytes[offset + 7]) << 56)
}

// ─── Public scan helpers ────────────────────────────────────────────────────

/// Find the first byte in `bytes[offset..]` that is not a space (0x20) or
/// tab (0x09).
///
/// Returns the number of leading space/tab bytes relative to `offset`.
/// If all remaining bytes are spaces/tabs, returns `bytes.len() - offset`.
///
/// This is safe for whitespace skipping in `skip_to_next_token` because
/// neither `\n` nor `\r` is a space or tab — a newline halts the scan.
#[must_use]
pub fn skip_spaces_tabs(bytes: &[u8], offset: usize) -> usize {
    let mut i = offset;
    let end = bytes.len();

    while i + 8 <= end {
        let word = load_u64_le(bytes, i);
        let space_mask = has_value(word, b' ');
        let tab_mask = has_value(word, b'\t');
        let either = space_mask | tab_mask;
        // `either` has a high bit in each lane that IS a space or tab.
        // We want the first lane that is neither.
        let non_either = !either & 0x8080_8080_8080_8080_u64;
        if non_either != 0 {
            let skip = (non_either.trailing_zeros() / 8) as usize;
            return i + skip - offset;
        }
        i += 8;
    }

    // Scalar tail.
    while i < end {
        let b = bytes[i];
        if b != b' ' && b != b'\t' {
            return i - offset;
        }
        i += 1;
    }
    end - offset
}

/// Find the first line-break byte (`\n` or `\r`) in `bytes[offset..]`.
///
/// Returns the number of bytes before the first line break, or
/// `bytes.len() - offset` if no break is found.
///
/// Used to fast-forward comment-skip and other "scan to end of line" loops.
/// The caller still processes the actual line break via `skip_line()`.
#[must_use]
pub fn find_line_end(bytes: &[u8], offset: usize) -> usize {
    let mut i = offset;
    let end = bytes.len();

    while i + 8 <= end {
        let word = load_u64_le(bytes, i);
        let lf_mask = has_value(word, b'\n');
        let cr_mask = has_value(word, b'\r');
        let break_mask = lf_mask | cr_mask;
        if break_mask != 0 {
            let skip = match_offset(break_mask);
            return i + skip - offset;
        }
        i += 8;
    }

    while i < end {
        let b = bytes[i];
        if b == b'\n' || b == b'\r' {
            return i - offset;
        }
        i += 1;
    }
    end - offset
}

/// Find the first structural byte ending a plain scalar in block context.
///
/// Stops at: `\n`, `\r`, ` `, `\t`, `#`, `:`.
///
/// The `:` case is a conservative hint — the caller must verify that the
/// byte after the colon is a blank/break/EOF before treating it as a value
/// indicator.  If it is not, the caller advances one byte and calls this
/// helper again.
///
/// Returns number of bytes that are safe to skip (relative to `offset`).
#[must_use]
pub fn find_plain_scalar_end_block(bytes: &[u8], offset: usize) -> usize {
    let mut i = offset;
    let end = bytes.len();

    while i + 8 <= end {
        let word = load_u64_le(bytes, i);
        let stop = has_value(word, b'\n')
            | has_value(word, b'\r')
            | has_value(word, b' ')
            | has_value(word, b'\t')
            | has_value(word, b'#')
            | has_value(word, b':');
        if stop != 0 {
            let skip = match_offset(stop);
            return i + skip - offset;
        }
        i += 8;
    }

    while i < end {
        let b = bytes[i];
        if matches!(b, b'\n' | b'\r' | b' ' | b'\t' | b'#' | b':') {
            return i - offset;
        }
        i += 1;
    }
    end - offset
}

/// Find the first structural byte ending a plain scalar in flow context.
///
/// Same as [`find_plain_scalar_end_block`] plus the flow indicators
/// `{`, `}`, `[`, `]`, `,`.
#[must_use]
pub fn find_plain_scalar_end_flow(bytes: &[u8], offset: usize) -> usize {
    let mut i = offset;
    let end = bytes.len();

    while i + 8 <= end {
        let word = load_u64_le(bytes, i);
        let stop = has_value(word, b'\n')
            | has_value(word, b'\r')
            | has_value(word, b' ')
            | has_value(word, b'\t')
            | has_value(word, b'#')
            | has_value(word, b':')
            | has_value(word, b'{')
            | has_value(word, b'}')
            | has_value(word, b'[')
            | has_value(word, b']')
            | has_value(word, b',');
        if stop != 0 {
            let skip = match_offset(stop);
            return i + skip - offset;
        }
        i += 8;
    }

    while i < end {
        let b = bytes[i];
        if matches!(
            b,
            b'\n' | b'\r' | b' ' | b'\t' | b'#' | b':' | b'{' | b'}' | b'[' | b']' | b','
        ) {
            return i - offset;
        }
        i += 1;
    }
    end - offset
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Reference (scalar) implementations for cross-checking ─────────────

    fn scalar_skip_spaces_tabs(bytes: &[u8], offset: usize) -> usize {
        let mut i = offset;
        while i < bytes.len() {
            let b = bytes[i];
            if b != b' ' && b != b'\t' {
                break;
            }
            i += 1;
        }
        i - offset
    }

    fn scalar_find_line_end(bytes: &[u8], offset: usize) -> usize {
        let mut i = offset;
        while i < bytes.len() {
            if bytes[i] == b'\n' || bytes[i] == b'\r' {
                break;
            }
            i += 1;
        }
        i - offset
    }

    fn scalar_find_plain_end_block(bytes: &[u8], offset: usize) -> usize {
        let mut i = offset;
        while i < bytes.len() {
            let b = bytes[i];
            if matches!(b, b'\n' | b'\r' | b' ' | b'\t' | b'#' | b':') {
                break;
            }
            i += 1;
        }
        i - offset
    }

    fn scalar_find_plain_end_flow(bytes: &[u8], offset: usize) -> usize {
        let mut i = offset;
        while i < bytes.len() {
            let b = bytes[i];
            if matches!(
                b,
                b'\n' | b'\r' | b' ' | b'\t' | b'#' | b':' | b'{' | b'}' | b'[' | b']' | b','
            ) {
                break;
            }
            i += 1;
        }
        i - offset
    }

    // ─── Empty input ────────────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_zero() {
        assert_eq!(skip_spaces_tabs(&[], 0), 0);
        assert_eq!(find_line_end(&[], 0), 0);
        assert_eq!(find_plain_scalar_end_block(&[], 0), 0);
        assert_eq!(find_plain_scalar_end_flow(&[], 0), 0);
    }

    // ─── All 256 single-byte inputs ─────────────────────────────────────────

    #[test]
    fn skip_spaces_tabs_matches_scalar_all_bytes() {
        for b in 0u8..=255 {
            let input = [b];
            let swar = skip_spaces_tabs(&input, 0);
            let scalar = scalar_skip_spaces_tabs(&input, 0);
            assert_eq!(swar, scalar, "byte {b:#04x}");
        }
    }

    #[test]
    fn find_line_end_matches_scalar_all_bytes() {
        for b in 0u8..=255 {
            let input = [b];
            let swar = find_line_end(&input, 0);
            let scalar = scalar_find_line_end(&input, 0);
            assert_eq!(swar, scalar, "byte {b:#04x}");
        }
    }

    #[test]
    fn find_plain_end_block_matches_scalar_all_bytes() {
        for b in 0u8..=255 {
            let input = [b];
            let swar = find_plain_scalar_end_block(&input, 0);
            let scalar = scalar_find_plain_end_block(&input, 0);
            assert_eq!(swar, scalar, "byte {b:#04x}");
        }
    }

    #[test]
    fn find_plain_end_flow_matches_scalar_all_bytes() {
        for b in 0u8..=255 {
            let input = [b];
            let swar = find_plain_scalar_end_flow(&input, 0);
            let scalar = scalar_find_plain_end_flow(&input, 0);
            assert_eq!(swar, scalar, "byte {b:#04x}");
        }
    }

    // ─── Short inputs (< 8 bytes) ────────────────────────────────────────────

    #[test]
    fn skip_spaces_tabs_short() {
        let cases: &[(&[u8], usize, usize)] = &[
            (b" ", 0, 1),
            (b"\t", 0, 1),
            (b"  \t ", 0, 4),
            (b"  a", 0, 2),
            (b"\t\tb", 0, 2),
            (b"abc", 0, 0),
            (b"   \n", 0, 3),   // stops at newline
            (b"\t \r\n", 0, 2), // stops at CR
        ];
        for &(input, off, expected) in cases {
            let got = skip_spaces_tabs(input, off);
            let ref_got = scalar_skip_spaces_tabs(input, off);
            assert_eq!(got, expected, "input={input:?} off={off}");
            assert_eq!(got, ref_got, "swar/scalar mismatch for {input:?}");
        }
    }

    #[test]
    fn find_line_end_short() {
        let cases: &[(&[u8], usize, usize)] = &[
            (b"\n", 0, 0),
            (b"\r", 0, 0),
            (b"abc\n", 0, 3),
            (b"abc\r\n", 0, 3),
            (b"abcdefg", 0, 7),
            (b"abc", 2, 1),
        ];
        for &(input, off, expected) in cases {
            let got = find_line_end(input, off);
            let ref_got = scalar_find_line_end(input, off);
            assert_eq!(got, expected, "input={input:?} off={off}");
            assert_eq!(got, ref_got, "swar/scalar mismatch for {input:?}");
        }
    }

    #[test]
    fn find_plain_end_block_short() {
        let cases: &[(&[u8], usize, usize)] = &[
            (b"hello", 0, 5),
            (b"hello world", 0, 5),
            (b"hello\nworld", 0, 5),
            (b"key: value", 0, 3),
            (b"foo#bar", 0, 3),
            (b"abcdefgh", 0, 8),
            (b"abcdefgh!", 0, 9),
        ];
        for &(input, off, expected) in cases {
            let swar = find_plain_scalar_end_block(input, off);
            let scalar = scalar_find_plain_end_block(input, off);
            assert_eq!(swar, scalar, "swar/scalar mismatch {input:?} off={off}");
            assert_eq!(swar, expected, "input={input:?} off={off}");
        }
    }

    // ─── Exactly 8 bytes ────────────────────────────────────────────────────

    #[test]
    fn exactly_8_bytes_no_stop() {
        let input = b"abcdefgh";
        assert_eq!(skip_spaces_tabs(input, 0), 0);
        assert_eq!(find_line_end(input, 0), 8);
        assert_eq!(find_plain_scalar_end_block(input, 0), 8);
        assert_eq!(find_plain_scalar_end_flow(input, 0), 8);
    }

    #[test]
    fn exactly_8_spaces() {
        let input = b"        ";
        assert_eq!(skip_spaces_tabs(input, 0), 8);
        assert_eq!(scalar_skip_spaces_tabs(input, 0), 8);
    }

    // ─── Multi-word (> 8 bytes) ──────────────────────────────────────────────

    #[test]
    fn multi_word_no_stop() {
        let input = b"abcdefghijklmnopqrstuvwxyz";
        assert_eq!(find_line_end(input, 0), 26);
        assert_eq!(find_plain_scalar_end_block(input, 0), 26);
        assert_eq!(find_plain_scalar_end_flow(input, 0), 26);
    }

    #[test]
    fn multi_word_stop_in_second_word() {
        let input = b"abcdefgh:rest";
        let swar = find_plain_scalar_end_block(input, 0);
        let scalar = scalar_find_plain_end_block(input, 0);
        assert_eq!(swar, scalar);
        assert_eq!(swar, 8);
    }

    #[test]
    fn stop_at_every_position_in_two_words() {
        // Stopper bytes for block context
        let stoppers = [b'\n', b'\r', b' ', b'\t', b'#', b':'];
        for stopper in stoppers {
            for pos in 0usize..16 {
                let mut input: Vec<u8> = b"abcdefghijklmnop".to_vec();
                input[pos] = stopper;
                let swar = find_plain_scalar_end_block(&input, 0);
                let scalar = scalar_find_plain_end_block(&input, 0);
                assert_eq!(
                    swar, scalar,
                    "stopper={stopper:#04x} pos={pos}: swar={swar} scalar={scalar}"
                );
                assert_eq!(swar, pos, "stop at pos={pos} stopper={stopper:#04x}");
            }
        }
    }

    #[test]
    fn flow_stoppers_at_every_position() {
        let stoppers = [b'{', b'}', b'[', b']', b','];
        for stopper in stoppers {
            for pos in 0usize..16 {
                let mut input: Vec<u8> = b"abcdefghijklmnop".to_vec();
                input[pos] = stopper;
                let swar = find_plain_scalar_end_flow(&input, 0);
                let scalar = scalar_find_plain_end_flow(&input, 0);
                assert_eq!(
                    swar, scalar,
                    "stopper={stopper:#04x} pos={pos}: swar={swar} scalar={scalar}"
                );
                assert_eq!(swar, pos);
            }
        }
    }

    // ─── Non-zero offset correctness ────────────────────────────────────────

    #[test]
    fn non_zero_offset() {
        let input = b"   hello world";
        assert_eq!(skip_spaces_tabs(input, 3), 0);
        assert_eq!(find_line_end(input, 8), 6);

        for off in 0..input.len() {
            assert_eq!(
                skip_spaces_tabs(input, off),
                scalar_skip_spaces_tabs(input, off),
                "skip_spaces_tabs offset={off}"
            );
            assert_eq!(
                find_line_end(input, off),
                scalar_find_line_end(input, off),
                "find_line_end offset={off}"
            );
            assert_eq!(
                find_plain_scalar_end_block(input, off),
                scalar_find_plain_end_block(input, off),
                "plain_end_block offset={off}"
            );
        }
    }

    // ─── Large all-spaces-tabs ───────────────────────────────────────────────

    #[test]
    fn all_spaces_and_tabs_large() {
        let mut input = vec![b' '; 100];
        for i in (0..100).step_by(3) {
            input[i] = b'\t';
        }
        assert_eq!(
            skip_spaces_tabs(&input, 0),
            scalar_skip_spaces_tabs(&input, 0)
        );
        assert_eq!(skip_spaces_tabs(&input, 0), 100);
    }

    // ─── SWAR internals ─────────────────────────────────────────────────────

    #[test]
    fn has_zero_detects_zero_byte() {
        let word = u64::from_le_bytes([b'A', b'A', b'A', 0, b'A', b'A', b'A', b'A']);
        let mask = has_zero(word);
        let expected_bit_pos = 3 * 8 + 7;
        assert_ne!(mask, 0);
        assert!(mask & (1u64 << expected_bit_pos) != 0, "mask={mask:#018x}");
    }

    #[test]
    fn has_value_detects_colon() {
        let word = u64::from_le_bytes([b'h', b'e', b'l', b'l', b'o', b':', b'1', b'2']);
        let mask = has_value(word, b':');
        let expected_bit_pos = 5 * 8 + 7;
        assert_ne!(mask, 0);
        assert!(mask & (1u64 << expected_bit_pos) != 0, "mask={mask:#018x}");
    }

    #[test]
    fn match_offset_first_byte() {
        let mask: u64 = 0x80;
        assert_eq!(match_offset(mask), 0);
    }

    #[test]
    fn match_offset_last_byte() {
        let mask: u64 = 0x80_00_00_00_00_00_00_00;
        assert_eq!(match_offset(mask), 7);
    }

    #[test]
    fn match_offset_no_match() {
        assert_eq!(match_offset(0), 8);
    }

    // ─── Realistic YAML fragment ─────────────────────────────────────────────

    #[test]
    fn equivalence_on_yaml_fragment() {
        let input =
            b"apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: my-app\n  namespace: default\n";
        for off in 0..input.len() {
            assert_eq!(
                find_plain_scalar_end_block(input, off),
                scalar_find_plain_end_block(input, off),
                "plain_end_block off={off}"
            );
            assert_eq!(
                find_line_end(input, off),
                scalar_find_line_end(input, off),
                "find_line_end off={off}"
            );
            assert_eq!(
                skip_spaces_tabs(input, off),
                scalar_skip_spaces_tabs(input, off),
                "skip_spaces_tabs off={off}"
            );
        }
    }
}
