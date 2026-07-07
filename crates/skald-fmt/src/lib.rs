// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Formats `src` safely (trailing-whitespace trim + single final newline),
/// preserving comments, indentation, and block/quoted-scalar content. Returns
/// `Err(message)` if `src` is not valid YAML (a formatter must not touch
/// unparseable input).
pub fn format_str(src: &str) -> Result<String, String> {
    skald::from_str_node(src).map_err(|e| format!("parse error: {e}"))?;
    Ok(skald::cst::Document::parse(src).reformatted())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_trailing_whitespace() {
        assert_eq!(format_str("a: 1   \nb: 2\n").unwrap(), "a: 1\nb: 2\n");
    }

    #[test]
    fn rejects_invalid_yaml() {
        assert!(format_str("a: [1, 2").is_err());
    }

    #[test]
    fn idempotent() {
        let once = format_str("x: y  \n").unwrap();
        assert_eq!(format_str(&once).unwrap(), once);
    }

    #[test]
    fn preserves_comments() {
        assert_eq!(format_str("a: 1  # c\n").unwrap(), "a: 1  # c\n");
    }
}
