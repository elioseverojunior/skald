// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Deserialize as an untyped Value; must not panic on arbitrary input.
        if let Ok(value) = skald_serde::from_str::<skald_serde::Value>(s) {
            // Serialize back to string and deserialize again; must not panic.
            if let Ok(roundtripped) = skald_serde::to_string(&value) {
                let _ = skald_serde::from_str::<skald_serde::Value>(&roundtripped);
            }
        }
    }
});
