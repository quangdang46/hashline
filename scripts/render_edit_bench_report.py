#!/usr/bin/env python3
from __future__ import annotations

import json
import subprocess
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRITERION_DIR = ROOT / "target" / "criterion"
REPORT_DIR = ROOT / "bench-results"
REPORT_PATH = REPORT_DIR / "edit_bench.md"
NOW = datetime.now()
SNAPSHOT_PATH = REPORT_DIR / f"bench-{NOW.strftime('%Y-%m-%d-%H-%M-%S')}.md"
COMMAND = "cargo bench -p linehash --bench edit_bench"

SECTIONS = [
    (
        "End-to-end exact match",
        [
            "edit_linehash_single_edit_1k_exact_match",
            "edit_naive_str_replace_single_edit_1k_exact_match",
            "edit_linehash_single_edit_10k_exact_match",
            "edit_naive_str_replace_single_edit_10k_exact_match",
            "edit_linehash_single_edit_100k_exact_match",
            "edit_naive_str_replace_single_edit_100k_exact_match",
            "edit_linehash_single_edit_10k_long_lines_exact_match",
            "edit_naive_str_replace_single_edit_10k_long_lines_exact_match",
        ],
    ),
    (
        "Robustness scenarios",
        [
            "edit_linehash_single_edit_10k_whitespace_drift",
            "edit_naive_str_replace_single_edit_10k_whitespace_drift",
            "edit_linehash_single_edit_10k_target_whitespace_drift",
            "edit_naive_str_replace_single_edit_10k_target_whitespace_drift",
            "edit_linehash_single_edit_10k_duplicate_target",
            "edit_naive_str_replace_single_edit_10k_duplicate_target",
            "edit_linehash_single_edit_10k_line_shift_drift",
            "edit_naive_str_replace_single_edit_10k_line_shift_drift",
        ],
    ),
    (
        "Phase breakdown",
        [
            "edit_resolve_anchor_10k_exact_match",
            "edit_resolve_anchor_100k_exact_match",
            "edit_resolve_anchor_100k_prebuilt_exact_match",
            "edit_mutate_render_linehash_10k_single_line",
            "edit_mutate_render_linehash_100k_single_line",
            "edit_render_document_100k_exact_match",
            "edit_replace_naive_line_10k_exact_match",
        ],
    ),
]

NOTES = {
    "edit_linehash_single_edit_1k_exact_match": "linehash end-to-end exact-match edit on 1k short lines",
    "edit_naive_str_replace_single_edit_1k_exact_match": "naive exact-line replace on 1k short lines",
    "edit_linehash_single_edit_10k_exact_match": "linehash end-to-end exact-match edit on 10k short lines",
    "edit_naive_str_replace_single_edit_10k_exact_match": "naive exact-line replace on 10k short lines",
    "edit_linehash_single_edit_100k_exact_match": "linehash end-to-end exact-match edit on 100k short lines",
    "edit_naive_str_replace_single_edit_100k_exact_match": "naive exact-line replace on 100k short lines",
    "edit_linehash_single_edit_10k_long_lines_exact_match": "linehash end-to-end exact-match edit on 10k long lines",
    "edit_naive_str_replace_single_edit_10k_long_lines_exact_match": "naive exact-line replace on 10k long lines",
    "edit_linehash_single_edit_10k_whitespace_drift": "surrounding-context drift: linehash still succeeds",
    "edit_naive_str_replace_single_edit_10k_whitespace_drift": "stale exact block replacement under surrounding-context drift",
    "edit_linehash_single_edit_10k_target_whitespace_drift": "target-line drift: linehash fails stale",
    "edit_naive_str_replace_single_edit_10k_target_whitespace_drift": "target-line drift: naive exact-line replacement fails",
    "edit_linehash_single_edit_10k_duplicate_target": "duplicate target text: linehash edits intended occurrence",
    "edit_naive_str_replace_single_edit_10k_duplicate_target": "duplicate target text: naive edits first occurrence only",
    "edit_linehash_single_edit_10k_line_shift_drift": "line inserted above target: linehash anchor becomes stale",
    "edit_naive_str_replace_single_edit_10k_line_shift_drift": "line inserted above target: naive still finds matching text",
    "edit_resolve_anchor_10k_exact_match": "anchor resolution only on 10k exact-match fixture",
    "edit_resolve_anchor_100k_exact_match": "anchor resolution only on 100k exact-match fixture",
    "edit_resolve_anchor_100k_prebuilt_exact_match": "anchor resolution on a prebuilt 100k document to isolate lookup cost from parse cost",
    "edit_mutate_render_linehash_10k_single_line": "linehash mutation plus render on 10k exact-match fixture",
    "edit_mutate_render_linehash_100k_single_line": "linehash mutation plus render on 100k exact-match fixture",
    "edit_render_document_100k_exact_match": "render-only pass on a prebuilt 100k exact-match fixture",
    "edit_replace_naive_line_10k_exact_match": "naive exact-line replace only on 10k exact-match fixture",
}


