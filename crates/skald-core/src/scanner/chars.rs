// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitflag-based character classification for YAML scanning.
//!
//! Uses a 256-byte lookup table where each byte can have multiple properties.
//! This is cache-friendly (256 bytes fits in a single cache line pair) and
//! allows O(1) character classification with a single table lookup + bitwise AND.

/// Whitespace: space (0x20) or tab (0x09).
pub const FLAG_WHITESPACE: u8 = 1 << 0;
/// Line break: LF (0x0A) or CR (0x0D).
pub const FLAG_BREAK: u8 = 1 << 1;
/// ASCII digit: 0-9.
pub const FLAG_DIGIT: u8 = 1 << 2;
/// ASCII letter: a-z, A-Z.
pub const FLAG_ALPHA: u8 = 1 << 3;
/// YAML flow indicator: `{`, `}`, `[`, `]`, `,`.
pub const FLAG_FLOW: u8 = 1 << 4;
/// YAML indicator character (structural meaning in YAML).
pub const FLAG_INDICATOR: u8 = 1 << 5;
/// Valid in anchor/alias names: alphanumeric + `-` + `_`.
pub const FLAG_ANCHOR: u8 = 1 << 6;
/// Blank: whitespace or line break.
pub const FLAG_BLANK_OR_BREAK: u8 = FLAG_WHITESPACE | FLAG_BREAK;

/// Compile-time character classification table.
pub const CHAR_FLAGS: [u8; 256] = build_char_flags();

const fn build_char_flags() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0u16;
    while i < 256 {
        let b = i as u8;
        let mut flags = 0u8;

        // Whitespace
        if b == b' ' || b == b'\t' {
            flags |= FLAG_WHITESPACE;
        }

        // Line breaks
        if b == b'\n' || b == b'\r' {
            flags |= FLAG_BREAK;
        }

        // Digits
        if b >= b'0' && b <= b'9' {
            flags |= FLAG_DIGIT;
        }

        // Letters
        if (b >= b'a' && b <= b'z') || (b >= b'A' && b <= b'Z') {
            flags |= FLAG_ALPHA;
        }

        // Flow indicators
        if b == b'{' || b == b'}' || b == b'[' || b == b']' || b == b',' {
            flags |= FLAG_FLOW;
        }

        // YAML indicators (characters with special meaning)
        if b == b'-'
            || b == b'?'
            || b == b':'
            || b == b','
            || b == b'['
            || b == b']'
            || b == b'{'
            || b == b'}'
            || b == b'#'
            || b == b'&'
            || b == b'*'
            || b == b'!'
            || b == b'|'
            || b == b'>'
            || b == b'\''
            || b == b'"'
            || b == b'%'
            || b == b'@'
            || b == b'`'
        {
            flags |= FLAG_INDICATOR;
        }

        // Anchor-safe characters
        if (b >= b'a' && b <= b'z')
            || (b >= b'A' && b <= b'Z')
            || (b >= b'0' && b <= b'9')
            || b == b'-'
            || b == b'_'
        {
            flags |= FLAG_ANCHOR;
        }

        table[i as usize] = flags;
        i += 1;
    }
    table
}

/// Returns `true` if the byte has the given flag(s).
#[must_use]
#[inline(always)]
pub fn is(b: u8, flags: u8) -> bool {
    CHAR_FLAGS[b as usize] & flags != 0
}

/// Returns `true` if the byte is a YAML whitespace (space or tab).
#[must_use]
#[inline(always)]
pub fn is_whitespace(b: u8) -> bool {
    is(b, FLAG_WHITESPACE)
}

/// Returns `true` if the byte is a line break (LF or CR).
#[must_use]
#[inline(always)]
pub fn is_break(b: u8) -> bool {
    is(b, FLAG_BREAK)
}

/// Returns `true` if the byte is blank (whitespace or line break).
#[must_use]
#[inline(always)]
pub fn is_blank_or_break(b: u8) -> bool {
    is(b, FLAG_BLANK_OR_BREAK)
}

/// Returns `true` if the byte is a flow indicator.
#[must_use]
#[inline(always)]
pub fn is_flow(b: u8) -> bool {
    is(b, FLAG_FLOW)
}

/// Returns `true` if the byte is a YAML indicator.
#[must_use]
#[inline(always)]
pub fn is_indicator(b: u8) -> bool {
    is(b, FLAG_INDICATOR)
}

