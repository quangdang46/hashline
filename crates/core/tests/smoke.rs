mod support;

use std::fs;
use tempfile::TempDir;

use support::{
    assert_err_contains, do_edit, fixture_path, parse_json, run_hashline, run_hashline_in, tmpfile,
};
#[cfg(unix)]
use support::{chmod, mode};

#[test]
fn missing_file_read_reports_io_error() {
    let (_stdout, stderr, code) = run_hashline(&["read", "/definitely/missing/file.txt"]);

    assert_eq!(code, 1);
    assert!(stderr.contains("Error: I/O error:"));
}

#[test]
fn read_fixture_pretty_output_includes_anchors() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["read", &fixture_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("1:"));
    assert!(stdout.contains("|function greet(name) {"));
    assert!(stdout.contains("|export function main() {"));
}

#[test]
fn read_json_includes_file_metadata_and_lines() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &fixture_arg, "--json"]);
    let expected_newline = if fs::read_to_string(&fixture).unwrap().contains("\r\n") {
        "crlf"
    } else {
        "lf"
    };

    assert_eq!(parsed["file"], fixture_arg);
    assert_eq!(parsed["newline"], expected_newline);
    assert_eq!(parsed["trailing_newline"], true);
    assert!(parsed["mtime"].is_i64());
    assert!(parsed["mtime_nanos"].is_u64());
    assert!(parsed["inode"].is_u64());
    assert_eq!(parsed["lines"][0]["n"], 1);
    assert_eq!(parsed["lines"][0]["content"], "function greet(name) {");
}

#[test]
fn read_anchor_context_only_shows_neighborhood() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let full = parse_json(&["read", &fixture_arg, "--json"]);
    let anchor = format!("7:{}", full["lines"][6]["hash"].as_str().unwrap());
    let (stdout, stderr, code) =
        run_hashline(&["read", &fixture_arg, "--anchor", &anchor, "--context", "1"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("→7:"));
    assert!(stdout.contains(" 6:"));
    assert!(stdout.contains(" 8:"));
    assert!(!stdout.contains(" 1:"));
    assert!(!stdout.contains(" 9:"));
}

#[test]
fn index_pretty_output_shows_hashes_only() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["index", &fixture_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.lines().all(|line| !line.contains("|")));
    assert!(stdout.lines().all(|line| line.split(':').count() == 2));
}

#[test]
fn index_json_output_is_stable() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["index", &fixture_arg, "--json"]);

    assert_eq!(parsed["file"], fixture_arg);
    assert_eq!(parsed["lines"][0]["n"], 1);
    assert!(parsed["lines"][0]["hash"].is_string());
    assert!(parsed["lines"][0].get("content").is_none());
}

#[test]
fn invalid_anchor_still_errors_for_read_context() {
    assert_err_contains(
        &["read", "/definitely/missing/file.txt", "--anchor", "bogus"],
        "I/O error:",
    );
}

#[test]
fn read_binary_fixture_reports_binary_error_with_hint() {
    let fixture = fixture_path("binary.bin");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["read", &fixture_arg]);

    assert_eq!(code, 1);
    assert!(stderr.contains("appears to be binary and cannot be edited safely"));
    assert!(stderr.contains("hashline only supports UTF-8 text files"));
}

#[test]
fn verify_all_valid_anchors_exits_zero() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let full = parse_json(&["read", &fixture_arg, "--json"]);
    let anchor_a = format!("1:{}", full["lines"][0]["hash"].as_str().unwrap());
    let anchor_b = format!("7:{}", full["lines"][6]["hash"].as_str().unwrap());
    let (stdout, stderr, code) = run_hashline(&["verify", &fixture_arg, &anchor_a, &anchor_b]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("✓  1:"));
    assert!(stdout.contains("✓  7:"));
}

#[test]
fn verify_mixed_results_exit_nonzero() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let full = parse_json(&["read", &fixture_arg, "--json"]);
    let valid = format!("1:{}", full["lines"][0]["hash"].as_str().unwrap());
    let stale = "7:ff";
    let (stdout, stderr, code) = run_hashline(&["verify", &fixture_arg, &valid, stale]);

    assert_eq!(code, 1);
    assert!(stderr.is_empty());
    assert!(stdout.contains("✓  1:"));
    assert!(stdout.contains("✗  7:ff"));
    assert!(stdout.contains("expected hash ff"));
}

#[test]
fn verify_json_output_is_structured() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let full = parse_json(&["read", &fixture_arg, "--json"]);
    let valid = format!("1:{}", full["lines"][0]["hash"].as_str().unwrap());
    let (stdout, stderr, code) = run_hashline(&["verify", &fixture_arg, &valid, "bogus", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(code, 1);
    assert!(stderr.is_empty());
    assert!(parsed.is_array());
    assert_eq!(parsed[0]["status"], "ok");
    assert_eq!(parsed[0]["line_no"], 1);
    assert_eq!(parsed[1]["status"], "parse_error");
    assert!(parsed[1]["error"].is_string());
}

#[test]
fn verify_stale_anchor_with_unique_hash_succeeds_via_fuzzy_relocation() {
    // Phase 2: verify accepts anchors whose line+hash mismatch IF the hash
    // is unique elsewhere in the file (or within ±3 lines if collisions).
    // This matches the edit-side behavior so verify returns the same
    // outcome as the eventual edit.
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let full = parse_json(&["read", &file_arg, "--json"]);
    let moved_hash = full["lines"][0]["hash"].as_str().unwrap();
    let stale = format!("2:{moved_hash}");
    let (_stdout, stderr, code) = run_hashline(&["verify", &file_arg, &stale]);

    // Unique hash at line 1, requested line 2 → relocates → verify passes.
    assert_eq!(
        code, 0,
        "expected success after fuzzy relocation, stderr: {stderr}"
    );
}

#[test]
fn stats_json_includes_workflow_guidance_fields() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["stats", &fixture_arg, "--json"]);

    assert!(parsed["recommended_read_mode"].is_string());
    assert!(parsed["recommended_anchor_mode"].is_string());
    assert!(parsed["recommended_workflow"].is_string());
    assert!(parsed["warnings"].is_array());
}

