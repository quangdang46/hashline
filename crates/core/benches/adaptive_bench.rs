#![allow(unused_imports, dead_code)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};

#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../index.rs"]
mod index;
#[path = "../lang/mod.rs"]
mod lang;
mod support;

use index::adaptive::{PatternType, SearchResult, classify_pattern, search_adaptive};
use support::generate_short_fixture;

fn bench_classify_single_byte(c: &mut Criterion) {
    c.bench_function("classify_single_byte", |b| {
        b.iter(|| black_box(classify_pattern(black_box("a"))))
    });
}

fn bench_classify_short_literal(c: &mut Criterion) {
    c.bench_function("classify_short_literal", |b| {
        b.iter(|| black_box(classify_pattern(black_box("fn"))))
    });
}

fn bench_classify_literal(c: &mut Criterion) {
    c.bench_function("classify_literal", |b| {
        b.iter(|| black_box(classify_pattern(black_box("function"))))
    });
}

fn bench_classify_long_literal(c: &mut Criterion) {
    c.bench_function("classify_long_literal", |b| {
        b.iter(|| black_box(classify_pattern(black_box("pub fn get_outline_entries"))))
    });
}

fn bench_classify_regex_simple(c: &mut Criterion) {
    c.bench_function("classify_regex_simple", |b| {
        b.iter(|| black_box(classify_pattern(black_box("fn\\s+\\w+"))))
    });
}

fn bench_classify_regex_alternation(c: &mut Criterion) {
    c.bench_function("classify_regex_alternation", |b| {
        b.iter(|| black_box(classify_pattern(black_box("fn|struct|enum"))))
    });
}

fn bench_classify_regex_complex(c: &mut Criterion) {
    c.bench_function("classify_regex_complex", |b| {
        b.iter(|| black_box(classify_pattern(black_box(r"\b[A-Z][a-z]+\b"))))
    });
}

fn bench_classify_multi_literal(c: &mut Criterion) {
    c.bench_function("classify_multi_literal", |b| {
        b.iter(|| black_box(classify_pattern(black_box("fn|struct|enum|trait|impl"))))
    });
}

criterion_group!(
    benches,
    bench_classify_single_byte,
    bench_classify_short_literal,
    bench_classify_literal,
    bench_classify_long_literal,
    bench_classify_regex_simple,
    bench_classify_regex_alternation,
    bench_classify_regex_complex,
    bench_classify_multi_literal
);
criterion_main!(benches);
