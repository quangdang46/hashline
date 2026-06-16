#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REPORT_DIR = ROOT / "bench-results"
REPORT_PATH = REPORT_DIR / "workflow_bench.md"
NOW = datetime.now()
SNAPSHOT_PATH = REPORT_DIR / f"workflow-bench-{NOW.strftime('%Y-%m-%d-%H-%M-%S')}.md"


def git_commit() -> str:
    try:
        return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"], text=True).strip()
    except Exception:
        return "unknown"


def load_rows(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def summarize(rows: list[dict]) -> dict[str, dict]:
    grouped: dict[str, list[dict]] = defaultdict(list)
    for row in rows:
        grouped[row["mode"]].append(row)

    summary: dict[str, dict] = {}
    for mode, entries in grouped.items():
        matched_expected_outcome_count = sum(1 for row in entries if row["matched_expected_outcome_count"] == row["repeat_count"])
        correct_edit_count = sum(1 for row in entries if row["correct_edit_count"] > 0)
        safe_rejection_count = sum(1 for row in entries if row["safe_rejection_count"] > 0)
        unsafe_wrong_edit_count = sum(row["unsafe_wrong_edit_count"] for row in entries)
        avg_ms = sum(row["duration_ms"] for row in entries) / len(entries)
        avg_commands = sum(row["command_count"] for row in entries) / len(entries)
        avg_tokens = sum((row.get("estimated_tokens_processed") or 0) for row in entries) / len(entries)
        summary[mode] = {
            "entries": entries,
            "correct_outcome_rate": matched_expected_outcome_count / len(entries),
            "edit_success_rate": correct_edit_count / len(entries),
            "safe_rejection_rate": safe_rejection_count / len(entries),
            "unsafe_wrong_edit_count": unsafe_wrong_edit_count,
            "avg_ms": avg_ms,
            "avg_commands": avg_commands,
            "avg_tokens": avg_tokens,
        }
    return summary


def describe_outcome(row: dict) -> str:
    if row["unsafe_wrong_edit_count"] > 0:
        return "unsafe wrong edit"
    if row["safe_rejection_count"] > 0:
        return "safe rejection"
    if row["correct_edit_count"] > 0:
        return "edited expected target"
    return "did not match expected outcome"


def build_report(rows: list[dict], source_path: Path) -> str:
    commit = git_commit()
    summary = summarize(rows)
    lines: list[str] = []
    lines.append("# workflow benchmark results")
    lines.append("")
    lines.append(f"Date: {NOW.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append(f"Commit: `{commit}`")
    lines.append("Build: current working tree")
    lines.append(f"Input: `{source_path.name}`")
    lines.append("")
    lines.append("## Summary by workflow")
    lines.append("")
    for mode, data in summary.items():
        lines.append(f"### {mode}")
        lines.append(f"- Correct outcome rate: {data['correct_outcome_rate'] * 100:.1f}%")
        lines.append(f"- Intended edit success rate: {data['edit_success_rate'] * 100:.1f}%")
        lines.append(f"- Safe rejection rate: {data['safe_rejection_rate'] * 100:.1f}%")
        lines.append(f"- Unsafe wrong edits: {data['unsafe_wrong_edit_count']}")
        lines.append(f"- Median duration: {data['avg_ms']:.3f} ms")
        lines.append(f"- Median commands: {data['avg_commands']:.2f}")
        lines.append(f"- Median estimated tokens processed: {data['avg_tokens']:.1f}")
        lines.append("")

    lines.append("## Scenario comparison")
    lines.append("")
    lines.append("| Scenario | Expected behavior | hashline outcome | naive outcome | hashline ms | naive ms |")
    lines.append("|---|---|---|---|---:|---:|")
    by_scenario: dict[str, dict[str, dict]] = defaultdict(dict)
    for row in rows:
        by_scenario[row["scenario"]][row["mode"]] = row
    for scenario, modes in by_scenario.items():
        hashline_row = modes["hashline_workflow"]
        naive_row = modes["naive_replace_workflow"]
        lines.append(
            f"| `{scenario}` | {hashline_row['expected_outcome']} | {describe_outcome(hashline_row)} | {describe_outcome(naive_row)} | {hashline_row['duration_ms']:.3f} | {naive_row['duration_ms']:.3f} |"
        )
    lines.append("")

    lines.append("## Key takeaways")
    lines.append("")
    lines.append("- This workflow benchmark measures end-to-end outcomes, not just primitive throughput.")
    lines.append("- Safe stale/ambiguity rejections are reported separately from successful edits and unsafe wrong writes.")
    lines.append("- Command count and estimated tokens processed show how expensive a workflow is to drive, not merely how fast one operation is in isolation.")
    lines.append("- Use `edit_bench.md` for hot-path throughput and phase attribution; use this report for workflow correctness and operator trust.")
    lines.append("")

    lines.append("## Update instructions")
    lines.append("")
    lines.append("1. Run `python3 benchmark/run_workflows.py`.")
    lines.append("2. Run `python3 benchmark/analyze_workflows.py <result-jsonl>`.")
    lines.append("3. Review the diff in `bench-results/workflow_bench.md`.")
    lines.append("4. Keep the dated snapshot if you want a historical record.")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: python3 benchmark/analyze_workflows.py <result-jsonl>")
    source_path = Path(sys.argv[1]).resolve()
    rows = load_rows(source_path)
    report = build_report(rows, source_path)
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    REPORT_PATH.write_text(report + "\n")
    SNAPSHOT_PATH.write_text(report + "\n")
    print(f"Wrote {REPORT_PATH}")
    print(f"Wrote {SNAPSHOT_PATH}")


if __name__ == "__main__":
    main()