#[test]
fn doctor_pretty_recommends_next_commands() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["doctor", &fixture_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Recommended read mode:"));
    assert!(stdout.contains("Recommended workflow:"));
    assert!(stdout.contains("Next commands:"));
    assert!(stdout.contains("hashline annotate"));
}

#[test]
fn doctor_json_is_machine_readable() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["doctor", &fixture_arg, "--json"]);

    assert_eq!(parsed["file"], fixture_arg);
    assert!(parsed["recommended_read_mode"].is_string());
    assert!(parsed["next_commands"].is_array());
}

#[test]
fn edit_single_line_updates_file_contents() {
    let edited = do_edit(
        "alpha\nbeta\n",
        &anchor_for_line("alpha\nbeta\n", 2),
        "gamma",
    );
    assert_eq!(edited, "alpha\ngamma\n");
}

#[test]
fn edit_single_line_preserves_newline_edges() {
    for (name, input, line, expected) in [
        (
            "lf trailing first",
            "alpha\nbeta\ngamma\n",
            1,
            "ALPHA\nbeta\ngamma\n",
        ),
        (
            "lf trailing middle",
            "alpha\nbeta\ngamma\n",
            2,
            "alpha\nBETA\ngamma\n",
        ),
        (
            "lf trailing last",
            "alpha\nbeta\ngamma\n",
            3,
            "alpha\nbeta\nGAMMA\n",
        ),
        (
            "lf no trailing first",
            "alpha\nbeta\ngamma",
            1,
            "ALPHA\nbeta\ngamma",
        ),
        (
            "lf no trailing middle",
            "alpha\nbeta\ngamma",
            2,
            "alpha\nBETA\ngamma",
        ),
        (
            "lf no trailing last",
            "alpha\nbeta\ngamma",
            3,
            "alpha\nbeta\nGAMMA",
        ),
        (
            "crlf trailing first",
            "alpha\r\nbeta\r\ngamma\r\n",
            1,
            "ALPHA\r\nbeta\r\ngamma\r\n",
        ),
        (
            "crlf trailing middle",
            "alpha\r\nbeta\r\ngamma\r\n",
            2,
            "alpha\r\nBETA\r\ngamma\r\n",
        ),
        (
            "crlf trailing last",
            "alpha\r\nbeta\r\ngamma\r\n",
            3,
            "alpha\r\nbeta\r\nGAMMA\r\n",
        ),
        (
            "crlf no trailing first",
            "alpha\r\nbeta\r\ngamma",
            1,
            "ALPHA\r\nbeta\r\ngamma",
        ),
        (
            "crlf no trailing middle",
            "alpha\r\nbeta\r\ngamma",
            2,
            "alpha\r\nBETA\r\ngamma",
        ),
        (
            "crlf no trailing last",
            "alpha\r\nbeta\r\ngamma",
            3,
            "alpha\r\nbeta\r\nGAMMA",
        ),
    ] {
        let file = tmpfile(input);
        let file_arg = file.to_string_lossy().into_owned();
        let anchor = anchor_from_file(&file_arg, line);
        let replacement = match line {
            1 => "ALPHA",
            2 => "BETA",
            3 => "GAMMA",
            _ => unreachable!(),
        };
        let (stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, replacement]);

        assert_eq!(code, 0, "{name}: expected success, stderr: {stderr}");
        assert!(
            stdout.contains(&format!("Edited line {line}.")),
            "{name}: unexpected stdout {stdout:?}"
        );
        assert_eq!(
            fs::read(&file).unwrap(),
            expected.as_bytes(),
            "{name}: edited bytes changed unexpectedly"
        );
    }
}

#[test]
fn edit_range_replaces_lines_with_single_line() {
    let content = "alpha\nbeta\ngamma\ndelta\n";
    let start = anchor_for_line(content, 2);
    let end = anchor_for_line(content, 3);
    let edited = do_edit(content, &format!("{start}..{end}"), "merged");
    assert_eq!(edited, "alpha\nmerged\ndelta\n");
}

#[test]
fn edit_range_replaces_lines_with_multiple_lines() {
    let content = "alpha\nbeta\ngamma\ndelta\n";
    let start = anchor_for_line(content, 2);
    let end = anchor_for_line(content, 3);
    let edited = do_edit(content, &format!("{start}..{end}"), "left\nmiddle\nright");
    assert_eq!(edited, "alpha\nleft\nmiddle\nright\ndelta\n");
}

