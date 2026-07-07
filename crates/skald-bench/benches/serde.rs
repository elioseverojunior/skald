// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use skald_bench::fixtures::{
    MEDIUM_HELM, MEDIUM_HELM_JSON, SMALL_POD, SMALL_POD_JSON, generate_large, generate_large_json,
};
use std::hint::black_box;

fn serde_deserialize_benchmark(c: &mut Criterion) {
    let large_yaml = generate_large(800);
    let large_json = generate_large_json(800);

    let mut group = c.benchmark_group("serde_deserialize");
    group.measurement_time(Duration::from_secs(15));

    for (name, yaml, json) in [
        ("small", SMALL_POD, SMALL_POD_JSON),
        ("medium", MEDIUM_HELM, MEDIUM_HELM_JSON),
        ("large", large_yaml.as_str(), large_json.as_str()),
    ] {
        // Use YAML byte length for throughput — normalises the comparison.
        group.throughput(Throughput::Bytes(yaml.len() as u64));

        group.bench_with_input(BenchmarkId::new("skald", name), &yaml, |b, input| {
            b.iter(|| skald::from_str::<skald::Value>(black_box(input)).unwrap());
        });

        // Speed-of-light JSON baseline (deserializes the JSON equivalent, not YAML).
        group.bench_with_input(BenchmarkId::new("serde_json", name), &json, |b, input| {
            b.iter(|| serde_json::from_str::<serde_json::Value>(black_box(input)).unwrap());
        });

        // noyalib: from_str::<noyalib::Value>(&str) — confirmed from noyalib 0.0.7 lib.rs pub use
        group.bench_with_input(BenchmarkId::new("noyalib", name), &yaml, |b, input| {
            b.iter(|| noyalib::from_str::<noyalib::Value>(black_box(input)).unwrap());
        });

        // serde_yaml_ng: from_str::<serde_yaml_ng::Value>(&str) — standard serde_yaml API
        group.bench_with_input(
            BenchmarkId::new("serde_yaml_ng", name),
            &yaml,
            |b, input| {
                b.iter(|| {
                    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(black_box(input)).unwrap()
                });
            },
        );

        // serde_saphyr: from_str::<T>(&str) — no native Value type; deserialize into
        // serde_json::Value which implements serde::Deserialize (confirmed from serde-saphyr
        // 0.0.27 de/api.rs: `T: serde::de::Deserialize<'de>`).
        group.bench_with_input(BenchmarkId::new("serde_saphyr", name), &yaml, |b, input| {
            b.iter(|| serde_saphyr::from_str::<serde_json::Value>(black_box(input)).unwrap());
        });
    }

    group.finish();
}

fn serde_serialize_benchmark(c: &mut Criterion) {
    let large_yaml = generate_large(800);
    let large_json = generate_large_json(800);

    // Pre-deserialize values outside the hot loop.
    let skald_small: skald::Value = skald::from_str(SMALL_POD).unwrap();
    let skald_medium: skald::Value = skald::from_str(MEDIUM_HELM).unwrap();
    let skald_large: skald::Value = skald::from_str(&large_yaml).unwrap();

    // Speed-of-light JSON baseline.
    let json_small: serde_json::Value = serde_json::from_str(SMALL_POD_JSON).unwrap();
    let json_medium: serde_json::Value = serde_json::from_str(MEDIUM_HELM_JSON).unwrap();
    let json_large: serde_json::Value = serde_json::from_str(&large_json).unwrap();

    // noyalib: to_string(&T) — confirmed from noyalib 0.0.7 lib.rs pub use
    let noyalib_small: noyalib::Value = noyalib::from_str(SMALL_POD).unwrap();
    let noyalib_medium: noyalib::Value = noyalib::from_str(MEDIUM_HELM).unwrap();
    let noyalib_large: noyalib::Value = noyalib::from_str(&large_yaml).unwrap();

    // serde_yaml_ng: to_string(&T) — standard serde_yaml API
    let ng_small: serde_yaml_ng::Value = serde_yaml_ng::from_str(SMALL_POD).unwrap();
    let ng_medium: serde_yaml_ng::Value = serde_yaml_ng::from_str(MEDIUM_HELM).unwrap();
    let ng_large: serde_yaml_ng::Value = serde_yaml_ng::from_str(&large_yaml).unwrap();

    // serde_saphyr: to_string(&T) — confirmed from serde-saphyr 0.0.27 ser/api.rs.
    // No native Value; use serde_json::Value (which impls Serialize) for a fair
    // apples-to-apples comparison against the serde_saphyr deserialize group above.
    let saphyr_small: serde_json::Value = serde_saphyr::from_str(SMALL_POD).unwrap();
    let saphyr_medium: serde_json::Value = serde_saphyr::from_str(MEDIUM_HELM).unwrap();
    let saphyr_large: serde_json::Value = serde_saphyr::from_str(&large_yaml).unwrap();

    let mut group = c.benchmark_group("serde_serialize");
    group.measurement_time(Duration::from_secs(15));

    for (name, yaml_len, skald_val, json_val, noyalib_val, ng_val, saphyr_val) in [
        (
            "small",
            SMALL_POD.len(),
            &skald_small,
            &json_small,
            &noyalib_small,
            &ng_small,
            &saphyr_small,
        ),
        (
            "medium",
            MEDIUM_HELM.len(),
            &skald_medium,
            &json_medium,
            &noyalib_medium,
            &ng_medium,
            &saphyr_medium,
        ),
        (
            "large",
            large_yaml.len(),
            &skald_large,
            &json_large,
            &noyalib_large,
            &ng_large,
            &saphyr_large,
        ),
    ] {
        group.throughput(Throughput::Bytes(yaml_len as u64));

        group.bench_with_input(BenchmarkId::new("skald", name), skald_val, |b, val| {
            b.iter(|| skald::to_string(black_box(val)).unwrap());
        });

        // Speed-of-light JSON baseline.
        group.bench_with_input(BenchmarkId::new("serde_json", name), json_val, |b, val| {
            b.iter(|| serde_json::to_string(black_box(val)).unwrap());
        });

        group.bench_with_input(BenchmarkId::new("noyalib", name), noyalib_val, |b, val| {
            b.iter(|| noyalib::to_string(black_box(val)).unwrap());
        });

        group.bench_with_input(BenchmarkId::new("serde_yaml_ng", name), ng_val, |b, val| {
            b.iter(|| serde_yaml_ng::to_string(black_box(val)).unwrap());
        });

        // serde_saphyr: to_string supports any T: serde::Serialize; serializing the
        // serde_json::Value obtained from serde_saphyr::from_str above.
        group.bench_with_input(
            BenchmarkId::new("serde_saphyr", name),
            saphyr_val,
            |b, val| {
                b.iter(|| serde_saphyr::to_string(black_box(val)).unwrap());
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    serde_deserialize_benchmark,
    serde_serialize_benchmark
);
criterion_main!(benches);