/// Returns `true` if the byte is valid in an anchor or alias name.
#[must_use]
#[inline(always)]
pub fn is_anchor_char(b: u8) -> bool {
    is(b, FLAG_ANCHOR)
}

/// Returns `true` if the byte is an ASCII digit.
#[must_use]
#[inline(always)]
pub fn is_digit(b: u8) -> bool {
    is(b, FLAG_DIGIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_classification() {
        assert!(is_whitespace(b' '));
        assert!(is_whitespace(b'\t'));
        assert!(!is_whitespace(b'a'));
        assert!(!is_whitespace(b'\n'));
    }

    #[test]
    fn break_classification() {
        assert!(is_break(b'\n'));
        assert!(is_break(b'\r'));
        assert!(!is_break(b' '));
    }

    #[test]
    fn blank_or_break_classification() {
        assert!(is_blank_or_break(b' '));
        assert!(is_blank_or_break(b'\t'));
        assert!(is_blank_or_break(b'\n'));
        assert!(is_blank_or_break(b'\r'));
        assert!(!is_blank_or_break(b'a'));
    }

    #[test]
    fn flow_indicators() {
        for b in [b'{', b'}', b'[', b']', b','] {
            assert!(is_flow(b), "expected flow indicator: {}", b as char);
        }
        assert!(!is_flow(b'-'));
    }

    #[test]
    fn yaml_indicators() {
        for b in b"-?:,[]{}#&*!|>'\"%@`" {
            assert!(is_indicator(*b), "expected indicator: {}", *b as char);
        }
        assert!(!is_indicator(b'a'));
    }

    #[test]
    fn anchor_chars() {
        assert!(is_anchor_char(b'a'));
        assert!(is_anchor_char(b'Z'));
        assert!(is_anchor_char(b'0'));
        assert!(is_anchor_char(b'-'));
        assert!(is_anchor_char(b'_'));
        assert!(!is_anchor_char(b' '));
        assert!(!is_anchor_char(b':'));
    }

    #[test]
    fn digit_classification() {
        for b in b'0'..=b'9' {
            assert!(is_digit(b));
        }
        assert!(!is_digit(b'a'));
    }

    #[test]
    fn multi_flag_bytes() {
        // '-' is both an indicator and an anchor char
        assert!(is_indicator(b'-'));
        assert!(is_anchor_char(b'-'));
        // ',' is both an indicator and a flow indicator
        assert!(is_indicator(b','));
        assert!(is_flow(b','));
    }

    #[test]
    fn table_size() {
        assert_eq!(CHAR_FLAGS.len(), 256);
    }

    /// Tarpaulin instruments runtime execution, so the const fn body
    /// `build_char_flags` (which runs at *compile* time to initialize
    /// `CHAR_FLAGS`) shows as uncovered. `const fn` is callable at both
    /// compile time and runtime — invoking it from a test exercises the
    /// same code paths at runtime, giving coverage credit for every
    /// branch in the classification logic.
    #[test]
    fn const_fn_body_runtime_evaluation_matches_table() {
        let runtime_table = build_char_flags();
        assert_eq!(runtime_table, CHAR_FLAGS);
    }

    /// Asserts every byte's classification independently — guards against
    /// any future change to the const fn altering the bit pattern.
    #[test]
    fn exhaustive_byte_classification() {
        for b in 0u8..=255 {
            let actual = CHAR_FLAGS[b as usize];

            let mut expected = 0u8;
            if b == b' ' || b == b'\t' {
                expected |= FLAG_WHITESPACE;
            }
            if b == b'\n' || b == b'\r' {
                expected |= FLAG_BREAK;
            }
            if b.is_ascii_digit() {
                expected |= FLAG_DIGIT;
            }
            if b.is_ascii_alphabetic() {
                expected |= FLAG_ALPHA;
            }
            if matches!(b, b'{' | b'}' | b'[' | b']' | b',') {
                expected |= FLAG_FLOW;
            }
            if matches!(
                b,
                b'-' | b'?'
                    | b':'
                    | b','
                    | b'['
                    | b']'
                    | b'{'
                    | b'}'
                    | b'#'
                    | b'&'
                    | b'*'
                    | b'!'
                    | b'|'
                    | b'>'
                    | b'\''
                    | b'"'
                    | b'%'
                    | b'@'
                    | b'`'
            ) {
                expected |= FLAG_INDICATOR;
            }
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                expected |= FLAG_ANCHOR;
            }

            assert_eq!(
                actual, expected,
                "byte {b:#04x} ({:?}) classification mismatch",
                b as char
            );
        }
    }
}