#[test]
fn edit_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, "gamma", "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would change line 2:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn edit_json_dry_run_returns_mutation_receipt() {
    // PR-D: --dry-run --json now returns a compact mutation receipt, not the
    // entire proposed document.
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let parsed = parse_json(&["edit", &file_arg, &anchor, "gamma", "--dry-run", "--json"]);

    assert_eq!(parsed["op"], "edit");
    assert_eq!(parsed["dry_run"], true);
    assert_eq!(parsed["file"], file_arg);
    let changes = parsed["changes"].as_array().expect("changes is array");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["line_no"], 2);
    assert_eq!(changes[0]["kind"], "Modified");
    assert_eq!(changes[0]["before"], "beta");
    assert_eq!(changes[0]["after"], "gamma");
    // The full document should NOT be embedded in the receipt.
    assert!(parsed.get("lines").is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn edit_expect_mtime_rejects_stale_file() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_mtime = parsed["mtime"].as_i64().unwrap() - 1;
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--expect-mtime",
        &stale_mtime.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn edit_expect_inode_rejects_stale_file() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_inode = parsed["inode"].as_u64().unwrap() + 1;
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--expect-inode",
        &stale_inode.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn edit_accepts_matching_mtime_and_inode_guards() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--expect-mtime",
        &parsed["mtime"].as_i64().unwrap().to_string(),
        "--expect-inode",
        &parsed["inode"].as_u64().unwrap().to_string(),
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Edited line 2."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn edit_qualified_anchor_fuzzy_relocates_when_line_shifts() {
    // Phase 2: when the anchored hash exists elsewhere within ±3 lines
    // (or uniquely anywhere), edits silently relocate. This lets agents
    // survive small file drifts (formatter inserting blank lines, sibling
    // edits in the same batch, etc.) without re-reading.
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let stale_anchor = anchor_from_file(&file_arg, 2);
    // "beta" moves from line 2 to line 3 — anchor's hash is unique → relocate.
    fs::write(&file, "alpha\ngamma\nbeta\n").unwrap();

    let (_stdout, _stderr, code) = run_hashline(&["edit", &file_arg, &stale_anchor, "BETA"]);

    assert_eq!(code, 0);
    // The edit landed on the relocated line (the one with the matching hash).
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\nBETA\n");
}

