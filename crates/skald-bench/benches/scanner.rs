// SPDX-FileCopyrightText: 2026 Skald contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use skald_bench::fixtures::{MEDIUM_HELM, SMALL_POD, generate_large};
use std::hint::black_box;

fn scanner_benchmark(c: &mut Criterion) {
    let large = generate_large(800);

    let mut group = c.benchmark_group("scanner");
    group.measurement_time(Duration::from_secs(15));

    for (name, input) in [
        ("small", SMALL_POD),
        ("medium", MEDIUM_HELM),
        ("large", large.as_str()),
    ] {
        group.throughput(Throughput::Bytes(input.len() as u64));

        group.bench_with_input(BenchmarkId::new("skald", name), &input, |b, input| {
            b.iter(|| {
                let mut scanner = skald_core::scanner::Scanner::new(black_box(input));
                while let Some(tok) = scanner.next_token() {
                    let _ = black_box(tok);
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, scanner_benchmark);
criterion_main!(benches);
