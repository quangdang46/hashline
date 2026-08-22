# hashline-pi

Hashline `read`/`edit` override for [pi-coding-agent](https://github.com/badlogic/pi-mono) —
a **thin wrapper** that shells out to the real `hashline` Rust binary.

The wrapper never reimplements hashing, staleness detection, or merge recovery.
The binary is the single source of truth for hashes, staleness, and no-op
handling; this package is pure subprocess glue: argument translation + output
formatting.

## Install

`hashline-pi` is a **thin wrapper** — it does NOT bundle the `hashline` binary.
You must install both, in this order:

### Step 1 — install the `hashline` binary (required)

```bash
# macOS / Linux (auto-detects platform, verifies SHA-256, installs to ~/.local/bin)
curl -fsSL "https://raw.githubusercontent.com/quangdang46/hashline/main/install.sh" | bash

# Windows (PowerShell)
irm "https://raw.githubusercontent.com/quangdang46/hashline/main/install.ps1" | iex

# or via cargo
cargo install hashline
```

Requires **>= 0.9.12** (agent-first compact output). Without the binary the
extension fails at startup with an install hint; if it is installed somewhere
off PATH, point the extension at it (see [Binary discovery](#binary-discovery)).

### Step 2 — install the pi extension

```bash
pi install npm:hashline-pi
# or from this repo:
pi install git:github.com/quangdang46/hashline#main:integration/pi-hashline
# or from a local checkout:
pi install /path/to/hashline/integration/pi-hashline
```

Then `/reload` in pi. Verify with the `/hashline-status` command.

## Binary discovery

Resolution order (see `src/hashline.ts`):

1. `binary` field in `~/.pi/agent/hashline.json` (highest precedence).
2. `HASHLINE_BIN` env var (absolute path).
3. `hashline` / `hashline.exe` on `PATH`.
4. On Windows only: `~/.hashline/hashline.exe` and `hashline.exe` next to cwd.

If the binary cannot be found at tool-execute time, the tool returns a
structured `binary_not_found` error with an install hint — it never crashes.

## Config

Optional `~/.pi/agent/hashline.json`:

```jsonc
{
  "hashLength": 2,      // advisory: the binary always computes 2-hex hashes; 3|4 accepted but ignored
  "grep": false,        // gate the optional (deferred) grep tool
  "replaceText": true,  // expose the replace_text op translation
  "binary": "/path/to/hashline"   // HASHLINE_BIN equivalent, highest precedence
}
```

Loading errors never throw — invalid fields fall back to defaults and are
reported as warnings via `ctx.ui.notify` at session start.

## Usage

Read a file, then edit at the `N:hh` anchors it returns:

```
read { "path": "src/main.rs" }
```

Output (binary-native, exactly what `hashline read` prints):

```
[src/main.rs#5db5]
1:9b|fn main() {
2:f8|    let x = 1;
3:d2|    println!("ok");
4:88|}
```

Edit:

```json
{ "path": "src/main.rs", "edits": [
  { "op": "replace", "pos": "2:f8", "lines": ["    let x = 42;"] },
  { "op": "append", "lines": ["// done"] }
] }
```

Each successful edit appends an `--- Anchors ---` block with fresh anchors so
you can chain edits without a separate read.

## Tools

- `read` — overrides the built-in. Params: `path`, `offset`, `limit`, `raw`.
  Renders `N:hh|content` with a `[path#4hex]` header. `raw: true` drops anchors
  and the header to save tokens.
- `edit` — overrides the built-in. Params: `path`, `edits`. Ops:
  - `replace` (`pos`, optional `end`, `lines`) — single line or inclusive range.
    Empty `lines` deletes.
  - `append` (optional `pos`, `lines`) — insert after `pos`; omit = EOF.
  - `prepend` (optional `pos`, `lines`) — insert before `pos`; omit = BOF.
  - `delete` (`pos`, optional `end`) — delete single line or range.
  - `replace_text` (`oldText`, `newText`) — replaces an exact, unique single
    line match; fails (never a heuristic) on zero or multiple matches. Gated on
    config `replaceText`.
- `grep` — **deferred** (v0.1.0). The hashline binary has no grep subcommand.
  The stub registers only when config `grep: true` and reports the deferred
  state. Grep is owned by the sibling `ffs` repo.

## Failure modes

The binary's own diagnostics are authoritative. Stale anchors (content changed
since your last read) return the binary's `Error:`/`Hint:` text plus a teaching
footer telling you to re-read and retry with a fresh anchor. The wrapper never
performs a 3-way merge or anchor remap — recovery is delegated to the binary.

## Development

```bash
npm install
npm run typecheck
npm run lint
npm run contract      # compiles index.ts + tests to .tmp-tests, runs node --test
npm test              # typecheck + contract + lint
```

Contract tests stub the `ExtensionAPI` and assert the registered tool
definitions (name, schema, `renderShell: "default"`), the op→patch translation
table, and parse/format functions against the golden fixtures in
`../fixtures/` (captured from the release binary — `integration/CONTRACT.md`).

The manual install smoke test (`pi install .`, `/reload`, read+edit a temp
file) requires a real `pi` install and is documented in
`integration/implementation-plan.md` E.14.