#[test]
fn edit_ambiguous_hash_rejects_without_changing_file() {
    let (first, second) = find_collision_pair();
    let file = tmpfile(&format!("{first}\nunique\n{second}\n"));
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let ambiguous = parsed["lines"][0]["hash"].as_str().unwrap();

    let (_stdout, stderr, code) = run_hashline(&["edit", &file_arg, ambiguous, "updated"]);

    assert_eq!(code, 1);
    assert!(stderr.contains("matches 2 lines"));
    assert!(stderr.contains("use a line-qualified hash"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{first}\nunique\n{second}\n")
    );
}

#[test]
fn insert_after_anchor_updates_file_contents() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let (stdout, stderr, code) = run_hashline(&["insert", &file_arg, &anchor, "beta"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Inserted line 2.\n");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn insert_before_anchor_updates_file_contents() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&["insert", &file_arg, &anchor, "beta", "--before"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Inserted line 2.\n");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn insert_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let (stdout, stderr, code) = run_hashline(&["insert", &file_arg, &anchor, "beta", "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would insert line 2 after line 1:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn insert_json_dry_run_returns_mutation_receipt() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let parsed = parse_json(&["insert", &file_arg, &anchor, "beta", "--dry-run", "--json"]);

    assert_eq!(parsed["op"], "insert");
    assert_eq!(parsed["dry_run"], true);
    let changes = parsed["changes"].as_array().expect("changes is array");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["line_no"], 2);
    assert_eq!(changes[0]["kind"], "Inserted");
    assert_eq!(changes[0]["after"], "beta");
    assert!(parsed.get("lines").is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn insert_expect_mtime_rejects_stale_file() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_mtime = parsed["mtime"].as_i64().unwrap() - 1;
    let anchor = anchor_from_file(&file_arg, 1);
    let (_stdout, stderr, code) = run_hashline(&[
        "insert",
        &file_arg,
        &anchor,
        "beta",
        "--expect-mtime",
        &stale_mtime.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn insert_expect_inode_rejects_stale_file() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_inode = parsed["inode"].as_u64().unwrap() + 1;
    let anchor = anchor_from_file(&file_arg, 1);
    let (_stdout, stderr, code) = run_hashline(&[
        "insert",
        &file_arg,
        &anchor,
        "beta",
        "--expect-inode",
        &stale_inode.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn insert_accepts_matching_mtime_and_inode_guards() {
    let file = tmpfile("alpha\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let anchor = anchor_from_file(&file_arg, 1);
    let (stdout, stderr, code) = run_hashline(&[
        "insert",
        &file_arg,
        &anchor,
        "beta",
        "--expect-mtime",
        &parsed["mtime"].as_i64().unwrap().to_string(),
        "--expect-inode",
        &parsed["inode"].as_u64().unwrap().to_string(),
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Inserted line 2."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn insert_preserves_crlf_and_trailing_newline() {
    let file = tmpfile("alpha\r\ngamma\r\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let (_stdout, stderr, code) = run_hashline(&["insert", &file_arg, &anchor, "beta"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\r\nbeta\r\ngamma\r\n"
    );
}

#[test]
fn delete_removes_resolved_line() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&["delete", &file_arg, &anchor]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Deleted line 2.\n");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn delete_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&["delete", &file_arg, &anchor, "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would delete line 2:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn delete_json_dry_run_returns_mutation_receipt() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let parsed = parse_json(&["delete", &file_arg, &anchor, "--dry-run", "--json"]);

    assert_eq!(parsed["op"], "delete");
    assert_eq!(parsed["dry_run"], true);
    let changes = parsed["changes"].as_array().expect("changes is array");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["line_no"], 2);
    assert_eq!(changes[0]["kind"], "Deleted");
    assert_eq!(changes[0]["before"], "beta");
    assert!(parsed.get("lines").is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn delete_range_removes_resolved_lines() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let range = format!("{start}..{end}");
    let (stdout, stderr, code) = run_hashline(&["delete", &file_arg, &range]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Deleted lines 2-3.\n");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ndelta\n");
}

#[test]
fn delete_range_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let range = format!("{start}..{end}");
    let (stdout, stderr, code) = run_hashline(&["delete", &file_arg, &range, "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would delete lines 2-3:"));
    assert!(stdout.contains(r#"  - "beta""#));
    assert!(stdout.contains(r#"  - "gamma""#));
    assert!(stdout.contains("No file was written."));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nbeta\ngamma\ndelta\n"
    );
}

#[test]
fn delete_expect_mtime_rejects_stale_file() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_mtime = parsed["mtime"].as_i64().unwrap() - 1;
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "delete",
        &file_arg,
        &anchor,
        "--expect-mtime",
        &stale_mtime.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn delete_expect_inode_rejects_stale_file() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_inode = parsed["inode"].as_u64().unwrap() + 1;
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "delete",
        &file_arg,
        &anchor,
        "--expect-inode",
        &stale_inode.to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn delete_accepts_matching_mtime_and_inode_guards() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&[
        "delete",
        &file_arg,
        &anchor,
        "--expect-mtime",
        &parsed["mtime"].as_i64().unwrap().to_string(),
        "--expect-inode",
        &parsed["inode"].as_u64().unwrap().to_string(),
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Deleted line 2."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn delete_last_remaining_line_produces_empty_file() {
    let file = tmpfile("alpha");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let (_stdout, stderr, code) = run_hashline(&["delete", &file_arg, &anchor]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), "");
}

#[test]
fn edit_preserves_missing_trailing_newline() {
    let file = tmpfile("alpha\nbeta");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, "gamma"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(fs::read(&file).unwrap(), b"alpha\ngamma");
}

#[test]
fn swap_exchanges_two_lines() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor_a = anchor_from_file(&file_arg, 2);
    let anchor_b = anchor_from_file(&file_arg, 4);
    let (stdout, stderr, code) = run_hashline(&["swap", &file_arg, &anchor_a, &anchor_b]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Swapped lines 2 and 4.\n");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\ndelta\ngamma\nbeta\n"
    );
}

#[test]
fn swap_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor_a = anchor_from_file(&file_arg, 1);
    let anchor_b = anchor_from_file(&file_arg, 3);
    let (stdout, stderr, code) =
        run_hashline(&["swap", &file_arg, &anchor_a, &anchor_b, "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would swap line 1 with line 3:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nbeta\ngamma\ndelta\n"
    );
}

#[test]
fn swap_round_trips_back_to_original_bytes() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let original = fs::read(&file).unwrap();

    let anchor_a = anchor_from_file(&file_arg, 2);
    let anchor_b = anchor_from_file(&file_arg, 4);
    let (_stdout, stderr, code) = run_hashline(&["swap", &file_arg, &anchor_a, &anchor_b]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");

    let anchor_a = anchor_from_file(&file_arg, 2);
    let anchor_b = anchor_from_file(&file_arg, 4);
    let (_stdout, stderr, code) = run_hashline(&["swap", &file_arg, &anchor_a, &anchor_b]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");

    assert_eq!(fs::read(&file).unwrap(), original);
}

#[test]
fn swap_rejects_same_line() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&["swap", &file_arg, &anchor, &anchor]);

    assert_eq!(code, 1);
    assert!(stderr.contains("source and target must resolve to different lines"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[cfg(unix)]
#[test]
fn edit_preserves_existing_file_permissions() {
    let file = tmpfile("alpha\nbeta\n");
    chmod(&file, 0o640);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);

    let (_stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, "gamma"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
    assert_eq!(mode(&file), 0o640);
}

#[cfg(unix)]
#[test]
fn delete_to_empty_file_preserves_existing_permissions() {
    let file = tmpfile("alpha");
    chmod(&file, 0o600);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);

    let (_stdout, stderr, code) = run_hashline(&["delete", &file_arg, &anchor]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), "");
    assert_eq!(mode(&file), 0o600);
}

#[test]
fn patch_applies_edit_insert_and_delete_atomically() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let insert_anchor = anchor_from_file(&file_arg, 2);
    let delete_anchor = anchor_from_file(&file_arg, 4);
    let patch_file = tmpfile(&format!(
        "{{\n  \"file\": {:?},\n  \"ops\": [\n    {{ \"op\": \"edit\", \"anchor\": {:?}, \"content\": \"BETA\" }},\n    {{ \"op\": \"insert\", \"anchor\": {:?}, \"content\": \"between\" }},\n    {{ \"op\": \"delete\", \"anchor\": {:?} }}\n  ]\n}}\n",
        file_arg, edit_anchor, insert_anchor, delete_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Applied 3 ops: 1 edit, 1 insert, 1 delete."));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nBETA\nbetween\ngamma\n"
    );
}

#[test]
fn patch_dry_run_does_not_modify_file() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"BETA\"}}]}}",
        edit_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg, "--dry-run"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would apply 1 ops: 1 edit, 0 inserts, 0 deletes."));
    assert!(stdout.contains("No file was written."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn patch_json_dry_run_returns_mutation_receipt() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"BETA\"}}]}}",
        edit_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let parsed = parse_json(&["patch", &file_arg, &patch_arg, "--dry-run", "--json"]);

    assert_eq!(parsed["op"], "patch");
    assert_eq!(parsed["dry_run"], true);
    let changes = parsed["changes"].as_array().expect("changes is array");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["line_no"], 2);
    assert_eq!(changes[0]["kind"], "Modified");
    assert_eq!(changes[0]["before"], "beta");
    assert_eq!(changes[0]["after"], "BETA");
    assert!(parsed.get("lines").is_none());
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn patch_respects_matching_guards() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"gamma\"}}]}}",
        edit_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&[
        "patch",
        &file_arg,
        &patch_arg,
        "--expect-mtime",
        &parsed["mtime"].as_i64().unwrap().to_string(),
        "--expect-inode",
        &parsed["inode"].as_u64().unwrap().to_string(),
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Applied 1 ops: 1 edit, 0 inserts, 0 deletes."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn patch_rejects_stale_guard_without_writing() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"gamma\"}}]}}",
        edit_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&[
        "patch",
        &file_arg,
        &patch_arg,
        "--expect-mtime",
        &(parsed["mtime"].as_i64().unwrap() - 1).to_string(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("changed since the last read"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn patch_rejects_bad_anchor_without_writing() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let patch_file = tmpfile("{\"ops\":[{\"op\":\"delete\",\"anchor\":\"9:ff\"}]}");
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 1);
    assert!(stderr.contains("patch failed at operation 1"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn patch_reports_failing_operation_index() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"insert\",\"anchor\":{:?},\"content\":\"ok\"}},{{\"op\":\"delete\",\"anchor\":\"9:ff\"}}]}}",
        anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 1);
    assert!(stderr.contains("patch failed at operation 2"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\ngamma\n");
}

#[test]
fn patch_rejects_overlapping_operations_without_writing() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let range_start = anchor_from_file(&file_arg, 2);
    let range_end = anchor_from_file(&file_arg, 3);
    let delete_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"merged\"}},{{\"op\":\"delete\",\"anchor\":{:?}}}]}}",
        format!("{range_start}..{range_end}"),
        delete_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 1);
    assert!(stderr.contains("overlaps an earlier edit"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nbeta\ngamma\ndelta\n"
    );
}

#[test]
fn patch_rejects_mismatched_embedded_file_without_writing() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"file\":\"/definitely/other.txt\",\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"gamma\"}}]}}",
        anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 1);
    assert!(stderr.contains("operation 0"));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[test]
fn patch_uses_original_snapshot_for_later_ops() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let first_anchor = anchor_from_file(&file_arg, 1);
    let second_anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"insert\",\"anchor\":{:?},\"content\":\"before-beta\"}},{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"BETA\"}}]}}",
        first_anchor, second_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nbefore-beta\nBETA\ngamma\n"
    );
}

#[test]
fn patch_multiple_inserts_at_same_anchor_preserve_order() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"insert\",\"anchor\":{:?},\"content\":\"first\"}},{{\"op\":\"insert\",\"anchor\":{:?},\"content\":\"second\"}}]}}",
        anchor, anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nfirst\nsecond\nbeta\n"
    );
}

#[test]
fn patch_preserves_crlf_and_trailing_newline() {
    let file = tmpfile("alpha\r\nbeta\r\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"gamma\"}}]}}",
        anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&["patch", &file_arg, &patch_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\r\ngamma\r\n");
}

#[test]
fn edit_receipt_prints_json_and_updates_file() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, "gamma", "--receipt"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["op"], "edit");
    assert_eq!(parsed["file"], file_arg);
    assert_eq!(parsed["changes"][0]["line_no"], 2);
    assert_eq!(parsed["changes"][0]["kind"], "Modified");
    assert_eq!(parsed["changes"][0]["before"], "beta");
    assert_eq!(parsed["changes"][0]["after"], "gamma");
    assert_ne!(parsed["file_hash_before"], parsed["file_hash_after"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
}

#[test]
fn edit_range_receipt_reports_modified_and_inserted_lines() {
    let file = tmpfile("alpha\nbeta\ngamma\ndelta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let range = format!("{start}..{end}");
    let parsed = parse_json(&[
        "edit",
        &file_arg,
        &range,
        "left\nmiddle\nright",
        "--receipt",
    ]);

    assert_eq!(parsed["op"], "edit");
    assert_eq!(parsed["changes"][0]["line_no"], 2);
    assert_eq!(parsed["changes"][0]["kind"], "Modified");
    assert_eq!(parsed["changes"][0]["before"], "beta");
    assert_eq!(parsed["changes"][0]["after"], "left");
    assert_eq!(parsed["changes"][1]["line_no"], 3);
    assert_eq!(parsed["changes"][1]["kind"], "Modified");
    assert_eq!(parsed["changes"][1]["before"], "gamma");
    assert_eq!(parsed["changes"][1]["after"], "middle");
    assert_eq!(parsed["changes"][2]["line_no"], 4);
    assert_eq!(parsed["changes"][2]["kind"], "Inserted");
    assert_eq!(parsed["changes"][2]["before"], serde_json::Value::Null);
    assert_eq!(parsed["changes"][2]["after"], "right");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nleft\nmiddle\nright\ndelta\n"
    );
}

#[test]
fn insert_receipt_reports_inserted_line() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 1);
    let parsed = parse_json(&["insert", &file_arg, &anchor, "gamma", "--receipt"]);

    assert_eq!(parsed["op"], "insert");
    assert_eq!(parsed["changes"][0]["line_no"], 2);
    assert_eq!(parsed["changes"][0]["kind"], "Inserted");
    assert_eq!(parsed["changes"][0]["before"], serde_json::Value::Null);
    assert_eq!(parsed["changes"][0]["after"], "gamma");
}

#[test]
fn delete_receipt_reports_deleted_line() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let parsed = parse_json(&["delete", &file_arg, &anchor, "--receipt"]);

    assert_eq!(parsed["op"], "delete");
    assert_eq!(parsed["changes"][0]["line_no"], 2);
    assert_eq!(parsed["changes"][0]["kind"], "Deleted");
    assert_eq!(parsed["changes"][0]["before"], "beta");
    assert_eq!(parsed["changes"][0]["after"], serde_json::Value::Null);
}

#[test]
fn patch_receipt_contains_multiple_structured_changes() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let edit_anchor = anchor_from_file(&file_arg, 2);
    let delete_anchor = anchor_from_file(&file_arg, 3);
    let patch_file = tmpfile(&format!(
        "{{\"ops\":[{{\"op\":\"edit\",\"anchor\":{:?},\"content\":\"BETA\"}},{{\"op\":\"insert\",\"anchor\":{:?},\"content\":\"between\"}},{{\"op\":\"delete\",\"anchor\":{:?}}}]}}",
        edit_anchor, edit_anchor, delete_anchor
    ));
    let patch_arg = patch_file.to_string_lossy().into_owned();
    let parsed = parse_json(&["patch", &file_arg, &patch_arg, "--receipt"]);

    assert_eq!(parsed["op"], "patch");
    assert!(parsed["changes"].as_array().unwrap().len() >= 3);
    assert_eq!(parsed["changes"][0]["kind"], "Modified");
    assert_eq!(parsed["changes"][1]["kind"], "Inserted");
    assert_eq!(parsed["changes"][2]["kind"], "Deleted");
}

#[test]
fn audit_log_appends_on_success() {
    let file = tmpfile("alpha\nbeta\n");
    let audit = tmpfile("");
    let file_arg = file.to_string_lossy().into_owned();
    let audit_arg = audit.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--audit-log",
        &audit_arg,
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    let contents = fs::read_to_string(&audit).unwrap();
    let lines = contents.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed["op"], "edit");
}

#[test]
fn audit_log_appends_two_entries_without_truncation() {
    let file = tmpfile("alpha\nbeta\n");
    let audit = tmpfile("");
    let file_arg = file.to_string_lossy().into_owned();
    let audit_arg = audit.to_string_lossy().into_owned();

    let first_anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &first_anchor,
        "gamma",
        "--audit-log",
        &audit_arg,
    ]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");

    let second_anchor = anchor_from_file(&file_arg, 2);
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &second_anchor,
        "delta",
        "--audit-log",
        &audit_arg,
    ]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");

    let contents = fs::read_to_string(&audit).unwrap();
    let lines = contents.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first["changes"][0]["after"], "gamma");
    assert_eq!(second["changes"][0]["after"], "delta");
}

#[test]
fn failed_edit_does_not_append_audit_log() {
    let file = tmpfile("alpha\nbeta\n");
    let audit = tmpfile("");
    let file_arg = file.to_string_lossy().into_owned();
    let audit_arg = audit.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        "2:ff",
        "gamma",
        "--audit-log",
        &audit_arg,
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("expected hash ff"));
    assert_eq!(fs::read_to_string(&audit).unwrap(), "");
}

#[test]
fn dry_run_does_not_append_audit_log_or_emit_receipt() {
    let file = tmpfile("alpha\nbeta\n");
    let audit = tmpfile("");
    let file_arg = file.to_string_lossy().into_owned();
    let audit_arg = audit.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let (stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--dry-run",
        "--receipt",
        "--audit-log",
        &audit_arg,
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stdout.contains("Would change line 2:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
    assert_eq!(fs::read_to_string(&audit).unwrap(), "");
}

#[test]
fn audit_log_append_failure_warns_but_edit_succeeds() {
    let file = tmpfile("alpha\nbeta\n");
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 2);
    let audit_dir = tempfile::TempDir::new().unwrap();
    let audit_arg = audit_dir.path().to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "gamma",
        "--audit-log",
        &audit_arg,
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\ngamma\n");
    assert!(stderr.contains("Warning: wrote file but failed to append audit log"));
}

#[test]
fn indent_command_updates_file_contents() {
    let file = tmpfile("alpha\n  beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let (stdout, stderr, code) =
        run_hashline(&["indent", &file_arg, &format!("{start}..{end}"), "+2"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_eq!(stdout, "Indented lines 2-3 by 2 spaces.\n");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n    beta\n    gamma\n"
    );
}

#[test]
fn indent_dedent_round_trips_back_to_original_bytes() {
    let file = tmpfile("alpha\n  beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);

    let (_stdout, stderr, code) =
        run_hashline(&["indent", &file_arg, &format!("{start}..{end}"), "+2"]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");

    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let (_stdout, stderr, code) =
        run_hashline(&["indent", &file_arg, &format!("{start}..{end}"), "-2"]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n  beta\n  gamma\n"
    );
}

#[test]
fn indent_dry_run_reports_change_without_writing_file() {
    let file = tmpfile("alpha\n  beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let (stdout, stderr, code) = run_hashline(&[
        "indent",
        &file_arg,
        &format!("{start}..{end}"),
        "+2",
        "--dry-run",
    ]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Would indent lines 2-3 by 2 spaces:"));
    assert!(stdout.contains("No file was written."));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n  beta\n  gamma\n"
    );
}

#[test]
fn indent_json_dry_run_returns_mutation_receipt() {
    let file = tmpfile("alpha\n  beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let parsed = parse_json(&[
        "indent",
        &file_arg,
        &format!("{start}..{end}"),
        "+2",
        "--dry-run",
        "--json",
    ]);

    assert_eq!(parsed["op"], "indent");
    assert_eq!(parsed["dry_run"], true);
    let changes = parsed["changes"].as_array().expect("changes is array");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["after"], "    beta");
    assert_eq!(changes[1]["after"], "    gamma");
    assert!(parsed.get("lines").is_none());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n  beta\n  gamma\n"
    );
}

#[test]
fn indent_rejects_mixed_indentation_in_range() {
    let file = tmpfile("alpha\n  beta\n\tgamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let (_stdout, stderr, code) =
        run_hashline(&["indent", &file_arg, &format!("{start}..{end}"), "+2"]);

    assert_eq!(code, 1);
    assert!(stderr.contains("mixed indentation styles"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n  beta\n\tgamma\n"
    );
}

#[test]
fn indent_dedent_rejects_underflow_and_names_line() {
    let file = tmpfile("alpha\n beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let (_stdout, stderr, code) =
        run_hashline(&["indent", &file_arg, &format!("{start}..{end}"), "-2"]);

    assert_eq!(code, 1);
    assert!(stderr.contains("dedent by 2 would underflow line 2"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\n beta\n  gamma\n"
    );
}

#[test]
fn indent_receipt_reports_modified_lines() {
    let file = tmpfile("alpha\n  beta\n  gamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let start = anchor_from_file(&file_arg, 2);
    let end = anchor_from_file(&file_arg, 3);
    let parsed = parse_json(&[
        "indent",
        &file_arg,
        &format!("{start}..{end}"),
        "+2",
        "--receipt",
    ]);

    assert_eq!(parsed["op"], "indent");
    assert_eq!(parsed["changes"][0]["kind"], "Modified");
    assert_eq!(parsed["changes"][0]["line_no"], 2);
    assert_eq!(parsed["changes"][0]["after"], "    beta");
    assert_eq!(parsed["changes"][1]["line_no"], 3);
}

#[test]
fn stats_pretty_output_reports_summary_fields() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["stats", &file_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert!(stdout.contains("Lines: 3"));
    assert!(stdout.contains("Unique hashes (2-char):"));
    assert!(stdout.contains("Collisions:"));
    assert!(stdout.contains("Est. read tokens:"));
    assert!(stdout.contains("Hash length advice:"));
    assert!(stdout.contains("Suggested --context:"));
}

#[test]
fn stats_json_output_is_structured() {
    let file = tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["stats", &file_arg, "--json"]);

    assert_eq!(parsed["line_count"], 3);
    assert!(parsed["unique_hashes"].is_u64());
    assert!(parsed["collision_count"].is_u64());
    assert!(parsed["collision_pairs"].is_array());
    assert!(parsed["estimated_read_tokens"].is_u64());
    assert!(parsed["hash_length_advice"].is_u64());
    assert!(parsed["suggested_context_n"].is_u64());
}

#[test]
fn stats_reports_collision_pairs_for_collision_file() {
    let (first, second) = find_collision_pair();
    let file = tmpfile(&format!("{first}\n{second}\nunique\n"));
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["stats", &file_arg, "--json"]);

    assert_eq!(parsed["collision_count"], 2);
    assert_eq!(parsed["collision_pairs"][0][0], 1);
    assert_eq!(parsed["collision_pairs"][0][1], 2);
}

#[test]
fn helper_tmpfile_writes_expected_content() {
    let file = tmpfile("alpha\nbeta\n");
    let contents = std::fs::read_to_string(&file).unwrap();
    assert_eq!(contents, "alpha\nbeta\n");
}

fn anchor_for_line(content: &str, line_no: usize) -> String {
    let file = tmpfile(content);
    let file_arg = file.to_string_lossy().into_owned();
    anchor_from_file(&file_arg, line_no)
}

fn anchor_from_file(file_arg: &str, line_no: usize) -> String {
    let parsed = parse_json(&["read", file_arg, "--json"]);
    format!(
        "{}:{}",
        line_no,
        parsed["lines"][line_no - 1]["hash"].as_str().unwrap()
    )
}

fn find_collision_pair() -> (String, String) {
    ("line-1612".to_owned(), "line-2126".to_owned())
}

// ============================================================================
// Regression: atomic_write must succeed when called with a bare relative path
// (no directory component). `Path::new("sample.js").parent()` returns
// `Some(Path::new(""))`, not `None`, so the previous `unwrap_or` fallback to
// "." never fired and the post-write `sync_parent_directory("")` failed with
// ENOENT — even though the file had already been written.
// ============================================================================

#[test]
fn edit_succeeds_with_bare_relative_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sample.txt");
    fs::write(&path, "alpha\ngamma\n").unwrap();

    let parsed = parse_json(&["read", path.to_str().unwrap(), "--json"]);
    let anchor = format!("1:{}", parsed["lines"][0]["hash"].as_str().unwrap());

    let (stdout, stderr, code) =
        run_hashline_in(dir.path(), &["edit", "sample.txt", &anchor, "beta"]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty(), "expected no stderr, got: {stderr:?}");
    assert!(
        stdout.contains("Edited line"),
        "expected success message, got: {stdout:?}"
    );

    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "beta\ngamma\n");
}

#[test]
fn insert_succeeds_with_bare_relative_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sample.txt");
    fs::write(&path, "alpha\ngamma\n").unwrap();

    let parsed = parse_json(&["read", path.to_str().unwrap(), "--json"]);
    let anchor = format!("1:{}", parsed["lines"][0]["hash"].as_str().unwrap());

    let (stdout, stderr, code) =
        run_hashline_in(dir.path(), &["insert", "sample.txt", &anchor, "beta"]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty(), "expected no stderr, got: {stderr:?}");
    assert!(
        stdout.contains("Inserted line"),
        "expected success message, got: {stdout:?}"
    );

    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "alpha\nbeta\ngamma\n");
}

#[test]
fn delete_succeeds_with_bare_relative_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sample.txt");
    fs::write(&path, "alpha\nbeta\ngamma\n").unwrap();

    let parsed = parse_json(&["read", path.to_str().unwrap(), "--json"]);
    let anchor = format!("2:{}", parsed["lines"][1]["hash"].as_str().unwrap());

    let (stdout, stderr, code) = run_hashline_in(dir.path(), &["delete", "sample.txt", &anchor]);
    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty(), "expected no stderr, got: {stderr:?}");
    assert!(
        stdout.contains("Deleted line"),
        "expected success message, got: {stdout:?}"
    );

    let contents = fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "alpha\ngamma\n");
}

