// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests — real-world YAML formats through the serde pipeline.

use serde::{Deserialize, Serialize};

// ─── Kubernetes Pod Spec ─────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct K8sPod {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: K8sMetadata,
    spec: K8sPodSpec,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct K8sMetadata {
    name: String,
    #[serde(default)]
    labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct K8sPodSpec {
    containers: Vec<K8sContainer>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct K8sContainer {
    name: String,
    image: String,
    #[serde(default)]
    ports: Vec<K8sPort>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct K8sPort {
    container_port: u16,
}

#[test]
fn kubernetes_pod_spec() {
    let yaml = r#"
apiVersion: v1
kind: Pod
metadata:
  name: nginx-pod
  labels:
    app: nginx
    tier: frontend
spec:
  containers:
    - name: nginx
      image: nginx:1.25
      ports:
        - containerPort: 80
    - name: sidecar
      image: busybox:latest
"#;
    let pod: K8sPod = skald::from_str(yaml).unwrap();
    assert_eq!(pod.api_version, "v1");
    assert_eq!(pod.kind, "Pod");
    assert_eq!(pod.metadata.name, "nginx-pod");
    assert_eq!(pod.metadata.labels["app"], "nginx");
    assert_eq!(pod.spec.containers.len(), 2);
    assert_eq!(pod.spec.containers[0].name, "nginx");
    assert_eq!(pod.spec.containers[0].ports[0].container_port, 80);
    assert_eq!(pod.spec.containers[1].name, "sidecar");
    assert!(pod.spec.containers[1].ports.is_empty());

    // Round-trip
    let yaml_out = skald::to_string(&pod).unwrap();
    let roundtripped: K8sPod = skald::from_str(&yaml_out).unwrap();
    assert_eq!(pod, roundtripped);
}

// ─── Docker Compose ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct DockerCompose {
    services: std::collections::BTreeMap<String, DockerService>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct DockerService {
    image: Option<String>,
    #[serde(default)]
    ports: Vec<String>,
    #[serde(default)]
    environment: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[test]
fn docker_compose() {
    let yaml = r#"
services:
  web:
    image: nginx:latest
    ports:
      - "8080:80"
    depends_on:
      - api
    environment:
      BACKEND_URL: http://api:3000
  api:
    image: node:20
    ports:
      - "3000:3000"
    environment:
      DB_HOST: db
      DB_PORT: "5432"
  db:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: secret
"#;
    let compose: DockerCompose = skald::from_str(yaml).unwrap();
    assert_eq!(compose.services.len(), 3);
    assert!(compose.services.contains_key("web"));
    assert!(compose.services.contains_key("api"));
    assert!(compose.services.contains_key("db"));

    let web = &compose.services["web"];
    assert_eq!(web.image.as_deref(), Some("nginx:latest"));
    assert_eq!(web.ports, vec!["8080:80"]);
    assert_eq!(web.depends_on, vec!["api"]);
    assert_eq!(web.environment["BACKEND_URL"], "http://api:3000");

    let db = &compose.services["db"];
    assert_eq!(db.environment["POSTGRES_PASSWORD"], "secret");

    // Round-trip
    let yaml_out = skald::to_string(&compose).unwrap();
    let roundtripped: DockerCompose = skald::from_str(&yaml_out).unwrap();
    assert_eq!(compose, roundtripped);
}

// ─── GitHub Actions Workflow ─────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct GHWorkflow {
    name: String,
    on: GHOn,
    jobs: std::collections::BTreeMap<String, GHJob>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct GHOn {
    #[serde(default)]
    push: Option<GHPushTrigger>,
    #[serde(default)]
    pull_request: Option<GHPRTrigger>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct GHPushTrigger {
    branches: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct GHPRTrigger {
    branches: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct GHJob {
    runs_on: String,
    steps: Vec<GHStep>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct GHStep {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uses: Option<String>,
    #[serde(default)]
    run: Option<String>,
}

#[test]
fn github_actions_workflow() {
    let yaml = r#"
name: CI
on:
  push:
    branches:
      - main
  pull_request:
    branches:
      - main
jobs:
  build:
    runs-on: ${{ vars.RUNS_ON }}
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Build
        run: cargo build --workspace
      - name: Test
        run: cargo test --workspace
"#;
    let wf: GHWorkflow = skald::from_str(yaml).unwrap();
    assert_eq!(wf.name, "CI");
    assert_eq!(
        wf.on.push.as_ref().unwrap().branches,
        vec!["main".to_string()]
    );
    assert_eq!(wf.jobs.len(), 1);

    let build = &wf.jobs["build"];
    assert_eq!(build.runs_on, "${{ vars.RUNS_ON }}");
    assert_eq!(build.steps.len(), 3);
    assert_eq!(build.steps[0].name.as_deref(), Some("Checkout"));
    assert_eq!(build.steps[0].uses.as_deref(), Some("actions/checkout@v4"));
    assert_eq!(
        build.steps[2].run.as_deref(),
        Some("cargo test --workspace")
    );
}

// ─── Serde Attributes ────────────────────────────────────────────────

#[test]
fn serde_rename_all() {
    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct ApiConfig {
        api_key: String,
        max_retries: u32,
        base_url: String,
    }
    let yaml = "apiKey: sk-123\nmaxRetries: 3\nbaseUrl: https://api.example.com";
    let config: ApiConfig = skald::from_str(yaml).unwrap();
    assert_eq!(config.api_key, "sk-123");
    assert_eq!(config.max_retries, 3);

    let yaml_out = skald::to_string(&config).unwrap();
    let roundtripped: ApiConfig = skald::from_str(&yaml_out).unwrap();
    assert_eq!(config, roundtripped);
}

#[test]
fn serde_default() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Config {
        name: String,
        #[serde(default = "default_port")]
        port: u16,
        #[serde(default)]
        debug: bool,
    }

    fn default_port() -> u16 {
        8080
    }

    let config: Config = skald::from_str("name: app").unwrap();
    assert_eq!(config.name, "app");
    assert_eq!(config.port, 8080);
    assert!(!config.debug);
}

#[test]
fn serde_skip() {
    #[derive(Debug, Serialize)]
    #[allow(dead_code)]
    struct Secret {
        name: String,
        #[serde(skip)]
        password: String,
    }
    let yaml = skald::to_string(&Secret {
        name: "admin".into(),
        password: "hunter2".into(),
    })
    .unwrap();
    assert!(yaml.contains("name: admin"));
    assert!(!yaml.contains("hunter2"));
    assert!(!yaml.contains("password"));
}

#[test]
fn serde_enum_externally_tagged() {
    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    enum Action {
        Click { x: i32, y: i32 },
        Type(String),
        Wait,
    }

    // Unit variant
    let yaml = skald::to_string(&Action::Wait).unwrap();
    let a: Action = skald::from_str(&yaml).unwrap();
    assert_eq!(a, Action::Wait);

    // Newtype variant
    let yaml = skald::to_string(&Action::Type("hello".into())).unwrap();
    let a: Action = skald::from_str(&yaml).unwrap();
    assert_eq!(a, Action::Type("hello".into()));

    // Struct variant
    let yaml = skald::to_string(&Action::Click { x: 10, y: 20 }).unwrap();
    let a: Action = skald::from_str(&yaml).unwrap();
    assert_eq!(a, Action::Click { x: 10, y: 20 });
}

#[test]
fn serde_flatten() {
    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Request {
        method: String,
        #[serde(flatten)]
        extra: std::collections::BTreeMap<String, String>,
    }
    let yaml = "method: GET\nurl: /api/v1\ncontent-type: application/json";
    let req: Request = skald::from_str(yaml).unwrap();
    assert_eq!(req.method, "GET");
    assert_eq!(req.extra["url"], "/api/v1");
    assert_eq!(req.extra["content-type"], "application/json");
}

// ─── Value type integration ──────────────────────────────────────────

#[test]
fn value_as_untyped_yaml() {
    let value: skald::Value =
        skald::from_str("name: skald\nversion: 1\ntags:\n  - yaml\n  - rust").unwrap();
    assert!(value.is_mapping());
    let entries = value.as_mapping().unwrap();
    assert_eq!(entries.len(), 3);
}

// ─── Edge cases ──────────────────────────────────────────────────────

#[test]
fn empty_collections() {
    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    struct Data {
        #[serde(default)]
        items: Vec<String>,
        #[serde(default)]
        tags: std::collections::BTreeMap<String, String>,
    }
    let data: Data = skald::from_str("items: []\ntags: {}").unwrap();
    assert!(data.items.is_empty());
    assert!(data.tags.is_empty());

    let yaml = skald::to_string(&data).unwrap();
    let roundtripped: Data = skald::from_str(&yaml).unwrap();
    assert_eq!(data, roundtripped);
}

#[test]
fn nested_collections() {
    let yaml = "matrix:\n  - - 1\n    - 2\n  - - 3\n    - 4";
    #[derive(Debug, Deserialize, PartialEq)]
    struct Grid {
        matrix: Vec<Vec<i32>>,
    }
    let grid: Grid = skald::from_str(yaml).unwrap();
    assert_eq!(grid.matrix, vec![vec![1, 2], vec![3, 4]]);
}

#[test]
fn special_float_values() {
    #[derive(Debug, Deserialize, PartialEq)]
    struct Floats {
        pos_inf: f64,
        neg_inf: f64,
        nan: f64,
        zero: f64,
    }
    let yaml = "pos_inf: .inf\nneg_inf: -.inf\nnan: .nan\nzero: 0.0";
    let f: Floats = skald::from_str(yaml).unwrap();
    assert!(f.pos_inf.is_infinite() && f.pos_inf > 0.0);
    assert!(f.neg_inf.is_infinite() && f.neg_inf < 0.0);
    assert!(f.nan.is_nan());
    assert_eq!(f.zero, 0.0);
}

#[test]
fn quoted_booleans_are_strings() {
    #[derive(Debug, Deserialize)]
    struct Config {
        flag: String,
    }
    // Quoted "true" should be a string, not a bool
    let c: Config = skald::from_str("flag: 'true'").unwrap();
    assert_eq!(c.flag, "true");
}

#[test]
fn multiline_string_value() {
    #[derive(Debug, Deserialize)]
    struct Doc {
        content: String,
    }
    let yaml = "content: |\n  line1\n  line2\n  line3\n";
    let doc: Doc = skald::from_str(yaml).unwrap();
    assert_eq!(doc.content, "line1\nline2\nline3\n");
}

// ─── BorrowedValue facade smoke test ────────────────────────────────

#[test]
fn borrowed_value_via_facade() {
    let input = String::from("name: skald\n");
    let bv = skald::BorrowedValue::parse(&input).unwrap();
    assert!(bv.as_node().as_mapping().is_some());
}

// ─── Merge-keys end-to-end test ──────────────────────────────────────

#[test]
fn merge_keys_resolve_when_enabled() {
    use skald::error::ParserConfig;
    let cfg = ParserConfig {
        merge_keys: true,
        ..Default::default()
    };
    // Drive the composer with config (node API) — no *_node_with facade fn exists.
    let node = skald_ast::composer::Composer::with_config(
        "base: &b {a: 1}\nderived:\n  <<: *b\n  b: 2\n",
        cfg,
    )
    .next()
    .unwrap()
    .unwrap();
    let m = node.as_mapping().unwrap();
    let derived = m
        .iter()
        .find(|(k, _)| k.as_str() == Some("derived"))
        .unwrap()
        .1
        .as_mapping()
        .unwrap();
    let get = |k: &str| {
        derived
            .iter()
            .find(|(kk, _)| kk.as_str() == Some(k))
            .map(|(_, v)| v.as_str().unwrap().to_string())
    };
    assert_eq!(get("a"), Some("1".into())); // merged from anchor
    assert_eq!(get("b"), Some("2".into())); // own key
}

// ─── Lenient / Strict mode ───────────────────────────────────────────

#[test]
fn lenient_mode_allows_duplicate_keys() {
    use std::collections::HashMap;
    let config = skald::error::ParserConfig {
        strictness: skald::error::Strictness::Lenient,
        ..Default::default()
    };
    let result: HashMap<String, i32> = skald::from_str_with("a: 1\nb: 2\na: 3", config).unwrap();
    // Lenient: last value wins for duplicate key "a"
    assert_eq!(result.get("a"), Some(&3));
    assert_eq!(result.get("b"), Some(&2));
}

#[test]
fn strict_mode_rejects_duplicate_keys() {
    let config = skald::error::ParserConfig {
        strictness: skald::error::Strictness::Strict,
        ..Default::default()
    };
    let result: Result<std::collections::HashMap<String, i32>, _> =
        skald::from_str_with("a: 1\na: 2", config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("duplicate"),
        "expected duplicate key error, got: {err_msg}"
    );
}

// ─── ParserPolicies end-to-end tests ────────────────────────────────

#[test]
fn deny_anchors_policy_end_to_end() {
    use skald::error::{ParserConfig, ParserPolicies};
    let cfg = ParserConfig {
        policies: ParserPolicies {
            deny_anchors: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let res = skald_ast::composer::Composer::with_config("x: &a 1\ny: *a\n", cfg)
        .next()
        .unwrap();
    assert!(res.is_err());
}

#[test]
fn max_scalar_length_policy_end_to_end() {
    use skald::error::{ParserConfig, ParserPolicies};
    let cfg = ParserConfig {
        policies: ParserPolicies {
            max_scalar_length: Some(3),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        skald_ast::composer::Composer::with_config("k: toolong\n", cfg)
            .next()
            .unwrap()
            .is_err()
    );
}

#[test]
fn deny_tags_policy_end_to_end() {
    use skald::error::{ParserConfig, ParserPolicies};
    let cfg = ParserConfig {
        policies: ParserPolicies {
            deny_tags: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(
        skald_ast::composer::Composer::with_config("k: !!str v\n", cfg)
            .next()
            .unwrap()
            .is_err()
    );
}

// ─── YAML 1.1 compat end-to-end ─────────────────────────────────────

#[test]
fn yaml_1_1_compat_end_to_end() {
    use skald::error::ParserConfig;
    let cfg = ParserConfig {
        yaml_1_1: true,
        ..Default::default()
    };
    // "on" / "off" are booleans in YAML 1.1
    assert!(skald::from_str_with::<bool>("on", cfg.clone()).unwrap());
    assert!(!skald::from_str_with::<bool>("off", cfg).unwrap());
    // Under default YAML 1.2, "on" is just a plain string
    assert_eq!(skald::from_str::<String>("on").unwrap(), "on");
}

// ─── Output-style wrappers via facade ───────────────────────────────

#[test]
fn output_styling_via_facade() {
    let yaml = skald::to_string(&skald::FlowSeq(vec![1, 2, 3])).unwrap();
    assert!(
        yaml.contains('[') && yaml.trim_end().ends_with(']'),
        "got: {yaml:?}"
    );
    let lit = skald::to_string(&skald::LitStr("a\nb\n")).unwrap();
    assert!(lit.contains('|'), "got: {lit:?}");
    let fold = skald::to_string(&skald::FoldStr("x y\n")).unwrap();
    assert!(fold.contains('>'), "got: {fold:?}");
}

#[cfg(feature = "schema")]
#[test]
fn coerce_to_schema_via_facade_preserves_comments() {
    let sc_node = skald::from_str_node("type: object\nproperties:\n  port: {type: integer}\n")
        .unwrap()
        .into_owned();
    let sc = skald::schema::Schema::from_node(&sc_node);
    let mut doc = skald::cst::Document::parse("port: \"8080\"  # listen port\n");
    let changes = skald::schema::coerce_to_schema(&mut doc, &sc);
    assert_eq!(doc.to_string(), "port: 8080  # listen port\n");
    assert_eq!(changes.len(), 1);
}

#[cfg(feature = "schema")]
#[test]
fn apply_defaults_via_facade() {
    let sc_node =
        skald::from_str_node("type: object\nproperties:\n  port: {type: integer, default: 8080}\n")
            .unwrap()
            .into_owned();
    let sc = skald::schema::Schema::from_node(&sc_node);
    let mut doc = skald::cst::Document::parse("name: app  # keep\n");
    let inserted = skald::schema::apply_defaults(&mut doc, &sc);
    assert_eq!(doc.to_string(), "name: app  # keep\nport: 8080\n");
    assert_eq!(inserted.len(), 1);
}

#[cfg(feature = "schema")]
#[test]
fn coerce_array_items_via_facade() {
    let sc_node = skald::from_str_node(
        "type: object\nproperties:\n  ports: {type: array, items: {type: integer}}\n",
    )
    .unwrap()
    .into_owned();
    let sc = skald::schema::Schema::from_node(&sc_node);
    let mut doc = skald::cst::Document::parse("ports:\n  - \"80\"  # http\n  - \"443\"\n");
    let changes = skald::schema::coerce_to_schema(&mut doc, &sc);
    assert_eq!(doc.to_string(), "ports:\n  - 80  # http\n  - 443\n");
    assert_eq!(changes.len(), 2);
}

#[cfg(feature = "schema")]
#[test]
fn schema_validation_via_facade() {
    let schema_doc = skald::from_str_node(
        "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n  age: {type: integer}\n",
    )
    .unwrap()
    .into_owned();
    let sc = skald::schema::Schema::from_node(&schema_doc);
    let ok = skald::from_str_node("name: bob\nage: 5\n")
        .unwrap()
        .into_owned();
    assert!(skald::schema::validate(&ok, &sc).is_ok());
    let bad = skald::from_str_node("age: notnum\n").unwrap().into_owned();
    let errs = skald::schema::validate(&bad, &sc).unwrap_err();
    assert!(!errs.is_empty());
    // each error carries a span
    assert!(
        errs.iter()
            .all(|e| e.span.end.offset >= e.span.start.offset)
    );
}

#[cfg(feature = "schema")]
#[test]
fn nested_apply_defaults_via_facade() {
    let sc_node = skald::from_str_node("type: object\nproperties:\n  server: {type: object, properties: {port: {type: integer, default: 8080}}}\n").unwrap().into_owned();
    let sc = skald::schema::Schema::from_node(&sc_node);
    let mut doc = skald::cst::Document::parse("server:\n  host: h  # keep\n");
    let _ = skald::schema::apply_defaults(&mut doc, &sc);
    assert_eq!(
        doc.to_string(),
        "server:\n  host: h  # keep\n  port: 8080\n"
    );
}
