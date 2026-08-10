# hashline accuracy baseline — 2026-08-10

**Branch:** feat/low-breakage-accuracy (pre-changes baseline)
**Commit:** e04cd8e (after Credit README, before any core changes)
**Command:** `cargo bench --bench accuracy_bench`

## Collision rates (short fixture — distinct `fn generated_line_NNNNN() { ... }` lines)

| Lines | Collisions | Collision rate | Adjacent-pair collisions | Adjacent rate | Ambiguous hashes |
| ----- | ---------: | -------------: | -----------------------: | ------------: | ---------------: |
| 1K | — | — | — | — | — |
| 10K | — | — | — | — | — |
| 100K | 256 | 0.256% | 387 | 0.387% | 256 |

(1K/10K rows not captured in the truncated tail; re-run `cargo bench --bench accuracy_bench` for full rows.)

## Symbol-only distinctness (Phase 2 baseline)

> **5000 identical symbol-only lines (`}`, `)`, blank) → only 3 distinct 2-char hashes (0.1% distinct).**

This is the exact problem Phase 2 (position-seeded symbol-only hashing) targets: a file of `}` closers collapses to 3 hash values, so anchors are nearly useless there without line numbers.

## Stale-detection cost (re-hash all lines on patch)

| Lines | Mean time | Throughput |
| ----- | --------: | ---------: |
| 1K | ~92 µs | ~10.8 Melem/s |
| 10K | ~1.78 ms | ~5.6 Melem/s |
| 100K | ~18.3 ms | ~5.5 Melem/s |

Phase 8 (parallel hashing) only pays off if hashing is a meaningful share of total latency — this baseline shows it's linear in file size and small vs I/O, so parallelize later (per the low-breakage plan).

## How to use

Run `cargo bench --bench accuracy_bench` before and after each phase. Compare the `accuracy[...]` eprintln lines:
- `collision_rate` should fall after Phase 2/3
- `adjacent_collision_rate` should fall after Phase 3 (context)
- `symbol_only_distinctness` should jump from 3 → 5000 after Phase 2
- `stale_detection_cost` should stay flat (no format change)
