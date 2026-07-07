// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Permanent differential guard: the public streaming serializer
//! ([`skald_serde::to_string`]) must always produce byte-identical output to
//! the Node-path oracle ([`skald_serde::ser::to_node`] +
//! [`skald_ast::emitter::emit_to_string`]).
//!
//! If any case here fails, the two serialize paths have drifted. The Node path
//! is the oracle — fix `stream_ser.rs` until they reconverge.
//!
//! Note: the plan describes `skald::Value` / `skald::from_str`, but `skald-serde`
//! cannot depend on the `skald` facade (that would be circular — `skald`
//! depends on `skald-serde`). The crate-local equivalents `skald_serde::Value`
//! and `skald_serde::from_str::<Value>` are used instead; they are the exact
//! types the facade re-exports.

use std::collections::BTreeMap;

use serde::Serialize;

use skald_serde::Value;

/// For any serde-serializable value, the streaming path (public `to_string`)
/// must equal the Node-path (`to_node` + `emit_to_string`).
fn assert_same<T: Serialize>(v: &T) {
    let streamed = skald_serde::to_string(v).expect("stream");
    let via_node = {
        let node = skald_serde::ser::to_node(v).expect("to_node");
        skald_ast::emitter::emit_to_string(&node, &skald_ast::emitter::EmitterConfig::default())
    };
    assert_eq!(streamed, via_node, "streaming vs Node path diverged");
}

// ─── Derived shapes ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Nested {
    name: String,
    count: i64,
}

#[derive(Serialize)]
struct WithVecAndNested {
    items: Vec<i64>,
    inner: Nested,
}

#[derive(Serialize)]
enum Variant {
    Unit,
    Newtype(i64),
    Tuple(i64, String),
    Struct { x: i64, y: String },
}

// ─── 1. Primitives / containers ──────────────────────────────────────────────

#[test]
fn integers() {
    assert_same(&0_i64);
    assert_same(&i64::MIN);
    assert_same(&i64::MAX);
    assert_same(&(-42_i64));
    assert_same(&0_u64);
    assert_same(&u64::MAX);
}

#[test]
fn floats_including_special() {
    assert_same(&3.5_f64);
    assert_same(&(-2.25_f64));
    assert_same(&f64::NAN);
    assert_same(&f64::INFINITY);
    assert_same(&f64::NEG_INFINITY);
    assert_same(&0.0_f64);
    assert_same(&(-0.0_f64));
}

#[test]
fn booleans() {
    assert_same(&true);
    assert_same(&false);
}

#[test]
fn strings_plain_and_needing_quotes() {
    assert_same(&"hello".to_string());
    assert_same(&"123".to_string());
    assert_same(&"true".to_string());
    assert_same(&"null".to_string());
    assert_same(&String::new());
    assert_same(&"with: colon".to_string());
    assert_same(&"- leading dash".to_string());
    assert_same(&"# hash".to_string());
}

#[test]
fn options() {
    assert_same(&Some(7_i64));
    assert_same(&Option::<i64>::None);
}

#[test]
fn unit() {
    assert_same(&());
}

#[test]
fn vectors() {
    assert_same(&Vec::<i64>::new());
    assert_same(&vec![1_i64, 2, 3]);
    assert_same(&vec![vec![1_i64, 2], vec![3, 4], Vec::<i64>::new()]);
}

#[test]
fn derived_struct_with_vec_and_nested() {
    let v = WithVecAndNested {
        items: vec![10, 20, 30],
        inner: Nested {
            name: "core".to_string(),
            count: 5,
        },
    };
    assert_same(&v);
}

#[test]
fn btreemap() {
    let mut m: BTreeMap<String, i64> = BTreeMap::new();
    m.insert("alpha".to_string(), 1);
    m.insert("beta".to_string(), 2);
    m.insert("gamma".to_string(), 3);
    assert_same(&m);
    assert_same(&BTreeMap::<String, i64>::new());
}

#[test]
fn enum_variants() {
    assert_same(&Variant::Unit);
    assert_same(&Variant::Newtype(99));
    assert_same(&Variant::Tuple(1, "two".to_string()));
    assert_same(&Variant::Struct {
        x: 4,
        y: "five".to_string(),
    });
}

// ─── 2. Real YAML docs round-tripped through Value ───────────────────────────

const K8S_POD: &str = r#"apiVersion: v1
kind: Pod
metadata:
  name: nginx
  labels:
    app: web
spec:
  containers:
    - name: nginx
      image: nginx:1.25
      ports:
        - containerPort: 80
"#;

const DOCKER_COMPOSE: &str = r#"services:
  web:
    image: nginx:latest
    ports:
      - "8080:80"
    environment:
      DEBUG: "false"
    depends_on:
      - db
  db:
    image: postgres:16
    volumes:
      - data:/var/lib/postgresql/data
volumes:
  data: {}
"#;

const GITHUB_ACTIONS: &str = r#"name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  build:
    runs-on: ${{ vars.RUNS_ON }}
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: cargo build --workspace
"#;

const QUOTED_NUMERIC_BOOL_SCALARS: &str = r#"plain_int: 42
plain_float: 3.14
plain_bool: true
plain_null: null
quoted_int: "42"
quoted_bool: "true"
quoted_null: "null"
empty: ""
string_with_special: "a: b # c"
"#;

const FLOW_SEQUENCE: &str = r#"matrix:
  os: [ubuntu, macos, windows]
  rust: [stable, beta]
nested_flow: [[1, 2], [3, 4]]
flow_map: {a: 1, b: 2, c: 3}
"#;

const DEEPLY_NESTED: &str = r#"root:
  level1:
    level2:
      level3:
        level4:
          values:
            - one
            - two
          flag: true
          nested:
            deep: value
"#;

const MIXED_BLOCK_SEQ: &str = r#"- name: first
  tags:
    - a
    - b
- name: second
  tags: []
  meta:
    nested: yes
"#;

fn assert_doc_same(yaml: &str) {
    let v: Value = skald_serde::from_str(yaml).expect("parse YAML into Value");
    assert_same(&v);
}

#[test]
fn real_yaml_k8s_pod() {
    assert_doc_same(K8S_POD);
}

#[test]
fn real_yaml_docker_compose() {
    assert_doc_same(DOCKER_COMPOSE);
}

#[test]
fn real_yaml_github_actions() {
    assert_doc_same(GITHUB_ACTIONS);
}

#[test]
fn real_yaml_quoted_numeric_bool_scalars() {
    assert_doc_same(QUOTED_NUMERIC_BOOL_SCALARS);
}

#[test]
fn real_yaml_flow_sequence() {
    assert_doc_same(FLOW_SEQUENCE);
}

#[test]
fn real_yaml_deeply_nested() {
    assert_doc_same(DEEPLY_NESTED);
}

#[test]
fn real_yaml_mixed_block_seq() {
    assert_doc_same(MIXED_BLOCK_SEQ);
}
