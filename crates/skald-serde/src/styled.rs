// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-value output-style wrappers for serialization.
//!
//! Wrap a value to control its YAML rendering:
//! - [`FlowSeq`]/[`FlowMap`] → flow style (`[a, b]` / `{k: v}`)
//! - [`LitStr`] → literal block scalar (`|`)
//! - [`FoldStr`] → folded block scalar (`>`)
//!
//! Carried through serde via reserved newtype-struct names that the serializer
//! intercepts. Per-value *comments* are not offered here: the serialize→`Node`
//! path has no comment slot — use `skald::cst::Document` for comment-preserving
//! edits.

use serde::ser::{Serialize, Serializer};

/// Reserved newtype-struct name marking a flow-style sequence.
pub const FLOW_SEQ: &str = "$skald::flow_seq";
/// Reserved newtype-struct name marking a flow-style mapping.
pub const FLOW_MAP: &str = "$skald::flow_map";
/// Reserved newtype-struct name marking a literal block scalar.
pub const LIT_STR: &str = "$skald::lit_str";
/// Reserved newtype-struct name marking a folded block scalar.
pub const FOLD_STR: &str = "$skald::fold_str";

/// Serialize the inner sequence in YAML flow style: `[a, b, c]`.
#[derive(Debug, Clone)]
pub struct FlowSeq<T>(pub T);

/// Serialize the inner mapping in YAML flow style: `{k: v}`.
#[derive(Debug, Clone)]
pub struct FlowMap<T>(pub T);

/// Serialize the inner string as a literal block scalar (`|`).
#[derive(Debug, Clone)]
pub struct LitStr<S = String>(pub S);

/// Serialize the inner string as a folded block scalar (`>`).
#[derive(Debug, Clone)]
pub struct FoldStr<S = String>(pub S);

impl<T: Serialize> Serialize for FlowSeq<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(FLOW_SEQ, &self.0)
    }
}
impl<T: Serialize> Serialize for FlowMap<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(FLOW_MAP, &self.0)
    }
}
impl<S0: AsRef<str>> Serialize for LitStr<S0> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(LIT_STR, self.0.as_ref())
    }
}
impl<S0: AsRef<str>> Serialize for FoldStr<S0> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct(FOLD_STR, self.0.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrappers_construct() {
        let _ = FlowSeq(vec![1, 2]);
        let _ = FlowMap(std::collections::BTreeMap::<String, i32>::new());
        let _ = LitStr("a\nb\n");
        let _ = FoldStr(String::from("x"));
        assert_eq!(FLOW_SEQ, "$skald::flow_seq");
    }
}
