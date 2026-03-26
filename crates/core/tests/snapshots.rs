mod support;

use insta::{assert_json_snapshot, assert_snapshot};
use serde_json::Value;
use support::{fixture_path, parse_json, run_linehash};

fn normalize_path(input: &str, path: &str, replacement: &str) -> String {
    input.replace(path, replacement)
}

fn normalize_read_json(mut value: Value, fixture_arg: &str) -> Value {
    value["file"] = Value::String("<fixture>".into());
    value["mtime"] = Value::from(0);
    value["mtime_nanos"] = Value::from(0);
    value["inode"] = Value::from(0);

    if value["file"] == fixture_arg {
        value["file"] = Value::String("<fixture>".into());
    }

    value["newline"] = Value::String("lf".into());

    value
}

#[test]
fn snapshot_read_pretty_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_linehash(&["read", &fixture_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_snapshot!("read_pretty_simple_lf", stdout);
}

#[test]
fn snapshot_index_pretty_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (stdout, stderr, code) = run_linehash(&["index", &fixture_arg]);

    assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    assert!(stderr.is_empty());
    assert_snapshot!("index_pretty_simple_lf", stdout);
}

#[test]
fn snapshot_read_json_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = normalize_read_json(parse_json(&["read", &fixture_arg, "--json"]), &fixture_arg);

    assert_json_snapshot!("read_json_simple_lf", parsed);
}

#[test]
fn snapshot_index_json_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["index", &fixture_arg, "--json"]);

    assert_json_snapshot!("index_json_simple_lf", parsed, {
        ".file" => "<fixture>"
    });
}

#[test]
fn snapshot_stats_json_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let parsed = parse_json(&["stats", &fixture_arg, "--json"]);

    assert_json_snapshot!("stats_json_simple_lf", parsed);
}

#[test]
fn snapshot_doctor_json_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let mut parsed = parse_json(&["doctor", &fixture_arg, "--json"]);
    parsed["file"] = Value::String("<fixture>".into());
    if let Some(commands) = parsed["next_commands"].as_array_mut() {
        for command in commands {
            if let Some(value) = command.as_str() {
                *command = Value::String(value.replace(&fixture_arg, "<fixture>"));
            }
        }
    }

    assert_json_snapshot!("doctor_json_simple_lf", parsed);
}

#[test]
fn snapshot_verify_json_output() {
    let fixture = fixture_path("simple_lf.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let full = parse_json(&["read", &fixture_arg, "--json"]);
    let valid = format!("1:{}", full["lines"][0]["hash"].as_str().unwrap());
    let (stdout, stderr, code) = run_linehash(&["verify", &fixture_arg, &valid, "bogus", "--json"]);
    let parsed: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(code, 1);
    assert!(stderr.is_empty());
    assert_json_snapshot!("verify_json_mixed_results", parsed);
}

#[test]
fn snapshot_binary_file_error_output() {
    let fixture = fixture_path("binary.bin");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_linehash(&["read", &fixture_arg]);

    assert_eq!(code, 1);
    let normalized = normalize_path(&stderr, &fixture_arg, "<fixture>");
    assert_snapshot!("read_binary_error", normalized);
}

#[test]
fn snapshot_mixed_newline_error_output() {
    let fixture = fixture_path("mixed_newlines.js");
    let fixture_arg = fixture.to_string_lossy().into_owned();
    let (_stdout, stderr, code) = run_linehash(&["read", &fixture_arg]);

    assert_eq!(code, 1);
    let normalized = normalize_path(&stderr, &fixture_arg, "<fixture>");
    assert_snapshot!("read_mixed_newlines_error", normalized);
}

#[test]
fn snapshot_stale_anchor_error_output() {
    let file = support::tmpfile("alpha\nbeta\ngamma\n");
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let stale_anchor = format!("2:{}", parsed["lines"][1]["hash"].as_str().unwrap());
    std::fs::write(&file, "alpha\ngamma\nbeta\n").unwrap();

    let (_stdout, stderr, code) = run_linehash(&["edit", &file_arg, &stale_anchor, "BETA"]);

    assert_eq!(code, 1);
    let normalized = normalize_path(&stderr, &file_arg, "<fixture>");
    assert_snapshot!("edit_stale_anchor_error", normalized);
}

#[test]
fn snapshot_ambiguous_hash_error_output() {
    let (first, second) = find_collision_pair();
    let file = support::tmpfile(&format!("{first}\nunique\n{second}\n"));
    let file_arg = file.to_string_lossy().into_owned();
    let parsed = parse_json(&["read", &file_arg, "--json"]);
    let ambiguous = parsed["lines"][0]["hash"].as_str().unwrap();

    let (_stdout, stderr, code) = run_linehash(&["edit", &file_arg, ambiguous, "updated"]);

    assert_eq!(code, 1);
    let normalized = normalize_path(&stderr, &file_arg, "<fixture>");
    assert_snapshot!("edit_ambiguous_hash_error", normalized);
}

fn find_collision_pair() -> (String, String) {
    use std::collections::HashMap;

    let mut seen: HashMap<String, String> = HashMap::new();
    for i in 0..10_000 {
        let candidate = format!("line-{i}");
        let tmp = support::tmpfile(&candidate);
        let tmp_arg = tmp.to_string_lossy().into_owned();
        let parsed = parse_json(&["read", &tmp_arg, "--json"]);
        let short = parsed["lines"][0]["hash"].as_str().unwrap().to_owned();
        if let Some(existing) = seen.insert(short, candidate.clone()) {
            if existing != candidate {
                return (existing, candidate);
            }
        }
    }
    panic!("failed to find ambiguous short-hash fixture");
}
