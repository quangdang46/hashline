# workflow benchmark results

Date: 2026-03-26 14:48:14
Commit: `9c233b5`
Build: current working tree
Input: `workflow-results-2026-03-26-14-47-04.jsonl`

## Summary by workflow

### linehash_workflow
- Correct outcome rate: 100.0%
- Intended edit success rate: 60.0%
- Safe rejection rate: 40.0%
- Unsafe wrong edits: 0
- Median duration: 2233.618 ms
- Median commands: 2.00
- Median estimated tokens processed: 312247.2

### naive_replace_workflow
- Correct outcome rate: 60.0%
- Intended edit success rate: 40.0%
- Safe rejection rate: 20.0%
- Unsafe wrong edits: 6
- Median duration: 8.497 ms
- Median commands: 1.00
- Median estimated tokens processed: 134983.4

## Scenario comparison

| Scenario | Expected behavior | linehash outcome | naive outcome | linehash ms | naive ms |
|---|---|---|---|---:|---:|
| `exact_match_single_edit` | edit_expected_target | edited expected target | edited expected target | 3603.586 | 10.378 |
| `surrounding_drift_single_edit` | edit_expected_target | edited expected target | edited expected target | 2319.552 | 12.674 |
| `target_drift_single_edit` | safe_rejection | safe rejection | safe rejection | 1730.002 | 2.618 |
| `duplicate_target_single_edit` | edit_expected_target | edited expected target | unsafe wrong edit | 1988.329 | 3.788 |
| `line_shift_single_edit` | safe_rejection | safe rejection | unsafe wrong edit | 1526.620 | 13.028 |

## Key takeaways

- This workflow benchmark measures end-to-end outcomes, not just primitive throughput.
- Safe stale/ambiguity rejections are reported separately from successful edits and unsafe wrong writes.
- Command count and estimated tokens processed show how expensive a workflow is to drive, not merely how fast one operation is in isolation.
- Use `edit_bench.md` for hot-path throughput and phase attribution; use this report for workflow correctness and operator trust.

## Update instructions

1. Run `python3 benchmark/run_workflows.py`.
2. Run `python3 benchmark/analyze_workflows.py <result-jsonl>`.
3. Review the diff in `bench-results/workflow_bench.md`.
4. Keep the dated snapshot if you want a historical record.

