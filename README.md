# linehash

> Stable line-addressed file reading and editing for Claude Code, AI coding agents, and patch-safe automation.
> Every line gets a 2-char content hash, so edits target anchors instead of fragile whitespace-exact string replacement.

`linehash` is a Rust CLI for safe file editing with content-hashed line anchors. It helps Claude Code and other AI coding tools read files, locate lines, apply edits, and reject stale changes before they corrupt code.

## Installation

### From GitHub releases

#### Linux / macOS

Install the latest release with the generated installer:

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/linehash/main/install.sh?$(date +%s)" | bash
```

#### Windows (PowerShell 5.1+)

```powershell
irm "https://raw.githubusercontent.com/quangdang46/linehash/main/install.ps1" | iex
```

To pin a version or pass flags, download once and run:

```powershell
irm "https://raw.githubusercontent.com/quangdang46/linehash/main/install.ps1" -OutFile install.ps1
.\install.ps1 -Version v0.1.10 -EasyMode -Verify
```

The installer downloads the matching GitHub release asset for your platform, verifies the SHA-256 sidecar when available, can optionally add the install directory to your shell PATH (Bash/Zsh on Unix, user PATH on Windows), then auto-detects supported MCP providers and installs the `linehash` MCP entry for each detected host.

| Flag (sh / ps1)              | Effect                                                        |
|------------------------------|---------------------------------------------------------------|
| `--version vX.Y.Z` / `-Version` | Pin a specific release (default: latest)                   |
| `--dest <path>` / `-Dest`       | Install to a custom directory                              |
| `--system` / `-System`          | Install to `/usr/local/bin` / `%ProgramFiles%\linehash`    |
| `--easy-mode` / `-EasyMode`     | Append install dir to user PATH                            |
| `--verify` / `-Verify`          | Run `linehash --version` after install                     |
| `--from-source`                 | Build from source via `cargo` (Unix only)                  |
| `--quiet` / `-Quiet`            | Suppress info logs                                         |
| `--uninstall` / `-Uninstall`    | Remove the binary and any easy-mode PATH lines             |

### From source

```bash
cargo install --path crates/core
```

## Why linehash

- Built for **Claude Code** and **AI coding agents**
- Safer than `str_replace` for **file editing** and **patch workflows**
- Uses **content-hashed line anchors** instead of fragile exact-text matching
- Detects **stale reads**, **ambiguous anchors**, and **concurrent file changes**
- Written in **Rust** with simple CLI and JSON output for automation

## linehash vs str_replace vs patch editing

| Tool / workflow | How it locates code | Main failure mode | Best use case |
|---|---|---|---|
| `str_replace` | Exact old text match | Fails when whitespace or formatting differs | Small literal replacements when exact text is known |
| Unified diff / patch | Context lines around a hunk | Hunks can fail or apply badly after nearby edits | Reviewable multi-line changes and code review workflows |
| `linehash` | Content-hashed line anchors like `12:ab` | Rejects stale or ambiguous anchors instead of guessing | Safe AI-assisted file editing, targeted edits, and patch-safe automation |

**Why this matters for AI coding:** models often know *what* to change but are less reliable at reproducing the exact old text required by `str_replace`. `linehash` reduces that failure mode by letting tools edit by anchor, verify file state, and stop on stale reads before code is corrupted.

---

## The Problem

Claude Code uses `str_replace` to edit files — the model must reproduce the **exact** old text,
character by character, including whitespace and indentation.

The "String to replace not found in file" error has its own GitHub issues megathread
with 27+ related issues. It's not the model being dumb — it's the format demanding perfect recall.

From Can Bölük's harness benchmark across 16 models:
- `str_replace` failure rate: up to **50.7%** on some models
- Root cause: models can't reliably reproduce exact whitespace

## The Fix: Content-Hashed Lines

When Claude reads a file via `linehash read`, every line gets a stable 2-char hash:

```
1:a3| function verifyToken(token) {
2:f1|   const decoded = jwt.verify(token, process.env.SECRET)
3:0e|   if (!decoded.exp) throw new TokenError('missing expiry')
4:9c|   return decoded
5:b2| }
```

When Claude edits, it references hashes as anchors:

```bash
# Replace a single line
linehash edit src/auth.js 2:f1 "  const decoded = jwt.verify(token, env.SECRET)"

