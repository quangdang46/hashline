#!/usr/bin/env python3
"""
Benchmark runner for linehash performance evaluation.

Executes `claude -p` for each combination of (task, mode, model, repetition).
Records token usage, cost, correctness, and tool usage to JSONL format.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Optional

sys.path.insert(0, str(Path(__file__).parent))

from config import MODELS, MODES, REPOS, SYSTEM_PROMPT, DEFAULT_MAX_BUDGET_USD, SYNTHETIC_REPO, RESULTS_DIR, DEFAULT_REPS, PRICING
from parse import parse_stream_json, tool_call_counts
from tasks import TASKS


def get_linehash_version() -> Optional[str]:
    """Get installed linehash version via `linehash --version`."""
    try:
        result = subprocess.run(
            ["linehash", "--version"],
            capture_output=True, text=True, timeout=5,
        )
        return result.stdout.strip() if result.returncode == 0 else None
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None


def get_repo_path(repo_name: str) -> Path:
    """Resolve working directory for a task's repo."""
    if repo_name == "synthetic":
        return SYNTHETIC_REPO
    return REPOS[repo_name].path


def _compact_tool_sequence(result):
    """Extract ordered tool call names + key args from all turns."""
    seq = []
    for turn in result.turns:
        for tc in turn.tool_calls:
            entry = {"name": tc.name}
            args = {}
            for k, v in tc.input.items():
                if k == "command":
                    args[k] = str(v)[:80]
                elif k == "file_path":
                    args[k] = str(v).split("/")[-1]
                elif k in ("pattern", "query", "path", "scope", "kind", "section", "expand"):
                    args[k] = str(v)[:60]
            if args:
                entry["args"] = args
            seq.append(entry)
    return seq


def run_single(
    task_name: str,
    mode_name: str,
    model_name: str,
    repetition: int,
    verbose: bool = False,
) -> dict:
    """
    Run a single benchmark iteration.
    """
    task = TASKS[task_name]
    repo_path = get_repo_path(task.repo)
    mode = MODES[mode_name]
    model_id = MODELS[model_name]

    # Build system prompt based on mode
    system_prompt = SYSTEM_PROMPT.get(mode_name, SYSTEM_PROMPT["baseline"])

    # Build command
    cmd = [
        "claude", "-p",
        "--output-format", "stream-json",
        "--verbose",
        "--model", model_id,
        "--max-budget-usd", str(DEFAULT_MAX_BUDGET_USD),
        "--no-session-persistence",
        "--dangerously-skip-permissions",
        "--system-prompt", system_prompt + f"\nYour current working directory is: {repo_path}",
    ]

    if mode.tools:
        cmd += ["--tools", ",".join(mode.tools)]
    else:
        cmd += ["--tools", ""]

    cmd += ["--", task.prompt]

    if verbose:
        print(f"    Running: {' '.join(cmd)}")

    # Run subprocess (unset CLAUDECODE to allow nested claude -p)
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"}
    start_time = time.time()
    result = subprocess.run(
        cmd,
        cwd=str(repo_path),
        capture_output=True,
        text=True,
        timeout=300,
        env=env,
    )
    elapsed_ms = int((time.time() - start_time) * 1000)

    if result.returncode != 0:
        raise RuntimeError(
            f"claude -p failed with code {result.returncode}\n"
            f"stderr: {result.stderr}\n"
            f"stdout: {result.stdout[:500]}"
        )

    run_result = parse_stream_json(result.stdout)
    run_result.task_name = task_name
    run_result.mode_name = mode_name
    run_result.model_name = model_name
    run_result.repetition = repetition

    if run_result.duration_ms == 0:
        run_result.duration_ms = elapsed_ms

    # Check correctness
    correct, reason = task.check_correctness(
        run_result.result_text,
        str(repo_path),
    )
    run_result.correct = correct
    run_result.correctness_reason = reason

    tool_breakdown = tool_call_counts(run_result)
    per_turn_context = [turn.context_tokens for turn in run_result.turns]
    total_context = sum(per_turn_context)

    return {
        "task": task_name,
        "repo": task.repo,
        "mode": mode_name,
        "model": model_name,
        "repetition": repetition,
        "linehash_version": get_linehash_version() if mode_name == "linehash" else None,
        "num_turns": run_result.num_turns,
        "num_tool_calls": sum(tool_breakdown.values()),
        "tool_calls": tool_breakdown,
        "total_cost_usd": run_result.total_cost_usd,
        "duration_ms": run_result.duration_ms,
        "context_tokens": total_context,
        "output_tokens": run_result.total_output_tokens,
        "input_tokens": run_result.total_input_tokens,
        "cache_creation_tokens": run_result.total_cache_creation_tokens,
        "cache_read_tokens": run_result.total_cache_read_tokens,
        "per_turn_context_tokens": per_turn_context,
        "correct": correct,
        "correctness_reason": reason,
        "result_text": run_result.result_text[:5000],
        "tool_sequence": _compact_tool_sequence(run_result),
    }


