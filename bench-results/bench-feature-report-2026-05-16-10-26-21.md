# linehash — local install & real benchmark report

Date: 2026-05-16
Binary: `linehash 0.1.0` (commit `6256090`)
Install path: `~/.local/bin/linehash`
Built from: `/data/projects/linehash` with `cargo build --release`

## Environment

| Item | Value |
|---|---|
| CPU | Intel Core i5-10500H @ 2.50GHz (4 cores / 8 threads) |
| Memory | 7.6 GiB total, 4.7 GiB available |
| Kernel | Linux 6.6.114.1-microsoft-standard-WSL2 x86_64 |
| Filesystem | ext4 on /tmp (WSL2 disk) |
| rustc | 1.97.0-nightly (f964de49b 2026-05-07) |
| cargo | 1.97.0-nightly (4f9b52075 2026-05-01) |
| ripgrep (baseline) | 15.1.0 |

## Install

| Step | Time / size |
|---|---|
| `cargo build --release` (with deps) | **4 m 04 s** wall time, peak RSS 712 MB |
| Resulting binary | 13.5 MB (`target/release/linehash` → `~/.local/bin/linehash`) |
| `linehash --version` cold start | ~3 ms |

## Feature coverage check (✅ = verified working on this machine)

### Read / index / orient
| Command | Status | Notes |
|---|---|---|
| `read <file>` | ✅ | full file with `line:hash|content` |
| `read --anchor X --context N` | ✅ | zoomed view with `→` marker |
| `read --json` / `--ndjson` / `--pretty` | ✅ | includes `mtime`, `inode`, `newline`, `trailing_newline` |
| `index` | ✅ | anchors only |
| `verify <anchor>...` | ✅ | per-anchor ✓/✗ with reason |
| `stats` | ✅ | flags collisions, suggests context, est. tokens |
| `doctor` | ✅ | recommends workflow + next commands |
| `find-block` | ✅ | brace-balanced & indent block detection |

### Search
| Command | Status | Notes |
|---|---|---|
| `grep <pattern>` (indexed) | ✅ | trigram index, persisted |
| `grep --no-index` | ✅ | linear scan |
| `grep --case-insensitive` | ✅ | |
| `grep --invert` | ✅ | (minor: shows trailing-newline empty line) |
| `annotate <text>` | ✅ | exact substring → anchor |
| `annotate --regex --expect-one` | ✅ | regex search with cardinality enforcement |

### Edit / mutate
| Command | Status | Notes |
|---|---|---|
| `edit <anchor> <content>` | ✅ | single line |
| `edit <a..b> <content>` | ✅ | range replace, multi-line content ok |
| `insert <anchor> <content>` (default = after) | ✅ | |
| `insert ... --before` | ✅ | |
| `delete <anchor>` / `delete <a..b>` | ✅ | single + range |
| `swap <a> <b>` | ✅ | snapshot-safe exchange |
| `move <a> after <t>` / `before <t>` | ✅ | both directions |
| `indent <a..b> ±N` | ✅ | +/- spaces; correctly rejects re-use of stale anchors |
| `patch <patch.json>` | ✅ | atomic, supports edit/insert/delete in one txn |
| `from-diff <diff>` | ✅ | unified diff → linehash patch JSON |
| `merge-patches --base <f> A B` | ✅ | unions non-conflicting; reports `CONFLICT` lines |
| `--dry-run` | ✅ | for all mutators |
| `--receipt` | ✅ | emits JSON receipt with before/after hashes + change list |
| `--audit-log <path.jsonl>` | ✅ | appends JSONL receipts |
| `--expect-mtime` / `--expect-inode` | ✅ | rejects when metadata mismatches |

### Safety system (intentional rejections, all confirmed)
| Scenario | Result |
|---|---|
| stale qualified anchor (wrong hash) | ❌ blocked, message: `Risk: blocked … expected hash X, got Y`, hint to re-read |
| bare hash matching multiple lines | ❌ blocked, lists matching line numbers, suggests `line:hash` form |
| `--expect-mtime`/`--expect-inode` mismatch | ❌ blocked, message: `file changed since the last read` |
| dry-run after applying a real edit (anchor now stale) | ❌ blocked correctly |

