// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! YAML test suite integration for skald.
//!
//! This crate is dev-only and not published.
//! It consumes the official YAML test suite (`data/` directory format)
//! and generates test cases.

use skald_core::parser::Parser;
use skald_core::parser::event::EventKind;
use skald_core::types::{CollectionStyle, ScalarStyle};
use std::path::Path;

// ─── Data-directory test case ────────────────────────────────────────

/// A test case from the `data/` directory format.
#[derive(Debug)]
pub struct DataTestCase {
    /// Test ID (e.g. "229Q" or "2G84/00").
    pub id: String,
    /// Human-readable name from `===` file.
    pub name: String,
    /// Raw YAML input from `in.yaml`.
    pub yaml: String,
    /// Expected event lines from `test.event`.
    pub expected_events: Vec<String>,
    /// Whether `error` file exists (expect parse failure).
    pub expect_error: bool,
}

/// Load a single test case from a directory containing `in.yaml`, `test.event`, etc.
fn load_single_test(dir: &Path, id: String) -> Option<DataTestCase> {
    let in_yaml = dir.join("in.yaml");
    if !in_yaml.exists() {
        return None;
    }

    let yaml = std::fs::read_to_string(&in_yaml)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", in_yaml.display()));

    let name_path = dir.join("===");
    let name = if name_path.exists() {
        std::fs::read_to_string(name_path)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };

    let event_path = dir.join("test.event");
    let expected_events = if event_path.exists() {
        let content = std::fs::read_to_string(event_path).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let expect_error = dir.join("error").exists();

    Some(DataTestCase {
        id,
        name,
        yaml,
        expected_events,
        expect_error,
    })
}

/// Load all tests from the `data/` directory.
///
/// Discovers 4-char alphanumeric test directories, handles both single-test
/// directories (containing `in.yaml`) and multi-subtest directories (containing
/// numbered subdirectories `00/`, `01/`, …).
pub fn load_all_tests(data_dir: &Path) -> Vec<DataTestCase> {
    let mut entries: Vec<_> = std::fs::read_dir(data_dir)
        .expect("failed to read data directory")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            // 4-char alphanumeric test IDs only (skips `name/`, `tags/`)
            name.len() == 4 && name.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut tests = Vec::new();

    for entry in entries {
        let test_id = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();

        if dir.join("in.yaml").exists() {
            // Single test
            if let Some(tc) = load_single_test(&dir, test_id) {
                tests.push(tc);
            }
        } else {
            // Multi-subtest: iterate sorted numeric subdirectories
            let mut subdirs: Vec<_> = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            subdirs.sort_by_key(|e| e.file_name());

            for sub in subdirs {
                let sub_name = sub.file_name().to_string_lossy().to_string();
                let sub_id = format!("{test_id}/{sub_name}");
                if let Some(tc) = load_single_test(&sub.path(), sub_id) {
                    tests.push(tc);
                }
            }
        }
    }

    tests
}

// ─── Event format conversion ────────────────────────────────────────

/// Run the skald parser on input and return events in the test suite tree format.
///
/// Returns `Ok(lines)` on success or `Err(message)` if the parser produces an error.
pub fn events_to_tree(input: &str) -> Result<Vec<String>, String> {
    let parser = Parser::new(input);
    let mut lines = Vec::new();

    for result in parser {
        match result {
            Ok(event) => lines.push(event_to_tree_line(&event.kind)),
            Err(e) => return Err(e.to_string()),
        }
    }

    Ok(lines)
}

/// Convert a single event to its tree format string.
fn event_to_tree_line(kind: &EventKind<'_>) -> String {
    match kind {
        EventKind::StreamStart => "+STR".to_string(),
        EventKind::StreamEnd => "-STR".to_string(),
        EventKind::DocumentStart { explicit } => {
            if *explicit {
                "+DOC ---".to_string()
            } else {
                "+DOC".to_string()
            }
        }
        EventKind::DocumentEnd { explicit } => {
            if *explicit {
                "-DOC ...".to_string()
            } else {
                "-DOC".to_string()
            }
        }
        EventKind::MappingStart { anchor, tag, style } => {
            let mut s = "+MAP".to_string();
            if *style == CollectionStyle::Flow {
                s.push_str(" {}");
            }
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some((h, sf)) = tag {
                s.push_str(&format!(" {}", format_tag(h, sf)));
            }
            s
        }
        EventKind::MappingEnd => "-MAP".to_string(),
        EventKind::SequenceStart { anchor, tag, style } => {
            let mut s = "+SEQ".to_string();
            if *style == CollectionStyle::Flow {
                s.push_str(" []");
            }
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some((h, sf)) = tag {
                s.push_str(&format!(" {}", format_tag(h, sf)));
            }
            s
        }
        EventKind::SequenceEnd => "-SEQ".to_string(),
        EventKind::Scalar {
            value,
            style,
            anchor,
            tag,
        } => {
            let mut s = "=VAL".to_string();
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some((h, sf)) = tag {
                s.push_str(&format!(" {}", format_tag(h, sf)));
            }
            let style_char = match style {
                ScalarStyle::Plain => ':',
                ScalarStyle::SingleQuoted => '\'',
                ScalarStyle::DoubleQuoted => '"',
                ScalarStyle::Literal => '|',
                ScalarStyle::Folded => '>',
            };
            s.push_str(&format!(" {style_char}{}", escape_value(value)));
            s
        }
        EventKind::Alias { name } => format!("=ALI *{name}"),
    }
}

/// Format a resolved tag (prefix + suffix) into the test suite tag format.
///
/// The parser pre-resolves tag handles using `%TAG` directives, so the handle
/// is already the resolved prefix (e.g., `"tag:yaml.org,2002:"` for `!!`).
fn format_tag(handle: &str, suffix: &str) -> String {
    format!("<{handle}{suffix}>")
}

/// Escape special characters in scalar values for the tree format.
fn escape_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\x08', "\\b")
        .replace('\0', "\\0")
}

