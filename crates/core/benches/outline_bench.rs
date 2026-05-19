#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../hash_cache.rs"]
mod hash_cache;
#[path = "../index.rs"]
mod index;
#[path = "../lang/mod.rs"]
mod lang;
mod support;

use index::adaptive::{PatternType, SearchResult, classify_pattern, search_adaptive};
use lang::detect::Lang;
use lang::outline::get_outline_entries;
use support::{generate_long_fixture, generate_short_fixture};

// --- Scaling across Rust fixture sizes ---
fn bench_outline_rust_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("outline_rust");
    for size in [500, 2_000, 10_000] {
        let content = generate_short_fixture(size);
        group.throughput(Throughput::Bytes(content.len() as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &content, |b, content| {
            b.iter(|| black_box(get_outline_entries(content, Lang::Rust)))
        });
    }
    group.finish();
}

// --- Long lines ---
fn bench_outline_rust_long_lines(c: &mut Criterion) {
    let content = generate_long_fixture(2_000);
    let mut group = c.benchmark_group("outline_long_lines");
    group.throughput(Throughput::Bytes(content.len() as u64));
    group.bench_function("rust_2k", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Rust)))
    });
    group.finish();
}

// --- Cross-language comparison at 1k lines ---
fn bench_outline_cross_language(c: &mut Criterion) {
    let content = generate_short_fixture(1_000);
    let mut group = c.benchmark_group("outline_language");
    group.throughput(Throughput::Bytes(content.len() as u64));

    for lang in [Lang::Rust, Lang::Python, Lang::Go, Lang::PlainText] {
        group.bench_with_input(
            BenchmarkId::new("lang", format!("{lang:?}")),
            &content,
            |b, content| b.iter(|| black_box(get_outline_entries(content, lang))),
        );
    }
    group.finish();
}

// --- Real-world content ---
fn bench_outline_real_world(c: &mut Criterion) {
    let mut group = c.benchmark_group("outline_real_world");

    let real_rust = include_str!("../document.rs");
    group.throughput(Throughput::Bytes(real_rust.len() as u64));
    group.bench_function("document.rs", |b| {
        b.iter(|| black_box(get_outline_entries(real_rust, Lang::Rust)))
    });

    let real_rust_large = include_str!("../orchestration.rs");
    group.throughput(Throughput::Bytes(real_rust_large.len() as u64));
    group.bench_function("orchestration.rs", |b| {
        b.iter(|| black_box(get_outline_entries(real_rust_large, Lang::Rust)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_outline_rust_scaling,
    bench_outline_rust_long_lines,
    bench_outline_cross_language,
    bench_outline_real_world
);
criterion_main!(benches);
