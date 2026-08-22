# hashline-opencode-plugin

Hashline plugin for [OpenCode](https://opencode.ai) — a **thin wrapper** that
shells out to the [hashline](https://github.com/quangdang46/hashline) binary.
It does **not** reimplement hashing, staleness detection, or merge recovery:
the Rust binary (`crates/core`, >= 0.9.12) is the single source of truth.

The plugin registers six tools covering the full file lifecycle:

| Tool | Purpose |
|------|---------|
| `hashline_read` | Read a file rendered as `N:hh|content` lines (binary-native format). |
| `hashline_edit` | Edit via `N:hh` anchors; batched ops validated atomically; stale anchors rejected. Supports tree-sitter block ops (`replace_block`, `delete_block`, `insert_block_after`). |
| `hashline_write` | Create a new file or fully replace one (pass `force`). Response includes fresh anchors. |
| `hashline_find_block` | Show the syntactic block around a line — pair with block ops. |
| `hashline_remove_file` | Delete a file (explicit, auditable). |
| `hashline_rename_file` | Move/rename a file (`force` overwrites). |

`hashline_grep` is **deferred** — search is owned by the sibling
[`ffs`](https://github.com/quangdang46/fast_file_search) repo, not hashline.

## Install

Requires the `hashline` binary on `PATH` (or `HASHLINE_BIN`), version `>= 0.9.12`
(agent-first compact output):

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/hashline/main/install.sh" | bash   # macOS / Linux
irm "https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1" | iex          # Windows
cargo install hashline                                                                       # or via cargo
```

`opencode.json` — **OpenCode 2.x** uses the `plugins` array (the plugin auto-registers
all six tools and the hashline system prompt); disable the built-in `edit` so the
model is routed through `hashline_edit`. **OpenCode 1.x** keeps the singular
`plugin` form:

```jsonc title="OpenCode 2.x — opencode.json"
{
  "plugins": ["hashline-opencode-plugin"],
  "agent": {
    "build": { "tools": { "edit": false } }
  }
}
```

```jsonc title="OpenCode 1.x — opencode.json"
{
  "plugin": ["hashline-opencode-plugin"],
  "agent": {
    "build": {
      "tools": { "edit": false },
      "prompt": "Always use hashline_read to inspect and hashline_edit to modify files. Anchors are N:hh."
    }
  }
}
```

Local development:

```bash
npm install hashline-opencode-plugin        # then reference ./node_modules/... in config
```

## Binary discovery

Resolution order:

1. `HASHLINE_BIN` env var (absolute path) — highest precedence.
2. `hashline` / `hashline.exe` on `PATH`.

If the binary cannot be spawned, the tools return an install hint
(`hashline --help` / add to PATH / set `HASHLINE_BIN`). On plugin load the
package probes `hashline --version` and shows a one-line warning in tool
output when the binary is missing or older than `0.9.1`.

## Anchor format

Hashline's format is `N:hh|content` — N is the 1-based line number, `hh` is a
2-char hex content hash computed by the binary (xxh32 low byte). Anchors are
`N:hh`. This matches exactly what `hashline read` prints and what
`hashline patch` parses. The binary hashes **all** lines with content-derived
hashes; blank/symbol-only lines are position-seeded.

## CLI wrappers

The package also ships two thin `bin` wrappers over the same spawn logic:

```bash
hread <file> [--offset <n>] [--limit <n>] [--json]   # hashline_read for the shell
hedit <file> --json '<edits>' [--dry-run]            # hashline_edit for the shell
```

## Development

```bash
bun install
bun run typecheck     # tsc --noEmit
bun run build:all     # bun build + build:cli + tsc --emitDeclarationOnly
bun test src/tests/   # bun test (spawn-seam + translation + golden fixtures)
```

The test suite uses an injectable spawn seam and golden fixtures from
`integration/fixtures/` (captured from the release binary). The e2e test
(`src/tests/e2e.test.ts`) runs against a real `hashline` when one is
reachable and skips otherwise.

## Deferred / out of scope

- `hashline_grep` — not hashline's job; would spawn `ffs` and re-hash via
  `hashline read --json`. Not implemented in v0.1.0.
- Structured stale-anchor recovery (`remaps` map) — the binary does not emit
  one today; wrappers surface the binary's message verbatim.
