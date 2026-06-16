#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import subprocess
import time
from dataclasses import dataclass
from statistics import median
from datetime import datetime
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
RESULTS_DIR = ROOT / "bench-results"
NOW = datetime.now()
RESULT_PATH = RESULTS_DIR / f"workflow-results-{NOW.strftime('%Y-%m-%d-%H-%M-%S')}.jsonl"
REPEATS = 3
PROFILE = os.environ.get("LINEHASH_BENCH_PROFILE", "fast")


def binary_path() -> Path:
    if PROFILE == "release":
        profile_dir = "release"
    elif PROFILE == "dev":
        profile_dir = "debug"
    else:
        profile_dir = PROFILE
    return ROOT / "target" / profile_dir / "hashline"


def ensure_binary() -> Path:
    target = binary_path()
    if target.exists():
        return target

    build_cmd = ["cargo", "build", "-q", "-p", "hashline"]
    if PROFILE not in {"dev", "debug"}:
        build_cmd.extend(["--profile", PROFILE])
    subprocess.run(build_cmd, cwd=ROOT, check=True)
    return target


@dataclass(frozen=True)
class Scenario:
    name: str
    kind: str
    initial: str
    working: str
    target_line: int
    old_line: str
    new_line: str
    expect_line: str
    expect_success: bool
    notes: str


