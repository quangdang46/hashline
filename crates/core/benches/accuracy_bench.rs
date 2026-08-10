#![allow(unused_imports, dead_code)]

//! Accuracy / regression benchmark baseline (Phase 0).
//!
//! Measures the exact properties the low-breakage improvement phases target,
//! WITHOUT changing any behavior or hash format. Run before and after each
//! phase to quantify how much each optimization actually helps:
//!
//!   cargo bench --bench accuracy
//!
//! Metrics:
//! - `collision_rate` — fraction of lines whose 2-char short hash collides
//!   with another line in the same file. Baseline for the position-seeded
//!   symbol-hash and context-hash ideas.
//! - `collision_adjacent_rate` — fraction of *adjacent* line pairs sharing a
//!   hash. Drives whether adjacent-line context is worth mixing in.
//! - `ambiguous_anchor_rate` — fraction of target anchors that match more
//!   than one line (the "hash 'ab' matches 3 lines" failure mode).
//! - `symbol_only_distinctness` — how often identical symbol-only lines
//!   (`}`, `)`, `]`, blank) share a hash. Baseline for position-seeding.
//! - `stale_anchor_detection` — micro-benchmark of a full-file re-hash on
//!   patch (the cost of staleness detection).

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::document::FileContent;
use hashline::hash;

fn bench_collision_rates(c: &mut Criterion) {
    let mut group = c.benchmark_group("collision_rates");
    for size in [1_000, 10_000, 100_000] {
        let fc = support::generate_short_fixture(size);
        let entries = fc.lines_with_hashes();
        let n = entries.len();

        // Per-hash histogram of 2-char short hashes.
        let mut seen: std::collections::HashMap<u8, usize> = std::collections::HashMap::new();
        let mut collisions = 0usize;
        for e in &entries {
            let h = e.short_hash;
            let cnt = seen.entry(h).or_insert(0);
            *cnt += 1;
            if *cnt == 2 {
                collisions += 1;
            }
        }

        // Adjacent-pair collisions.
        let mut adjacent_collisions = 0usize;
        for w in entries.windows(2) {
            if w[0].short_hash == w[1].short_hash {
                adjacent_collisions += 1;
            }
        }

        // Ambiguous anchors: a hash value occurring 2+ times.
        let ambiguous = seen.values().filter(|&&c| c >= 2).count();

        let coll_rate = collisions as f64 / n as f64;
        let adj_rate = adjacent_collisions as f64 / (n.saturating_sub(1)) as f64;
        let amb_rate = ambiguous as f64 / seen.len().max(1) as f64;

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(
            BenchmarkId::new("collision_rate", size),
            &coll_rate,
            |b, r| b.iter(|| black_box(*r)),
        );
        group.bench_with_input(
            BenchmarkId::new("adjacent_collision_rate", size),
            &adj_rate,
            |b, r| b.iter(|| black_box(*r)),
        );
        group.bench_with_input(
            BenchmarkId::new("ambiguous_anchor_rate", size),
            &amb_rate,
            |b, r| b.iter(|| black_box(*r)),
        );

        eprintln!(
            "accuracy[{}]: lines={} collisions={} ({:.4}%) adjacent_collisions={} ({:.4}%) ambiguous_hashes={}",
            size,
            n,
            collisions,
            coll_rate * 100.0,
            adjacent_collisions,
            adj_rate * 100.0,
            ambiguous,
        );
    }
    group.finish();
}

fn bench_symbol_only_distinctness(c: &mut Criterion) {
    let mut group = c.benchmark_group("symbol_only_distinctness");

    // A fixture of identical symbol-only lines (like a file of `}` closers).
    let mut lines: Vec<String> = Vec::new();
    for i in 0..5000 {
        lines.push(if i % 3 == 0 {
            "}".to_string()
        } else if i % 3 == 1 {
            ")".to_string()
        } else {
            String::new()
        });
    }
    let text = lines.join("\n");
    let fc = support::make_fc(&text);
    let entries = fc.lines_with_hashes();

    // How many distinct short-hash values among 5000 identical-ish lines?
    let mut distinct = std::collections::HashSet::new();
    for e in &entries {
        distinct.insert(e.short_hash);
    }
    let distinct_rate = distinct.len() as f64 / entries.len() as f64;

    // After position-seeding (Phase 2): distinctness should jump from 3 (all
    // symbol-only lines share a hash) toward the 256-value ceiling. With 5000
    // lines the rate is capped by the 2-char hash space; the improvement is
    // systematic→uniform collision, not zero collision.
    eprintln!(
        "accuracy[symbol]: {} identical symbol-only lines → {} distinct hashes ({:.1}% distinct; 2-char ceiling = 256)",
        entries.len(),
        distinct.len(),
        distinct_rate * 100.0,
    );

    group.bench_function("symbol_only_identical_lines", |b| {
        b.iter(|| black_box(distinct_rate))
    });
    group.finish();
}

fn bench_stale_detection_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("stale_detection_cost");
    for size in [1_000, 10_000, 100_000] {
        let fc = support::generate_short_fixture(size);
        group.throughput(Throughput::Elements(size as u64));
        // Cost of computing a fresh set of line hashes to compare against a patch's anchors.
        group.bench_with_input(BenchmarkId::new("rehash_all_lines", size), &fc, |b, fc| {
            b.iter(|| black_box(fc.lines_with_hashes()))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_collision_rates,
    bench_symbol_only_distinctness,
    bench_stale_detection_cost
);
criterion_main!(benches);
