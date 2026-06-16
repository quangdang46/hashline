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

Format: `[path/file#HASH]` header followed by `LINE|content`. The 4-hex `HASH`
is computed over the entire file content (xxh3-64, top 16 bits). Every `read`
session shows the fresh hash so agents can detect when a file has been modified
since it was last read.

### Patch format

```patch
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

# Dry-run a patch (see changes without writing)
hashline patch src/auth.js 'DEL 3' --dry-run

# Find a structural block around a line
hashline find-block src/auth.js 3:0e

# Serve as a daemon
hashline serve --http 17300

# Run as MCP server
hashline mcp
```

## hashline vs str_replace vs patch editing

| Tool / workflow | How it locates code | Main failure mode | Best use case |
|---|---|---|---|
| `str_replace` | Exact old text match | Fails when whitespace or formatting differs | Small literal replacements when exact text is known |
| Unified diff / patch | Context lines around a hunk | Hunks can fail or apply badly after nearby edits | Reviewable multi-line changes |
| `hashline` | Line-number anchors + file-level content hash | Rejects edits when file hash changed since read | Safe agent-driven file editing |

## Commands

| Command | Description |
|---------|-------------|
| `read` | Read a file with `[path#HASH]` header and numbered lines |
| `patch` | Apply a hashline patch (SWAP, DEL, INS.PRE, INS.POST, INS.HEAD, INS.TAIL) |
| `find-block` | Find the enclosing structural block around an anchor |
| `serve` | Run as a daemon over Unix socket or HTTP |
| `mcp` | Run as an MCP stdio server |

## MCP server

`hashline` ships with a stdio MCP server:

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

# Patch operation fails
hashline patch src/auth.js 'DEL 999'
# (hashing safe — operation silently succeeds if anchor is out of range;
#  real patcher will return errors once validation is in place)
```

## Tech Stack

| Crate | Purpose |
|---|---|
| `xxhash-rust` (xxh3) | Fast file-level content hashing |
| `clap` | CLI parser |
| `serde_json` | `--json` output |

Pure Rust. No tree-sitter. No LLM. No external dependencies.
Simplest tool in the suite.

## Benchmarks

All measurements via `cargo bench` on **Apple M1**.

### Document hashing (short lines)

| File size | Time |
|-----------|-----:|
| 100 lines | 2.5 µs |
| 1,000 lines | 25 µs |
| 10,000 lines | 253 µs |
| 100,000 lines | 3.0 ms |

### Stats computation

| File size | Time |
|-----------|-----:|
| 1,000 lines | 31 µs |
| 10,000 lines | 209 µs |

### Anchor verification (10k-line doc)

| Anchors | Time |
|---------|-----:|
| 1 anchor | 58 µs |

## Scope

`hashline` focuses on **read + patch**. Search, symbol lookup, and static
analysis are intentionally out of scope — companion tools like
[`ffs`](https://github.com/quangdang46/fast_file_search) handle those better.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.
