#![allow(unused_imports, dead_code)]

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};

mod support;

use hashline::anchor::{parse_anchor, resolve, resolve_without_index};
use hashline::document::Document;
use hashline::error::HashlineError;
use hashline::fast as hashline_fast;
use hashline::mutation::replace_line;
use support::{
    EditScenario, generate_duplicate_target_edit_scenario, generate_exact_match_edit_scenario,
    generate_line_shift_edit_scenario, generate_long_line_exact_match_edit_scenario,
    generate_target_whitespace_drift_edit_scenario, generate_whitespace_drift_edit_scenario,
};

fn hashline_edit_once(scenario: &EditScenario) -> Result<usize, HashlineError> {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc)?;

    replace_line(&mut doc, resolved.index, &scenario.replacement_line)
        .expect("replace target line");

    Ok(doc.render().len())
}

fn hashline_parse_once(scenario: &EditScenario) -> usize {
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    doc.lines.len()
}

fn hashline_resolve_once(scenario: &EditScenario) -> Result<usize, HashlineError> {
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc)?;
    Ok(resolved.index)
}

fn hashline_mutate_render_once(scenario: &EditScenario) -> usize {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let target_index = scenario.target_line_number - 1;
    replace_line(&mut doc, target_index, &scenario.replacement_line).expect("replace target line");
    doc.render().len()
}

fn hashline_mutate_render_with_receipt_once(scenario: &EditScenario) -> (usize, usize) {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let before_len = doc.render().len();
    let target_index = scenario.target_line_number - 1;
    replace_line(&mut doc, target_index, &scenario.replacement_line).expect("replace target line");
    let after_len = doc.render().len();
    (before_len, after_len)
}

fn hashline_resolve_prebuilt_exact_match(
    doc: &Document,
    anchor: &str,
) -> Result<usize, HashlineError> {
    let parsed = parse_anchor(anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&parsed, doc)?;
    Ok(resolved.index)
}

fn hashline_render_prebuilt(doc: &Document) -> usize {
    doc.render().len()
}

fn naive_str_replace_line_once(scenario: &EditScenario) -> bool {
    let content = scenario.drifted_content.clone();
    if !content.contains(&scenario.naive_old_line) {
        return false;
    }

    let replaced = content.replacen(&scenario.naive_old_line, &scenario.naive_new_line, 1);
    replaced.contains(&scenario.expected_target_line)
}

fn naive_str_replace_block_once(scenario: &EditScenario) -> bool {
    let content = scenario.drifted_content.clone();
    if !content.contains(&scenario.naive_old_block) {
        return false;
    }

    let replaced = content.replacen(&scenario.naive_old_block, &scenario.naive_new_block, 1);
    replaced.contains(&scenario.expected_target_line)
}

fn assert_exact_match_scenario(scenario: &EditScenario, expected_lines: usize) {
    assert_eq!(scenario.drifted_content.lines().count(), expected_lines);

    hashline_edit_once(scenario).expect("hashline exact-match edit succeeds");
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc).expect("anchor resolves");
    replace_line(&mut doc, resolved.index, &scenario.replacement_line)
        .expect("replace target line");
    assert_eq!(
        doc.lines[scenario.target_line_number - 1].content.as_ref(),
        scenario.expected_target_line
    );
    assert!(
        naive_str_replace_line_once(scenario),
        "naive exact-line replacement should succeed"
    );
}

fn assert_surrounding_drift_scenario(scenario: &EditScenario) {
    assert_eq!(scenario.drifted_content.lines().count(), 10_000);

    hashline_edit_once(scenario).expect("hashline drift edit succeeds");
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc).expect("anchor resolves");
    replace_line(&mut doc, resolved.index, &scenario.replacement_line)
        .expect("replace target line");
    assert_eq!(
        doc.lines[scenario.target_line_number - 1].content.as_ref(),
        scenario.expected_target_line
    );

    assert!(
        !scenario.drifted_content.contains(&scenario.naive_old_block),
        "stale exact block should be absent after surrounding-context drift"
    );
    assert!(
        !naive_str_replace_block_once(scenario),
        "naive stale block replacement should fail in the surrounding-drift scenario"
    );
    assert!(
        naive_str_replace_line_once(scenario),
        "exact-line replacement should still succeed when only surrounding context drifted"
    );
}

fn assert_target_drift_scenario(scenario: &EditScenario) {
    assert_eq!(scenario.drifted_content.lines().count(), 10_000);

    let error =
        hashline_edit_once(scenario).expect_err("hashline should fail on target-line drift");
    assert!(matches!(error, HashlineError::StaleAnchor { .. }));
    assert!(
        !naive_str_replace_line_once(scenario),
        "naive exact-line replacement should fail when the target line text changed"
    );
}

