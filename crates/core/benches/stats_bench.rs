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
use support::{generate_collision_fixture, generate_short_fixture};

fn build_document(content: &str) -> Document {
    Document::from_str(Path::new("bench.rs"), content).expect("build benchmark document")
}

fn bench_stats_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("stats");
    for size in [1_000, 10_000, 100_000] {
        let content = generate_short_fixture(size);
        let doc = build_document(&content);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &doc, |b, doc| {
            b.iter(|| black_box(doc.compute_stats()))
        });
    }
    group.finish();
}

fn bench_stats_collision_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("stats_collision");
    for size in [1_000, 10_000] {
        let content = generate_collision_fixture(size, hash::short_hash);
        let doc = build_document(&content);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &doc, |b, doc| {
            b.iter(|| black_box(doc.compute_stats()))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_stats_scaling, bench_stats_collision_heavy);
criterion_main!(benches);
