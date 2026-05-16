#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

#[path = "../anchor.rs"]
mod anchor;
#[path = "../document.rs"]
mod document;
#[path = "../error.rs"]
mod error;
#[path = "../hash.rs"]
mod hash;
#[path = "../hash_cache.rs"]
mod hash_cache;
mod support;

use anchor::{parse_anchor, resolve};
use document::Document;
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
                document::format_short_hash(line.short_hash)
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
                    document::format_short_hash(line.short_hash)
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
                    mutate_short_hash(&document::format_short_hash(line.short_hash))
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

fn bench_verify_10_anchors(c: &mut Criterion) {
    let (doc, anchors) = build_anchor_batch(10_000, 10);
    c.bench_function("verify_10_anchors", |b| {
        b.iter(|| black_box(count_verify_successes(&doc, &anchors)))
    });
}

fn bench_verify_100_anchors(c: &mut Criterion) {
    let (doc, anchors) = build_anchor_batch(10_000, 100);
    c.bench_function("verify_100_anchors", |b| {
        b.iter(|| black_box(count_verify_successes(&doc, &anchors)))
    });
}

fn bench_verify_mixed_100_anchors(c: &mut Criterion) {
    let (doc, anchors) = build_mixed_anchor_batch(10_000, 100);
    c.bench_function("verify_mixed_100_anchors", |b| {
        b.iter(|| black_box(count_verify_successes(&doc, &anchors)))
    });
}

criterion_group!(
    benches,
    bench_verify_10_anchors,
    bench_verify_100_anchors,
    bench_verify_mixed_100_anchors
);
criterion_main!(benches);