fn assert_duplicate_target_scenario(scenario: &EditScenario) {
    assert_eq!(scenario.drifted_content.lines().count(), 10_000);
    let target_index = scenario.target_line_number - 1;
    let original_lines = scenario.drifted_content.lines().collect::<Vec<_>>();
    let duplicate_count = original_lines
        .iter()
        .filter(|line| **line == scenario.naive_old_line)
        .count();
    assert!(
        duplicate_count >= 2,
        "fixture should contain at least two identical target lines"
    );

    hashline_edit_once(scenario).expect("hashline duplicate-target edit succeeds");
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc).expect("anchor resolves");
    replace_line(&mut doc, resolved.index, &scenario.replacement_line)
        .expect("replace target line");
    assert_eq!(
        doc.lines[target_index].content.as_ref(),
        scenario.expected_target_line
    );

    let naive_replaced = scenario.drifted_content.clone().replacen(
        &scenario.naive_old_line,
        &scenario.naive_new_line,
        1,
    );
    let naive_lines = naive_replaced.lines().collect::<Vec<_>>();
    assert_eq!(
        naive_lines[target_index], scenario.naive_old_line,
        "naive exact-line replacement should leave the intended later duplicate unchanged"
    );
}

fn assert_line_shift_drift_scenario(scenario: &EditScenario) {
    // 10_000 original + 5 inserted lines = 10_005
    assert_eq!(scenario.drifted_content.lines().count(), 10_005);

    let error = hashline_edit_once(scenario)
        .expect_err("hashline should fail when lines shift above the target");
    assert!(matches!(
        error,
        HashlineError::StaleAnchor { .. } | HashlineError::InvalidAnchor { .. }
    ));
    assert!(
        naive_str_replace_line_once(scenario),
        "naive exact-line replacement should still find the moved text"
    );
}

fn bench_edit_hashline_single_edit_1k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(1_000);
    assert_exact_match_scenario(&scenario, 1_000);

    c.bench_function("edit_hashline_single_edit_1k_exact_match", |b| {
        b.iter(|| {
            black_box(hashline_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
        })
    });
}

fn bench_edit_naive_str_replace_single_edit_1k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(1_000);
    assert_exact_match_scenario(&scenario, 1_000);

    c.bench_function("edit_naive_str_replace_single_edit_1k_exact_match", |b| {
        b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario))))
    });
}

fn bench_edit_hashline_single_edit_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_hashline_single_edit_10k_exact_match", |b| {
        b.iter(|| {
            black_box(hashline_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
        })
    });
}

fn bench_edit_naive_str_replace_single_edit_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_naive_str_replace_single_edit_10k_exact_match", |b| {
        b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario))))
    });
}

fn bench_edit_hashline_single_edit_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_hashline_single_edit_100k_exact_match", |b| {
        b.iter(|| {
            black_box(hashline_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
        })
    });
}

fn bench_edit_naive_str_replace_single_edit_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_naive_str_replace_single_edit_100k_exact_match", |b| {
        b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario))))
    });
}

fn bench_edit_hashline_single_edit_10k_long_lines_exact_match(c: &mut Criterion) {
    let scenario = generate_long_line_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function(
        "edit_hashline_single_edit_10k_long_lines_exact_match",
        |b| {
            b.iter(|| {
                black_box(
                    hashline_edit_once(black_box(&scenario))
                        .expect("long-line exact-match edit succeeds"),
                )
            })
        },
    );
}

fn bench_edit_naive_str_replace_single_edit_10k_long_lines_exact_match(c: &mut Criterion) {
    let scenario = generate_long_line_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function(
        "edit_naive_str_replace_single_edit_10k_long_lines_exact_match",
        |b| b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario)))),
    );
}

fn bench_edit_hashline_single_edit_10k_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_whitespace_drift_edit_scenario(10_000);
    assert_surrounding_drift_scenario(&scenario);

    c.bench_function("edit_hashline_single_edit_10k_whitespace_drift", |b| {
        b.iter(|| black_box(hashline_edit_once(black_box(&scenario)).expect("drift edit succeeds")))
    });
}

fn bench_edit_naive_str_replace_single_edit_10k_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_whitespace_drift_edit_scenario(10_000);
    assert_surrounding_drift_scenario(&scenario);

    c.bench_function(
        "edit_naive_str_replace_single_edit_10k_whitespace_drift",
        |b| b.iter(|| black_box(naive_str_replace_block_once(black_box(&scenario)))),
    );
}

