mod support;

use std::time::{Duration, Instant};

use support::{parse_json, run_hashline, tmpfile};

const PERF_ENV: &str = "HASHLINE_RUN_PERF";
const RUNS: usize = 5;

#[test]
fn hash_read_json_10k_lines_stays_within_envelope() {
    if std::env::var_os(PERF_ENV).is_none() {
        eprintln!("skipping performance test; set {PERF_ENV}=1 to enable");
        return;
    }

    let file = tmpfile(&generate_fixture(10_000));
    let file_arg = file.to_string_lossy().into_owned();
    let best = best_duration(|| {
        let (_stdout, stderr, code) = run_hashline(&["read", &file_arg, "--json"]);
        assert_eq!(code, 0, "expected success, got stderr: {stderr}");
    });

    assert!(
        best <= Duration::from_millis(80),
        "expected read --json best-of-{RUNS} to stay under 80ms, got {:?}",
        best
    );
}

#[test]
fn stats_json_10k_lines_stays_within_envelope() {
    if std::env::var_os(PERF_ENV).is_none() {
        eprintln!("skipping performance test; set {PERF_ENV}=1 to enable");
        return;
    }

    let file = tmpfile(&generate_fixture(10_000));
    let file_arg = file.to_string_lossy().into_owned();
    let best = best_duration(|| {
        let parsed = parse_json(&["stats", &file_arg, "--json"]);
        assert_eq!(parsed["line_count"], 10_000);
    });

    assert!(
        best <= Duration::from_millis(120),
        "expected stats --json best-of-{RUNS} to stay under 120ms, got {:?}",
        best
    );
}

#[test]
fn verify_json_100_anchors_stays_within_envelope() {
    if std::env::var_os(PERF_ENV).is_none() {
        eprintln!("skipping performance test; set {PERF_ENV}=1 to enable");
        return;
    }

    let file = tmpfile(&generate_fixture(10_000));
    let file_arg = file.to_string_lossy().into_owned();
    let read = parse_json(&["read", &file_arg, "--json"]);
    let anchor_values = read["lines"]
        .as_array()
        .expect("read lines array")
        .iter()
        .take(100)
        .map(|line| {
            format!(
                "{}:{}",
                line["n"].as_u64().unwrap(),
                line["hash"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    let anchor_refs = anchor_values
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>();

    let best = best_duration(|| {
        let mut args = vec!["verify", file_arg.as_str()];
        args.extend(anchor_refs.iter().copied());
        let (stdout, stderr, code) = run_hashline(&args);
        assert_eq!(
            code, 0,
            "expected success, got stderr: {stderr}, stdout: {stdout}"
        );
    });

    assert!(
        best <= Duration::from_millis(120),
        "expected verify 100 anchors best-of-{RUNS} to stay under 120ms, got {:?}",
        best
    );
}

fn best_duration(mut f: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..RUNS {
        let start = Instant::now();
        f();
        best = best.min(start.elapsed());
    }
    best
}

fn generate_fixture(line_count: usize) -> String {
    let mut lines = Vec::with_capacity(line_count);
    for i in 0..line_count {
        lines.push(format!(
            "fn generated_line_{i:05}() {{ let value = \"{:08x}\"; }}",
            i.wrapping_mul(2654435761_u32 as usize)
        ));
    }
    lines.join("\n") + "\n"
}