// ============================================================================
// PR-C: --json defaults to compact, --pretty opts in to pretty-printed JSON
// ============================================================================

#[test]
fn read_json_default_is_compact() {
    // PR-C: --json without --pretty produces compact (single-line) JSON.
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["read", &fixture_arg, "--json"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    let trimmed = stdout.trim_end_matches('\n');
    // Compact JSON has no embedded newlines.
    assert!(
        !trimmed.contains('\n'),
        "compact JSON should be a single line, got {} newlines",
        trimmed.matches('\n').count()
    );
    // And no two-space pretty-printed indentation.
    assert!(!stdout.contains("\n  \""));
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["lines"].is_array());
}

#[test]
fn read_json_pretty_flag_enables_pretty_printing() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["read", &fixture_arg, "--json", "--pretty"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    // Pretty JSON spans multiple lines with two-space indentation on top-level keys.
    assert!(stdout.contains("\n  \"file\""));
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(parsed["lines"].is_array());
}

#[test]
fn index_json_default_is_compact() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, _stderr, code) = run_hashline(&["index", &fixture_arg, "--json"]);
    assert_eq!(code, 0);
    let trimmed = stdout.trim_end_matches('\n');
    assert!(!trimmed.contains('\n'));
}

#[test]
fn stats_json_default_is_compact() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, _stderr, code) = run_hashline(&["stats", &fixture_arg, "--json"]);
    assert_eq!(code, 0);
    let trimmed = stdout.trim_end_matches('\n');
    assert!(!trimmed.contains('\n'));
}