def parse_comma_list(value: str, valid_options: dict, name: str) -> list[str]:
    """Parse comma-separated list and validate against valid options."""
    if value.lower() == "all":
        return list(valid_options.keys())

    items = [item.strip() for item in value.split(",") if item.strip()]
    invalid = [item for item in items if item not in valid_options]
    if invalid:
        raise ValueError(
            f"Invalid {name}: {', '.join(invalid)}. "
            f"Valid options: {', '.join(valid_options.keys())}"
        )
    return items


def main():
    parser = argparse.ArgumentParser(
        description="Run linehash benchmarks",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python run.py --models sonnet --reps 3 --tasks all --modes all
  python run.py --models sonnet --reps 1 --tasks find_definition --modes baseline,linehash
  python run.py --models sonnet --reps 3 --tasks all --modes all --repos all
        """,
    )

    parser.add_argument(
        "--models",
        default="sonnet",
        help="Comma-separated model names or 'all' (default: sonnet)",
    )
    parser.add_argument(
        "--reps",
        type=int,
        default=DEFAULT_REPS,
        help=f"Number of repetitions (default: {DEFAULT_REPS})",
    )
    parser.add_argument(
        "--tasks",
        default="all",
        help="Comma-separated task names or 'all' (default: all)",
    )
    parser.add_argument(
        "--modes",
        default="all",
        help="Comma-separated mode names or 'all' (default: all)",
    )
    parser.add_argument(
        "--repos",
        default="synthetic",
        help="Comma-separated repo names or 'all' (default: synthetic)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Print detailed output for debugging",
    )

    args = parser.parse_args()

    try:
        models = parse_comma_list(args.models, MODELS, "models")
        tasks_list = parse_comma_list(args.tasks, TASKS, "tasks")
        modes = parse_comma_list(args.modes, MODES, "modes")
    except ValueError as e:
        parser.error(str(e))
        return

    # Filter tasks by repo
    if args.repos.lower() != "all":
        requested_repos = set(r.strip() for r in args.repos.split(",") if r.strip())
        tasks_list = [t for t in tasks_list if TASKS[t].repo in requested_repos]
        if not tasks_list:
            parser.error(f"No tasks found for repos: {args.repos}")

    # Validate synthetic repo exists (only if synthetic tasks are selected)
    if "synthetic" in set(TASKS[t].repo for t in tasks_list):
        if not SYNTHETIC_REPO.exists():
            print("ERROR: Synthetic repo not found.")
            print(f"Expected at: {SYNTHETIC_REPO}")
            print("Run setup.py to create the test repository:")
            print("  python benchmark/fixtures/setup.py")
            sys.exit(1)

    # Validate real-world repos exist (for selected tasks)
    selected_repos = set(TASKS[t].repo for t in tasks_list) - {"synthetic"}
    for repo_name in selected_repos:
        repo_path = REPOS[repo_name].path
        if not repo_path.exists():
            print(f"ERROR: Repo '{repo_name}' not found.")
            print(f"Expected at: {repo_path}")
            print("Run setup_repos.py to clone repositories:")
            print("  python benchmark/fixtures/setup_repos.py --repos all")
            sys.exit(1)

    # Clean repos before starting
    for repo_name in selected_repos:
        repo_path = REPOS[repo_name].path
        from fixtures.reset import ensure_repo_clean
        ensure_repo_clean(repo_path, REPOS[repo_name].commit_sha)
        if args.verbose:
            print(f"Cleaned repo: {repo_name}")

    RESULTS_DIR.mkdir(exist_ok=True)

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    model_suffix = f"_{models[0]}" if len(models) == 1 else ""
    output_file = RESULTS_DIR / f"benchmark_{timestamp}{model_suffix}.jsonl"

    print("=" * 70)
    print("linehash Benchmark Runner")
    print("=" * 70)
    print(f"Models:      {', '.join(models)}")
    print(f"Tasks:       {', '.join(tasks_list)}")
    print(f"Modes:       {', '.join(modes)}")
    repos_used = sorted(set(TASKS[t].repo for t in tasks_list))
    print(f"Repos:       {', '.join(repos_used)}")
    print(f"Repetitions: {args.reps}")
    print(f"Output:      {output_file}")
    print("=" * 70)
    print()

    total_runs = len(tasks_list) * len(modes) * len(models) * args.reps
    current_run = 0

    with open(output_file, "w") as f:
        for task_name in tasks_list:
            task = TASKS[task_name]

            for mode_name in modes:
                for model_name in models:
                    for rep in range(args.reps):
                        current_run += 1
                        run_id = f"{task_name}/{mode_name}/{model_name}/rep{rep}"

                        # Reset repo before each run
                        repo_path = get_repo_path(task.repo)
                        if task.repo == "synthetic":
                            subprocess.run(["git", "checkout", "--", "."], cwd=str(SYNTHETIC_REPO), capture_output=True)
                            subprocess.run(["git", "clean", "-fd"], cwd=str(SYNTHETIC_REPO), capture_output=True)
                        else:
                            from fixtures.reset import ensure_repo_clean
                            ensure_repo_clean(repo_path, REPOS[task.repo].commit_sha)

                        print(f"[{current_run}/{total_runs}] {run_id}")

                        try:
                            result = run_single(
                                task_name,
                                mode_name,
                                model_name,
                                rep,
                                verbose=args.verbose,
                            )

                            f.write(json.dumps(result) + "\n")
                            f.flush()

                            status = "✓" if result["correct"] else "✗"
                            print(
                                f"  {status} "
                                f"{result['num_turns']}t "
                                f"{result['context_tokens']:,}ctx "
                                f"{result['output_tokens']:,}out "
                                f"${result['total_cost_usd']:.4f} "
                                f"{result['duration_ms']:,}ms"
                            )

                            if not result["correct"]:
                                print(f"  → {result['correctness_reason']}")

                        except subprocess.TimeoutExpired:
                            print(f"  ✗ TIMEOUT (>300s)")
                            error_result = {
                                "task": task_name,
                                "mode": mode_name,
                                "model": model_name,
                                "repetition": rep,
                                "error": "timeout",
                                "correct": False,
                                "correctness_reason": "Subprocess timed out",
                            }
                            f.write(json.dumps(error_result) + "\n")
                            f.flush()

                        except Exception as e:
                            print(f"  ✗ ERROR: {e}")
                            if args.verbose:
                                import traceback
                                traceback.print_exc()
                            error_result = {
                                "task": task_name,
                                "mode": mode_name,
                                "model": model_name,
                                "repetition": rep,
                                "error": str(e),
                                "correct": False,
                                "correctness_reason": f"Exception: {e}",
                            }
                            f.write(json.dumps(error_result) + "\n")
                            f.flush()

    # Clean repos after run
    for repo_name in selected_repos:
        repo_path = REPOS[repo_name].path
        from fixtures.reset import ensure_repo_clean
        ensure_repo_clean(repo_path, REPOS[repo_name].commit_sha)

    print()
    print("=" * 70)
    print("Benchmark complete!")
    print(f"Results saved to: {output_file}")
    print("=" * 70)
    print()
    print("To generate a report, run:")
    print(f"  python benchmark/analyze.py {output_file}")


if __name__ == "__main__":
    main()