fn bench_edit_hashline_single_edit_10k_target_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_target_whitespace_drift_edit_scenario(10_000);
    assert_target_drift_scenario(&scenario);

    c.bench_function(
        "edit_hashline_single_edit_10k_target_whitespace_drift",
        |b| b.iter(|| black_box(hashline_edit_once(black_box(&scenario)).is_err())),
    );
}

fn bench_edit_naive_str_replace_single_edit_10k_target_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_target_whitespace_drift_edit_scenario(10_000);
    assert_target_drift_scenario(&scenario);

    c.bench_function(
        "edit_naive_str_replace_single_edit_10k_target_whitespace_drift",
        |b| b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario)))),
    );
}

fn bench_edit_hashline_single_edit_10k_duplicate_target(c: &mut Criterion) {
    let scenario = generate_duplicate_target_edit_scenario(10_000);
    assert_duplicate_target_scenario(&scenario);

    c.bench_function("edit_hashline_single_edit_10k_duplicate_target", |b| {
        b.iter(|| {
            black_box(
                hashline_edit_once(black_box(&scenario)).expect("duplicate-target edit succeeds"),
            )
        })
    });
}

fn bench_edit_naive_str_replace_single_edit_10k_duplicate_target(c: &mut Criterion) {
    let scenario = generate_duplicate_target_edit_scenario(10_000);
    assert_duplicate_target_scenario(&scenario);

    c.bench_function(
        "edit_naive_str_replace_single_edit_10k_duplicate_target",
        |b| b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario)))),
    );
}

fn bench_edit_hashline_single_edit_10k_line_shift_drift(c: &mut Criterion) {
    let scenario = generate_line_shift_edit_scenario(10_000);
    assert_line_shift_drift_scenario(&scenario);

    c.bench_function("edit_hashline_single_edit_10k_line_shift_drift", |b| {
        b.iter(|| black_box(hashline_edit_once(black_box(&scenario)).is_err()))
    });
}

fn bench_edit_naive_str_replace_single_edit_10k_line_shift_drift(c: &mut Criterion) {
    let scenario = generate_line_shift_edit_scenario(10_000);
    assert_line_shift_drift_scenario(&scenario);

    c.bench_function(
        "edit_naive_str_replace_single_edit_10k_line_shift_drift",
        |b| b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario)))),
    );
}

fn bench_edit_parse_document_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_parse_document_10k_exact_match", |b| {
        b.iter(|| black_box(hashline_parse_once(black_box(&scenario))))
    });
}

fn bench_edit_resolve_anchor_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_resolve_anchor_10k_exact_match", |b| {
        b.iter(|| black_box(hashline_resolve_once(black_box(&scenario)).expect("anchor resolves")))
    });
}

fn bench_edit_resolve_anchor_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_resolve_anchor_100k_exact_match", |b| {
        b.iter(|| black_box(hashline_resolve_once(black_box(&scenario)).expect("anchor resolves")))
    });
}

fn bench_edit_parse_document_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_parse_document_100k_exact_match", |b| {
        b.iter(|| black_box(hashline_parse_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_hashline_10k_single_line(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_mutate_render_hashline_10k_single_line", |b| {
        b.iter(|| black_box(hashline_mutate_render_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_hashline_100k_single_line(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_mutate_render_hashline_100k_single_line", |b| {
        b.iter(|| black_box(hashline_mutate_render_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_hashline_10k_single_line_with_receipt(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function(
        "edit_mutate_render_hashline_10k_single_line_with_receipt",
        |b| {
            b.iter(|| {
                black_box(hashline_mutate_render_with_receipt_once(black_box(
                    &scenario,
                )))
            })
        },
    );
}

fn bench_edit_mutate_render_hashline_100k_single_line_with_receipt(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function(
        "edit_mutate_render_hashline_100k_single_line_with_receipt",
        |b| {
            b.iter(|| {
                black_box(hashline_mutate_render_with_receipt_once(black_box(
                    &scenario,
                )))
            })
        },
    );
}

fn bench_edit_resolve_anchor_100k_prebuilt_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = scenario.target_anchor.clone();

    c.bench_function("edit_resolve_anchor_100k_prebuilt_exact_match", |b| {
        b.iter(|| {
            black_box(
                hashline_resolve_prebuilt_exact_match(black_box(&doc), black_box(&anchor))
                    .expect("anchor resolves"),
            )
        })
    });
}

fn bench_edit_render_document_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");

    c.bench_function("edit_render_document_100k_exact_match", |b| {
        b.iter(|| black_box(hashline_render_prebuilt(black_box(&doc))))
    });
}

fn bench_edit_replace_naive_line_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_replace_naive_line_10k_exact_match", |b| {
        b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario))))
    });
}

