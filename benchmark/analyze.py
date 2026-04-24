"""Benchmark analysis and report generation.

Reads JSONL results from run.py and generates a markdown report
with context efficiency metrics and comparisons.
"""

import argparse
import json
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from statistics import median, mean, stdev


# Anthropic Claude pricing (per million tokens)
PRICING = {
    "cache_creation": 3.75,  # $3.75 per MTok
    "cache_read": 0.30,      # $0.30 per MTok
    "output": 15.00,         # $15.00 per MTok
    "input": 3.00,           # $3.00 per MTok
}


def compute_cost_breakdown(run: dict) -> dict[str, float]:
    """Compute cost breakdown by token category."""
    return {
        "cache_creation_cost": run.get("cache_creation_tokens", 0) * PRICING["cache_creation"] / 1_000_000,
        "cache_read_cost": run.get("cache_read_tokens", 0) * PRICING["cache_read"] / 1_000_000,
        "output_cost": run.get("output_tokens", 0) * PRICING["output"] / 1_000_000,
        "input_cost": run.get("input_tokens", 0) * PRICING["input"] / 1_000_000,
    }


def format_cost_breakdown(costs: dict[str, float], indent: str = "  ") -> str:
    """Format cost breakdown as single line."""
    parts = [
        f"cache_create=${costs['cache_creation_cost']:.3f}",
        f"cache_read=${costs['cache_read_cost']:.3f}",
        f"output=${costs['output_cost']:.3f}",
        f"input=${costs['input_cost']:.3f}",
    ]
    return f"{indent}{' '.join(parts)}"


def format_cost_delta(baseline_costs: dict[str, float], linehash_costs: dict[str, float], indent: str = "  ") -> str:
    """Format cost delta breakdown."""
    deltas = {
        "cache_creation": linehash_costs['cache_creation_cost'] - baseline_costs['cache_creation_cost'],
        "cache_read": linehash_costs['cache_read_cost'] - baseline_costs['cache_read_cost'],
        "output": linehash_costs['output_cost'] - baseline_costs['output_cost'],
        "input": linehash_costs['input_cost'] - baseline_costs['input_cost'],
    }
    parts = [
        f"{'Δcache_create='}{'+' if deltas['cache_creation'] >= 0 else ''}{deltas['cache_creation']:.3f}",
        f"{'Δcache_read='}{'+' if deltas['cache_read'] >= 0 else ''}{deltas['cache_read']:.3f}",
        f"{'Δoutput='}{'+' if deltas['output'] >= 0 else ''}{deltas['output']:.3f}",
        f"{'Δinput='}{'+' if deltas['input'] >= 0 else ''}{deltas['input']:.3f}",
    ]
    return f"{indent}{' '.join(parts)}"


def load_results(path: Path) -> list[dict]:
    """Load JSONL results file."""
    results = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                results.append(json.loads(line))
    return results


def group_by(results: list[dict], *keys: str) -> dict:
    """Group results by specified keys."""
    groups = defaultdict(list)
    for result in results:
        if "error" in result:
            continue
        key = tuple(result.get(k) for k in keys)
        groups[key].append(result)
    return dict(groups)


def compute_stats(values: list) -> dict:
    """Compute statistics for a list of values."""
    if not values:
        return {
            "median": 0,
            "mean": 0,
            "stdev": 0,
            "min": 0,
            "max": 0,
        }

    return {
        "median": median(values),
        "mean": mean(values),
        "stdev": stdev(values) if len(values) > 1 else 0,
        "min": min(values),
        "max": max(values),
    }


def ascii_sparkline(values: list[int]) -> str:
    """Generate ASCII sparkline from values."""
    if not values:
        return ""

    if max(values) == min(values):
        return "▄" * len(values)

    chars = " ▁▂▃▄▅▆▇█"
    lo, hi = min(values), max(values)
    return "".join(
        chars[min(int((v - lo) / (hi - lo) * 8), 8)]
        for v in values
    )


def format_delta(baseline_val: float, linehash_val: float) -> str:
    """Format delta as percentage change."""
    if baseline_val == 0:
        return "—"
    pct_change = ((linehash_val - baseline_val) / baseline_val) * 100
    sign = "+" if pct_change > 0 else ""
    return f"{sign}{pct_change:.0f}%"


