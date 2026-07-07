// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;
use skald_ast::{composer::compose_all, emitter::emit_to_string};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Compose then emit must not panic; errors at either stage are acceptable.
        if let Ok(docs) = compose_all(s) {
            for doc in docs {
                let _ = emit_to_string(&doc, &Default::default());
            }
        }
    }
});
