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
use support::{generate_long_fixture, generate_short_fixture};

fn bench_hash_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    c.bench_function("hash_1k_lines", |b| {
        b.iter(|| black_box(Document::from_str(Path::new("bench.rs"), &file).unwrap()))
    });
}

fn bench_hash_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    c.bench_function("hash_10k_lines", |b| {
        b.iter(|| black_box(Document::from_str(Path::new("bench.rs"), &file).unwrap()))
    });
}

fn bench_hash_10k_long_lines(c: &mut Criterion) {
    let file = generate_long_fixture(10_000);
    c.bench_function("hash_10k_long_lines", |b| {
        b.iter(|| black_box(Document::from_str(Path::new("bench.rs"), &file).unwrap()))
    });
}

criterion_group!(
    benches,
    bench_hash_1k_lines,
    bench_hash_10k_lines,
    bench_hash_10k_long_lines
);
criterion_main!(benches);
