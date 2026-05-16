#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

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

fn bench_stats_1k_lines(c: &mut Criterion) {
    let doc = build_document(&generate_short_fixture(1_000));
    c.bench_function("stats_1k_lines", |b| {
        b.iter(|| black_box(doc.compute_stats()))
    });
}

fn bench_stats_10k_lines(c: &mut Criterion) {
    let doc = build_document(&generate_short_fixture(10_000));
    c.bench_function("stats_10k_lines", |b| {
        b.iter(|| black_box(doc.compute_stats()))
    });
}

fn bench_stats_collision_heavy_10k(c: &mut Criterion) {
    let content = generate_collision_fixture(10_000, hash::short_hash);
    let doc = build_document(&content);
    c.bench_function("stats_collision_heavy_10k", |b| {
        b.iter(|| black_box(doc.compute_stats()))
    });
}

criterion_group!(
    benches,
    bench_stats_1k_lines,
    bench_stats_10k_lines,
    bench_stats_collision_heavy_10k
);
criterion_main!(benches);