// ─── Diff display ───────────────────────────────────────────────────

/// Show the first difference between expected and actual event trees.
pub fn show_diff(expected: &[String], actual: &[String]) -> String {
    let max = expected.len().max(actual.len());
    for i in 0..max {
        let exp = expected.get(i).map(|s| s.as_str()).unwrap_or("<missing>");
        let act = actual.get(i).map(|s| s.as_str()).unwrap_or("<missing>");
        if exp != act {
            return format!(
                "  line {}: expected: {exp:?}\n  line {}:   actual: {act:?}",
                i + 1,
                i + 1
            );
        }
    }
    "  (no difference found)".to_string()
}

// ─── Test runner ────────────────────────────────────────────────────

/// Result of running a single test case.
#[derive(Debug)]
pub enum TestResult {
    /// Test passed — events match expected tree.
    Pass,
    /// Test failed — events don't match expected tree.
    Fail(String),
}

/// Run a single test case and return the result.
pub fn run_test(tc: &DataTestCase) -> TestResult {
    if tc.expect_error {
        match events_to_tree(&tc.yaml) {
            Err(_) => TestResult::Pass,
            Ok(_) => TestResult::Fail("expected error but parser succeeded".to_string()),
        }
    } else {
        match events_to_tree(&tc.yaml) {
            Ok(actual) => {
                if actual == tc.expected_events {
                    TestResult::Pass
                } else {
                    TestResult::Fail(show_diff(&tc.expected_events, &actual))
                }
            }
            Err(e) => TestResult::Fail(format!("unexpected error: {e}")),
        }
    }
}

