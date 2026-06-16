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
- File-level snapshot tags: `[path#1A2B]` header + `LINE|content` lines
- Simplified command surface: `read`, `patch`, `find-block`, `serve`, `mcp`
- Giong oh-my-pi format: `[path#HASH]` header, `LINE|content`, `SWAP`/`DEL`/`INS.*` operations

## The Format

### Read output (snapshot-tag format)

```
[/path/to/file#1A2B]
1|function verifyToken(token) {
2|  const decoded = jwt.verify(token, process.env.SECRET)
3|  if (!decoded.exp) throw new TokenError('missing expiry')
4|  return decoded
5|}
```

Format: `[path/file#HASH]` header followed by `LINE|content`. The 4-hex HASH
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

## hashline vs str_replace

| Feature | `str_replace` | **hashline** |
|---------|:-------------:|:------------:|
| **Target by** | Exact old text match | Line anchor `N` or `N..M` |
| **Reject stale changes** | No | **Yes** — hash mismatch |
| **Model reproduces whitespace** | Required | Not needed |
| **Edit failure rate (AI)** | Up to 50% | Near 0% |
| **Token cost** | Full old + new content | Just line number + new content |
| **Block-aware ops** | No | **Yes** — SWAP.BLK / DEL.BLK / INS.BLK.POST |
| **Multiple inserts at once** | No | **Yes** — INS.POST N: +a +b +c |
| **Dry-run preview** | No | **Yes** — `--dry-run` |
| **JSON output** | No | **Yes** — `--json` |
| **MCP server** | Built into Claude | 10 tools over stdio |

## All operations

| Op | Syntax | Description |
|----|--------|-------------|
| Read | `hashline read <file>` | `[path#HASH]` header + `LINE|content` |
| Read JSON | `hashline read <file> --json` | Machine-readable JSON output |
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
| `find-block` | Find the enclosing structural block around an anchor |
| `serve` | Run as a daemon over Unix socket or HTTP |
| `mcp` | Run as an MCP stdio server with 10 tools |

## MCP server

`hashline` ships with a stdio MCP server exposing 10 tools:

```
hashline_read     hashline_index      hashline_annotate
hashline_grep     hashline_find_block hashline_verify
hashline_edit     hashline_insert     hashline_delete
hashline_patch
```

```bash
hashline mcp
```

The `install.sh` / `install.ps1` scripts auto-detect supported MCP host configs
(claude-code, codex, cursor, windsurf, vscode, gemini, opencode, amp, droid)
and upsert a `hashline` server entry for each.

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

All measurements via `cargo bench` on **Apple M1** (`cargo build --release`).

### Hash: `lines_with_hashes()` (short lines)

| Lines | Time | Throughput |
|-------|:----:|:----------:|
| 100 | 2.9 µs | 1.79 GiB/s |
| 1,000 | 28.7 µs | 1.84 GiB/s |
| 10,000 | 278 µs | 1.92 GiB/s |
| 100,000 | 2.71 ms | 1.97 GiB/s |

### Hash: long lines

| Lines | Time | Throughput |
|-------|:----:|:----------:|
| 1,000 | 38.2 µs | 4.33 GiB/s |
| 10,000 | 385 µs | 4.34 GiB/s |

### Hash: real-world files

| File | Time |
|------|:----:|
| support.rs | 3.07 µs |
| document.rs | 6.34 µs |

### Resolve: hash + find anchor

| Lines | Time | Throughput |
|-------|:----:|:----------:|
| 1,000 | 28.5 µs | 1.84 GiB/s |
| 10,000 | 278 µs | 1.92 GiB/s |
| 100,000 | 2.68 ms | 1.99 GiB/s |

### Verify: pre-hashed anchors (10k-line file)

| Anchors | Time | Throughput |
|---------|:----:|:----------:|
| 1 | 69 ns | 14.5 Melem/s |
| 10 | 710 ns | 14.1 Melem/s |
| 100 | 7.4 µs | 13.5 Melem/s |
| 1,000 | 82 µs | 12.2 Melem/s |

### Memory footprint

File content loaded as `FileContent`:
- Raw text + normalized text = 2x file size
- No per-line hash precomputation until `lines_with_hashes()` called
- Lazy hashing: hash only what you need, when you need it

## Scope

`hashline` focuses on **read + patch**. Search, symbol lookup, and static
analysis are intentionally out of scope — companion tools like
[`ffs`](https://github.com/quangdang46/fast_file_search) handle those better.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
