# @scope/hashline-opencode-plugin

Hashline plugin for [OpenCode](https://opencode.ai) — a **thin wrapper** that
shells out to the [hashline](https://github.com/quangdang46/hashline) binary.
It does **not** reimplement hashing, staleness detection, or merge recovery:
the Rust binary (`crates/core`, v0.9.1) is the single source of truth.

The plugin registers two tools that replace the built-in `read`/`edit` flow:

| Tool | Purpose |
|------|---------|
| `hashline_read` | Read a file rendered as `N:hh|content` lines (binary-native hashline format). |
| `hashline_edit` | Edit a file using `N:hh` anchors. All anchors are validated atomically by the binary; stale anchors are rejected with a mismatch error. |

`hashline_grep` is **deferred** — search is owned by the sibling
[`ffs`](https://github.com/quangdang46/fast_file_search) repo, not hashline.

## Install

Requires the `hashline` binary on `PATH` (or `HASHLINE_BIN`), version `>= 0.9.1`:

```bash
cargo install hashline        # or download from the hashline releases page
```

Add the plugin to `opencode.json`. Use the **`plugin` array** form, and disable
the built-in `edit` tool so the model is forced through `hashline_edit`:

```jsonc
{
  "plugin": ["@scope/hashline-opencode-plugin"],
  "agent": {
    "build": {
      "tools": { "edit": false },
      "prompt": "Always use hashline_read to inspect and hashline_edit to modify files. Anchors are N:hh."
    }
  }
}
```

Local development (instead of publishing under a real scope):

```jsonc
{
  "plugin": ["./.opencode/node_modules/@scope/hashline-opencode-plugin/dist/index.js"],
  "agent": {
    "build": {
      "tools": { "edit": false }
    }
  }
}
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