def run_hashline(args: list[str]) -> tuple[str, str, int, float]:
    binary = ensure_binary()
    start = time.perf_counter()
    proc = subprocess.run(
        [str(binary), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    duration_ms = (time.perf_counter() - start) * 1000.0
    return proc.stdout, proc.stderr, proc.returncode, duration_ms


def decode_json_output(raw: str) -> dict:
    try:
        return json.JSONDecoder().decode(raw)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"hashline read returned invalid JSON: {error}") from error


def build_base_content(line_count: int = 10_000) -> list[str]:
    lines: list[str] = []
    for i in range(line_count):
        lines.append(f"fn generated_line_{i:05}() {{ let value = \"{(i * 2654435761) & 0xffffffff:08x}\"; }}")
    target_index = line_count // 2
    drift_index = target_index - 1
    after_index = target_index + 1
    lines[drift_index] = "    let surrounding_context = compute_timeout_window();"
    lines[target_index] = "    timeout: 3000,"
    lines[after_index] = "    retry: true,"
    return lines


def scenario_exact_match() -> Scenario:
    lines = build_base_content()
    content = "\n".join(lines) + "\n"
    return Scenario(
        name="exact_match_single_edit",
        kind="exact_match",
        initial=content,
        working=content,
        target_line=(len(lines) // 2) + 1,
        old_line="    timeout: 3000,",
        new_line="    timeout: 5000,",
        expect_line="    timeout: 5000,",
        expect_success=True,
        notes="Single-line exact match in a large file.",
    )


def scenario_surrounding_drift() -> Scenario:
    lines = build_base_content()
    initial = "\n".join(lines) + "\n"
    drift_index = (len(lines) // 2) - 1
    lines[drift_index] = "  let surrounding_context = compute_timeout_window();"
    working = "\n".join(lines) + "\n"
    return Scenario(
        name="surrounding_drift_single_edit",
        kind="surrounding_drift",
        initial=initial,
        working=working,
        target_line=(len(lines) // 2) + 1,
        old_line="    timeout: 3000,",
        new_line="    timeout: 5000,",
        expect_line="    timeout: 5000,",
        expect_success=True,
        notes="Target line is stable but nearby block context has drifted.",
    )


def scenario_target_drift() -> Scenario:
    lines = build_base_content()
    initial = "\n".join(lines) + "\n"
    target_index = len(lines) // 2
    lines[target_index] = "  timeout: 3000,"
    working = "\n".join(lines) + "\n"
    return Scenario(
        name="target_drift_single_edit",
        kind="target_drift",
        initial=initial,
        working=working,
        target_line=(len(lines) // 2) + 1,
        old_line="    timeout: 3000,",
        new_line="    timeout: 5000,",
        expect_line="    timeout: 5000,",
        expect_success=False,
        notes="Target line content changed after the original read.",
    )


def scenario_duplicate_target() -> Scenario:
    lines = build_base_content()
    target_index = len(lines) // 2
    duplicate_index = target_index - 3
    lines[duplicate_index] = "    timeout: 3000,"
    content = "\n".join(lines) + "\n"
    return Scenario(
        name="duplicate_target_single_edit",
        kind="duplicate_target",
        initial=content,
        working=content,
        target_line=target_index + 1,
        old_line="    timeout: 3000,",
        new_line="    timeout: 5000,",
        expect_line="    timeout: 5000,",
        expect_success=True,
        notes="Two lines share the same content; only the intended line should change.",
    )


def scenario_line_shift() -> Scenario:
    lines = build_base_content()
    initial = "\n".join(lines) + "\n"
    target_index = len(lines) // 2
    lines.insert(target_index - 1, "fn inserted_line_before_target() { let marker = \"line_shift\"; }")
    working = "\n".join(lines) + "\n"
    return Scenario(
        name="line_shift_single_edit",
        kind="line_shift",
        initial=initial,
        working=working,
        target_line=(len(lines) // 2),
        old_line="    timeout: 3000,",
        new_line="    timeout: 5000,",
        expect_line="    timeout: 5000,",
        expect_success=False,
        notes="A line was inserted above the target after the original read.",
    )


def scenarios() -> list[Scenario]:
    return [
        scenario_exact_match(),
        scenario_surrounding_drift(),
        scenario_target_drift(),
        scenario_duplicate_target(),
        scenario_line_shift(),
    ]


def hashline_workflow(path: Path, scenario: Scenario) -> dict:
    with TemporaryDirectory() as snapshot_dir:
        snapshot_path = Path(snapshot_dir) / "snapshot.txt"
        snapshot_path.write_text(scenario.initial)
        read_json, read_err, read_code, read_ms = run_hashline(["read", str(snapshot_path), "--json"])
        if read_code != 0:
            raise RuntimeError(f"hashline read failed: {read_err}")
        parsed = decode_json_output(read_json)
        anchor = f"{scenario.target_line}:{parsed['lines'][scenario.target_line - 1]['hash']}"

    command_count = 1
    total_ms = read_ms
    stdout, stderr, code, edit_ms = run_hashline(["edit", str(path), anchor, scenario.new_line])
    command_count += 1
    total_ms += edit_ms
    final_content = path.read_text()
    edited_expected_target = code == 0 and scenario.expect_line in final_content
    safe_rejection = code != 0 and not scenario.expect_success
    mutated_file = final_content != scenario.working
    matched_expected_outcome = edited_expected_target if scenario.expect_success else safe_rejection

    return {
        "mode": "hashline_workflow",
        "anchor": anchor,
        "command_count": command_count,
        "duration_ms": round(total_ms, 3),
        "stdout": stdout.strip(),
        "stderr": stderr.strip(),
        "exit_code": code,
        "edited_expected_target": edited_expected_target,
        "safe_rejection": safe_rejection,
        "unsafe_wrong_edit": code == 0 and not edited_expected_target and scenario.expect_success,
        "mutated_file": mutated_file,
        "matched_expected_outcome": matched_expected_outcome,
        "estimated_tokens_processed": len(read_json) // 4,
    }


def naive_workflow(path: Path, scenario: Scenario) -> dict:
    start = time.perf_counter()
    original = path.read_text()
    replaced = original.replace(scenario.old_line, scenario.new_line, 1)
    path.write_text(replaced)
    duration_ms = (time.perf_counter() - start) * 1000.0
    final_content = path.read_text()
    target_line = final_content.splitlines()[scenario.target_line - 1]
    edited_expected_target = target_line == scenario.expect_line
    mutated_file = final_content != scenario.working
    safe_rejection = (not scenario.expect_success) and not mutated_file
    unsafe_wrong_edit = mutated_file and not edited_expected_target
    matched_expected_outcome = edited_expected_target if scenario.expect_success else safe_rejection

    return {
        "mode": "naive_replace_workflow",
        "anchor": None,
        "command_count": 1,
        "duration_ms": round(duration_ms, 3),
        "stdout": "",
        "stderr": "",
        "exit_code": 0,
        "edited_expected_target": edited_expected_target,
        "safe_rejection": safe_rejection,
        "unsafe_wrong_edit": unsafe_wrong_edit,
        "mutated_file": mutated_file,
        "matched_expected_outcome": matched_expected_outcome,
        "estimated_tokens_processed": len(original) // 4,
    }


def classify_expected_outcome(scenario: Scenario) -> str:
    return "edit_expected_target" if scenario.expect_success else "safe_rejection"


def run_repeated(mode_name: str, runner, scenario: Scenario) -> dict:
    attempts: list[dict] = []
    for _ in range(REPEATS):
        with TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "scenario.txt"
            path.write_text(scenario.working)
            attempts.append(runner(path, scenario))

    durations = [attempt["duration_ms"] for attempt in attempts]
    commands = [attempt["command_count"] for attempt in attempts]
    tokens = [attempt.get("estimated_tokens_processed") or 0 for attempt in attempts]
    success_count = sum(1 for attempt in attempts if attempt["matched_expected_outcome"])
    safe_rejection_count = sum(1 for attempt in attempts if attempt["safe_rejection"])
    unsafe_wrong_edit_count = sum(1 for attempt in attempts if attempt["unsafe_wrong_edit"])
    mutated_file_count = sum(1 for attempt in attempts if attempt.get("mutated_file"))
    correct_edit_count = sum(1 for attempt in attempts if attempt.get("edited_expected_target"))
    matched_expected_outcome_count = sum(1 for attempt in attempts if attempt.get("matched_expected_outcome"))

    sample = attempts[0].copy()
    sample.update(
        {
            "mode": mode_name,
            "scenario": scenario.name,
            "scenario_kind": scenario.kind,
            "expected_outcome": classify_expected_outcome(scenario),
            "notes": scenario.notes,
            "timestamp": NOW.isoformat(timespec="seconds"),
            "repeat_count": REPEATS,
            "duration_ms": round(median(durations), 3),
            "duration_ms_min": round(min(durations), 3),
            "duration_ms_max": round(max(durations), 3),
            "command_count": round(median(commands), 2),
            "estimated_tokens_processed": round(median(tokens), 1),
            "success_count": success_count,
            "safe_rejection_count": safe_rejection_count,
            "unsafe_wrong_edit_count": unsafe_wrong_edit_count,
            "mutated_file_count": mutated_file_count,
            "correct_edit_count": correct_edit_count,
            "matched_expected_outcome_count": matched_expected_outcome_count,
            "success": matched_expected_outcome_count == REPEATS,
            "attempts": attempts,
        }
    )
    return sample


def run_scenario(scenario: Scenario) -> list[dict]:
    return [
        run_repeated("hashline_workflow", hashline_workflow, scenario),
        run_repeated("naive_replace_workflow", naive_workflow, scenario),
    ]


def main() -> None:
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    rows: list[dict] = []
    for scenario in scenarios():
        rows.extend(run_scenario(scenario))

    with RESULT_PATH.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row) + "\n")

    print(f"Wrote {RESULT_PATH}")


if __name__ == "__main__":
    main()