// --- Comparison group: hashline vs str_replace scaling ---
fn bench_edit_comparison_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit_comparison");
    for size in [1_000, 10_000, 100_000] {
        let scenario = generate_exact_match_edit_scenario(size);
        group.throughput(Throughput::Bytes(scenario.drifted_content.len() as u64));

        group.bench_with_input(
            BenchmarkId::new("hashline", size),
            &scenario,
            |b, scenario| {
                b.iter(|| {
                    black_box(
                        hashline_edit_once(black_box(scenario)).expect("exact-match edit succeeds"),
                    )
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("str_replace", size),
            &scenario,
            |b, scenario| b.iter(|| black_box(naive_str_replace_line_once(black_box(scenario)))),
        );

        group.bench_with_input(
            BenchmarkId::new("fast_edit", size),
            &scenario,
            |b, scenario| {
                b.iter(|| {
                    let content = &scenario.drifted_content;
                    let target_line = scenario.target_line_number - 1;
                    let line = content.lines().nth(target_line).unwrap();
                    let short_hash = hashline::hash::short_hash_value(line);
                    black_box(
                        hashline_fast::fast_replace_line(
                            black_box(content),
                            target_line,
                            short_hash,
                            &scenario.replacement_line,
                        )
                        .expect("fast_edit succeeds"),
                    )
                })
            },
        );
    }
    group.finish();
}

// --- Pipeline breakdown: parse vs resolve vs mutate ---
fn bench_edit_pipeline_breakdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit_pipeline");
    let scenario = generate_exact_match_edit_scenario(10_000);
    let bytes = scenario.drifted_content.len() as u64;
    group.throughput(Throughput::Bytes(bytes));

    group.bench_function("1_parse_document", |b| {
        b.iter(|| black_box(hashline_parse_once(black_box(&scenario))))
    });

    group.bench_function("2_resolve_anchor", |b| {
        b.iter(|| black_box(hashline_resolve_once(black_box(&scenario)).expect("resolves")))
    });

    group.bench_function("3_mutate_render", |b| {
        b.iter(|| black_box(hashline_mutate_render_once(black_box(&scenario))))
    });

    group.bench_function("4_full_edit", |b| {
        b.iter(|| {
            black_box(hashline_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_edit_hashline_single_edit_1k_exact_match,
    bench_edit_naive_str_replace_single_edit_1k_exact_match,
    bench_edit_hashline_single_edit_10k_exact_match,
    bench_edit_naive_str_replace_single_edit_10k_exact_match,
    bench_edit_hashline_single_edit_100k_exact_match,
    bench_edit_naive_str_replace_single_edit_100k_exact_match,
    bench_edit_hashline_single_edit_10k_long_lines_exact_match,
    bench_edit_naive_str_replace_single_edit_10k_long_lines_exact_match,
    bench_edit_hashline_single_edit_10k_whitespace_drift,
    bench_edit_naive_str_replace_single_edit_10k_whitespace_drift,
    bench_edit_hashline_single_edit_10k_target_whitespace_drift,
    bench_edit_naive_str_replace_single_edit_10k_target_whitespace_drift,
    bench_edit_hashline_single_edit_10k_duplicate_target,
    bench_edit_naive_str_replace_single_edit_10k_duplicate_target,
    bench_edit_hashline_single_edit_10k_line_shift_drift,
    bench_edit_naive_str_replace_single_edit_10k_line_shift_drift,
    bench_edit_parse_document_10k_exact_match,
    bench_edit_resolve_anchor_10k_exact_match,
    bench_edit_resolve_anchor_100k_exact_match,
    bench_edit_resolve_anchor_100k_prebuilt_exact_match,
    bench_edit_parse_document_100k_exact_match,
    bench_edit_mutate_render_hashline_10k_single_line,
    bench_edit_mutate_render_hashline_100k_single_line,
    bench_edit_render_document_100k_exact_match,
    bench_edit_mutate_render_hashline_10k_single_line_with_receipt,
    bench_edit_mutate_render_hashline_100k_single_line_with_receipt,
    bench_edit_replace_naive_line_10k_exact_match,
    bench_edit_comparison_scaling,
    bench_edit_pipeline_breakdown
);
criterion_main!(benches);