// ─── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // ── helpers ──────────────────────────────────────────────────────

    /// Create a temporary directory for a test case, clean it up on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!("skald_test_{name}_{}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).unwrap();
    }

    // ── load_single_test: missing === file (line 46) ─────────────────

    #[test]
    fn load_single_test_no_name_file_returns_empty_name() {
        let dir = TempDir::new("no_name");
        write(dir.path(), "in.yaml", "key: value\n");
        write(dir.path(), "test.event", "+STR\n+DOC\n-DOC\n-STR\n");
        // No "===" file — exercises the else branch at line 46
        let tc = load_single_test(dir.path(), "TEST".to_string()).unwrap();
        assert_eq!(tc.name, "");
        assert_eq!(tc.yaml, "key: value\n");
    }

    // ── load_single_test: missing test.event file (line 58) ──────────

    #[test]
    fn load_single_test_no_event_file_returns_empty_events() {
        let dir = TempDir::new("no_events");
        write(dir.path(), "in.yaml", "hello\n");
        write(dir.path(), "===", "My Test\n");
        // No "test.event" file — exercises the else branch at line 58
        let tc = load_single_test(dir.path(), "TEST".to_string()).unwrap();
        assert_eq!(tc.expected_events, Vec::<String>::new());
        assert_eq!(tc.name, "My Test");
    }

    // ── load_single_test: no in.yaml returns None ────────────────────

    #[test]
    fn load_single_test_no_in_yaml_returns_none() {
        let dir = TempDir::new("no_yaml");
        write(dir.path(), "===", "Ignored\n");
        let result = load_single_test(dir.path(), "NONE".to_string());
        assert!(result.is_none());
    }

    // ── show_diff: first difference found (lines 239-248) ────────────

    #[test]
    fn show_diff_reports_first_differing_line() {
        let expected = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let actual = vec!["a".to_string(), "X".to_string(), "c".to_string()];
        let diff = show_diff(&expected, &actual);
        assert!(diff.contains("line 2"), "expected line number in: {diff}");
        assert!(diff.contains("\"b\""), "expected expected value in: {diff}");
        assert!(diff.contains("\"X\""), "expected actual value in: {diff}");
    }

    #[test]
    fn show_diff_actual_shorter_shows_missing() {
        let expected = vec!["a".to_string(), "b".to_string()];
        let actual = vec!["a".to_string()];
        let diff = show_diff(&expected, &actual);
        assert!(diff.contains("<missing>"), "should show <missing>: {diff}");
    }

    #[test]
    fn show_diff_expected_shorter_shows_missing() {
        let expected = vec!["a".to_string()];
        let actual = vec!["a".to_string(), "extra".to_string()];
        let diff = show_diff(&expected, &actual);
        assert!(diff.contains("<missing>"), "should show <missing>: {diff}");
    }

    // ── show_diff: no difference found (line 250) ─────────────────────

    #[test]
    fn show_diff_no_difference_returns_sentinel() {
        let same = vec!["a".to_string(), "b".to_string()];
        let result = show_diff(&same, &same);
        assert_eq!(result, "  (no difference found)");
    }

    #[test]
    fn show_diff_both_empty_returns_sentinel() {
        let result = show_diff(&[], &[]);
        assert_eq!(result, "  (no difference found)");
    }

    // ── run_test: expect_error=true but parser succeeds (line 269) ───

    #[test]
    fn run_test_expect_error_but_parser_succeeds_is_fail() {
        let tc = DataTestCase {
            id: "T001".to_string(),
            name: "should fail".to_string(),
            yaml: "valid: yaml\n".to_string(),
            expected_events: vec![],
            expect_error: true,
        };
        match run_test(&tc) {
            TestResult::Fail(msg) => {
                assert!(
                    msg.contains("expected error but parser succeeded"),
                    "got: {msg}"
                );
            }
            TestResult::Pass => panic!("expected Fail, got Pass"),
        }
    }

    // ── run_test: events mismatch → Fail(show_diff(...)) (line 277) ──

    #[test]
    fn run_test_event_mismatch_is_fail_with_diff() {
        let tc = DataTestCase {
            id: "T002".to_string(),
            name: "mismatch".to_string(),
            yaml: "hello\n".to_string(),
            expected_events: vec![
                "+STR".to_string(),
                "+DOC".to_string(),
                "=VAL :WRONG".to_string(),
                "-DOC".to_string(),
                "-STR".to_string(),
            ],
            expect_error: false,
        };
        match run_test(&tc) {
            TestResult::Fail(msg) => {
                // The diff should mention the line where events diverge
                assert!(!msg.is_empty(), "diff message should not be empty");
            }
            TestResult::Pass => panic!("expected Fail due to event mismatch, got Pass"),
        }
    }

    // ── run_test: unexpected parse error (line 280) ──────────────────

    #[test]
    fn run_test_unexpected_parse_error_is_fail() {
        // Tab characters in a block mapping are forbidden by YAML spec (§6.1).
        // Skald correctly rejects this as an error (confirmed by 735/735 test
        // suite pass rate). With expect_error=false the run_test path at
        // line 280 is exercised: Err(e) => Fail("unexpected error: …").
        let tc = DataTestCase {
            id: "T003".to_string(),
            name: "unexpected error".to_string(),
            yaml: "---\na:\n\tb:\n\t\tc: value\n".to_string(), // tabs — invalid block indent
            expected_events: vec![],
            expect_error: false,
        };
        match run_test(&tc) {
            TestResult::Fail(msg) => {
                assert!(msg.contains("unexpected error"), "got: {msg}");
            }
            TestResult::Pass => panic!("expected Fail for tab-in-block-mapping, got Pass"),
        }
    }
}
