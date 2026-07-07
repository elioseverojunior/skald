// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let doc = skald_cst::Document::parse(s);
        assert_eq!(doc.to_string(), s, "CST round-trip must be lossless");
    }
});