# Replace a range
linehash edit src/auth.js 2:f1..4:9c "  return jwt.verify(token, env.SECRET)"

# Insert after a line
linehash insert src/auth.js 3:0e "  if (!decoded.iat) throw new TokenError('missing iat')"

# Delete a line
linehash delete src/auth.js 3:0e
```

If the file changed since last read, hashes won't match → edit **rejected** before corruption.

## Why This Is Better Than str_replace

| | str_replace | linehash |
|---|---|---|
| Model must reproduce whitespace | ✅ required | ❌ not needed |
| Stable after file changes | ❌ line numbers shift | ✅ hash tied to content |
| Edit failure rate | Up to 50% | Near 0% |
| Detects stale reads | ❌ | ✅ hash mismatch = reject |
| Token cost | High (full old content) | Low (just hash + new line) |

## How Hashes Work

Each hash is a **2-char truncated xxHash** of the raw line content:

```
line content → xxhash32 → take low byte as 2 hex chars
"  return decoded" → 0x...9c → "9c"
```

- Same content = same hash (stable across reads)
- Different content = different hash (edit safety)
- 2 chars = 256 possible values — good enough for line-level anchoring
- Collisions are rare and recoverable (linehash detects ambiguity)

## Tech Stack

| Crate | Purpose |
|---|---|
| `xxhash-rust` | Fast content hashing per line |
| `clap` | CLI |
| `serde_json` | `--json` output for scripts |

Pure Rust. No tree-sitter. No LLM. No external dependencies.
Simplest tool in the suite.

## Instant Grep (Trigram Index)

`linehash grep` uses a **trigram inverted index** for fast regex search, inspired by Cursor's instant grep algorithm. This provides 20-100× speedup over linear scanning for large files.

### How It Works

1. **Trigram Decomposition**: Each line is split into overlapping 3-byte sequences:
   ```
   "hello" → ["hel", "ell", "llo"]
   ```

2. **Inverted Index**: Maps each trigram to posting lists recording which lines contain it

3. **Candidate Filtering**: Uses bloom filters to quickly reject non-matching lines

4. **Regex Verification**: Full regex check only on candidate lines

### Auto-Indexing

- Index is built automatically on first search
- Content hash validates index freshness
- LRU cache prevents memory bloat (configurable capacity)
- Persistent storage available for instant warm restarts

### Using `--no-index`

For small files or one-off searches:
```bash
linehash grep --no-index file.txt "pattern"
```

### Hot-loop grep with the daemon

For repeated searches over the same files, keep the Unix daemon warm and route
grep through it:

```bash
linehash daemon >/tmp/linehash-daemon.log 2>&1 &
linehash grep src/auth.rs "verify_token" --daemon
```

`grep --daemon` auto-starts the daemon when it is not already running on Unix.
The daemon caches file contents in memory and verifies the same regex semantics
as the normal grep path before returning anchor-addressed matches. Other
commands still use the regular CLI paths today; do not document or script
daemon-backed read/edit unless those flags are added.

### Architecture

| Component | Purpose |
|---|---|
| `search/decompose.rs` | Regex → trigram decomposition |
| `search/filter.rs` | Candidate filtering using masks |
| `search/verify.rs` | Full regex verification on candidates |
| `search/cache.rs` | LRU cache with content-hash validation |
| `search/persist.rs` | Persistent index storage |

## MCP server

`linehash` now ships with a stdio MCP server that exposes the existing read/search/edit workflow as MCP tools:

```bash
linehash mcp
```

Use `linehash install-mcp` to auto-detect local MCP host configs, upsert a `linehash` server entry for every detected provider, and log the install results.

Current auto-install targets:
- `claude-code` via `~/.claude.json`
- `codex` via `~/.codex/config.toml`
- `cursor` via `~/.cursor/mcp.json`
- `windsurf` via `~/.codeium/windsurf/mcp_config.json`
- `vscode` via `.vscode/mcp.json`
- `gemini` via `~/.gemini/settings.json`
- `opencode` via `~/.opencode.json`
- `amp` via `~/.config/amp/settings.json`
- `droid` via `~/.factory/mcp.json`

Auto-detect is the default. Set `LINEHASH_MCP_HOST=codex` or a comma-separated host list only when you want to override detection and target a specific subset.

For watch behavior, the current split is intentional:
- CLI supports `linehash watch --continuous`
- MCP supports only single-event watch calls today
- `linehash watch-capabilities --json` or MCP `linehash_watch_capabilities` returns the evaluated capability contract and recommended fallback modes

## Usage

Common workflows for Claude Code, AI code editing, and patch-safe file automation:

```bash
# Read file with hash tags
linehash read src/auth.js

