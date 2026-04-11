#![allow(unused_imports, dead_code)]

use std::path::Path;
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};

#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../search/mod.rs"]
mod search;
mod support;

use document::Document;
use search::cache::SharedIndexCache;
use search::index::IndexBuilder;
use support::{generate_long_fixture, generate_short_fixture};

fn build_index_for_content(content: &str) -> search::TrigramIndex {
    let lines: Vec<Arc<str>> = content.lines().map(|l| Arc::from(l)).collect();
    let mut builder = IndexBuilder::new();
    for (idx, line) in lines.iter().enumerate() {
        builder.add_line(idx, line.as_bytes());
    }
    builder.build()
}

fn bench_index_build_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    c.bench_function("index_build_1k_lines", |b| {
        b.iter(|| black_box(build_index_for_content(&file)))
    });
}

fn bench_index_build_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    c.bench_function("index_build_10k_lines", |b| {
        b.iter(|| black_box(build_index_for_content(&file)))
    });
}

fn bench_index_build_100k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(100_000);
    c.bench_function("index_build_100k_lines", |b| {
        b.iter(|| black_box(build_index_for_content(&file)))
    });
}

fn bench_cached_index_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    let cache = SharedIndexCache::default();
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    let mtime = doc
        .file_meta
        .as_ref()
        .map(|m| m.mtime_secs as u64)
        .unwrap_or(0);
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // First call builds the index
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, mtime);

    c.bench_function("cached_index_1k_lines", |b| {
        b.iter(|| {
            let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
            let mtime = doc
                .file_meta
                .as_ref()
                .map(|m| m.mtime_secs as u64)
                .unwrap_or(0);
            black_box(cache.get_index(Path::new("bench.rs"), &content_bytes, mtime))
        })
    });
}

fn bench_cached_index_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let cache = SharedIndexCache::default();
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // Build initial index
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 1000);

    c.bench_function("cached_index_10k_lines", |b| {
        b.iter(|| black_box(cache.get_index(Path::new("bench.rs"), &content_bytes, 1000)))
    });
}

fn bench_linear_grep_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("linear_grep_1k_lines", |b| {
        b.iter(|| {
            black_box(crate::orchestration::grep_lines(
                &doc,
                "generated_line_05000",
                false,
                false,
            ))
        })
    });
}

fn bench_linear_grep_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("linear_grep_10k_lines", |b| {
        b.iter(|| {
            black_box(crate::orchestration::grep_lines(
                &doc,
                "generated_line_05000",
                false,
                false,
            ))
        })
    });
}

fn bench_indexed_grep_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("indexed_grep_1k_lines", |b| {
        b.iter(|| {
            black_box(crate::orchestration::grep_lines_indexed(
                &doc,
                "generated_line_05000",
                false,
                false,
            ))
        })
    });
}

fn bench_indexed_grep_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("indexed_grep_10k_lines", |b| {
        b.iter(|| {
            black_box(crate::orchestration::grep_lines_indexed(
                &doc,
                "generated_line_05000",
                false,
                false,
            ))
        })
    });
}

fn bench_indexed_grep_100k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(100_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("indexed_grep_100k_lines", |b| {
        b.iter(|| {
            black_box(crate::orchestration::grep_lines_indexed(
                &doc,
                "generated_line_05000",
                false,
                false,
            ))
        })
    });
}

fn bench_index_stats_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let index = build_index_for_content(&file);
    c.bench_function("index_stats_10k_lines", |b| {
        b.iter(|| black_box(index.stats()))
    });
}

criterion_group!(
    benches,
    bench_index_build_1k_lines,
    bench_index_build_10k_lines,
    bench_index_build_100k_lines,
    bench_cached_index_1k_lines,
    bench_cached_index_10k_lines,
    bench_linear_grep_1k_lines,
    bench_linear_grep_10k_lines,
    bench_indexed_grep_1k_lines,
    bench_indexed_grep_10k_lines,
    bench_indexed_grep_100k_lines,
    bench_index_stats_10k_lines
);
criterion_main!(benches);
