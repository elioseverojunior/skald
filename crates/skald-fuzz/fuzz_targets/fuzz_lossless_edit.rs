// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut doc = skald_cst::Document::parse(s);
        let _ = doc.set("a", "x"); // may Err; must not panic
        let _ = doc.set("a.b.c", "y"); // nested path; must not panic
        let out = doc.to_string(); // must not panic
        // re-parsing the edited output must also be lossless on itself
        let doc2 = skald_cst::Document::parse(&out);
        assert_eq!(doc2.to_string(), out);
    }
});