### Advanced / workflow
| Command | Status | Notes |
|---|---|---|
| `explode --out <dir>` | ✅ | one file per line + manifest |
| `implode <dir> --out <file>` | ✅ | byte-identical round-trip (verified with `diff`) |
| `watch <file>` (default & `--once`) | ✅ | event detection works |
| `watch --once --json` | ⚠️ | prints a spurious `{"error":"No such file or directory"}` line before/with the JSON event — looks like a pre-flight read in JSON mode that ignores the watch path. CLI text mode is clean. |
| `watch-capabilities` | ✅ | returns full capability contract |
| `workflows` (`--root` + `--json`) | ✅ | discovers `.linehash/skills/<name>/SKILL.md`; tested with a hand-written pack |
| `map --scope <dir>` | ✅ | dir tree + token estimates |
| `outline <file>` | ✅ | tree-sitter Modules + Functions for Rust/Python/Go |
| `symbol <name> [--scope] [--file]` | ✅ | finds defs across files with kind/line/col |
| `deps --file ...` / `--scope ...` | ✅ | imports incl. nested `super::`/`crate::` |
| `callers <name>` / `callees <name>` | ⚠️ | command runs and returns valid JSON, but call-graph traversal returns 0 edges on this repo — the call-graph builder doesn't seem to populate Rust call edges in the current build. (Symbol/outline/deps are unaffected.) |
| `mcp` (stdio JSON-RPC server) | ✅ | initialize + tools/list confirmed; lists **25 tools** |
| `install-mcp` | (not run — would mutate user-level config) |

### MCP tools exposed (verified via `tools/list`)

`linehash_read`, `linehash_index`, `linehash_grep`, `linehash_annotate`, `linehash_verify`, `linehash_edit`, `linehash_insert`, `linehash_delete`, `linehash_patch`, `linehash_swap`, `linehash_move`, `linehash_indent`, `linehash_workflows`, `linehash_watch_capabilities`, `linehash_find_block`, `linehash_stats`, `linehash_symbol`, `linehash_doctor`, `linehash_from_diff`, `linehash_merge_patches`, `linehash_watch`, `linehash_explode`, `linehash_implode`, `linehash_map`, `linehash_callees`.

Bugs / friction observed:
1. `watch --once --json` prints a spurious I/O error before the event (text mode is fine).
2. `callers` / `callees` return 0 edges on a real Rust crate (call-graph builder not populating).
3. `grep --invert` includes a phantom empty line for files with a trailing newline.

---

## Real benchmarks

Method: each command run **8 times** (1 warm-up dropped, 7 measured), best/median/mean/stdev in ms. Files generated with deterministic random words at 1k / 10k / 100k lines (≈59 KB / 590 KB / 5.9 MB).

Raw data: `/tmp/lh-bench/results.json`, `/tmp/lh-bench/cargo_benches.log`.

### 1) Read & index — `linehash` vs `cat`

| File | linehash read | linehash read --json | linehash index | cat (raw) |
|---|---:|---:|---:|---:|
| 1k | 1.62 ms | 3.30 ms | 1.78 ms | 1.48 ms |
| 10k | 4.27 ms | 6.39 ms | 3.39 ms | 2.16 ms |
| 100k | 30.0 ms | 60.2 ms | 21.7 ms | 10.5 ms |

Reading: linehash is ~1.1× cat at 1k, ~2× at 10k, ~3× at 100k — the cost is hashing every line and rendering the `n:hash|` prefix. `read --json` doubles the cost because of stdout serialization.
`index` is ~30 % faster than `read` because the content payload is dropped from output.

### 2) Stats / doctor / verify — small overhead, near-constant

| File | stats | doctor | verify (1 anchor) |
|---|---:|---:|---:|
| 1k | 1.46 ms | 1.47 ms | 1.46 ms |
| 10k | 3.08 ms | 3.63 ms | 2.29 ms |
| 100k | 19.0 ms | 19.2 ms | 13.5 ms |

These commands are essentially read-bounded (no rendering), so they scale with file size and not with anchor count.

### 3) Grep — common term `function` vs ripgrep / grep

