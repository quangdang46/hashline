#![allow(unused_imports, dead_code)]

use std::path::Path;
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use regex::RegexBuilder;

#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../search/mod.rs"]
mod search;
mod support;

use document::Document;
use search::cache::SharedIndexCache;
use search::filter::filter_candidates;
use search::index::IndexBuilder;
use search::verify::verify_candidates;
use support::{generate_long_fixture, generate_short_fixture};

// LineView struct for grep results
#[derive(Clone, Debug)]
struct LineView {
    n: usize,
    hash: String,
    content: String,
}

// Helper to format short hash (short_hash is u8)
fn format_short_hash(short_hash: u8) -> String {
    format!("{:02x}", short_hash)
}

// Linear grep implementation for benchmarks
fn bench_grep_lines(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
) -> Result<Vec<LineView>, error::LinehashError> {
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| error::LinehashError::InvalidPattern {
            pattern: pattern.to_owned(),
            message: e.to_string(),
        })?;

    Ok(doc
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let is_match = regex.is_match(&line.content);
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: index + 1,
                hash: format_short_hash(line.short_hash),
                content: line.content.clone(),
            })
        })
        .collect())
}

// Indexed grep (builds index each time) for benchmarks
fn bench_grep_lines_indexed(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
) -> Result<Vec<LineView>, error::LinehashError> {
    let lines: Vec<Arc<str>> = doc
        .lines
        .iter()
        .map(|l| Arc::from(l.content.as_str()))
        .collect();

    let mut builder = IndexBuilder::new();
    for (idx, line) in lines.iter().enumerate() {
        builder.add_line(idx, line.as_bytes());
    }
    let index = builder.build();

    let (candidates, is_match_all) = filter_candidates(&index, pattern);

    if is_match_all {
        return bench_grep_lines(doc, pattern, invert, case_insensitive);
    }

    let results = verify_candidates(&candidates, &lines, pattern, case_insensitive);

    let filtered: Vec<LineView> = results
        .into_iter()
        .filter_map(|r| {
            let is_match = true;
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: r.line_idx as usize + 1,
                hash: format_short_hash(doc.lines[r.line_idx as usize].short_hash),
                content: r.content.to_string(),
            })
        })
        .collect();

    Ok(filtered)
}

// Cached indexed grep for benchmarks
fn bench_grep_lines_indexed_cached(
    doc: &Document,
    pattern: &str,
    invert: bool,
    case_insensitive: bool,
    cache: &SharedIndexCache,
) -> Result<Vec<LineView>, error::LinehashError> {
    let mtime = doc
        .file_meta
        .as_ref()
        .map(|m| m.mtime_secs as u64)
        .unwrap_or(0);

    let content_bytes: Vec<u8> = doc
        .lines
        .iter()
        .flat_map(|l| l.content.as_bytes().to_vec())
        .collect();

    let (index, lines, _) = cache
        .get_index_data(&doc.path, &content_bytes, mtime)
        .map_err(error::LinehashError::Io)?;

    let (candidates, is_match_all) = filter_candidates(&index, pattern);

    if is_match_all {
        return bench_grep_lines(doc, pattern, invert, case_insensitive);
    }

    let results = verify_candidates(&candidates, &lines, pattern, case_insensitive);

    let filtered: Vec<LineView> = results
        .into_iter()
        .filter_map(|r| {
            let is_match = true;
            let include = if invert { !is_match } else { is_match };
            include.then_some(LineView {
                n: r.line_idx as usize + 1,
                hash: format_short_hash(doc.lines[r.line_idx as usize].short_hash),
                content: r.content.to_string(),
            })
        })
        .collect();

    Ok(filtered)
}

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
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // First call builds the index
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 0);

    c.bench_function("cached_index_1k_lines", |b| {
        b.iter(|| black_box(cache.get_index(Path::new("bench.rs"), &content_bytes, 0)))
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
        b.iter(|| black_box(bench_grep_lines(&doc, "SPARSE_MARKER_12345", false, false)))
    });
}

fn bench_linear_grep_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("linear_grep_10k_lines", |b| {
        b.iter(|| black_box(bench_grep_lines(&doc, "SPARSE_MARKER_12345", false, false)))
    });
}

fn bench_indexed_grep_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    c.bench_function("indexed_grep_1k_lines", |b| {
        b.iter(|| {
            black_box(bench_grep_lines_indexed(
                &doc,
                "SPARSE_MARKER_12345",
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
            black_box(bench_grep_lines_indexed(
                &doc,
                "SPARSE_MARKER_12345",
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
            black_box(bench_grep_lines_indexed(
                &doc,
                "SPARSE_MARKER_12345",
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

// Cached grep benchmarks - these use the shared cache and should be much faster
fn bench_cached_grep_1k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(1_000);
    let cache = SharedIndexCache::default();
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // Pre-warm the cache
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 0);

    c.bench_function("cached_grep_1k_lines", |b| {
        b.iter(|| {
            black_box(bench_grep_lines_indexed_cached(
                &doc,
                "SPARSE_MARKER_12345",
                false,
                false,
                &cache,
            ))
        })
    });
}

fn bench_cached_grep_10k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(10_000);
    let cache = SharedIndexCache::default();
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // Pre-warm the cache
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 0);

    c.bench_function("cached_grep_10k_lines", |b| {
        b.iter(|| {
            black_box(bench_grep_lines_indexed_cached(
                &doc,
                "SPARSE_MARKER_12345",
                false,
                false,
                &cache,
            ))
        })
    });
}

fn bench_cached_grep_100k_lines(c: &mut Criterion) {
    let file = generate_short_fixture(100_000);
    let cache = SharedIndexCache::default();
    let doc = Document::from_str(Path::new("bench.rs"), &file).unwrap();
    let content_bytes: Vec<u8> = file.as_bytes().to_vec();

    // Pre-warm the cache
    let _ = cache.get_index(Path::new("bench.rs"), &content_bytes, 0);

    c.bench_function("cached_grep_100k_lines", |b| {
        b.iter(|| {
            black_box(bench_grep_lines_indexed_cached(
                &doc,
                "SPARSE_MARKER_12345",
                false,
                false,
                &cache,
            ))
        })
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
    bench_index_stats_10k_lines,
    bench_cached_grep_1k_lines,
    bench_cached_grep_10k_lines,
    bench_cached_grep_100k_lines
);
criterion_main!(benches);