# Read just the neighborhood around one or more anchors
linehash read src/auth.js --anchor 2:f1 --context 2

# View just line numbers + hashes (no content) — for orientation
linehash index src/auth.js

# Check whether one or more anchors still resolve
linehash verify src/auth.js 2:f1 4:9c

# Search content and return anchors for matching lines
linehash grep src/auth.js "verifyToken"
linehash annotate src/auth.js "missing expiry"
linehash annotate src/auth.js "^export function" --regex --expect-one

# Edit by hash anchor
linehash edit <file> <hash-or-line:hash> <new_content>
linehash edit <file> <start-line:hash>..<end-line:hash> <new_content>
linehash insert <file> <hash-or-line:hash> <new_line>     # insert AFTER anchor line
linehash insert <file> <hash-or-line:hash> <new_line> --before
linehash delete <file> <hash-or-line:hash>

# Structural mutations
linehash swap <file> <anchor-a> <anchor-b>
linehash move <file> <anchor> before <target-anchor>
linehash move <file> <anchor> after <target-anchor>
linehash indent <file> <start-line:hash>..<end-line:hash> +2
linehash find-block <file> <anchor>

# Multi-op workflows
linehash patch <file> <patch.json>
# patch.json shape:
# {"ops":[{"op":"edit","anchor":"3:64","content":"  return message.toUpperCase()"}]}
linehash from-diff <file> <diff.patch>
linehash merge-patches <patch-a.json> <patch-b.json> --base <file>

# Inspect collision/token-budget guidance for large files
linehash stats src/auth.js

# Watch for live hash changes (v1 defaults to one change event, then exit)
linehash watch src/auth.js
linehash watch src/auth.js --continuous
linehash watch-capabilities --json

# List repo-local markdown workflow packs / skills
linehash workflows
linehash workflows --root /path/to/repo --json

# Explode / implode workflow
linehash explode src/auth.js --out out/auth.lines
linehash implode out/auth.lines --out src/auth.js --dry-run
```

## Integration with Claude Code

Add to your project's `CLAUDE.md`:

```markdown
## File Editing Rules

When editing an existing file with linehash:

1. Read: `linehash read <file>`
2. Copy the anchor as `line:hash` (for example `2:f1`) — do not include the trailing `|`
3. Edit using the anchor only; never reproduce old content just to locate the line
4. If the file may have changed, prefer `linehash read <file> --json` first and carry `mtime` / `inode` into mutation commands with `--expect-mtime` / `--expect-inode`
5. If an edit is rejected as stale or ambiguous, re-read and retry with a fresh qualified anchor

Example:
  linehash read src/auth.js
  # line 2 shows as `2:f1|   const decoded = ...`
  linehash edit src/auth.js 2:f1 "  const decoded = jwt.verify(token, env.SECRET)"
