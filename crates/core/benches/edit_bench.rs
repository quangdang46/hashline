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
#[path = "../mutation.rs"]
mod mutation;
mod support;

use anchor::{parse_anchor, resolve, resolve_without_index};
use document::Document;
use error::LinehashError;
use mutation::replace_line;
use support::{
    EditScenario, generate_duplicate_target_edit_scenario, generate_exact_match_edit_scenario,
    generate_line_shift_edit_scenario, generate_long_line_exact_match_edit_scenario,
    generate_target_whitespace_drift_edit_scenario, generate_whitespace_drift_edit_scenario,
};

fn linehash_edit_once(scenario: &EditScenario) -> Result<usize, LinehashError> {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc)?;

    replace_line(&mut doc, resolved.index, &scenario.replacement_line)
        .expect("replace target line");

    Ok(doc.render().len())
}

fn linehash_parse_once(scenario: &EditScenario) -> usize {
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    doc.lines.len()
}

fn linehash_resolve_once(scenario: &EditScenario) -> Result<usize, LinehashError> {
    let doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let anchor = parse_anchor(&scenario.target_anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&anchor, &doc)?;
    Ok(resolved.index)
}

fn linehash_mutate_render_once(scenario: &EditScenario) -> usize {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let target_index = scenario.target_line_number - 1;
    replace_line(&mut doc, target_index, &scenario.replacement_line).expect("replace target line");
    doc.render().len()
}

fn linehash_mutate_render_with_receipt_once(scenario: &EditScenario) -> (usize, usize) {
    let mut doc = Document::from_str(Path::new("bench.rs"), &scenario.drifted_content)
        .expect("build benchmark document");
    let before_len = doc.render().len();
    let target_index = scenario.target_line_number - 1;
    replace_line(&mut doc, target_index, &scenario.replacement_line).expect("replace target line");
    let after_len = doc.render().len();
    (before_len, after_len)
}

fn linehash_resolve_prebuilt_exact_match(
    doc: &Document,
    anchor: &str,
) -> Result<usize, LinehashError> {
    let parsed = parse_anchor(anchor).expect("parse target anchor");
    let resolved = resolve_without_index(&parsed, doc)?;
    Ok(resolved.index)
}

fn linehash_render_prebuilt(doc: &Document) -> usize {
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

    linehash_edit_once(scenario).expect("linehash exact-match edit succeeds");
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

    linehash_edit_once(scenario).expect("linehash drift edit succeeds");
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
        linehash_edit_once(scenario).expect_err("linehash should fail on target-line drift");
    assert!(matches!(error, LinehashError::StaleAnchor { .. }));
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

    linehash_edit_once(scenario).expect("linehash duplicate-target edit succeeds");
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
    assert_eq!(scenario.drifted_content.lines().count(), 10_001);

    let error = linehash_edit_once(scenario)
        .expect_err("linehash should fail when lines shift above the target");
    assert!(matches!(
        error,
        LinehashError::StaleAnchor { .. } | LinehashError::InvalidAnchor { .. }
    ));
    assert!(
        naive_str_replace_line_once(scenario),
        "naive exact-line replacement should still find the moved text"
    );
}

