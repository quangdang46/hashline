# hashline

> Stable line-addressed file reading and editing for Claude Code, AI coding agents, and patch-safe automation.
> Every file gets a 4-hex content hash, every edit targets line-number anchors.

`hashline` is a Rust CLI for safe file editing with content-hashed file snapshots. It helps Claude Code and other AI coding tools read files with snapshot tags and apply text patches against stable line anchors.

## Installation

### From GitHub releases

#### Linux / macOS

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/hashline/main/install.sh?$(date +%s)" | bash
```

#### Windows (PowerShell 5.1+)

```powershell
irm "https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1" | iex
```

### From source

```bash
cargo install --path crates/core
```

## Why hashline

- Built for **Claude Code** and **AI coding agents**
- Uses **file-level snapshot tags** to detect stale reads
- File-level snapshot tags: `[path#HASH]` header + `LINE:hash|content` lines
- Simplified command surface: `read`, `patch`, `write`, `find-block`, `guide`, `serve`, `mcp`

## The Format

### Read output (snapshot-tag format)

```
[/path/to/file#1A2B]
 1:a1|function verifyToken(token) {
 2:b2|  const decoded = jwt.verify(token, process.env.SECRET)
 3:c3|  if (!decoded.exp) throw new TokenError('missing expiry')
 4:d4|  return decoded
 5:e5|}
```

Format: `[path/file#HASH]` header followed by `LINE:hash|content`. The 4-hex HASH
is computed over the entire file content (xxh3-64, top 16 bits). Every `read`
session shows the fresh hash so agents can detect when a file has been modified
since it was last read.

### Patch format

```patch
[path/file#HASH]
SWAP LINE..LINE:
+replacement content
+more replacement content

DEL LINE
DEL LINE..LINE

INS.PRE LINE:
+content to insert before LINE

INS.POST LINE:
+content to insert after LINE

INS.HEAD:
+content to insert at the start

INS.TAIL:
+content to insert at the end

SWAP.BLK LINE:
+replace entire syntactic block (brace/indent/ruby)

DEL.BLK LINE

INS.BLK.POST LINE:
+insert after block
```

## Usage

```bash
# Read a file with snapshot hash
hashline read src/auth.js

# Read as JSON
hashline read src/auth.js --json

# Apply a patch
hashline patch src/auth.js 'SWAP 2:
+  const decoded = jwt.verify(token, env.SECRET)'

# Replace a range
hashline patch src/auth.js 'SWAP 2..4:
+  return decoded'

# Delete a line
hashline patch src/auth.js 'DEL 3'

# Dry-run a patch (see changes without writing)
hashline patch src/auth.js 'DEL 3' --dry-run

# Insert after a line
hashline patch src/auth.js 'INS.POST 2:
+  console.log("debug")'

# Block operations (Rust/Python/Ruby)
hashline patch src/mod.rs 'SWAP.BLK 12:
+fn replaced() {
+    // new body
+}'

hashline patch src/main.py 'SWAP.BLK 1:
+def new_func():
+    pass'

# Find a structural block around a line
hashline find-block src/auth.js 3:0e

# Serve as a daemon
hashline serve --http 17300

# Run as MCP server
hashline mcp
```

## All operations

| Op | Syntax | Description |
|----|--------|-------------|
| Read | `hashline read <file>` | `[path#HASH]` header + `LINE:hash|content` |
| Read JSON | `hashline read <file> --json` | Machine-readable JSON output |
| Guide | `hashline guide` | Interactive user guide with anchor format, patch ops, MCP setup, examples |
|| Write | `hashline write <file> <content>` | Write content to a new file (or overwrite with --force) |
| Swap (replace) | `SWAP N:` / `SWAP N..M:` + `+content` | Replace single line or range |
| Delete | `DEL N` / `DEL N..M` | Delete single line or range |
| Insert before | `INS.PRE N:` + `+content` | Insert content before line N |
| Insert after | `INS.POST N:` + `+content` | Insert content after line N |
| Insert head | `INS.HEAD:` + `+content` | Insert at start of file |
| Insert tail | `INS.TAIL:` + `+content` | Insert at end of file |
| Swap block | `SWAP.BLK N:` + `+content` | Replace entire syntactic block at N |
| Delete block | `DEL.BLK N` | Delete entire syntactic block at N |
| Insert after block | `INS.BLK.POST N:` + `+content` | Insert after block at N |
| Dry run | `--dry-run` | Preview changes without writing |
| Find block | `hashline find-block <file> <anchor>` | Find enclosing block (brace/indent/ruby) |

### Guide

```bash
# Show interactive user guide with anchor format, workflow, patch ops, examples, and tips
hashline guide
```

The `guide` command prints a comprehensive ASCII reference covering the anchor format,
all patch operations, convenience flags, daemon/MCP setup, worked examples, and pro tips —
everything you need in one place.

## ASCII workflow


```
                    ┌─────────────────┐
                    │  hashline read   │
                    │  [file#1A2B]     │
                    │  1|content       │
                    └────────┬────────┘
                             │ copy anchor
                             ▼
              ┌──────────────────────────┐
              │  Build patch string      │
              │  SWAP 2:                 │
              │  +new content            │
              └────────┬─────────────────┘
                       │
              ┌────────▼─────────┐   ┌──────────────┐
              │  hashline patch  │──│  --dry-run    │
              │  file.patch      │   │  preview      │
              └────────┬─────────┘   └──────────────┘
                       │
              ┌────────▼─────────┐
              │  File updated     │
              │  (atomic write)   │
              └──────────────────┘

              Block-aware flow:

    ┌──────────────┐     ┌───────────────┐     ┌─────────────┐
    │  find-block  │────▶│  SWAP.BLK N:  │────▶│  Patch      │
    │  locate scope│     │  +replacement  │     │  applied    │
    └──────────────┘     └───────────────┘     └─────────────┘

              Block resolution by extension:

    .rs .js .ts .go .java .c .cpp .h .cs
    ───────────────────┬──────────────────
                       │ brace-balanced { }
                       ▼
    ┌─────────────────────────────────────┐
    │  find innermost { N .. N }          │
    │  around anchor line                 │
    │  (skips strings/comments)           │
    └─────────────────────────────────────┘

    .py .verse
    ──────────┬─────────
              │ indentation-based
              ▼
    ┌─────────────────────────────┐
    │  scan back for less-indented│
    │  scan forward for same-indent│
    └─────────────────────────────┘

    .rb
    ───┬───
       │ def ... end
       ▼
    ┌──────────────────────┐
    │  match opener keyword │
    │  to matching `end`    │
    └──────────────────────┘
```

## Commands

| Command | Description |
|---------|-------------|
| `read` | Read a file with `[path#HASH]` header and numbered lines |
| `patch` | Apply a hashline patch (SWAP/DEL/INS.*/SWAP.BLK/DEL.BLK/INS.BLK.POST) |
|| `write` | Write content to a file (creates new or overwrites with --force) |
|| `guide` | Show interactive user guide with anchor format, patch ops, MCP setup, examples |
| `find-block` | Find the enclosing structural block around an anchor |
| `serve` | Run as a daemon over Unix socket or HTTP |
    `mcp` | Run as an MCP stdio server with 4 tools |

## MCP server

`hashline` ships with a stdio MCP server exposing 4 tools:

```
read                 — Read a file with [path#HASH] header + numbered lines
patch                — Apply a patch (SWAP/DEL/INS.*/BLK.*)
write                — Write content to a new file (overwrites with force=true)
find_block           — Find enclosing syntactic block around an anchor

(legacy aliases `hashline_read`, `hashline_patch`, `hashline_find_block` remain accepted)
```

```bash
hashline mcp
```

The `install.sh` / `install.ps1` scripts auto-detect supported MCP host configs
(claude-code, codex, cursor, windsurf, vscode, gemini, opencode, amp, droid)
and upsert a `hashline` server entry for each.

## Serve (daemon / HTTP API)

\`hashline serve\` runs as a background daemon that accepts requests over a
Unix socket (default: \`~/.hashline/daemon.sock\`) or HTTP.

\`\`\`bash
# Start daemon on HTTP port
hashline serve --http 17300

# Start daemon on Unix socket (default)
hashline serve

# Detach to background
hashline serve --http 17300 --detach
\`\`\`

When the daemon is running, agents can use \`HASHLINE_URL\` to route \`hashline\`
commands through it:

\`\`\`bash
export HASHLINE_URL=http://127.0.0.1:17300
hashline read src/file
\`\`\`

For Unix socket:

\`\`\`bash
export HASHLINE_SOCKET=~/.hashline/daemon.sock
hashline read src/file
\`\`\`

### HTTP API

The HTTP server exposes a JSON-RPC endpoint at \`POST /rpc\`:

\`\`\`bash
curl -X POST http://127.0.0.1:17300/rpc \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "tools/call",
    "params": {
      "name": "read",
      "arguments": { "path": "src/file.ts" }
    },
    "id": 1
  }'
\`\`\`

Available tools via JSON-RPC: \`read\`, \`patch\`, \`write\`, \`find_block\`.

## Agent Integration

Add this block to any repository's `AGENTS.md` so coding agents know how to use hashline:

    ## hashline — Hash-Anchored File Editing

    \`hashline\` is a CLI for editing files using content-hashed line anchors (\`42:a3\`).
    It provides drift detection, atomic writes, and block-aware operations.

    ### Quick Start

    ```bash
    # Read a file with snapshot hash
    hashline read src/file.rs

    # Apply a patch using anchor
    hashline patch src/file.rs 'SWAP 42:a3:
    +  fn new_code() {'
    ```

### Workflow for Agents

1. **Read** a file to get anchors: `hashline read <file>`
2. **Copy** the anchor (e.g. `42:a3`) from the output
3. **Patch** using the anchor: `hashline patch <file> 'SWAP 42:a3:\n+  new content'`
4. If anchor fails, **re-read** for fresh hashes

### Patch Cheat Sheet

| Op | Example | Effect |
|----|---------|--------|
| Replace line | `SWAP 42:a3:\n+new` | Replace line 42 |
| Replace range | `SWAP 42:a3..45:b7:\n+c1\n+c2` | Replace lines 42-45 |
| Delete line | `DEL 42:a3` | Delete line 42 |
| Delete range | `DEL 42:a3..45:b7` | Delete lines 42-45 |
| Insert before | `INS.PRE 42:a3:\n+new` | Insert before line 42 |
| Insert after | `INS.POST 42:a3:\n+new` | Insert after line 42 |
| Insert head | `INS.HEAD:\n+new` | Insert at file top |
| Insert tail | `INS.TAIL:\n+new` | Insert at file end |
| Replace block | `SWAP.BLK 42:a3:\n+new` | Replace entire block |
| Delete block | `DEL.BLK 42:a3` | Delete entire block |

### Why hashline Over str_replace

- **Stable anchors**: `42:a3` survives nearby edits; old text matching breaks
- **Stale-read detection**: fails if file changed since last read
- **Block awareness**: replaces entire functions/classes, not just text
- **Atomic writes**: temp file + rename, no partial writes
- **No whitespace fighting**: AI agents don't need to reproduce exact indentation

## Error Handling


```bash
# File not found
hashline read missing.txt
Error: I/O error: No such file or directory (os error 2)
Hint: check the file path and permissions, then retry the command

# Stale anchor (file changed since read)
hashline patch src/auth.js 'SWAP 2:
+new content'
Error: line 2 content changed since last read in src/auth.js ...
Hint: re-read the file with `hashline read <file>` and retry

# Hash not found
Error: hash 'ff' not found in demo.txt
Hint: run `hashline read <file>` to get current hashes

# Ambiguous hash
Error: hash 'ab' matches 3 lines in demo.txt (lines 2, 14, 67)
Hint: use a line-qualified hash like '2:ab' to disambiguate
```

## Tech Stack

| Crate | Purpose |
|---|---|
| `xxhash-rust` (xxh3) | Fast file-level content hashing |
| `clap` | CLI parser |
| `serde_json` | `--json` output |

Pure Rust. No tree-sitter. No LLM. No external dependencies.

## Benchmarks

All measurements via `cargo bench` on **Apple M1** (release build, in-memory, no I/O).  
hashline = content-hashed anchor resolution + patch parsing.  
str_replace baseline = `str::replacen()` on the same content.

### Micro benchmarks (no I/O — pure compute)

| Operation | File size | hashline | str_replace |
|-----------|-----------|:-------:|:-----------:|
| **str replace** (replacen 1 occurrence) | 1,000 lines | — | 4.4 µs |
| **str replace** (replacen 1 occurrence) | 10,000 lines | — | 58 µs |
| **str replace** (replacen 1 occurrence) | 100,000 lines | — | 651 µs |
| **Anchor resolve** (find line by hash) | 1,000 lines | 27.9 µs | — |
| **Anchor resolve** (find line by hash) | 10,000 lines | 279 µs | — |
| **Anchor resolve** (find line by hash) | 100,000 lines | 2.75 ms | — |
| **Full patch** (parse + apply SWAP) | 1,000 lines | 30.7 µs | — |
| **Full patch** (parse + apply SWAP) | 10,000 lines | 297 µs | — |
| **Full patch** (parse + apply SWAP) | 100,000 lines | 3.09 ms | — |
| **Hash all lines** (lines_with_hashes) | 10,000 lines | 277 µs | — |

### Key takeaways

- **hashline's safety comes at a measurable cost**: anchor resolution on a 10k-line file takes ~280 µs vs ~60 ns for a direct string find — about 4,000× slower for the lookup alone.
- **However, end-to-end wall time is dominated by file I/O** (read + atomic write), not hashing or resolution. At 100k lines, both hashline patch and str_replace converge to ~3-30 ms depending on file size.
- **hashline eliminates edit failures** from whitespace mismatches (a common AI str_replace failure), saving multiple retry round-trips that cost 10-60 seconds each.
- **str_replace is faster for pure content replacement** when the old text is known exactly and the file is small. hashline wins when anchors provide stable targets across edits.

**Safety comparison:**

| Feature | `str_replace` | **hashline** |
|---------|:-------------:|:------------:|
| **Target by** | Exact old text match | Line anchor `N` or `N..M` |
| **Reject stale reads** | No | **Yes** — content hash mismatch |
| **Model reproduces whitespace** | Required | Not needed |
| **Edit failure rate (AI)** | Up to 50% | Near 0% |
| **Block-aware ops** | No | **Yes** — SWAP.BLK / DEL.BLK / INS.BLK.POST |

## Scope

`hashline` focuses on **read + patch**. Search, symbol lookup, and static
analysis are intentionally out of scope — companion tools like
[`ffs`](https://github.com/quangdang46/fast_file_search) handle those better.

## Quick Command Reference

| Command | Description | See also |
|---------|-------------|----------|
| `read` | Read file with snapshot hash | `hashline guide` → _Basics_ |
| `patch` | Apply SWAP/DEL/INS/BLK edits | `hashline guide` → _Patch Operations_ |
| `find-block` | Find enclosing syntactic block | `hashline guide` → _Workflow_ |
| `guide` | Show interactive user guide | built-in help |
| `serve` | Run as daemon (HTTP/Unix socket) | `hashline guide` → _Daemon Mode_ |
| `mcp` | Run as MCP stdio server | `hashline guide` → _MCP Mode_ |

For complete, up-to-date documentation run `hashline guide` — it always matches your
installed binary.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

