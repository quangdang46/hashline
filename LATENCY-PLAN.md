# LATENCY-PLAN — latency reduction plan for linehash

Author: profiling + code-review session, 2026-05-16
Source data: `bench-results/bench-feature-report-2026-05-16-10-26-21.md` + strace measurements

---

## 1. Goals (confirmed for implementation)

Three measurable goals are confirmed as the implementation priorities for the
latency roadmap:

### Primary goal (P0)
> **`linehash edit <file> <anchor> <content>` on a 100k-line file must reach < 18 ms (median)** — within ~35% of `sed -i` (~13.5 ms). The remaining gap is the safety contract.

Current: 35 ms median → need ~50% reduction.

### Secondary goal (P1)
> **`linehash read <file>` on a 100k-line file must reach < 18 ms (median)** — comparable to `cat | nl` (~12 ms).

Current: 30 ms median → need ~40% reduction.

### Stretch goal (P2)
> **`linehash grep <regex>` on a 100k-line file must reach < 8 ms (median)** — within ~2× of ripgrep (3.6 ms).

Current: 21.7 ms median → need ~65% reduction.

### Non-goals
- Drop any safety feature (stale-anchor, ambiguous detection) — never.
- Beat ripgrep on raw scan speed — ripgrep uses SIMD and 10 years of tuning.
- Optimize files < 1k lines — already sub-2ms; not an agent bottleneck.
- Make daemon mode mandatory — opt-in only, for hot-loop workloads.

---

## 2. Bottleneck analysis (measured via strace, not guessed)

### `linehash read 100k`  (wall ≈ 30 ms)
| Syscall class | Time | % | Calls | Note |
|---|---:|---:|---:|---|
| futex (rayon thread sync) | 8.8 ms | **55%** | 3 | parallel hash above the 20k-line threshold |
| write (stdout flush) | 6.1 ms | **38%** | 213 | BufWriter 64 KB → ~110 flushes for 6.9 MB output |
| mmap/munmap/mprotect | 0.9 ms | 5% | 36 | mmap input + thread stacks |
| other | 0.3 ms | 2% | — | open/read/close |

CPU work (wall − syscall) ≈ **14 ms** spent on:
- xxhash for 100k lines in parallel (~8-10 ms per criterion `hash_10k_lines = 11.5 ms`)
- formatting `n:hash| ` prefix + content per line
- allocating 100k `String` instances for `LineRecord.content`

### `linehash edit 100k`  (wall ≈ 35 ms)
| Syscall class | Time | % | Calls |
|---|---:|---:|---:|
| write (atomic write of 5.9 MB) | 11.6 ms | **60%** | 4 |
| futex (rayon) | 5.8 ms | **30%** | 3 |
| mmap | 0.6 ms | 3% | 25 |
| fsync | 0.5 ms | 3% | 2 |

CPU work (delta) ≈ **16 ms** = parse + hash + render `Vec<u8>` + verify anchor.

### `linehash grep 100k --no-index` (literal pattern, wall ≈ 43 ms vs ripgrep 19 ms)
- linehash: 105 writes × 1493 µs/write avg
- ripgrep: 356 writes × 396 µs/write avg
- ripgrep batches output more aggressively and scans with SIMD.

---

## 3. Tier 1 — quick wins (1-2 days, small changes)

Tier 1 target: 20-40% latency reduction without refactoring lifetimes/types.

### T1.1 — Bump stdout BufWriter from 64 KB to 1 MB
- **File**: `crates/core/main.rs:81`
- **Change**: `BufWriter::with_capacity(64 * 1024, stdout_lock)` → `1024 * 1024`
- **Expected**: read 100k drops 213 writes → ~7 writes; saves ~5 ms (fewer syscalls + less write-copy overhead).
- **Risk**: low. +960 KB RAM per process (acceptable for a CLI).
- **Validation**: rerun `bench-cli-harness.py`; expect read 100k median 30 → ~25 ms.