```

### Recommended agent workflow

- Use `read` for the full file view.
- Use `read --anchor ... --context N` when you already know the target anchor and want a smaller local window.
- Use `index` for fast orientation when content is not needed.
- Use `verify` to confirm anchors still resolve before building a larger edit plan.
- Use `grep` / `annotate` when you know content but need current anchors.
- Use `swap`, `move`, `indent`, and `find-block` instead of simulating structural edits with multiple fragile single-line operations.
- Use `patch`, `from-diff`, and `merge-patches` for multi-step or reviewable change sets.
- Use `stats` when a file is large, collisions are likely, or you want guidance on whether short hashes and small context windows are still ergonomic.
- Use `doctor` when you want a read-only recommendation for how to approach a file before reading or editing it.
- Use `explode` / `implode` only when you explicitly want a filesystem-native round-trip workflow.
- Use qualified anchors like `12:ab` whenever possible; they are safer than bare `ab` when collisions or stale reads matter.

## Workflow playbooks

## Markdown workflow packs

`linehash` now supports repo-local markdown skill packs under `.linehash/skills/<name>/SKILL.md`.
Each pack uses TOML frontmatter with bounded CLI and MCP surfaces, then a Markdown body with
the actual workflow instructions:

```toml
---
title = "Anchored Read"
description = "Orient before mutating."
allowed_cli_commands = ["linehash index", "linehash read"]
allowed_mcp_tools = ["linehash_index", "linehash_read"]
---
```

Use `linehash workflows` to inspect the loaded pack catalog locally, or call the MCP
tool `linehash_workflows` to retrieve the same catalog from an integration client.
The bundled packs cover anchored reads, verify-then-edit, patch transactions, and
stale-anchor repair.

### Targeted edit

1. `linehash read <file>`
2. Copy the qualified anchor as `line:hash`
3. `linehash edit <file> <line:hash> <new_content>`
4. `linehash verify <file> <line:hash>` or re-read the local neighborhood

### Search → anchor → edit

1. `linehash annotate <file> <text>` when you know exact content
2. `linehash grep <file> <pattern>` when you know a regex or broader pattern
3. `linehash read <file> --anchor <line:hash> --context N`
4. `linehash edit` / `linehash patch`

### Large-file workflow

1. `linehash stats <file>` to inspect token cost, collisions, and suggested context
2. `linehash doctor <file>` to get a read-only workflow recommendation
3. `linehash index <file>` if you only need orientation
4. `linehash read <file> --anchor <line:hash> --context N` instead of repeatedly dumping the whole file

### Stale-anchor recovery

1. Treat stale-anchor failures as the safety system working correctly
2. Re-run `linehash read <file>` or `linehash read <file> --json`
3. If the error reports relocated lines, rebuild a fresh qualified anchor from that neighborhood
4. Retry the mutation with the refreshed anchor

### Multi-op patch workflow

1. Use `annotate` / `grep` / `find-block` to collect target anchors
2. Build a patch JSON file
3. Run `linehash patch <file> <patch.json> --dry-run`
4. Apply the patch once the dry-run output looks correct
5. Use `merge-patches` when combining independently prepared change sets

### Structural edit workflow

- Use `find-block` before editing a function/class-sized region
- Use `move` or `swap` for reordering instead of rewriting text by hand
- Use `indent` after movement or when shifting a whole block
- Prefer `patch` over many tiny single-line edits when the change is coordinated

## Output Modes

```bash
# Pretty (default) — for Claude to read
linehash read src/auth.js
  1:a3| function verifyToken(token) {
  2:f1|   const decoded = jwt.verify(token, SECRET)
  ...

# JSON — for scripts and stale-guard workflows
linehash read src/auth.js --json
{
  "file": "src/auth.js",
  "newline": "lf",
  "trailing_newline": true,
  "mtime": 1714001321,
  "mtime_nanos": 0,
  "inode": 12345,
  "lines": [
    { "n": 1, "hash": "a3", "content": "function verifyToken(token) {" },
    { "n": 2, "hash": "f1", "content": "  const decoded = jwt.verify(token, SECRET)" },
    ...
  ]
}

# NDJSON event stream for agents / scripts
linehash watch src/auth.js --json
{"timestamp":1714001321,"event":"changed","path":"src/auth.js","changes":[...],"total_lines":847}
```

## Additional Commands

- `verify` checks whether anchors still resolve and returns a non-zero exit code if any do not.
- `grep` searches by regex using trigram index for speed (20-100× faster than linear on large files). Use `--no-index` to force linear scan.
- `annotate` maps exact substrings or regex matches back to current anchors.
- `doctor` recommends a read-only workflow for a file using current size/collision heuristics.
- `patch` applies a JSON patch transaction atomically.
- `swap` exchanges two lines in one snapshot-safe operation.
- `move` repositions one line before or after another anchor.
- `indent` indents or dedents an anchor-qualified range.
- `find-block` discovers a likely structural block around an anchor.
- `from-diff` compiles a unified diff into linehash patch JSON.
- `merge-patches` merges two patch files and reports conflicts.
- `explode` writes one file per source line plus metadata.
- `implode` validates and reassembles an exploded directory back into a file.

## Error Handling

```bash
# Hash not found
linehash edit src/auth.js xx "new content"
Error: hash 'xx' not found in src/auth.js
Hint: run `linehash read <file>` to get current hashes

# Ambiguous hash (collision)
linehash edit src/auth.js f1 "new content"
Error: hash 'f1' matches 3 lines in src/auth.js (lines 2, 14, 67)
Hint: use a line-qualified hash like '2:f1' to disambiguate

# File changed since read (stale qualified anchor)
linehash edit src/auth.js 2:f1 "new content"
Error: line 2 content changed since last read in src/auth.js (expected hash f1, got 3a)
Hint: re-read the file with `linehash read <file>` and retry the edit

# File metadata changed since JSON read / guard capture
linehash edit src/auth.js 2:f1 "new content" --expect-mtime 1714001321 --expect-inode 12345
Error: file 'src/auth.js' changed since the last read
Hint: re-read the file metadata and retry with fresh --expect-mtime/--expect-inode values
```

## Recovery loops

- **Stale anchor:** re-run `linehash read <file>` or `linehash read <file> --json`; if the error reports relocated line(s), use those to rebuild a fresh qualified anchor before retrying.
- **Ambiguous hash:** switch from bare `ab` to qualified `12:ab`.
- **Large file / too much output:** use `index`, `stats`, or `read --anchor ... --context N` instead of a full read.
- **Concurrent edits:** treat a stale-anchor or stale-file rejection as success of the safety system, not as something to bypass.

---

## Benchmarks

Real-feature numbers produced by `scripts/bench-features.sh` on a 4-vCPU Ubuntu 24.04 VM, `cargo build --release`, hyperfine 1.12 with `--warmup 1-2` / `--runs 5`. Each row reports **mean (min … max)** in milliseconds.

Fixtures (regenerated locally on first run):

- `small.rs` — 100 lines, ~6 KB
- `medium.rs` — 10 000 lines, ~660 KB
- `large.rs` — 100 000 lines, ~7.0 MB
- `core/` — the linehash `crates/core` source tree (used by the language-aware commands)

### Read & orient

| Command | Mean | Range |
|---|---:|---:|
| `read small.rs` | 1.12 ms | 0.92 – 1.44 |
| `read medium.rs` | 2.32 ms | 2.17 – 2.54 |
| `read large.rs` | 12.50 ms | 11.98 – 13.00 |
| `read large.rs --json` | 33.66 ms | 32.02 – 35.98 |
| `read large.rs --anchor … --context 5` | 11.17 ms | 10.68 – 11.57 |
| `index small.rs` | 1.14 ms | 0.89 – 1.59 |
| `index medium.rs` | 2.50 ms | 2.33 – 2.59 |
| `index large.rs` | 15.87 ms | 15.47 – 16.58 |

### Verify

| Command | Mean | Range |
|---|---:|---:|
| `verify large.rs <anchor>` | 11.80 ms | 11.18 – 12.82 |
| `verify large.rs <10 anchors>` | 11.51 ms | 11.09 – 11.93 |

### Search (`grep`, `annotate`)

Single regex match in `large.rs` (100 k lines). `rg` is included as a reference baseline — note that `linehash grep` returns anchor-addressed matches, not just byte offsets.

| Command | Mean | Range |
|---|---:|---:|
| `grep` trigram (cold cache) | 13.05 ms | 12.01 – 14.67 |
| `grep` trigram (warm cache) | 13.10 ms | 12.02 – 16.38 |
| `grep --no-index` | 11.48 ms | 11.08 – 12.38 |
| `grep --daemon` (warm) | 15.12 ms | 14.57 – 16.58 |
| `rg <pattern> large.rs` (ref) | 2.53 ms | 2.25 – 2.80 |
| `annotate large.rs <substring>` | 15.47 ms | 15.05 – 15.95 |

> On a single one-shot search of a 7 MB file the trigram index, `--no-index` linear scan, and `--daemon` paths land within ~4 ms of each other on this hardware — the per-call cost is dominated by file I/O and anchor formatting, not regex work. `rg` will win on raw throughput when you only need byte offsets; `linehash grep`'s value-add is that every match comes back as a `line:hash` anchor ready to feed into `edit` / `patch`. Persistent index + warm daemon mostly pay off in agent loops that issue many searches against the same file.

### Mutations

Each run is prepared with a fresh copy of `large.rs` (100 000 lines, 7 MB) so the I/O cost is realistic.

| Command | Mean | Range |
|---|---:|---:|
| `edit small.rs <anchor>` | 0.88 ms | 0.72 – 1.01 |
| `edit medium.rs <anchor>` | 2.28 ms | 2.14 – 2.44 |
| `edit large.rs <anchor>` | 15.74 ms | 15.46 – 16.34 |
| `edit large.rs <2k-line range>` | 50.49 ms | 48.82 – 53.10 |
| `insert large.rs <anchor>` | 59.51 ms | 56.29 – 63.66 |
| `delete large.rs <anchor>` | 57.41 ms | 55.96 – 60.84 |
| `swap large.rs <a> <b>` | 59.13 ms | 57.22 – 61.37 |
| `move large.rs <a> after <b>` | 61.76 ms | 56.27 – 72.09 |
| `indent large.rs <range> +2` | 65.39 ms | 62.59 – 68.57 |
| `patch large.rs <10-op patch>` | 38.84 ms | 35.42 – 42.52 |

> Single-line `edit` uses an mmap fast-path that only rewrites the changed byte range, hence the ~16 ms / 7 MB number. Structural mutations (`insert` / `delete` / `swap` / `move` / `indent`, multi-line `edit`, `patch`) rewrite the whole file via atomic-rename, which is where the ~50–65 ms band comes from at this file size.

### Block & diagnostics

| Command | Mean | Range |
|---|---:|---:|
| `find-block large.rs <anchor>` | 31.56 ms | 29.67 – 34.65 |
| `stats large.rs` | 15.82 ms | 14.11 – 19.22 |
| `doctor large.rs` | 14.68 ms | 14.49 – 15.02 |

### Tree-sitter / language tools (real `crates/core` source tree)

| Command | Mean | Range |
|---|---:|---:|
| `map core/ --json` | 1.91 ms | 1.69 – 2.08 |
| `outline cli.rs` (667 L) | 2.78 ms | 2.55 – 2.92 |
| `outline context.rs` | 2.53 ms | 2.32 – 2.89 |
| `symbol EditCmd --scope core --json` | 4.30 ms | 4.03 – 4.67 |
| `callers parse_anchor --scope core --depth 3 --json` | 149.02 ms | 144.71 – 154.87 |
| `callees run --scope core --depth 2 --json` | 118.06 ms | 114.86 – 121.32 |
| `deps --file cli.rs --json` | 1.04 ms | 0.87 – 1.48 |

> `callers` / `callees` parse the whole scope with tree-sitter on every call and BFS the call graph, so they sit in the ~100 ms range on a multi-file scope. Single-file `outline` / `deps` stay in the low-millisecond range.

### Misc

| Command | Mean | Range |
|---|---:|---:|
| `workflows --root core` | 0.92 ms | 0.86 – 0.99 |
| `watch-capabilities --json` | 1.00 ms | 0.84 – 1.32 |

### Reproducing locally

```bash
cargo build --release
scripts/bench-features.sh > bench-results/full-feature.tsv
```

The script generates `/tmp/lh-bench/{small,medium,large}.rs` on first run (cached afterwards), then drives `hyperfine` over each public subcommand and prints one tab-separated `label\tmean_ms\tmin_ms\tmax_ms` row per benchmark. It needs `hyperfine`, `ripgrep` (`rg`), and `python3` on `PATH`, and a release build of `linehash` at `target/release/linehash` (override with `LINEHASH_BIN=...`).

---

## Roadmap

- [ ] `linehash diff` — show pending edits before applying
- [ ] `linehash undo` — revert last edit
- [ ] Multi-line insert block support
- [ ] Integration test suite against real codebases
- [x] Workflow benchmark harness with raw result artifacts and markdown reports