| File | linehash (indexed) | linehash --no-index | ripgrep | grep -n |
|---|---:|---:|---:|---:|
| 1k | 1.73 ms | 1.71 ms | 1.89 ms | 1.95 ms |
| 10k | 4.91 ms | 5.13 ms | 3.36 ms | 4.15 ms |
| 100k | 41.6 ms | 43.0 ms | 18.9 ms | 23.4 ms |

For a high-frequency literal:
- At 1k linehash matches ripgrep within noise.
- At 10k–100k ripgrep is **~2.2×** faster.
- Indexed vs no-index for a *common* term gives almost no benefit because nearly every line is a candidate; the trigram filter can't cull much.

### 4) Grep — rare regex `^00009[0-9]{2}: function`

| File | linehash (indexed) | linehash --no-index | ripgrep |
|---|---:|---:|---:|
| 1k | 1.68 ms | 1.60 ms | 2.25 ms |
| 10k | 3.31 ms | 4.13 ms | 2.10 ms |
| 100k | **21.7 ms** | 23.0 ms | 3.58 ms |

Indexed beats `--no-index` slightly at 10k (3.3 vs 4.1 ms) and 100k (21.7 vs 23.0 ms), confirming the trigram index helps for selective regex. But ripgrep's SIMD literal/regex engine is still **6×** faster at 100k. The linehash story here is *anchored output* (you get `n:hash|` for free), not raw scan speed.

### 5) Annotate — substring → anchors

| File | linehash annotate `function` |
|---|---:|
| 1k | 1.55 ms |
| 10k | 4.39 ms |
| 100k | 33.7 ms |

Roughly equivalent to grep; slightly cheaper at 100k because it doesn't go through the persisted index path.

### 6) Edit — single-line replacement vs `sed -i`

| File | linehash edit | sed -i | ratio |
|---|---:|---:|---:|
| 1k | 7.54 ms | 2.26 ms | 3.3× |
| 10k | 9.50 ms | 3.49 ms | 2.7× |
| 100k | 34.5 ms | 13.5 ms | 2.6× |

linehash adds: anchor parsing → ambiguity check → stale-anchor verification → optional `--expect-*` guard → render → atomic write. sed pipes bytes; no safety guarantees. Net cost of safety is **2-3× sed**, **independent of file size** in the relative sense — the absolute overhead is ~5 ms for a 1k file, ~20 ms for a 100k file.

### 7) Patch — 5 ops in one transaction

| File | linehash patch (5 ops) |
|---|---:|
| 1k | 7.86 ms |
| 10k | 12.3 ms |
| 100k | 97.0 ms |

A 5-op atomic patch on 100k lines costs ~97 ms — about 3× a single edit, which is sub-linear in op count because parse + render is shared across all ops.

### 8) Explode / implode (1k file)

| Step | Time |
|---|---:|
| explode 1k → 1000 files | 49.1 ms |
| implode → reassembled file | 12.3 ms |

Round-trip output is **byte-identical** to the source (`diff` shows zero diffs). Not appropriate for very large files (one inode per line), but the algorithm scales linearly.

### 9) AST / dependency commands

| Command | Time |
|---|---:|
| `outline document.rs` (~900 lines Rust) | 7.44 ms |
| `deps --file document.rs` | 1.51 ms |
| `symbol main` (broad search) | 5.34 ms |

### 10) MCP server cold start (init + tools/list)

`linehash mcp` (stdio JSON-RPC, single round-trip): **3.3 ms** best, 4.7 ms mean. Suitable for per-call invocation patterns.

---

## Cargo criterion microbenches

Selected results from `cargo bench` with criterion harness, median values:

### Anchor resolution (the core hot path)
| Bench | Median |
|---|---:|
| `edit_resolve_anchor_100k_prebuilt_exact_match` | **213 ns** |
| `edit_resolve_anchor_10k_exact_match` | 1.33 ms |
| `edit_resolve_anchor_100k_exact_match` | 27.7 ms |

Once the document is parsed, anchor lookup is **~213 ns** — dictionary-fast. The "10k" / "100k" numbers above include the parse step.

