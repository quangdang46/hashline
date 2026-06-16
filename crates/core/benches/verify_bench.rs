#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::anchor::{parse_anchor, resolve_with_entries};
use hashline::document::{FileContent, LineEntry};
use support::generate_short_fixture;

fn count_resolve_successes(entries: &[LineEntry], fc: &FileContent, anchor_strings: &[String]) -> usize {
    let mut ok_count = 0;
    for anchor_str in anchor_strings {
        if let Ok(anchor) = parse_anchor(anchor_str) {
            if resolve_with_entries(&anchor, entries, fc).is_ok() {
                ok_count += 1;
            }
        }
    }
    ok_count
}

fn bench_verify_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify");
    for count in [1, 10, 100, 1_000] {
        let fc = generate_short_fixture(10_000);
        let entries = fc.lines_with_hashes();
        let anchors: Vec<String> = entries
            .iter()
            .enumerate()
            .take(count)
            .map(|(index, entry)| {
                format!("{}:{}", index + 1, hashline::hash::format_short_hash(entry.short_hash))
            })
            .collect();
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::new("anchors", count),
            &(entries, fc, anchors),
            |b, (entries, fc, anchors)| b.iter(|| black_box(count_resolve_successes(entries, fc, anchors))),
        );
    }
    group.finish();
}

criterion_group!(benches, bench_verify_scaling);
criterion_main!(benches);
