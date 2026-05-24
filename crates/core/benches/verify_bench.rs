#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::anchor::{parse_anchor, resolve};
use hashline::document::Document;
use support::{generate_short_fixture, mutate_short_hash};

fn build_document(content: &str) -> Document {
    Document::from_str(Path::new("bench.rs"), content).expect("build benchmark document")
}

fn build_anchor_batch(line_count: usize, anchor_count: usize) -> (Document, Vec<String>) {
    let doc = build_document(&generate_short_fixture(line_count));
    let anchors = doc
        .lines
        .iter()
        .enumerate()
        .take(anchor_count)
        .map(|(index, line)| {
            format!(
                "{}:{}",
                index + 1,
                hashline::document::format_short_hash(line.short_hash)
            )
        })
        .collect();
    (doc, anchors)
}

fn build_mixed_anchor_batch(line_count: usize, anchor_count: usize) -> (Document, Vec<String>) {
    let doc = build_document(&generate_short_fixture(line_count));
    let valid_count = anchor_count.saturating_mul(3) / 5;
    let stale_count = anchor_count / 5;
    let invalid_count = anchor_count.saturating_sub(valid_count + stale_count);
    let mut anchors = Vec::with_capacity(anchor_count);

    anchors.extend(
        doc.lines
            .iter()
            .enumerate()
            .take(valid_count)
            .map(|(index, line)| {
                format!(
                    "{}:{}",
                    index + 1,
                    hashline::document::format_short_hash(line.short_hash)
                )
            }),
    );

    anchors.extend(
        doc.lines
            .iter()
            .enumerate()
            .skip(valid_count)
            .take(stale_count)
            .map(|(index, line)| {
                format!(
                    "{}:{}",
                    index + 1,
                    mutate_short_hash(&hashline::document::format_short_hash(line.short_hash))
                )
            }),
    );

    anchors.extend((0..invalid_count).map(|i| format!("bogus-anchor-{i}")));

    (doc, anchors)
}

fn count_verify_successes(doc: &Document, anchor_strings: &[String]) -> usize {
    let index = doc.build_index();
    let mut ok_count = 0;

    for anchor_str in anchor_strings {
        if let Ok(anchor) = parse_anchor(anchor_str) {
            if resolve(&anchor, doc, &index).is_ok() {
                ok_count += 1;
            }
        }
    }

    ok_count
}

// Scaling: vary anchor count on a 10k-line document
fn bench_verify_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify");
    for count in [1, 10, 100, 1_000] {
        let (doc, anchors) = build_anchor_batch(10_000, count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::new("anchors", count),
            &(doc, anchors),
            |b, (doc, anchors)| b.iter(|| black_box(count_verify_successes(doc, anchors))),
        );
    }
    group.finish();
}

// Mixed anchors: valid + stale + invalid
fn bench_verify_mixed(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify_mixed");
    for count in [10, 100] {
        let (doc, anchors) = build_mixed_anchor_batch(10_000, count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::new("anchors", count),
            &(doc, anchors),
            |b, (doc, anchors)| b.iter(|| black_box(count_verify_successes(doc, anchors))),
        );
    }
    group.finish();
}

criterion_group!(benches, bench_verify_scaling, bench_verify_mixed);
criterion_main!(benches);
