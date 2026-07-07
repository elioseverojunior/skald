// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Integration tests for the source-preserving `Document` API:
//! byte-for-byte identity on realistic config files, and surgical edits that
//! leave every untouched byte alone.

use skald_cst::Document;

/// A realistic Kubernetes-style manifest exercising comments, blank lines,
/// nested block structure, and inline comments at several depths.
const K8S_MANIFEST: &str = "\
# Production deployment
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web        # the web frontend
  labels:
    app: web
spec:
  replicas: 3      # scale here

  template:
    spec:
      containers:
        - name: web
          image: nginx:1.25   # pin the tag
";

#[test]
fn k8s_manifest_round_trips_byte_for_byte() {
    let doc = Document::parse(K8S_MANIFEST);
    assert_eq!(doc.to_string(), K8S_MANIFEST);
}

#[test]
fn k8s_manifest_scalar_edit_is_surgical() {
    let mut doc = Document::parse(K8S_MANIFEST);
    doc.set("spec.replicas", "5").unwrap();
    let expected = K8S_MANIFEST.replace(
        "replicas: 3      # scale here",
        "replicas: 5      # scale here",
    );
    assert_eq!(doc.to_string(), expected);
}

#[test]
fn deep_path_through_sequence_edit() {
    let mut doc = Document::parse(K8S_MANIFEST);
    // Get the image value through nested path (comment is separate)
    assert_eq!(
        doc.get("spec.template.spec.containers.0.image"),
        Some("nginx:1.25")
    );
    doc.set("spec.template.spec.containers.0.image", "nginx:1.29")
        .unwrap();
    assert!(doc.to_string().contains("nginx:1.29"));
    // The inline comment survived because we didn't touch it.
    assert!(doc.to_string().contains("# pin the tag"));
}

#[test]
fn helm_values_style_quoting_survives_neighbor_edit() {
    let input = "\
image:
  repository: 'nginx'
  tag: \"1.25\"
  pullPolicy: IfNotPresent
resources: {}
";
    let mut doc = Document::parse(input);
    doc.set("image.pullPolicy", "Always").unwrap();
    let out = doc.to_string();
    // Neighboring quote styles and the empty flow mapping are untouched.
    assert!(out.contains("repository: 'nginx'"));
    assert!(out.contains("tag: \"1.25\""));
    assert!(out.contains("resources: {}"));
    assert!(out.contains("pullPolicy: Always"));
}

#[test]
fn multi_document_stream_preserves_structure() {
    let input = "---\n# doc one\na: 1\n---\n# doc two\nb: 2\n";
    let doc = Document::parse(input);
    // The CST preserves the full source byte-for-byte
    assert_eq!(doc.to_string(), input);
}

#[test]
fn crlf_file_round_trips_and_edits() {
    let input = "a: 1\r\nb: 2\r\n";
    let mut doc = Document::parse(input);
    assert_eq!(doc.to_string(), input);
    doc.set("a", "9").unwrap();
    assert_eq!(doc.to_string(), "a: 9\r\nb: 2\r\n");
}

#[test]
fn bom_file_round_trips_and_edits() {
    let input = "\u{feff}a: 1\nb: 2\n";
    let mut doc = Document::parse(input);
    assert_eq!(doc.to_string(), input);
    doc.set("b", "7").unwrap();
    assert_eq!(doc.to_string(), "\u{feff}a: 1\nb: 7\n");
}

#[test]
fn anchor_and_alias_bytes_survive_unrelated_edit() {
    let input = "\
defaults: &defaults
  timeout: 30
  retries: 3
service:
  <<: *defaults
  name: web
";
    let mut doc = Document::parse(input);
    doc.set("service.name", "api").unwrap();
    let out = doc.to_string();
    assert!(out.contains("&defaults"));
    assert!(out.contains("<<: *defaults"));
    assert!(out.contains("name: api"));
}

#[test]
fn block_scalar_content_is_addressable_verbatim() {
    let input = "script: |\n  echo one\n  echo two\nafter: 1\n";
    let doc = Document::parse(input);
    let got = doc.get("script").unwrap();
    assert!(got.starts_with('|'));
    assert!(got.contains("echo one"));
}

#[test]
fn nested_mapping_with_comments_preserved() {
    let input = "\
# Global config
server:
  # Server settings
  host: localhost  # bind address
  port: 8080       # listen port
  debug: false
";
    let mut doc = Document::parse(input);
    assert_eq!(doc.to_string(), input);
    doc.set("server.host", "0.0.0.0").unwrap();
    let out = doc.to_string();
    assert!(out.contains("# bind address"));
    assert!(out.contains("# listen port"));
    assert!(out.contains("host: 0.0.0.0"));
}

#[test]
fn flow_collection_styles_preserved() {
    let input = "items: [1, 2, 3]\nmap: {a: 1, b: 2}\n";
    let doc = Document::parse(input);
    assert_eq!(doc.to_string(), input);
}

#[test]
fn mixed_block_and_flow_collections() {
    let input = "\
data:
  items:
    - {id: 1, name: alice}
    - {id: 2, name: bob}
  summary: [total, count]
";
    let doc = Document::parse(input);
    assert_eq!(doc.to_string(), input);
}