def find_median_run(runs: list[dict], metric: str) -> dict:
    """Find the run with median value for given metric."""
    if not runs:
        return {}
    sorted_runs = sorted(runs, key=lambda r: r.get(metric, 0))
    return sorted_runs[len(sorted_runs) // 2]


def merge_tool_calls(runs: list[dict]) -> dict[str, float]:
    """Merge tool_calls dicts from multiple runs and compute median counts."""
    all_tools = set()
    for run in runs:
        if "tool_calls" in run:
            all_tools.update(run["tool_calls"].keys())

    result = {}
    for tool in all_tools:
        counts = [run.get("tool_calls", {}).get(tool, 0) for run in runs]
        result[tool] = median(counts)

    return result


def compute_dollar_per_correct(runs: list[dict]) -> dict:
    """Compute $/correct metric for a set of runs.

    $/correct = avg_cost / accuracy
    This is the expected cost before one correct answer under retry model.
    """
    if not runs:
        return {"cost_per_correct": float('inf'), "accuracy": 0.0, "avg_cost": 0.0}

    total_cost = sum(r.get("total_cost_usd", 0.0) for r in runs)
    correct_count = sum(1 for r in runs if r.get("correct", False))
    accuracy = correct_count / len(runs)
    avg_cost = total_cost / len(runs)

    if accuracy > 0:
        cost_per_correct = avg_cost / accuracy
    else:
        cost_per_correct = float('inf')

    return {
        "cost_per_correct": cost_per_correct,
        "accuracy": accuracy,
        "avg_cost": avg_cost,
        "correct_count": correct_count,
        "total_runs": len(runs),
    }


def generate_report(results: list[dict]) -> str:
    """Generate markdown report from results."""
    if not results:
        return "# Error\n\nNo valid results found in file.\n"

    # Filter out error entries
    valid_results = [r for r in results if "error" not in r]
    error_count = len(results) - len(valid_results)

    if not valid_results:
        return f"# Error\n\nAll {len(results)} runs failed.\n"

    # Extract metadata
    models = sorted(set(r["model"] for r in valid_results))
    tasks = sorted(set(r["task"] for r in valid_results))
    modes = sorted(set(r["mode"] for r in valid_results))
    repos = sorted(set(r.get("repo", "synthetic") for r in valid_results))
    max_rep = max(r["repetition"] for r in valid_results)
    num_reps = max_rep + 1

    # Build header
    lines = [
        "# linehash Benchmark Results",
        "",
        f"**Generated:** {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}",
        "",
        f"**Runs:** {len(valid_results)} valid",
    ]

    if error_count > 0:
        lines.append(f" ({error_count} errors)")

    lines.extend([
        f" | **Models:** {', '.join(models)} | **Repos:** {', '.join(repos)} | **Reps:** {num_reps}",
        "",
        "## Summary",
        "",
        f"**Primary metric: $/correct** = avg_cost / accuracy",
        "",
        "The $/correct metric represents the expected cost before one correct answer",
        "under a geometric retry model. Lower is better.",
        "",
    ])

    # Group by task
    task_groups = group_by(valid_results, "task")

    for task_name in tasks:
        task_results = task_groups.get((task_name,), [])
        if not task_results:
            continue

        lines.append(f"### {task_name}")
        lines.append("")

        # Group by mode
        mode_groups = group_by(task_results, "mode")

        has_baseline = ("baseline",) in mode_groups
        has_linehash = ("linehash",) in mode_groups

        if has_baseline and has_linehash:
            baseline_runs = mode_groups[("baseline",)]
            linehash_runs = mode_groups[("linehash",)]

            baseline_stats = compute_dollar_per_correct(baseline_runs)
            linehash_stats = compute_dollar_per_correct(linehash_runs)

            # Compute accuracy
            baseline_acc = baseline_stats["accuracy"] * 100
            linehash_acc = linehash_stats["accuracy"] * 100

            # Compute avg cost per task
            baseline_avg_cost = baseline_stats["avg_cost"]
            linehash_avg_cost = linehash_stats["avg_cost"]

            # Compute cost per correct
            baseline_cpc = baseline_stats["cost_per_correct"]
            linehash_cpc = linehash_stats["cost_per_correct"]

            # Compute delta
            if baseline_cpc == float('inf'):
                delta_str = "—"
            elif linehash_cpc == float('inf'):
                delta_str = "+∞"
            else:
                delta_pct = ((linehash_cpc - baseline_cpc) / baseline_cpc) * 100
                sign = "+" if delta_pct > 0 else ""
                delta_str = f"{sign}{delta_pct:.0f}%"

            lines.append("| Mode | Accuracy | Avg Cost | $/correct |")
            lines.append("|------|----------|----------|-----------|")
            lines.append(f"| baseline | {baseline_acc:.0f}% | ${baseline_avg_cost:.4f} | ${baseline_cpc:.4f} |")
            lines.append(f"| linehash | {linehash_acc:.0f}% | ${linehash_avg_cost:.4f} | ${linehash_cpc:.4f} |")
            lines.append(f"| **Delta** | | | **{delta_str}** |")
            lines.append("")

            # Per-task metrics table
            metrics = [
                ("Context tokens", "context_tokens"),
                ("Output tokens", "output_tokens"),
                ("Turns", "num_turns"),
                ("Tool calls", "num_tool_calls"),
                ("Cost USD", "total_cost_usd"),
                ("Duration ms", "duration_ms"),
            ]

            lines.append("| Metric | baseline | linehash | delta |")
            lines.append("|--------|----------|---------|-------|")

            for label, key in metrics:
                baseline_m = compute_stats([r[key] for r in baseline_runs])
                linehash_m = compute_stats([r[key] for r in linehash_runs])
                delta = format_delta(baseline_m["median"], linehash_m["median"])

                if key == "total_cost_usd":
                    baseline_fmt = f"${baseline_m['median']:.4f}"
                    linehash_fmt = f"${linehash_m['median']:.4f}"
                else:
                    baseline_fmt = f"{baseline_m['median']:.0f}"
                    linehash_fmt = f"{linehash_m['median']:.0f}"

                lines.append(f"| {label} (median) | {baseline_fmt} | {linehash_fmt} | {delta} |")

            lines.append("")

            # Cost breakdown for median run
            baseline_median_run = find_median_run(baseline_runs, "total_cost_usd")
            linehash_median_run = find_median_run(linehash_runs, "total_cost_usd")

            baseline_costs = compute_cost_breakdown(baseline_median_run)
            linehash_costs = compute_cost_breakdown(linehash_median_run)

            baseline_total = baseline_median_run.get("total_cost_usd", 0.0)
            linehash_total = linehash_median_run.get("total_cost_usd", 0.0)
            total_delta = linehash_total - baseline_total

            baseline_turns = baseline_median_run.get("num_turns", 0)
            linehash_turns = linehash_median_run.get("num_turns", 0)
            turns_delta = linehash_turns - baseline_turns

            baseline_correct_str = "correct" if baseline_median_run.get("correct", False) else "incorrect"
            linehash_correct_str = "correct" if linehash_median_run.get("correct", False) else "incorrect"

            lines.append("**Cost breakdown (median run):**")
            lines.append("")
            lines.append(f"  baseline: {baseline_turns} turns, ${baseline_total:.4f}, {baseline_correct_str}")
            lines.append(format_cost_breakdown(baseline_costs))
            lines.append(f"  linehash:   {linehash_turns} turns, ${linehash_total:.4f}, {linehash_correct_str}")
            lines.append(format_cost_breakdown(linehash_costs))
            lines.append(f"  delta:    {'+' if turns_delta >= 0 else ''}{turns_delta} turns, {'+' if total_delta >= 0 else ''}${total_delta:.4f}")
            lines.append(format_cost_delta(baseline_costs, linehash_costs))
            lines.append("")

            # Per-turn sparklines
            baseline_per_turn = baseline_median_run.get("per_turn_context_tokens", [])
            linehash_per_turn = linehash_median_run.get("per_turn_context_tokens", [])

            if baseline_per_turn and linehash_per_turn:
                lines.append("**Per-turn context tokens (median run):**")
                lines.append("")
                baseline_spark = ascii_sparkline(baseline_per_turn)
                linehash_spark = ascii_sparkline(linehash_per_turn)
                baseline_range = f"{min(baseline_per_turn):,} → {max(baseline_per_turn):,}"
                linehash_range = f"{min(linehash_per_turn):,} → {max(linehash_per_turn):,}"
                lines.append(f"  baseline: {baseline_spark} ({baseline_range})")
                lines.append(f"  linehash:    {linehash_spark} ({linehash_range})")
                lines.append("")

            # Tool breakdown
            baseline_tools = merge_tool_calls(baseline_runs)
            linehash_tools = merge_tool_calls(linehash_runs)

            if baseline_tools or linehash_tools:
                lines.append("**Tool breakdown (median counts):**")
                lines.append("")
                if baseline_tools:
                    tool_strs = [f"{name}={count:.0f}" for name, count in baseline_tools.items()]
                    lines.append(f"  baseline: {', '.join(tool_strs)}")
                if linehash_tools:
                    tool_strs = [f"{name}={count:.0f}" for name, count in linehash_tools.items()]
                    lines.append(f"  linehash:    {', '.join(tool_strs)}")
                lines.append("")

        else:
            # Only one mode available
            for mode_name in modes:
                mode_results = mode_groups.get((mode_name,), [])
                if not mode_results:
                    continue

                stats = compute_dollar_per_correct(mode_results)
                acc = stats["accuracy"] * 100
                avg_cost = stats["avg_cost"]
                cpc = stats["cost_per_correct"]

                lines.append(f"**Mode: {mode_name}**")
                lines.append("")
                lines.append(f"| Metric | Value |")
                lines.append("|--------|-------|")
                lines.append(f"| Accuracy | {acc:.0f}% |")
                lines.append(f"| Avg Cost | ${avg_cost:.4f} |")
                lines.append(f"| $/correct | ${cpc:.4f} |")
                lines.append("")

                lines.append("| Metric | Median |")
                lines.append("|--------|--------|")

                metrics = [
                    ("Context tokens", "context_tokens"),
                    ("Output tokens", "output_tokens"),
                    ("Turns", "num_turns"),
                    ("Tool calls", "num_tool_calls"),
                    ("Cost USD", "total_cost_usd"),
                    ("Duration ms", "duration_ms"),
                ]

                for label, key in metrics:
                    m = compute_stats([r[key] for r in mode_results])
                    if key == "total_cost_usd":
                        val_fmt = f"${m['median']:.4f}"
                    else:
                        val_fmt = f"{m['median']:.0f}"
                    lines.append(f"| {label} | {val_fmt} |")
                lines.append("")

        lines.append("")

    # Aggregate summary
    baseline_all = [r for r in valid_results if r["mode"] == "baseline"]
    linehash_all = [r for r in valid_results if r["mode"] == "linehash"]

    if baseline_all and linehash_all:
        lines.append("## Aggregate Summary")
        lines.append("")

        baseline_stats = compute_dollar_per_correct(baseline_all)
        linehash_stats = compute_dollar_per_correct(linehash_all)

        baseline_acc = baseline_stats["accuracy"] * 100
        linehash_acc = linehash_stats["accuracy"] * 100
        baseline_cpc = baseline_stats["cost_per_correct"]
        linehash_cpc = linehash_stats["cost_per_correct"]

        if baseline_cpc == float('inf'):
            delta_str = "—"
        elif linehash_cpc == float('inf'):
            delta_str = "+∞"
        else:
            delta_pct = ((linehash_cpc - baseline_cpc) / baseline_cpc) * 100
            sign = "+" if delta_pct > 0 else ""
            delta_str = f"{sign}{delta_pct:.0f}%"

        lines.append("| Mode | Accuracy | Avg Cost | $/correct |")
        lines.append("|------|----------|----------|-----------|")
        lines.append(f"| baseline | {baseline_acc:.0f}% | ${baseline_stats['avg_cost']:.4f} | ${baseline_cpc:.4f} |")
        lines.append(f"| linehash | {linehash_acc:.0f}% | ${linehash_stats['avg_cost']:.4f} | ${linehash_cpc:.4f} |")
        lines.append(f"| **Delta** | | | **{delta_str}** |")
        lines.append("")
        lines.append("All values averaged across all tasks and repetitions.")
        lines.append("")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(
        description="Analyze benchmark results and generate report",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python analyze.py results/benchmark_20260212_150000.jsonl
  python analyze.py results/benchmark_20260212_150000.jsonl -o report.md
        """,
    )

    parser.add_argument(
        "results_file",
        type=Path,
        help="Path to JSONL results file from run.py",
    )
    parser.add_argument(
        "-o", "--output",
        type=Path,
        help="Output path for markdown report (default: print to stdout)",
    )

    args = parser.parse_args()

    if not args.results_file.exists():
        print(f"ERROR: File not found: {args.results_file}", file=sys.stderr)
        sys.exit(1)

    try:
        results = load_results(args.results_file)
    except Exception as e:
        print(f"ERROR: Failed to load results: {e}", file=sys.stderr)
        sys.exit(1)

    report = generate_report(results)

    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report)
        print(f"Report written to: {args.output}")
    else:
        print(report)


if __name__ == "__main__":
    main()
