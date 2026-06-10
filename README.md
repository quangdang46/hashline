# hashline

> Stable line-addressed file reading and editing for Claude Code, AI coding agents, and patch-safe automation.
> Every line gets a 2-char content hash, so edits target anchors instead of fragile whitespace-exact string replacement.

`hashline` is a Rust CLI for safe file editing with content-hashed line anchors. It helps Claude Code and other AI coding tools read files, locate lines, apply edits, and reject stale changes before they corrupt code.

## Installation

### From GitHub releases

#### Linux / macOS

Install the latest release with the generated installer:

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/hashline/main/install.sh?$(date +%s)" | bash
```

#### Windows (PowerShell 5.1+)

```powershell
irm "https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1" | iex
```

To pin a version or pass flags, download once and run:

```powershell
irm "https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1" -OutFile install.ps1
.\install.ps1 -Version v0.1.10 -EasyMode -Verify
```

The installer downloads the matching GitHub release asset for your platform, verifies the SHA-256 sidecar when available, can optionally add the install directory to your shell PATH (Bash/Zsh on Unix, user PATH on Windows), then auto-detects supported MCP providers and installs the `hashline` MCP entry for each detected host.

| Flag (sh / ps1)              | Effect                                                        |
|------------------------------|---------------------------------------------------------------|
| `--version vX.Y.Z` / `-Version` | Pin a specific release (default: latest)                   |
| `--dest <path>` / `-Dest`       | Install to a custom directory                              |
| `--system` / `-System`          | Install to `/usr/local/bin` / `%ProgramFiles%\hashline`    |
| `--easy-mode` / `-EasyMode`     | Append install dir to user PATH                            |
| `--verify` / `-Verify`          | Run `hashline --version` after install                     |
| `--from-source`                 | Build from source via `cargo` (Unix only)                  |
| `--quiet` / `-Quiet`            | Suppress info logs                                         |
| `--uninstall` / `-Uninstall`    | Remove the binary and any easy-mode PATH lines             |

### From source

```bash
cargo install --path crates/core
```

## Why hashline

- Built for **Claude Code** and **AI coding agents**
- Safer than `str_replace` for **file editing** and **patch workflows**
- Uses **content-hashed line anchors** instead of fragile exact-text matching
- Detects **stale reads**, **ambiguous anchors**, and **concurrent file changes**
- Written in **Rust** with simple CLI and JSON output for automation

## hashline vs str_replace vs patch editing

| Tool / workflow | How it locates code | Main failure mode | Best use case |
|---|---|---|---|
| `str_replace` | Exact old text match | Fails when whitespace or formatting differs | Small literal replacements when exact text is known |
| Unified diff / patch | Context lines around a hunk | Hunks can fail or apply badly after nearby edits | Reviewable multi-line changes and code review workflows |
| `hashline` | Content-hashed line anchors like `12:ab` | Rejects stale or ambiguous anchors instead of guessing | Safe AI-assisted file editing, targeted edits, and patch-safe automation |

**Why this matters for AI coding:** models often know *what* to change but are less reliable at reproducing the exact old text required by `str_replace`. `hashline` reduces that failure mode by letting tools edit by anchor, verify file state, and stop on stale reads before code is corrupted.

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

When Claude reads a file via `hashline read`, every line gets a stable 2-char hash:

```
1:a3|function verifyToken(token) {
2:f1|  const decoded = jwt.verify(token, process.env.SECRET)
3:0e|  if (!decoded.exp) throw new TokenError('missing expiry')
4:9c|  return decoded
5:b2|}
```

Format: `LINE:HASH|content` — no padding, no space after `|`. The leading whitespace
of `content` is the file's own indentation, preserved verbatim. This is intentional:
it keeps output dense (saves ~6% tokens on large files) and lets the model copy
indentation byte-exact when constructing replacements.

When Claude edits, it references hashes as anchors:

```bash
# Replace a single line
hashline edit src/auth.js 2:f1 "  const decoded = jwt.verify(token, env.SECRET)"

# Replace a range
hashline edit src/auth.js 2:f1..4:9c "  return jwt.verify(token, env.SECRET)"

# Insert after a line
hashline insert src/auth.js 3:0e "  if (!decoded.iat) throw new TokenError('missing iat')"

