#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::document::FileContent;
use support::{generate_long_fixture, generate_short_fixture};

fn bench_hash_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_document");
    for size in [100, 1_000, 10_000, 100_000] {
        let fc = generate_short_fixture(size);
        let bytes = fc.raw.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::new("short_lines", size),
            &fc,
            |b, fc| {
                b.iter(|| black_box(FileContent::lines_with_hashes(fc)))
            },
        );
    }
    group.finish();
}

fn bench_hash_long_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_long_lines");
    for size in [1_000, 10_000] {
        let fc = generate_long_fixture(size);
        let bytes = fc.raw.len() as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(BenchmarkId::new("lines", size), &fc, |b, fc| {
            b.iter(|| black_box(FileContent::lines_with_hashes(fc)))
        });
    }
    group.finish();
}

fn bench_hash_real_world(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_real_world");

    let small_real = include_str!("support.rs");
    let fc_small = support::make_fc(small_real);
    group.throughput(Throughput::Bytes(small_real.len() as u64));
    group.bench_function("support.rs", |b| {
        b.iter(|| black_box(FileContent::lines_with_hashes(&fc_small)))
    });

    let large_real = include_str!("../src/document.rs");
    let fc_large = support::make_fc(large_real);
    group.throughput(Throughput::Bytes(large_real.len() as u64));
    group.bench_function("document.rs", |b| {
        b.iter(|| black_box(FileContent::lines_with_hashes(&fc_large)))
    });

    group.finish();
}

criterion_group!(benches, bench_hash_scaling, bench_hash_long_lines, bench_hash_real_world);
criterion_main!(benches);