fn bench_edit_linehash_single_edit_1k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(1_000);
    assert_exact_match_scenario(&scenario, 1_000);

    c.bench_function("edit_linehash_single_edit_1k_exact_match", |b| {
        b.iter(|| {
            black_box(linehash_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
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

fn bench_edit_linehash_single_edit_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_linehash_single_edit_10k_exact_match", |b| {
        b.iter(|| {
            black_box(linehash_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
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

fn bench_edit_linehash_single_edit_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_linehash_single_edit_100k_exact_match", |b| {
        b.iter(|| {
            black_box(linehash_edit_once(black_box(&scenario)).expect("exact-match edit succeeds"))
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

fn bench_edit_linehash_single_edit_10k_long_lines_exact_match(c: &mut Criterion) {
    let scenario = generate_long_line_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function(
        "edit_linehash_single_edit_10k_long_lines_exact_match",
        |b| {
            b.iter(|| {
                black_box(
                    linehash_edit_once(black_box(&scenario))
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

fn bench_edit_linehash_single_edit_10k_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_whitespace_drift_edit_scenario(10_000);
    assert_surrounding_drift_scenario(&scenario);

    c.bench_function("edit_linehash_single_edit_10k_whitespace_drift", |b| {
        b.iter(|| black_box(linehash_edit_once(black_box(&scenario)).expect("drift edit succeeds")))
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

fn bench_edit_linehash_single_edit_10k_target_whitespace_drift(c: &mut Criterion) {
    let scenario = generate_target_whitespace_drift_edit_scenario(10_000);
    assert_target_drift_scenario(&scenario);

    c.bench_function(
        "edit_linehash_single_edit_10k_target_whitespace_drift",
        |b| b.iter(|| black_box(linehash_edit_once(black_box(&scenario)).is_err())),
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

fn bench_edit_linehash_single_edit_10k_duplicate_target(c: &mut Criterion) {
    let scenario = generate_duplicate_target_edit_scenario(10_000);
    assert_duplicate_target_scenario(&scenario);

    c.bench_function("edit_linehash_single_edit_10k_duplicate_target", |b| {
        b.iter(|| {
            black_box(
                linehash_edit_once(black_box(&scenario)).expect("duplicate-target edit succeeds"),
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

fn bench_edit_linehash_single_edit_10k_line_shift_drift(c: &mut Criterion) {
    let scenario = generate_line_shift_edit_scenario(10_000);
    assert_line_shift_drift_scenario(&scenario);

    c.bench_function("edit_linehash_single_edit_10k_line_shift_drift", |b| {
        b.iter(|| black_box(linehash_edit_once(black_box(&scenario)).is_err()))
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
        b.iter(|| black_box(linehash_parse_once(black_box(&scenario))))
    });
}

fn bench_edit_resolve_anchor_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_resolve_anchor_10k_exact_match", |b| {
        b.iter(|| black_box(linehash_resolve_once(black_box(&scenario)).expect("anchor resolves")))
    });
}

fn bench_edit_resolve_anchor_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_resolve_anchor_100k_exact_match", |b| {
        b.iter(|| black_box(linehash_resolve_once(black_box(&scenario)).expect("anchor resolves")))
    });
}

fn bench_edit_parse_document_100k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_parse_document_100k_exact_match", |b| {
        b.iter(|| black_box(linehash_parse_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_linehash_10k_single_line(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_mutate_render_linehash_10k_single_line", |b| {
        b.iter(|| black_box(linehash_mutate_render_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_linehash_100k_single_line(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function("edit_mutate_render_linehash_100k_single_line", |b| {
        b.iter(|| black_box(linehash_mutate_render_once(black_box(&scenario))))
    });
}

fn bench_edit_mutate_render_linehash_10k_single_line_with_receipt(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function(
        "edit_mutate_render_linehash_10k_single_line_with_receipt",
        |b| {
            b.iter(|| {
                black_box(linehash_mutate_render_with_receipt_once(black_box(
                    &scenario,
                )))
            })
        },
    );
}

fn bench_edit_mutate_render_linehash_100k_single_line_with_receipt(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(100_000);
    assert_exact_match_scenario(&scenario, 100_000);

    c.bench_function(
        "edit_mutate_render_linehash_100k_single_line_with_receipt",
        |b| {
            b.iter(|| {
                black_box(linehash_mutate_render_with_receipt_once(black_box(
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
                linehash_resolve_prebuilt_exact_match(black_box(&doc), black_box(&anchor))
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
        b.iter(|| black_box(linehash_render_prebuilt(black_box(&doc))))
    });
}

fn bench_edit_replace_naive_line_10k_exact_match(c: &mut Criterion) {
    let scenario = generate_exact_match_edit_scenario(10_000);
    assert_exact_match_scenario(&scenario, 10_000);

    c.bench_function("edit_replace_naive_line_10k_exact_match", |b| {
        b.iter(|| black_box(naive_str_replace_line_once(black_box(&scenario))))
    });
}

criterion_group!(
    benches,
    bench_edit_linehash_single_edit_1k_exact_match,
    bench_edit_naive_str_replace_single_edit_1k_exact_match,
    bench_edit_linehash_single_edit_10k_exact_match,
    bench_edit_naive_str_replace_single_edit_10k_exact_match,
    bench_edit_linehash_single_edit_100k_exact_match,
    bench_edit_naive_str_replace_single_edit_100k_exact_match,
    bench_edit_linehash_single_edit_10k_long_lines_exact_match,
    bench_edit_naive_str_replace_single_edit_10k_long_lines_exact_match,
    bench_edit_linehash_single_edit_10k_whitespace_drift,
    bench_edit_naive_str_replace_single_edit_10k_whitespace_drift,
    bench_edit_linehash_single_edit_10k_target_whitespace_drift,
    bench_edit_naive_str_replace_single_edit_10k_target_whitespace_drift,
    bench_edit_linehash_single_edit_10k_duplicate_target,
    bench_edit_naive_str_replace_single_edit_10k_duplicate_target,
    bench_edit_linehash_single_edit_10k_line_shift_drift,
    bench_edit_naive_str_replace_single_edit_10k_line_shift_drift,
    bench_edit_parse_document_10k_exact_match,
    bench_edit_resolve_anchor_10k_exact_match,
    bench_edit_resolve_anchor_100k_exact_match,
    bench_edit_resolve_anchor_100k_prebuilt_exact_match,
    bench_edit_parse_document_100k_exact_match,
    bench_edit_mutate_render_linehash_10k_single_line,
    bench_edit_mutate_render_linehash_100k_single_line,
    bench_edit_render_document_100k_exact_match,
    bench_edit_mutate_render_linehash_10k_single_line_with_receipt,
    bench_edit_mutate_render_linehash_100k_single_line_with_receipt,
    bench_edit_replace_naive_line_10k_exact_match
);
criterion_main!(benches);