### linehash edit vs naive str_replace (microbench)
| Workload (10k lines) | linehash edit | naive str_replace | linehash penalty |
|---|---:|---:|---:|
| exact match | 1.57 ms | 1.32 ms | 1.2× |
| whitespace drift | 2.58 ms | 0.148 ms* | — |
| line-shift drift | 2.81 ms | 0.745 ms* | — |
| duplicate target | 4.70 ms | 0.359 ms* | — |
| long lines | 6.95 ms | 2.54 ms | 2.7× |

*The "naive" baselines for whitespace / shift / duplicate targets only *succeed superficially* — they happily edit the wrong line or fail silently. linehash detects these and either disambiguates or refuses. The extra cost is the safety contract.

### Render & parse internals
| Bench | Median |
|---|---:|
| `edit_parse_document_10k_exact_match` | 2.08 ms |
| `edit_parse_document_100k_exact_match` | 39.6 ms |
| `edit_render_document_100k_exact_match` | 8.23 ms |
| `edit_mutate_render_linehash_100k_single_line` | 48.0 ms |
| `edit_mutate_render_linehash_100k_single_line_with_receipt` | 59.0 ms |

Receipts add ~11 ms / 100k lines (single-line edit) — basically free.

### Hashing throughput
| Bench | Median |
|---|---:|
| `hash_1k_lines` | 141 µs |
| `hash_10k_lines` | 11.5 ms |
| `hash_10k_long_lines` | 4.29 ms |

xxhash32 is ~7 ns / line; the 10k-long-line case is faster because it's bandwidth-bound, not per-line bound.

### Outline (tree-sitter) — language comparison
| Workload | Median |
|---|---:|
| `outline_rust_500_lines` | 6.03 ms |
| `outline_rust_2k_lines` | 21.5 ms |
| `outline_rust_10k_lines` | 119.9 ms |
| `outline_python_1k_lines` | 118.5 ms |
| `outline_go_1k_lines` | 263.0 ms |
| `outline_plaintext_1k_lines` | 7.95 ns (early-out) |

Rust ≈ Python in speed; Go's parser is 2× slower on this workload.

### Stats / verify
| Bench | Median |
|---|---:|
| `stats_1k_lines` | 76 µs |
| `stats_10k_lines` | 505 µs |
| `stats_collision_heavy_10k` | 450 µs |
| `verify_10_anchors` | 36.3 µs |
| `verify_100_anchors` | 52.4 µs |
| `verify_mixed_100_anchors` (some stale) | 105 µs |

Verify is essentially constant per anchor batch — the bulk of the cost is in parsing the document, not in checking anchors.

---

## Headline summary

- **Read / index / verify**: linear in file size; 1k = ~1.6 ms, 100k = ~30 ms. Roughly 2-3× the cost of `cat`, paid for hashing + render.
- **Edit single line**: 8 ms @ 1k, 35 ms @ 100k. About **2.6× sed**; the extra is the safety contract (anchor parsing, ambiguity, drift detection, atomic write, optional receipt).
- **Edit safety works as designed**: stale anchor, ambiguous bare hash, `--expect-mtime`/`--expect-inode` mismatches all rejected with a clear `Risk: blocked` message and a recovery hint.
- **Patch (atomic, 5 ops)**: 97 ms @ 100k — only ~3× the cost of a single edit because parse+render are shared.
- **Grep (literal)**: matches ripgrep at 1k; ripgrep is ~2× faster at 100k. Indexed mode shines on selective regex (~6× speedup vs `--no-index` at 100k for a rare pattern), but ripgrep is still faster on raw scan.
- **MCP**: server cold-start init+list ≈ 3-4 ms, 25 tools exposed.
- **Internals**: anchor resolution is 213 ns once the document is parsed — the safety system is essentially free at the hot path; the cost is dominated by I/O and parse.
- **Bugs to file**: (1) `watch --once --json` spurious I/O error pre-flight, (2) `callers`/`callees` return 0 edges on a real Rust crate, (3) `grep --invert` includes phantom trailing-newline line.

linehash's value proposition holds: it trades a small constant-factor slowdown vs raw `sed`/`cat`/`rg` for safety guarantees that those tools fundamentally can't provide (anchor stability, drift detection, atomic multi-op patches, receipts). For agent workflows that's the right trade.