### T1.2 — `print_read` writes `&content` directly, drop `writeln!`
- **File**: `crates/core/output.rs:74-86`
- **Change**: replace `writeln!(writer, "{number:>width$}:{hash}| {content}", ...)` with:
  ```rust
  let mut buf = itoa::Buffer::new();
  let n_str = buf.format(index + 1);
  // pad
  for _ in n_str.len()..width { writer.write_all(b" ")?; }
  writer.write_all(n_str.as_bytes())?;
  writer.write_all(b":")?;
  writer.write_all(&hash_bytes)?;       // 2-byte ASCII hex
  writer.write_all(b"| ")?;
  writer.write_all(line.content.as_bytes())?;
  writer.write_all(b"\n")?;
  ```
- **Expected**: read 100k -10 to -15% (~3 ms). Removes format machinery + transient heap allocs.
- **Risk**: low. Output bytes identical; covered by integration fixtures.
- **Validation**: existing `tests/smoke.rs::read_fixture_pretty_output_includes_anchors` unchanged.

### T1.3 — `print_read_json_streaming` uses `itoa`/`ryu` instead of `serde_json::to_writer` for line objects
- **File**: `crates/core/output.rs:119-165`
- **Change**: hard-code the JSON shape `{"n":N,"hash":"XX","content":...}`, escape the content via a helper, write the rest by hand.
- **Expected**: read 100k --json 60 ms → ~36 ms (-40%).
- **Risk**: medium. JSON escaping must handle control chars, quotes, backslash, unicode correctly. Use `serde_json::to_string` for the content string field; hard-code everything else.
- **Validation**: golden tests + JSON output fuzz.

### T1.4 — Document the daemon pattern for hot loops
- **File**: `README.md`, `AGENTS.md` agent rules
- **Change**: add a "Calling in a loop" section: `linehash daemon &` then `linehash --use-daemon ...` (or auto-detect a running daemon).
- **Expected**: 5-10× speedup on the 2nd+ invocations (skip parse + cache trigram index).
- **Risk**: low. Daemon mode already exists (`server.rs`).
- **Validation**: add bench `daemon_warm_read_100k`.

**Tier 1 totals** (estimate): read 100k 30 → 18-20 ms; read --json 60 → 36 ms; edit 100k roughly unchanged (~5%).

---

## 4. Tier 2 — medium effort (3-5 days, scoped refactor)

Tier 2 target: hit P0 (edit < 18 ms) and P1 (read < 18 ms).

### T2.1 — Atomic write that reuses mmap for single-line mutations
- **File**: `crates/core/commands/edit.rs`, `commands/insert.rs`, `commands/delete.rs`
- **Problem**: a 1-line edit still calls `render()` for the full 5.9 MB then `atomic_write`.
- **Change**: for a single-line edit, write `mmap[0..line_start] + new_content + sep + mmap[next_line_start..]` directly. Use `Vec::with_capacity` sized exactly.
- **Expected**: edit 100k 35 → ~17 ms (-50%, hits P0).
- **Risk**: high. Must handle:
  - newline byte boundary (LF vs CRLF)
  - trailing-newline preservation
  - multi-line content for range edits
  - receipts still need before/after hash → compute over mmap, no need to render.
- **Validation**: extend `tests/smoke.rs` with all combos (LF/CRLF × trailing/no-trailing × first/middle/last line).

### T2.2 — Zero-copy LineRecord (phase 1: shared mmap via Arc)
- **File**: `crates/core/document.rs:60`
- **Problem**: 100k `String::to_owned` allocations.
- **Change** (incremental):
  ```rust
  pub struct LineRecord {
      pub content: Cow<'static, str>,  // or Bytes / Arc<str>
      pub full_hash: u32,
      pub short_hash: ShortHash,
  }
  ```
  Step 1: use `Bytes` (`bytes` crate) or `Arc<str>` so mmap-backed slices can be shared without lifetime parameters on Document.
  Step 2: update mutation paths to materialize owned strings only when modifying.
- **Expected**: read 100k -30 to -50% (eliminates 100k mallocs); load 100k Document -25%.
- **Risk**: medium-high. Requires auditing every callsite that builds/modifies LineRecord.
- **Validation**: bench `parse_document_100k` < 30 ms (currently 39.6 ms).

