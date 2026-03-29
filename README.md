# linehash

> Stable line-addressed file reading and editing for Claude Code, AI coding agents, and patch-safe automation.
> Every line gets a 2-char content hash, so edits target anchors instead of fragile whitespace-exact string replacement.

`linehash` is a Rust CLI for safe file editing with content-hashed line anchors. It helps Claude Code and other AI coding tools read files, locate lines, apply edits, and reject stale changes before they corrupt code.


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



## Installation

### From GitHub releases

Install the latest release with the generated installer:

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/linehash/main/install.sh?$(date +%s)" | bash
```


The installer downloads the matching GitHub release asset for your platform, verifies checksums when available, can optionally add the install directory to your shell PATH, then auto-detects supported MCP providers and installs the `linehash` MCP entry for each detected host.

### From source

```bash
cargo install --path crates/core
```

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
- `grep` searches by regex and returns anchor-addressed matches.
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

## Roadmap

- [ ] `linehash diff` — show pending edits before applying
- [ ] `linehash undo` — revert last edit
- [ ] Multi-line insert block support
- [ ] Integration test suite against real codebases
- [x] Workflow benchmark harness with raw result artifacts and markdown reports
