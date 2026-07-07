// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Resource limits for safe YAML processing.
//!
//! Provides configurable limits to protect against adversarial inputs
//! such as billion laughs attacks, deep nesting, and memory exhaustion.
//!
//! # Defaults
//!
//! All limits have safe defaults. Users must explicitly opt in to raise them.
//!
//! ```
//! use skald_core::limits::ResourceLimits;
//!
//! let limits = ResourceLimits::default();
//! assert_eq!(limits.max_depth, 128);
//! ```

/// Configurable resource limits for YAML processing.
///
/// These limits protect against known attack vectors:
/// - **Billion Laughs**: Exponential alias expansion → [`max_alias_expansions`](Self::max_alias_expansions)
/// - **Deep nesting**: Stack overflow → [`max_depth`](Self::max_depth)
/// - **Huge documents**: Memory exhaustion → [`max_document_size`](Self::max_document_size)
/// - **Huge keys**: Memory exhaustion → [`max_key_length`](Self::max_key_length)
/// - **Node flood**: CPU exhaustion → [`max_node_count`](Self::max_node_count)
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct ResourceLimits {
    /// Maximum nesting depth for collections. Default: 128.
    pub max_depth: usize,
    /// Maximum number of alias expansions allowed. Default: 1,024.
    pub max_alias_expansions: usize,
    /// Maximum document size in bytes. Default: 256 MiB.
    pub max_document_size: usize,
    /// Maximum length of a mapping key in bytes. Default: 1,024.
    pub max_key_length: usize,
    /// Maximum number of nodes in the representation graph. Default: 1,000,000.
    pub max_node_count: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_alias_expansions: 1_024,
            max_document_size: 256 * 1024 * 1024, // 256 MiB
            max_key_length: 1_024,
            max_node_count: 1_000_000,
        }
    }
}

impl ResourceLimits {
    /// Creates limits with no restrictions. Use with caution — only for trusted inputs.
    pub fn unlimited() -> Self {
        Self {
            max_depth: usize::MAX,
            max_alias_expansions: usize::MAX,
            max_document_size: usize::MAX,
            max_key_length: usize::MAX,
            max_node_count: usize::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_are_safe() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_depth, 128);
        assert_eq!(limits.max_alias_expansions, 1_024);
        assert_eq!(limits.max_document_size, 256 * 1024 * 1024);
        assert_eq!(limits.max_key_length, 1_024);
        assert_eq!(limits.max_node_count, 1_000_000);
    }

    #[test]
    fn unlimited_is_unrestricted() {
        let limits = ResourceLimits::unlimited();
        assert_eq!(limits.max_depth, usize::MAX);
        assert_eq!(limits.max_alias_expansions, usize::MAX);
    }
}
