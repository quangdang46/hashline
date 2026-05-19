#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../hash_cache.rs"]
mod hash_cache;
mod support;

use document::Document;
use support::{generate_long_fixture, generate_short_fixture};

// --- Scaling: short lines ---
fn bench_hash_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_document");
    for size in [100, 1_000, 10_000, 100_000] {
        let file = generate_short_fixture(size);
        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("short_lines", size),
            &file,
            |b, content| {
                b.iter(|| black_box(Document::from_str(Path::new("bench.rs"), content).unwrap()))
            },
        );
    }
    group.finish();
}

// --- Long lines at different sizes ---
fn bench_hash_long_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_long_lines");
    for size in [1_000, 10_000] {
        let file = generate_long_fixture(size);
        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &file, |b, content| {
            b.iter(|| black_box(Document::from_str(Path::new("bench.rs"), content).unwrap()))
        });
    }
    group.finish();
}

// --- Real-world content: benchmark on repo source files ---
fn bench_hash_real_world(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash_real_world");

    // Use the support module itself as a small real file
    let small_real = include_str!("support.rs");
    group.throughput(Throughput::Bytes(small_real.len() as u64));
    group.bench_function("support.rs", |b| {
        b.iter(|| black_box(Document::from_str(Path::new("support.rs"), small_real).unwrap()))
    });

    // Use the document module as a larger real file
    let large_real = include_str!("../document.rs");
    group.throughput(Throughput::Bytes(large_real.len() as u64));
    group.bench_function("document.rs", |b| {
        b.iter(|| black_box(Document::from_str(Path::new("document.rs"), large_real).unwrap()))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_hash_scaling,
    bench_hash_long_lines,
    bench_hash_real_world
);
criterion_main!(benches);
