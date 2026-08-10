# hashline accuracy — branch `feat/low-breakage-accuracy` (2026-08-10)

**Baseline:** `accuracy-baseline-2026-08-10.md` (commit `e04cd8e`, pre-changes)
**Current:** HEAD of `feat/low-breakage-accuracy` (after P1/P2/P3/P7)
**Command:** `cargo bench --bench accuracy_bench`

## Symbol-only distinctness — the Phase 2 headline

| State | 5000 identical symbol lines (`}`/`)`/blank) | Distinct hashes | % distinct |
|-------|:---:|---:|---:|
| Baseline | systematic collapse | **3** | 0.1% |
| After P2 | uniform spread | **256** (= 2-char ceiling) | 5.1% |

The `}` → 256 means identical symbol lines no longer share one hash by construction;
remaining collisions are uniform (random) over the 256-value space, disambiguated by the
line-number part of the `42:ab` anchor. This is the intended fix.

## Content-line collision rates — unchanged (by design)

Content lines keep the seed-0 hash (backward-compatible), so collision rates on the
distinct-content fixtures are identical to baseline:

| Lines | Collision rate | Adjacent-collision rate | Ambiguous hashes |
| ----- | -------------: | ----------------------: | ---------------: |
| 1K | 22.68% | 0.30% | 227 |
| 10K | 2.56% | 0.41% | 256 |
| 100K | 0.256% | 0.387% | 256 |

These are inherent to 2-char (8-bit) hashes on files with >256 distinct lines — the
`LINE:HASH` pairing + collision-detection (ambiguous-hash error) is the mitigation, not
lengthening hashes (that is the deferred V2 work).

## What these numbers mean per phase

- **P2 (symbol seeding):** the 3→256 jump is the measured win.
- **P3 (snapshot recovery):** not visible in this micro-bench (it's a behavior change:
  external-shift patches now recover instead of erroring) — covered by
  `test_phase3_recovery_on_external_shift`.
- **P1 (noop guard, snapshot collision, structured errors):** zero hash cost, covered by unit tests.
- **P7 (boundary-echo repair):** behavior change, covered by `apply` tests.

## How to re-measure after future phases

```bash
cargo bench --bench accuracy_bench   # prints accuracy[...] lines to stderr
```
Compare `accuracy[symbol]` (P2 win), collision/adjacent rates (flat until V2 hash), and
`stale_detection_cost` (flat — no format change).

## Additions on this branch (P8/P9, commit `9b1c073`)

### P8 — parallel line hashing (`document.rs`)
`lines_with_hashes` fans out per-line xxh32 across `std::thread::scope` workers
(no rayon). Below 4096 lines the exact original serial path is kept. Result is
bit-for-bit identical to serial (asserted in tests; `accuracy_bench` numbers
unchanged). Measured with `cargo bench --bench hash_bench`:

| Path | Serial | Parallel | Speedup |
|---|---:|---:|---:|
| 100k-line hash throughput | ~590 MiB/s | **~2.14 GiB/s** | **~3.6×** |
| long-lines fixture | ~0.4 GiB/s | **~1.6 GiB/s** | **~3.6×** |

### P9 — named registers (`CUT N..M [@name]` / `PUT [@name] <N`)
Moves code across positions in one patch; per-patch clipboard; fail-closed on
missing register. Verified end-to-end via CLI. No hash-cost impact.

### P4/P5 — format reader / adaptive length: **DEFER** (data-driven)
Measured ambiguity by hash width (2/3/4 hex) on the accuracy fixtures. Longer
hashes make UNQUALIFIED resolution worse at scale (e.g. 100K lines: 2-char has
256 ambiguous values; 3-char has 4096). Qualified `LINE:HASH` anchors are
already collision-free. Recommendation: keep 2-char emission + LINE:HASH +
ambiguous-hash error; revisit only with real-world evidence of a wrong edit.
