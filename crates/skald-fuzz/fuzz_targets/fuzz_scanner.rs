// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;
use skald_core::scanner::Scanner;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Scanner must not panic on arbitrary UTF-8 input; errors are acceptable.
        for result in Scanner::new(s) {
            let _ = result;
        }
    }
});
