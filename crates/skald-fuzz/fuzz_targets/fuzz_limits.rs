// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

#![no_main]
use libfuzzer_sys::fuzz_target;
use skald_ast::composer::Composer;
use skald_core::error::ParserConfig;
use skald_core::limits::ResourceLimits;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Tight resource limits: the composer must not panic — errors are fine.
        let config = ParserConfig {
            limits: ResourceLimits {
                max_depth: 4,
                max_alias_expansions: 8,
                max_document_size: 4096,
                max_key_length: 64,
                max_node_count: 64,
            },
            ..Default::default()
        };
        for result in Composer::with_config(s, config) {
            let _ = result;
        }
    }
});