// ============================================================================
// PR-D: --dry-run --json returns compact mutation receipt, not full document
// ============================================================================

#[test]
fn dry_run_json_does_not_dump_full_file() {
    // PR-D: dry-run JSON output should be O(edit size), not O(file size).
    // Build a 200-line file and verify the dry-run JSON for a single-line edit
    // is dramatically smaller than the file content.
    let mut content = String::new();
    for i in 1..=200 {
        content.push_str(&format!("line {}\n", i));
    }
    let file = tmpfile(&content);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_from_file(&file_arg, 100);
    let (stdout, _stderr, code) = run_hashline(&[
        "edit",
        &file_arg,
        &anchor,
        "REPLACED",
        "--dry-run",
        "--json",
    ]);

    assert_eq!(code, 0);
    // Receipt for a 1-line edit should be much smaller than the original file.
    assert!(
        stdout.len() < content.len() / 2,
        "dry-run receipt ({} bytes) should be much smaller than file ({} bytes)",
        stdout.len(),
        content.len()
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["op"], "edit");
    assert_eq!(parsed["dry_run"], true);
    assert!(parsed.get("lines").is_none());
    let changes = parsed["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["after"], "REPLACED");
}

// ============================================================================
// Tier 1 NDJSON: read/index/grep/annotate streaming output
// ============================================================================

#[test]
fn read_ndjson_emits_header_and_line_per_line() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_hashline(&["read", &fixture_arg, "--ndjson"]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() > 1);
    let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(header["event"], "header");
    assert!(header["total_lines"].as_u64().unwrap() > 0);
    assert!(header["file"].is_string());

    // Every subsequent line is its own valid JSON object with n/hash/content.
    for raw in &lines[1..] {
        let line: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert!(line["n"].is_number(), "line missing 'n' field: {raw}");
        assert!(line["hash"].is_string(), "line missing 'hash' field: {raw}");
        assert!(line["content"].is_string(), "line missing 'content': {raw}");
    }
}

