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
#[path = "../search/mod.rs"]
mod search;
mod support;

use document::{Document, LineRecord};
use search::cache::SharedIndexCache;
use search::decompose::decompose_regex;
use search::filter::CandidateFilter;
use search::index::IndexBuilder;
use search::verify::verify_candidates;
use support::{generate_long_fixture, generate_short_fixture};

fn build_index_for_content(content: &str) -> search::TrigramIndex {
    let lines: Vec<Box<str>> = content.lines().map(Box::from).collect();
    let mut builder = IndexBuilder::new();
    for (idx, line) in lines.iter().enumerate() {
        builder.add_line(idx, line.as_bytes());
    }
    builder.build()
}

fn do_indexed_grep(doc: &Document, index: &search::TrigramIndex, pattern: &str) -> usize {
    let decomposed = decompose_regex(pattern);
    let filter = CandidateFilter::new(index, &decomposed);
    let candidates = filter.filter();
    let results = verify_candidates(&candidates, &doc.lines, pattern, false);
    results.len()
}

fn do_linear_grep(doc: &Document, pattern: &str) -> usize {
    let re = regex::Regex::new(pattern).unwrap();
    doc.lines
        .iter()
        .filter(|line| re.is_match(&line.content))
        .count()
}

// --- Index build scaling ---
fn bench_index_build_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_build");
    for size in [1_000, 10_000] {
        let file = generate_short_fixture(size);
        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &file, |b, content| {
            b.iter(|| black_box(build_index_for_content(content)))
        });
    }
    group.finish();
}

// --- Cached index ---
fn bench_cached_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("cached_index");
    for size in [1_000, 10_000] {
        let file = generate_short_fixture(size);
        let cache = SharedIndexCache::default();
        let content_bytes: Vec<u8> = file.as_bytes().to_vec();
        // Warm cache
        let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 1000);

        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("hit", size),
            &(cache, content_bytes),
            |b, (cache, bytes)| {
                b.iter(|| black_box(cache.get_index(Path::new("bench.rs"), bytes, 1000)))
            },
        );
    }
    group.finish();
}

// --- Trigram grep scaling (1k–100k) ---
fn bench_grep_trigram_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_trigram");
    for size in [1_000, 10_000] {
        let file = generate_short_fixture(size);
        let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
        let index = build_index_for_content(&file);
        let pattern = &format!("generated_line_{:05}", size / 2);

        group.throughput(Throughput::Bytes(file.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("lines", size),
            &(&doc, &index, pattern.as_str()),
            |b, &(doc, idx, pat)| b.iter(|| black_box(do_indexed_grep(doc, idx, pat))),
        );
    }
    group.finish();
}

// --- Trigram vs linear comparison (capped at 10k to keep runtime sane) ---
fn bench_grep_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("grep_comparison");
    for size in [1_000, 10_000] {
        let file = generate_short_fixture(size);
        let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
        let index = build_index_for_content(&file);
        let pattern = &format!("generated_line_{:05}", size / 2);

        group.throughput(Throughput::Bytes(file.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("trigram", size),
            &(&doc, &index, pattern.as_str()),
            |b, &(doc, idx, pat)| b.iter(|| black_box(do_indexed_grep(doc, idx, pat))),
        );

        group.bench_with_input(
            BenchmarkId::new("linear", size),
            &(&doc, pattern.as_str()),
            |b, &(doc, pat)| b.iter(|| black_box(do_linear_grep(doc, pat))),
        );
    }
    group.finish();
}

// --- Index stats ---
fn bench_index_stats(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let index = build_index_for_content(&file);
    c.bench_function("index_stats_10k_lines", |b| {
        b.iter(|| black_box(index.stats()))
    });
}

criterion_group!(
    benches,
    bench_index_build_scaling,
    bench_cached_index,
    bench_grep_trigram_scaling,
    bench_grep_comparison,
    bench_index_stats
);
criterion_main!(benches);