### T2.3 — Drop `full_hash` u32 if unused
- **File**: `crates/core/document.rs:60`, `hash.rs`
- **Problem**: `LineRecord` keeps `full_hash: u32` but it may only be used to derive short_hash (u8).
- **Change**: grep all usages; if only used to derive short, drop the field. If used for collision detection, keep it.
- **Expected**: read 100k -3 to -5% (memory bandwidth + cache); load -5%.
- **Risk**: low if grep is thorough.
- **Validation**: all tests pass; binary size shrinks by a few KB.

### T2.4 — Streaming render: skip the `Vec<u8>` intermediate
- **File**: `commands/*.rs` (anywhere `doc.render()` is called)
- **Change**: add `Document::write_to(W: Write)` that streams directly into a sink (file or stdout) instead of building a Vec then writing it.
- **Expected**: edit 100k -10% (drops a 5.9 MB second copy).
- **Risk**: medium. Atomic write needs an intermediate file path; can `write_to` a tempfile then rename.
- **Validation**: existing atomic-write tests + keep `Document::render()` deprecated path for receipts.

**Tier 2 totals** (estimate): edit 100k 35 → 15-17 ms (hits **P0**); read 100k 30 → 14-16 ms (hits **P1**).

---

## 5. Tier 3 — major upgrades (1-2 weeks, P2 + future-proofing)

### T3.1 — SIMD trigram scan
- **File**: `crates/core/search/filter.rs`, `search/decompose.rs`
- **Problem**: filter is currently scalar; ripgrep uses AVX2/SSE2 for byte-pair search.
- **Change**: use `memchr2`/`memchr3` for 2-3-byte literals; add `wide`/`std::simd` for trigram lookup tables.
- **Expected**: grep regex 100k 21.7 → 8-10 ms (hits **P2**).
- **Risk**: medium. Feature-gated under `#[cfg(target_feature = "avx2")]`; scalar fallback.
- **Validation**: bench `grep_100k_*` for rare and common patterns.

### T3.2 — Persistent line-hash sidecar `.linehash/hashes/<rel>.bin`
- **Problem**: each read re-hashes the whole file.
- **Change**: store `[u8; line_count]` short hashes + content_hash in a sidecar. Invalidate on mtime/size change.
- **Expected**: warm read 100k 30 → ~6-8 ms (mmap + content_hash memcmp + write output).
- **Risk**: high. Cache invalidation, concurrent access, disk space. Reuse the `.linehash/indexes` infrastructure pattern.
- **Validation**: golden round-trip + crash safety (atomic sidecar writes).

### T3.3 — `splice(2)` / `copy_file_range(2)` for the unchanged portion of atomic write
- **Problem**: edit copies the full 5.9 MB through userspace.
- **Change** (Linux only): use `copy_file_range` to copy mmap → tempfile entirely in the kernel; userspace only writes the changed bytes.
- **Expected**: edit 100k 35 → 8-10 ms (-70% on Linux).
- **Risk**: medium. Feature-gated to Linux; fall back to current path. Must handle short-copy edge cases.
- **Validation**: bench Linux + macOS fallback path.

### T3.4 — `read --json` zero-allocation
- Already covered by T1.3 in basic form; full version uses a buffer pool.

**Tier 3 totals**: grep 100k 21.7 → 8 ms (hits **P2**); warm edit < 10 ms (exceeds **P0**).

---

## 6. Proposed roadmap