#[test]
fn index_ndjson_emits_header_and_anchors_only() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, _stderr, code) = run_hashline(&["index", &fixture_arg, "--ndjson"]);

    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(header["event"], "header");

    // Index NDJSON does not include line content.
    for raw in &lines[1..] {
        let line: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert!(line.get("content").is_none());
        assert!(line["n"].is_number());
        assert!(line["hash"].is_string());
    }
}

#[test]
fn ndjson_takes_precedence_over_json() {
    // When both --json and --ndjson are passed, --ndjson wins.
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, _stderr, code) =
        run_hashline(&["read", &fixture_arg, "--json", "--pretty", "--ndjson"]);

    assert_eq!(code, 0);
    // First line should parse as a JSON object (NDJSON header), not a fragment
    // of a pretty document.
    let first_line = stdout.lines().next().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(first_line).unwrap();
    assert_eq!(parsed["event"], "header");
}

#[test]
fn edit_interpret_escapes_expands_newline_into_multiple_lines() {
    let content = "alpha\nbeta\ngamma\n";
    let file = tmpfile(content);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_for_line(content, 2);
    let (stdout, stderr, code) =
        run_hashline(&["edit", "-e", &file_arg, &anchor, "BETA-1\\nBETA-2"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.starts_with("Edited line") || stdout.starts_with("Edited lines"),
        "stdout: {stdout:?}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nBETA-1\nBETA-2\ngamma\n",
    );
}

#[test]
fn edit_without_interpret_escapes_leaves_backslash_n_literal() {
    let content = "alpha\nbeta\ngamma\n";
    let file = tmpfile(content);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_for_line(content, 2);
    let (_stdout, stderr, code) = run_hashline(&["edit", &file_arg, &anchor, "BETA-1\\nBETA-2"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "alpha\nBETA-1\\nBETA-2\ngamma\n",
    );
}

#[test]
fn insert_interpret_escapes_expands_newline() {
    let content = "alpha\nbeta\n";
    let file = tmpfile(content);
    let file_arg = file.to_string_lossy().into_owned();
    let anchor = anchor_for_line(content, 1);
    let (_stdout, stderr, code) =
        run_hashline(&["insert", "--interpret-escapes", &file_arg, &anchor, "x\\ny"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nx\ny\nbeta\n");
}
