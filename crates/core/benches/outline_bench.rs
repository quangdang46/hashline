#![allow(unused_imports, dead_code)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};

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

fn bench_outline_rust_small(c: &mut Criterion) {
    let content = generate_short_fixture(500);
    c.bench_function("outline_rust_500_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Rust)))
    });
}

fn bench_outline_rust_medium(c: &mut Criterion) {
    let content = generate_short_fixture(2_000);
    c.bench_function("outline_rust_2k_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Rust)))
    });
}

fn bench_outline_rust_large(c: &mut Criterion) {
    let content = generate_short_fixture(10_000);
    c.bench_function("outline_rust_10k_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Rust)))
    });
}

fn bench_outline_rust_long_lines(c: &mut Criterion) {
    let content = generate_long_fixture(2_000);
    c.bench_function("outline_rust_2k_long_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Rust)))
    });
}

fn bench_outline_python(c: &mut Criterion) {
    let content = generate_short_fixture(1_000);
    c.bench_function("outline_python_1k_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Python)))
    });
}

fn bench_outline_go(c: &mut Criterion) {
    let content = generate_short_fixture(1_000);
    c.bench_function("outline_go_1k_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::Go)))
    });
}

fn bench_outline_plaintext(c: &mut Criterion) {
    let content = generate_short_fixture(1_000);
    c.bench_function("outline_plaintext_1k_lines", |b| {
        b.iter(|| black_box(get_outline_entries(&content, Lang::PlainText)))
    });
}

criterion_group!(
    benches,
    bench_outline_rust_small,
    bench_outline_rust_medium,
    bench_outline_rust_large,
    bench_outline_rust_long_lines,
    bench_outline_python,
    bench_outline_go,
    bench_outline_plaintext
);
criterion_main!(benches);