def git_commit() -> str:
    try:
        return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"


def read_range(name: str) -> str:
    estimates_path = CRITERION_DIR / name / "new" / "estimates.json"
    if not estimates_path.exists():
        raise FileNotFoundError(f"Missing Criterion estimates for {name}: {estimates_path}")

    estimates = json.loads(estimates_path.read_text())
    lower = estimates["mean"]["confidence_interval"]["lower_bound"]
    upper = estimates["mean"]["confidence_interval"]["upper_bound"]
    return f"{format_ns(lower)} – {format_ns(upper)}"


def format_ns(value: float) -> str:
    if value >= 1_000_000_000:
        return f"{value / 1_000_000_000:.4f} s"
    if value >= 1_000_000:
        return f"{value / 1_000_000:.4f} ms"
    if value >= 1_000:
        return f"{value / 1_000:.2f} µs"
    return f"{value:.0f} ns"


def build_report() -> str:
    today = NOW.strftime('%Y-%m-%d %H:%M:%S')
    commit = git_commit()

    lines: list[str] = []
    lines.append("# edit benchmark results")
    lines.append("")
    lines.append(f"Date: {today}")
    lines.append(f"Commit: `{commit}`")
    lines.append("Build: current working tree")
    lines.append(f"Command: `{COMMAND}`")
    lines.append("")
    lines.append("## Current performance state")
    lines.append("")

    for section_title, benchmark_names in SECTIONS:
        lines.append(f"### {section_title}")
        for name in benchmark_names:
            lines.append(f"- `{name}`: {read_range(name)}")
        lines.append("")

    lines.append("## Scenario notes")
    lines.append("")
    for _, benchmark_names in SECTIONS:
        for name in benchmark_names:
            lines.append(f"- `{name}`: {NOTES[name]}")
    lines.append("")

    lines.append("## Assessment")
    lines.append("")
    lines.append("- Exact-match benchmarks show the raw throughput gap between linehash end-to-end edits and naive exact-line replacement at 1k, 10k, and 100k lines.")
    lines.append("- Long-line exact-match benchmarks show how wider line content changes the tradeoff for both strategies.")
    lines.append("- Robustness scenarios intentionally separate stale surrounding context, stale target content, duplicate target text, and line-shift drift so the report does not collapse correctness and speed into one misleading number.")
    lines.append("- Phase-breakdown benchmarks help explain whether large-file cost on the linehash side is concentrated in parse, lookup, or mutation+render work.")
    lines.append("")

    lines.append("## Update instructions")
    lines.append("")
    lines.append("1. Run `cargo bench -p linehash --bench edit_bench`.")
    lines.append("2. Run `python3 scripts/render_edit_bench_report.py`.")
    lines.append("3. Review the diff in `bench-results/edit_bench.md`.")
    lines.append("4. If you want a dated snapshot, keep the generated `bench-results/bench-YYYY-MM-DD-HH-MM-SS.md` file too.")
    lines.append("")

    return "\n".join(lines)


def main() -> None:
    report = build_report()
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    REPORT_PATH.write_text(report + "\n")
    SNAPSHOT_PATH.write_text(report + "\n")
    print(f"Wrote {REPORT_PATH}")
    print(f"Wrote {SNAPSHOT_PATH}")


if __name__ == "__main__":
    main()
