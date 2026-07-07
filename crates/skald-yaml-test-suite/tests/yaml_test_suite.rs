// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests using the official YAML test suite.
//!
//! Runs all test cases from <https://github.com/yaml/yaml-test-suite>
//! using the `data/` directory format and reports pass/fail statistics.

use skald_yaml_test_suite::{TestResult, load_all_tests, run_test};
use std::path::Path;

#[test]
fn yaml_test_suite() {
    let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data");

    if !data_dir.exists() {
        eprintln!(
            "YAML test suite data not found at {}. Skipping.",
            data_dir.display()
        );
        eprintln!("Run: git submodule update --init");
        return;
    }

    let all_tests = load_all_tests(&data_dir);

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for tc in &all_tests {
        total += 1;

        match run_test(tc) {
            TestResult::Pass => passed += 1,
            TestResult::Fail(reason) => {
                failed += 1;
                failures.push(format!("{} ({}):\n{reason}", tc.id, tc.name));
            }
        }
    }

    let pass_rate = if total > 0 {
        (passed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    eprintln!();
    eprintln!("╔══════════════════════════════════════╗");
    eprintln!("║     YAML Test Suite Results          ║");
    eprintln!("╠══════════════════════════════════════╣");
    eprintln!("║  Total:   {total:>5}                      ║");
    eprintln!("║  Passed:  {passed:>5}                      ║");
    eprintln!("║  Failed:  {failed:>5}                      ║");
    eprintln!("║  Skipped:     0                      ║");
    eprintln!("║  Rate:   {pass_rate:>5.1}%                      ║");
    eprintln!("╚══════════════════════════════════════╝");

    // Categorize failures
    let mut cat_wrong_accept = 0usize; // accepted invalid YAML
    let mut cat_wrong_reject = 0usize; // rejected valid YAML
    let mut cat_wrong_events = 0usize; // wrong event stream

    for f in &failures {
        if f.contains("expected error but parser succeeded") {
            cat_wrong_accept += 1;
        } else if f.contains("unexpected error") {
            cat_wrong_reject += 1;
        } else {
            cat_wrong_events += 1;
        }
    }

    eprintln!();
    eprintln!("  Failure breakdown:");
    eprintln!("    Wrong accept (should reject): {cat_wrong_accept}");
    eprintln!("    Wrong reject (should accept): {cat_wrong_reject}");
    eprintln!("    Wrong events (diff):          {cat_wrong_events}");

    if !failures.is_empty() {
        eprintln!();
        eprintln!("=== All failures ===");
        for f in &failures {
            eprintln!();
            eprintln!("{f}");
        }
    }

    // Track progress: assert a minimum pass rate.
    // Increase this threshold as we fix more tests.
    assert!(
        pass_rate >= 10.0,
        "Pass rate {pass_rate:.1}% is below minimum threshold of 10%"
    );
}