# Delete a line
hashline delete src/auth.js 3:0e
```

If the file changed since last read, hashes won't match → edit **rejected** before corruption.

## Why This Is Better Than str_replace

| | str_replace | hashline |
|---|---|---|
| Model must reproduce whitespace | ✅ required | ❌ not needed |
| Stable after file changes | ❌ line numbers shift | ✅ hash tied to content |
| Edit failure rate | Up to 50% | Near 0% |
| Detects stale reads | ❌ | ✅ hash mismatch = reject |
| Token cost | High (full old content) | Low (just hash + new line) |

## How Hashes Work

Each hash is a **2-char truncated xxHash** of the line content (with trailing
whitespace stripped before hashing):

```
line content → trim_end → xxhash32 → take low byte as 2 hex chars
"  return decoded" → trim → xxh32 → 0x...9c → "9c"
```

- Same content = same hash (stable across reads)
- Different content = different hash (edit safety)
- 2 chars = 256 possible values — good enough for line-level anchoring
- **Trailing whitespace is ignored** — anchors survive Prettier / Black / gofmt
  runs, and CRLF↔LF line-ending changes
- Collisions are rare and recoverable (hashline detects ambiguity, plus fuzzy
  relocation accepts a unique match anywhere or the closest match within ±3
  lines when an anchor goes stale)

## Fuzzy anchor relocation

When an exact `line:hash` lookup misses (because lines shifted), hashline tries
to recover before failing:

| Situation | Behavior |
|---|---|
| `line:hash` exact match at requested line | use it (fast path) |
| Hash exists at exactly one other line | silently relocate there |
| Hash exists at multiple lines | relocate to the closest IF it is within ±3 lines of the requested line |
| Hash gone / no nearby match | reject with a `>>>`-formatted stale-anchor error showing fresh anchors |

```
Error: line 2 content changed since last read in src/auth.js (expected hash 89, got 1d)
    1:c8|alpha
>>> 2:1d|DELTA-NEW-XYZ
    3:6d|gamma
