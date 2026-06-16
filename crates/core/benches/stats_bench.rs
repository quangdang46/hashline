#![allow(unused_imports, dead_code)]

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::document::FileContent;
use support::{generate_collision_fixture, generate_short_fixture};

fn bench_lines_with_hashes(c: &mut Criterion) {
    let mut group = c.benchmark_group("lines_with_hashes");
    for size in [1_000, 10_000, 100_000] {
        let fc = generate_short_fixture(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &fc, |b, fc| {
            b.iter(|| black_box(FileContent::lines_with_hashes(fc)))
        });
    }
    group.finish();
}

fn bench_collision_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("lines_collision");
    for size in [1_000, 10_000] {
        let fc = generate_collision_fixture(size);
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("lines", size), &fc, |b, fc| {
            b.iter(|| black_box(FileContent::lines_with_hashes(fc)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_lines_with_hashes, bench_collision_heavy);
criterion_main!(benches);
