#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::anchor::resolve;
use hashline::document::FileContent;
use support::generate_exact_match_edit_scenario;

fn bench_edit_comparison_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit_comparison");
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

fn bench_edit_pipeline_breakdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit_pipeline");
    let scenario = generate_exact_match_edit_scenario(10_000);
    let fc = support::make_fc(&scenario.content);
    let bytes = scenario.content.len() as u64;
    group.throughput(Throughput::Bytes(bytes));

    group.bench_function("1_lines_with_hashes", |b| {
        b.iter(|| black_box(FileContent::lines_with_hashes(&fc)))
    });

    let anchor = hashline::anchor::parse_anchor(&scenario.target_anchor).unwrap();
    group.bench_function("2_resolve_anchor", |b| {
        b.iter(|| black_box(resolve(&anchor, &fc).unwrap()))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_edit_comparison_scaling,
    bench_edit_pipeline_breakdown
);
criterion_main!(benches);
