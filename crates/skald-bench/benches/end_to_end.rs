// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rust_yaml::Yaml;
use serde::{Deserialize, Serialize};
use skald_bench::fixtures::{MEDIUM_HELM, SMALL_POD, generate_large};
use std::hint::black_box;

// ─── Typed struct for realistic deserialization ────────────────────────

#[derive(Deserialize, Serialize)]
struct PodSpec {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: Metadata,
    spec: Spec,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    name: String,
    labels: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, Serialize)]
struct Spec {
    containers: Vec<Container>,
}

#[derive(Deserialize, Serialize)]
struct Container {
    name: String,
    image: String,
    ports: Vec<Port>,
}

#[derive(Deserialize, Serialize)]
struct Port {
    #[serde(rename = "containerPort")]
    container_port: u16,
    protocol: String,
}

// ─── Benchmarks ───────────────────────────────────────────────────────

/// Parse → emit → parse roundtrip through the node API.
fn roundtrip_node_benchmark(c: &mut Criterion) {
    let large = generate_large(800);

    let mut group = c.benchmark_group("roundtrip_node");
    group.measurement_time(Duration::from_secs(25));

    for (name, input) in [
        ("small", SMALL_POD),
        ("medium", MEDIUM_HELM),
        ("large", large.as_str()),
    ] {
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(BenchmarkId::from_parameter(name), &input, |b, input| {
            b.iter(|| {
                let node = skald::from_str_node(black_box(input)).unwrap();
                let yaml = skald::to_string_node(&node);
                let _rt = skald::from_str_node(black_box(&yaml)).unwrap();
            });
        });
    }

    group.finish();
}

/// Cross-library load → emit roundtrip: skald vs yaml-rust2 vs rust-yaml.
/// Each library uses its own idiomatic parse-and-serialize path.
fn roundtrip_compare_benchmark(c: &mut Criterion) {
    let large = generate_large(800);
    let ry = Yaml::new();

    let mut group = c.benchmark_group("roundtrip_compare");
    group.measurement_time(Duration::from_secs(40));

    for (name, input) in [
        ("small", SMALL_POD),
        ("medium", MEDIUM_HELM),
        ("large", large.as_str()),
    ] {
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(BenchmarkId::new("skald", name), &input, |b, input| {
            b.iter(|| {
                let node = skald::from_str_node(black_box(input)).unwrap();
                skald::to_string_node(&node)
            });
        });

        group.bench_with_input(BenchmarkId::new("yaml_rust2", name), &input, |b, input| {
            b.iter(|| {
                let docs = yaml_rust2::YamlLoader::load_from_str(black_box(input)).unwrap();
                let mut out = String::new();
                yaml_rust2::YamlEmitter::new(&mut out)
                    .dump(black_box(&docs[0]))
                    .unwrap();
                out
            });
        });

        group.bench_with_input(BenchmarkId::new("rust_yaml", name), &input, |b, input| {
            b.iter(|| {
                let value = ry.load_str(black_box(input)).unwrap();
                ry.dump_str(black_box(&value)).unwrap()
            });
        });
    }

    group.finish();
}

/// Facade `from_str_node` vs raw `compose_all` — measures facade overhead.
fn full_pipeline_node_benchmark(c: &mut Criterion) {
    let large = generate_large(800);

    let mut group = c.benchmark_group("full_pipeline_node");
    group.measurement_time(Duration::from_secs(15));

    for (name, input) in [
        ("small", SMALL_POD),
        ("medium", MEDIUM_HELM),
        ("large", large.as_str()),
    ] {
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(BenchmarkId::new("facade", name), &input, |b, input| {
            b.iter(|| skald::from_str_node(black_box(input)).unwrap());
        });

        group.bench_with_input(BenchmarkId::new("compose_all", name), &input, |b, input| {
            b.iter(|| skald_ast::composer::compose_all(black_box(input)).unwrap());
        });
    }

    group.finish();
}

/// Typed struct deserialization via the serde API.
fn typed_struct_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed_struct");
    group.measurement_time(Duration::from_secs(15));
    group.throughput(Throughput::Bytes(SMALL_POD.len() as u64));

    group.bench_function("deserialize", |b| {
        b.iter(|| skald::from_str::<PodSpec>(black_box(SMALL_POD)).unwrap());
    });

    group.bench_function("roundtrip", |b| {
        b.iter(|| {
            let pod: PodSpec = skald::from_str(black_box(SMALL_POD)).unwrap();
            let _yaml = skald::to_string(black_box(&pod)).unwrap();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    roundtrip_node_benchmark,
    roundtrip_compare_benchmark,
    full_pipeline_node_benchmark,
    typed_struct_benchmark,
);
criterion_main!(benches);
