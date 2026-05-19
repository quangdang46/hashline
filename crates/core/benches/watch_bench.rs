#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

#[path = "../cli.rs"]
mod cli;
#[path = "../context.rs"]
mod context;
#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../hash_cache.rs"]
mod hash_cache;
mod support;
#[path = "../commands/watch.rs"]
mod watch;

use document::Document;
use support::generate_short_fixture;
use watch::diff_documents;

fn build_document(content: &str) -> Document {
    Document::from_str(Path::new("bench.rs"), content).expect("build benchmark document")
}

fn build_diff_documents_with_single_change(line_count: usize) -> (Document, Document) {
    let old_content = generate_short_fixture(line_count);
    let mut new_lines = old_content
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let changed_index = line_count / 2;
    new_lines[changed_index] = format!(
        "fn generated_line_{changed_index:05}() {{ let value = \"changed_{changed_index:08x}\"; }}"
    );
    let new_content = new_lines.join("\n") + "\n";
    (build_document(&old_content), build_document(&new_content))
}

fn build_diff_documents_with_append(
    line_count: usize,
    appended_lines: usize,
) -> (Document, Document) {
    let old_content = generate_short_fixture(line_count);
    let mut new_lines = old_content
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    for i in 0..appended_lines {
        new_lines.push(format!(
            "fn appended_line_{i:05}() {{ let value = \"append_{:08x}\"; }}",
            i.wrapping_mul(1664525)
        ));
    }
    let new_content = new_lines.join("\n") + "\n";
    (build_document(&old_content), build_document(&new_content))
}

// Scaling: no-change diff across file sizes
fn bench_watch_diff_no_changes(c: &mut Criterion) {
    let mut group = c.benchmark_group("watch_no_change");
    for size in [1_000, 10_000, 100_000] {
        let content = generate_short_fixture(size);
        let doc = build_document(&content);
        let doc2 = doc.clone();
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("lines", size),
            &(doc, doc2),
            |b, (old, new)| b.iter(|| black_box(diff_documents(old, new))),
        );
    }
    group.finish();
}

// Scaling: single-change diff across file sizes
fn bench_watch_diff_single_change(c: &mut Criterion) {
    let mut group = c.benchmark_group("watch_single_change");
    for size in [1_000, 10_000, 100_000] {
        let (old_doc, new_doc) = build_diff_documents_with_single_change(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(
            BenchmarkId::new("lines", size),
            &(old_doc, new_doc),
            |b, (old, new)| b.iter(|| black_box(diff_documents(old, new))),
        );
    }
    group.finish();
}

// Append scenario at different scales
fn bench_watch_diff_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("watch_append");
    for append in [10, 100, 1_000] {
        let (old_doc, new_doc) = build_diff_documents_with_append(10_000, append);
        group.throughput(Throughput::Elements(append as u64));
        group.bench_with_input(
            BenchmarkId::new("appended_lines", append),
            &(old_doc, new_doc),
            |b, (old, new)| b.iter(|| black_box(diff_documents(old, new))),
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_watch_diff_no_changes,
    bench_watch_diff_single_change,
    bench_watch_diff_append
);
criterion_main!(benches);
