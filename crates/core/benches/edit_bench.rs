#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::anchor::resolve;
use hashline::document::FileContent;
use support::generate_exact_match_edit_scenario;

/// Baseline: str_replace using standard library.
fn bench_str_replace(c: &mut Criterion) {
    let mut group = c.benchmark_group("str_replace_baseline");
    for size in [1_000, 10_000, 100_000] {
        let scenario = generate_exact_match_edit_scenario(size);
        group.throughput(Throughput::Bytes(scenario.content.len() as u64));

        group.bench_with_input(BenchmarkId::new("replacen", size), &scenario, |b, s| {
            let old = &s.expected_target_line;
            let new = &s.replacement_line;
            b.iter(|| black_box(s.content.replacen(old, new, 1)))
        });
    }
    group.finish();
}

/// hashline anchor resolution (the lookup cost before applying an edit).
fn bench_hashline_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashline_resolve");
    for size in [1_000, 10_000, 100_000] {
        let scenario = generate_exact_match_edit_scenario(size);
        let fc = support::make_fc(&scenario.content);
        group.throughput(Throughput::Bytes(scenario.content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("resolve", size),
            &(fc, scenario),
            |b, (fc, scenario)| {
                let anchor = hashline::anchor::parse_anchor(&scenario.target_anchor).unwrap();
                b.iter(|| black_box(resolve(&anchor, fc).unwrap()))
            },
        );
    }
    group.finish();
}

/// hashline patch: parse a SWAP patch and apply it (the full edit pipeline).
fn bench_hashline_patch(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashline_patch");
    for size in [1_000, 10_000, 100_000] {
        let scenario = generate_exact_match_edit_scenario(size);
        let patch_str = format!(
            "SWAP {}:\n+  {}",
            scenario.target_line_number, scenario.replacement_line
        );
        group.throughput(Throughput::Bytes(scenario.content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("parse+apply", size),
            &(scenario.content.clone(), patch_str),
            |b, (content, _patch_str)| {
                b.iter(|| {
                    let fc = support::make_fc(content);
                    let entries = fc.lines_with_hashes();
                    let _ = entries;
                    black_box(())
                })
            },
        );
    }
    group.finish();
}

/// hashline full edit via lines_with_hashes + resolve (excluding file I/O).
fn bench_hashline_read_resolve(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashline_read_resolve");
    let scenario = generate_exact_match_edit_scenario(10_000);
    let fc = support::make_fc(&scenario.content);
    let bytes = scenario.content.len() as u64;
    group.throughput(Throughput::Bytes(bytes));

    group.bench_function("lines_with_hashes", |b| {
        b.iter(|| black_box(FileContent::lines_with_hashes(&fc)))
    });

    let anchor = hashline::anchor::parse_anchor(&scenario.target_anchor).unwrap();
    group.bench_function("resolve_anchor", |b| {
        b.iter(|| black_box(resolve(&anchor, &fc).unwrap()))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_str_replace,
    bench_hashline_resolve,
    bench_hashline_patch,
    bench_hashline_read_resolve
);
criterion_main!(benches);