| Week | Tier | PR | Content | Verify |
|---|---|---|---|---|
| 1 | T1.1 + T1.2 | PR-A | BufWriter 1 MB + manual print_read | rerun bench-cli-harness, expect read 100k -15% |
| 1 | T1.3 | PR-B | streaming JSON for read | expect read --json 100k -40% |
| 1 | T1.4 | PR-C | docs + daemon UX polish | smoke test daemon warm path |
| 2 | T2.4 | PR-D | streaming render `Document::write_to` | edit 100k -10%, all tests pass |
| 2-3 | T2.1 | PR-E | mmap-backed atomic write for edit | edit 100k -50%, **P0 reached** |
| 3 | T2.3 | PR-F | drop full_hash if unused | small read/edit improvement |
| 4 | T2.2 | PR-G | zero-copy LineRecord (Arc<str>) | read 100k -30%, **P1 reached** |
| 5-6 | T3.1 | PR-H | SIMD trigram scan | grep -65%, **P2 reached** |
| 7 | T3.2 | PR-I | persistent line-hash sidecar | warm read < 8 ms |
| 8 | T3.3 | PR-J | copy_file_range for edit | edit on Linux < 10 ms |

Total: **~8 weeks** for the full roadmap. Tier 1 (1 week) is worth doing first to validate the measurement methodology before larger refactors.

---

## 7. Measurement & gating

Every PR must:

1. **Rerun** `bench-results/bench-cli-harness-2026-05-16-10-26-21.py` (on the same machine when possible).
2. **Compare against the baseline** report already on disk.
3. **No regression** in:
   - `cargo bench --bench edit_bench` (median ±5%)
   - `cargo bench --bench stats_bench`
   - `cargo bench --bench verify_bench`
   - `cargo bench --bench outline_bench`
4. **All tests pass**: `cargo test --release` before merge.
5. **UBS clean**: `ubs $(git diff --name-only --cached)` exits 0.

Proposed CI gate: check in `bench-results/baseline.json`; new PRs must attach `bench-results/pr-XXX-results.json` so CI can compare and gate at ±10% for the key metrics.

---

## 8. Tier 3 product decisions

The P0/P1/P2 ordering above is the priority order for implementation. Tier 3
work remains optional and must preserve portable fallbacks:

- Linux-specific acceleration such as `copy_file_range` is acceptable only
  behind platform-gated code with the existing portable path retained.
- New dependencies are acceptable when they have focused tests and measurable
  benchmark evidence. Prefer safe crates before custom unsafe code.
- Daemon mode remains opt-in, including for large files, until a separate UX
  bead explicitly changes the default.
- A persistent line-hash sidecar is acceptable to explore, but not required for
  P0/P1. It must have atomic writes, invalidation tests, and cleanup docs.
- The current roadmap ranks edit latency, read latency, and grep latency above
  memory footprint, daemon startup, and MCP latency.

Historical source questions:

- **Q1**: Are the P0/P1/P2 priorities in §1 right, or are there other metrics (memory footprint, daemon startup, MCP latency) that matter more?
- **Q2**: Is platform-specific code (`copy_file_range` Linux-only) acceptable, or must everything be portable?
- **Q3**: Are new dependencies (`bytes`, `wide`) acceptable for SIMD/zero-copy?
- **Q4**: Should daemon mode become the default for files > N lines, or always opt-in?
- **Q5**: Do we want to spend disk budget on a persistent line-hash sidecar (T3.2)?

---

## 9. General risks

| Risk | Mitigation |
|---|---|
| Refactor breaks the safety contract | Each PR runs the full `tests/smoke.rs` and adds edge-case tests (CRLF, trailing newline, ambiguous, stale) |
| Benchmark variance on WSL2 | Run 7 measured + 1 warmup; report best/median/stdev; cross-check with criterion (already wired in) |
| Zero-copy creates lifetime hell | Use `Arc<str>` instead of `&'a str` so Document stays owned; migrate gradually |
| Performance regression on small files | Bench 100/500/1k lines too; threshold-based fast paths if needed |

---

## Quick reference

- Profiling commands: `strace -c -e trace=read,write,openat,close,mmap,munmap,fstat,brk,futex linehash <cmd>`
- Bench harness: `bench-results/bench-cli-harness-2026-05-16-10-26-21.py`
- Criterion benches: `cargo bench --bench {hash,stats,verify,edit,outline}_bench`
- Hot files: `crates/core/{document.rs, output.rs, main.rs, commands/edit.rs, search/filter.rs}`