```

The agent can copy the `>>> 2:1d` anchor verbatim and retry without re-reading
the whole file.

## Post-edit snippet

After a successful single-line or range edit, hashline auto-emits the changed
region with fresh anchors so the agent can verify or chain further edits
without an extra `read` call:

```
$ hashline edit src/main.rs 3:e2 "    let y = 999;"
Edited line 3.
1:9b|fn main() {
2:f8|    let x = 1;
3:4d|    let y = 999;          ← new hash for the changed line
4:83|    let z = 3;
5:a5|    println!("{}", x + y + z);
```

## Tech Stack

| Crate | Purpose |
|---|---|
| `xxhash-rust` | Fast content hashing per line |
| `clap` | CLI |
| `serde_json` | `--json` output for scripts |

Pure Rust. No tree-sitter. No LLM. No external dependencies.
Simplest tool in the suite.

## Scope: edit, not search

`hashline` deliberately stays focused on **read + edit + delete + verify**.
Search, symbol lookup, and static analysis are intentionally out of scope —
companion tools like [`ffs`](https://github.com/quangdang46/fast_file_search)
handle those better. A typical agent workflow combines them:

```bash
ffs grep "fn verify" src/         # find candidate locations
hashline read src/auth.rs         # get fresh anchors
hashline edit src/auth.rs 42:a3 "new content"
```

This keeps the hashline CLI surface small (17 commands) and the MCP tool
list AI-friendly (24 tools), instead of duplicating an entire search
engine inside the file-edit binary.

## MCP server

`hashline` now ships with a stdio MCP server that exposes the existing read/search/edit workflow as MCP tools:

```bash
hashline mcp
```

The `install.sh` / `install.ps1` scripts auto-detect supported MCP host configs after installing the binary and upsert a `hashline` server entry for every detected provider, logging the install results.

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

Auto-detect is the default. Set `HASHLINE_MCP_HOST=codex` or a comma-separated host list only when you want to override detection and target a specific subset.

## Usage

Common workflows for Claude Code, AI code editing, and patch-safe file automation:

```bash
# Read file with hash tags
hashline read src/auth.js

# Read just the neighborhood around one or more anchors
hashline read src/auth.js --anchor 2:f1 --context 2

# View just line numbers + hashes (no content) — for orientation
hashline index src/auth.js

# Check whether one or more anchors still resolve
hashline verify src/auth.js 2:f1 4:9c

# Edit by hash anchor
hashline edit <file> <hash-or-line:hash> <new_content>
hashline edit <file> <start-line:hash>..<end-line:hash> <new_content>
hashline insert <file> <hash-or-line:hash> <new_line>     # insert AFTER anchor line
hashline insert <file> <hash-or-line:hash> <new_line> --before
hashline delete <file> <hash-or-line:hash>

# Structural mutations
hashline swap <file> <anchor-a> <anchor-b>
hashline move <file> <anchor> before <target-anchor>
hashline move <file> <anchor> after <target-anchor>
hashline indent <file> <start-line:hash>..<end-line:hash> +2

# Multi-op workflows
hashline patch <file> <patch.json>
# patch.json shape:
# {"ops":[{"op":"edit","anchor":"3:64","content":"  return message.toUpperCase()"}]}

# Inspect collision/token-budget guidance for large files
hashline stats src/auth.js

# Recommend a read-only workflow for a file
hashline doctor src/auth.js
```

## Integration with Claude Code

Add to your project's `CLAUDE.md`:

```markdown
## File Editing Rules

When editing an existing file with hashline:

1. Read: `hashline read <file>`
2. Copy the anchor as `line:hash` (for example `2:f1`) — do not include the trailing `|`
3. Edit using the anchor only; never reproduce old content just to locate the line
4. If the file may have changed, prefer `hashline read <file> --json` first and carry `mtime` / `inode` into mutation commands with `--expect-mtime` / `--expect-inode`
5. If an edit is rejected as stale or ambiguous, re-read and retry with a fresh qualified anchor

Example:
  hashline read src/auth.js
  # line 2 shows as `2:f1|   const decoded = ...`
  hashline edit src/auth.js 2:f1 "  const decoded = jwt.verify(token, env.SECRET)"
```

### Recommended agent workflow

- Use `read` for the full file view.
- Use `read --anchor ... --context N` when you already know the target anchor and want a smaller local window.
- Use `index` for fast orientation when content is not needed.
- Use `verify` to confirm anchors still resolve before building a larger edit plan.
- Use external tools like `ffs grep` / `ffs symbol` to locate targets when you only know content.
- Use `swap`, `move`, `indent` instead of simulating structural edits with multiple fragile single-line operations.
- Use `patch` for coordinated multi-line changes that should succeed or fail together.
- Use `stats` when a file is large, collisions are likely, or you want guidance on whether short hashes and small context windows are still ergonomic.
- Use `doctor` when you want a read-only recommendation for how to approach a file before reading or editing it.
- Use qualified anchors like `12:ab` whenever possible; they are safer than bare `ab` when collisions or stale reads matter.

## Workflow playbooks

### Targeted edit

1. `hashline read <file>`
2. Copy the qualified anchor as `line:hash`
3. `hashline edit <file> <line:hash> <new_content>`
4. `hashline verify <file> <line:hash>` or re-read the local neighborhood

### Search → anchor → edit

1. Use `ffs grep` / `ffs symbol` (or any external search tool) to locate target
2. `hashline read <file> --anchor <line:hash> --context N` for a focused window
3. `hashline edit` / `hashline patch`

### Large-file workflow

1. `hashline stats <file>` to inspect token cost, collisions, and suggested context
2. `hashline doctor <file>` to get a read-only workflow recommendation
3. `hashline index <file>` if you only need orientation
4. `hashline read <file> --anchor <line:hash> --context N` instead of repeatedly dumping the whole file

### Stale-anchor recovery

1. Treat stale-anchor failures as the safety system working correctly
2. Re-run `hashline read <file>` or `hashline read <file> --json`
3. If the error reports relocated lines, rebuild a fresh qualified anchor from that neighborhood
4. Retry the mutation with the refreshed anchor

### Multi-op patch workflow

1. Use external search (`ffs grep` / `ffs symbol`) to collect target lines
2. `hashline read <file> --anchor <line:hash> --context N` to confirm anchors
3. Build a patch JSON file
4. Run `hashline patch <file> <patch.json> --dry-run`
5. Apply the patch once the dry-run output looks correct

### Structural edit workflow

- Use `move` or `swap` for reordering instead of rewriting text by hand
- Use `indent` after movement or when shifting a whole block
- Prefer `patch` over many tiny single-line edits when the change is coordinated

## Output Modes

```bash
# Pretty (default) — for Claude to read
hashline read src/auth.js
1:a3|function verifyToken(token) {
2:f1|  const decoded = jwt.verify(token, SECRET)
  ...

# JSON — for scripts and stale-guard workflows (compact by default)
hashline read src/auth.js --json
{"file":"src/auth.js","newline":"lf","trailing_newline":true,"mtime":1714001321,"mtime_nanos":0,"inode":12345,"lines":[{"n":1,"hash":"a3","content":"function verifyToken(token) {"},{"n":2,"hash":"f1","content":"  const decoded = jwt.verify(token, SECRET)"}]}

# JSON pretty — for human inspection (multi-line, indented)
hashline read src/auth.js --json --pretty
{
  "file": "src/auth.js",
  ...
}

# NDJSON output: header line + one JSON object per file line (read/index)
hashline read src/auth.js --ndjson
```

The MCP server defaults to compact JSON in tool result text (saves ~30% tokens
vs pretty). Use the `--pretty` flag on the CLI when reading a JSON file by
hand.

## Additional Commands

- `verify` checks whether anchors still resolve and returns a non-zero exit code if any do not.
- `doctor` recommends a read-only workflow for a file using current size/collision heuristics.
- `patch` applies a JSON patch transaction atomically.
- `swap` exchanges two lines in one snapshot-safe operation.
- `move` repositions one line before or after another anchor.
- `indent` indents or dedents an anchor-qualified range.

## Error Handling

```bash
# Hash not found
hashline edit src/auth.js xx "new content"
Error: hash 'xx' not found in src/auth.js
Hint: run `hashline read <file>` to get current hashes

# Ambiguous hash (collision)
hashline edit src/auth.js f1 "new content"
Error: hash 'f1' matches 3 lines in src/auth.js (lines 2, 14, 67)
Hint: use a line-qualified hash like '2:f1' to disambiguate

# File changed since read (stale qualified anchor)
hashline edit src/auth.js 2:f1 "new content"
Error: line 2 content changed since last read in src/auth.js (expected hash f1, got 3a)
Hint: re-read the file with `hashline read <file>` and retry the edit

# File metadata changed since JSON read / guard capture
hashline edit src/auth.js 2:f1 "new content" --expect-mtime 1714001321 --expect-inode 12345
Error: file 'src/auth.js' changed since the last read
Hint: re-read the file metadata and retry with fresh --expect-mtime/--expect-inode values
```

## Recovery loops

- **Stale anchor:** re-run `hashline read <file>` or `hashline read <file> --json`; if the error reports relocated line(s), use those to rebuild a fresh qualified anchor before retrying.
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
- `core/` — the hashline `crates/core` source tree (used by the language-aware commands)

### Read & orient

| Command | Mean |
|---|---:|
| `read small.rs` (100 L) | ~6 ms |
| `read medium.rs` (10 k L) | ~6 ms |
| `read large.rs` (100 k L, 5.6 MB) | ~24 ms |
| `read large.rs --json` | ~54 ms |
| `index large.rs` | ~18 ms |

### Verify

| Command | Mean |
|---|---:|
| `verify large.rs <anchor>` | ~18 ms |

### Mutations

| Command | Mean |
|---|---:|
| `edit small.rs <anchor>` | ~7 ms |
| `edit medium.rs <anchor>` | ~12 ms |
| `edit large.rs <anchor>` | ~298 ms (atomic-rename whole file) |

### Core hashing speedups (criterion vs pre-Phase-1 baseline)

`trim_end()` before hashing made hashing significantly faster because the input
to xxHash32 is shorter:

| Bench | Time | Δ vs baseline |
|---|---:|---:|
| `hash_document/short_lines/100` | 6.4 µs | **-28 %** |
| `hash_document/short_lines/10000` | 760 µs | **-37 %** |
| `hash_document/short_lines/100000` | 8.5 ms | **-75 %** |
| `hash_long_lines/lines/10000` | 1.3 ms | **-68 %** |
| `edit_resolve_anchor (100k prebuilt)` | 868 µs | **-14 %** |
| `edit_mutate_render (100k)` | 877 µs | **-22 %** |
| `edit_parse_document (100k)` | 148 µs | **-30 %** |

### Diagnostics

| Command | Mean | Range |
|---|---:|---:|
| `stats large.rs` | 15.82 ms | 14.11 – 19.22 |
| `doctor large.rs` | 14.68 ms | 14.49 – 15.02 |

### Reproducing locally

```bash
cargo build --release
scripts/bench-features.sh > bench-results/full-feature.tsv
```

The script generates `/tmp/lh-bench/{small,medium,large}.rs` on first run (cached afterwards), then drives `hyperfine` over each public subcommand and prints one tab-separated `label\tmean_ms\tmin_ms\tmax_ms` row per benchmark. It needs `hyperfine` and `python3` on `PATH`, and a release build of `hashline` at `target/release/hashline` (override with `HASHLINE_BIN=...`).

---

## Roadmap

- [ ] `hashline diff` — show pending edits before applying
- [ ] `hashline undo` — revert last edit
- [ ] Multi-line insert block support
- [ ] Integration test suite against real codebases
- [x] Workflow benchmark harness with raw result artifacts and markdown reports
