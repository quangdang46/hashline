# Hashline Integration Packages — Implementation Plan

Two thin-wrapper integration packages for agent hosts, both of which **shell out to the real `hashline` binary** and never reimplement hashing, staleness detection, or merge recovery:

1. **`integration/pi-hashline`** — a pi-coding-agent extension (TypeScript). Modeled on `.tmp/pi-hashline-edit` (RimuruW) but architected like `.tmp/pi-hledit` (dabito): spawn `hashline` per operation.
2. **`integration/opencode-plugin`** — an OpenCode plugin package (TypeScript). Modeled on `.tmp/hashline-edit-opencode` (paulp-o) and `.tmp/opencode-hashlines` (tianhuil), with all hashing/merging delegated to the binary.

The single source of truth for hashes, staleness, and recovery is the **hashline Rust binary** (`crates/core`, v0.9.1). The wrappers are pure subprocess glue: argument translation + output formatting.

> **Formatting note that affects the repo (verified):** `.gitignore` line 9 ignores `*.md` and line 13 ignores `.tmp*`. There is **no `integration/` rule**, so the `integration/` directory itself is trackable — but **any `*.md` file under `integration/` will be silently gitignored** (that includes this plan, per-package `README.md`, and the fixtures doc). Plan accordingly:
> - Task E.1 adds an explicit exception to `.gitignore`: `!integration/**` (or the narrower `!integration/**/*.md`, `!integration/**/package.json`, `!integration/**/tsconfig*.json`, `!integration/**/*.ts`, etc.).
> - Until that lands, generated artifacts stay uncommitted and `git status` will not show them — do not be surprised.
> The rest of this plan assumes the `.gitignore` exception is added in E.1.

---

## A. Architecture Overview

### A.1 Design principle

```
┌─────────────────────┐   child_process / Bun.spawn    ┌──────────────────────────┐
│  pi-hashline (TS)   │ ──────────────────────────────► │                          │
│  opencode-plugin    │   read --json / patch / write   │   hashline binary        │
│  (arg translation   │                                 │   (Rust, crates/core)     │
│   + result render)  │ ◄────────────────────────────── │   hashing + staleness    │
└─────────────────────┘   stdout=data / stderr=diag     │   + recovery: SOURCE OF  │
                                                        │   TRUTH                  │
                                                        └──────────────────────────┘
```

- Both packages expose host-native tools (`read`/`edit` for pi; `hashline_read`/`hashline_edit`/`hashline_grep` for OpenCode) whose `execute` bodies translate parameters to `hashline` CLI invocations, spawn the binary, and render the result.
- **The TS code must NOT contain:** an xxHash32/xxh3 port, a nibble alphabet, a patch parser, a merge, a stale-anchor remap — none of it. That logic lives in `crates/core/src/hash.rs`, `document.rs`, `parser.rs`, `tokenizer.rs`, `commands/patch.rs`.
- The binary's own failure modes (stale anchor → exit 1 + `Error:`/`Hint:` on stderr, 3-way merge recovery where applicable, no-op handling) are authoritative. The wrapper surfaces them.

### A.2 Binary discovery (both packages share this)

Resolution order, in `src/hashline.ts` / `src/hashline-core.ts`:

1. `HASHLINE_BIN` env var (absolute path) — highest precedence.
2. `hashline` / `hashline.exe` on `PATH`.
3. On Windows only, also try `hashline.exe` next to the plugin's `process.cwd()` and the standard install location `~/.hashline/`.

Failure behavior: never throw at module load. At tool-execute time, `child.on("error")` / spawn rejection is mapped to a structured "binary-not-found" tool error that includes the install hint (mirror pi-hledit's `HLEDIT_INSTALL_HINT`, `.tmp/pi-hledit/index.ts:14-22`).

### A.3 Version contract with the binary (v0.9.1)

The wrappers depend on this CLI contract. It is **verified against source** (`crates/core/src/cli.rs`, `commands/read.rs`, `commands/patch.rs`, `output.rs`, `main.rs`). **Treat this table as the compatibility contract; bump `MIN_HASHLINE_VERSION` (constant in each package) if any shape changes.**

| Command | Flags | stdout (data) | stderr (diagnostics) | exit |
|---|---|---|---|---|
| `hashline read <file>` | `--json`, `--no-cache` | Text: header `[<path>#<4hex>]` then `N:hh\|content` per line (see A.4) | — | 0 |
| `hashline read <file> --json` | | Single-line JSON: `{"path": string, "hash": "<4hex lowercase>", "lines": [{"n": number, "hash": "<2hex>", "content": string}]}` | — | 0 |
| `hashline patch <file> <patch>` | `--dry-run`, `--json`, `--safe` | `[<path>#<4hex>]` + updated `N:hh\|content` lines; with `--json`: success `{"success": true, "file", "edits_applied": number, "lines": [{"line","hash","content"}]}` or dry-run `{"success": true, "file", "dry_run": true, "edits_applied", "diff": [...]}` | parser warnings `warning: ...`; errors `Error: ...\nHint: ...` or JSON object | 0 ok (incl. `*** Abort` no-op), 1 error (stale anchor, empty patch, binary file, missing file) |
| `hashline patch <file> -` | (stdin) | same as above | same | same |
| `hashline write <file> <content>` | `--force`, `--json`, `--safe` | text or JSON | same | 0/1 |
| `hashline find-block <file> <anchor>` | `--json`, `--pretty` | block text or `{"file","line_count","language","block_lines":[{"n","hash","content"}]}` | same | 0/1 |
| `hashline remove <file>` / `rename <src> <dst>` | `--json`; rename also `--force` | text or `{"success":true,...}` | same | 0/1 |
| `hashline --version` | | `hashline X.Y.Z` | — | 0 |

**Patch source resolution** (`commands/patch.rs:129-149`): `<patch>` of `-` reads all of stdin; `@path` reads a file; otherwise the literal string. The wrappers use **argv for single-op patches and stdin (`-`) for multi-op/envelope patches** (see A.5).

**Op syntax accepted by the binary** (case-insensitive keywords; range separator `..` or `.=`, `tokenizer.rs:181-220`):

| Op | Grammar | Notes |
|---|---|---|
| `SWAP N:HH:` | replace line at anchor | payload rows prefixed `+`; `++` → literal `+`, `+-` → literal `-` |
| `SWAP N..M:` | replace inclusive range | |
| `SWAP N.=M:` | alias for `..` | |
| `DEL N:HH` | delete single line at anchor | |
| `DEL N..M` / `DEL N.=M` | delete range | |
| `INS.PRE N:HH:` | insert before line N | |
| `INS.POST N:HH:` / `INS N:HH:` | insert after line N | `INS` is shorthand for `INS.POST` |
| `INS.HEAD:` | insert at BOF | |
| `INS.TAIL:` | insert at EOF | |
| `SWAP.BLK N:` / `SWAP.BLK N:HH M:HH:` | replace syntactic block | tree-sitter/brace based |
| `DEL.BLK N` / `DEL.BLK N:HH M:HH` | delete block | |
| `INS.BLK.POST N:` / `INS.BLK.PRE N:` / `INS.BLK N:` | insert block after/before N | |
| `REM` | delete whole file | |
| `MV "dest"` | rename file | |

Multi-op envelope (`messages.rs:6-8`): wrap ops between `*** Begin Patch` and `*** End Patch`; `*** Abort` discards the whole patch and the command exits 0 with no output. Header lines `[path#TAG]` may precede ops; the tag is optional and parser-stripped noise like `*** Update File:` is tolerated (`tokenizer.rs:671-731`).

**Anchor wire format.** Two families exist and are INCOMPATIBLE:
- The **binary** (`crates/core`) emits `N:hh|content` in read text output and JSON `{"n","hash","content"}`, where `hh` is 2 lowercase hex chars (xxh32 seed 0 of `line.trim_end()`, low byte — `hash.rs:8-68`). The patch parser accepts anchors `N:hh` (lowercase; tags in `[path#TAG]` headers are uppercased but per-line anchors are not).
- The **pi references** (`.tmp/pi-hashline-edit`) emit `LINE#HASH:content` with a **non-hex** nibble alphabet (`ZPMQVRWSNKTXJBYH`, xxh32-based) — those hashes are NOT what this binary computes and must NOT be mixed in.

**Decision (both wrappers):** render and accept anchors in the **binary's native format**. For pi, that is `N:hh|content` (exactly what `hashline read` prints), and edit ops accept `pos`/`end` anchors of the form `N:hh`. For OpenCode, `hashline_read` emits `N:hh|content` (binary-native, NOT paulp-o's `LINE#HASH:content` / tianhuil's `N:HH|content`) to avoid introducing a third incompatible format and to keep the edit anchor grammar identical to what the binary parses. The OpenCode prompt and zod `.describe()` strings advertise `N:hh|content` and `N:hh` anchors.

> **[VERIFY] Anchor separator for the OpenCode wrapper:** paulp-o uses `LINE#HASH:content` (`TAG_RE`, `.tmp/hashline-edit-opencode/src/lib/hashline-core.ts:99`), tianhuil uses `N:HH|content` (`.tmp/opencode-hashlines/src/lib/hashline.ts:58`), the binary uses `N:hh|content`. The plan chooses binary-native. Confirmed this repo's own docs (AGENTS.md lines 20-25, README.md) already use `N:hh|content` / `12:ab3f`, so binary-native is consistent with the repo. If a future reviewer prefers a `LINE#HASH`-style display to match the broader pi ecosystem, that is a **display-only** change in `formatReadPreview` and must be paired with a translator back to `N:hh` before building patch strings.

### A.4 read wire shape (what the wrapper parses)

Verified against `commands/read.rs:17-49`:

```json
{
  "path": "src/main.rs",
  "hash": "3a58",
  "lines": [
    {"n": 1, "hash": "9b", "content": "fn main() {"},
    {"n": 2, "hash": "89", "content": "    let name = \"hashline\";"}
  ]
}
```

- `n` is 1-based; content is LF-normalized, BOM-stripped text (`document.rs:60-70`). The trailing newline of the file does NOT produce a phantom line in `read --json` (`read.rs:21-23` filter). CRLF files appear LF-only in the JSON.
- Text output format to match (used by the pi read tool): first line `[<path>#<4hex>]`, then `N:hh|content` per line, no phantom trailing line.
- **`read` has NO `--offset`/`--limit`/`--anchor` flags** (cli.rs:33-39 — only `--json`, `--no-cache`). `read` always emits the whole file. If pagination is required, the wrapper slices the returned lines in TS (offset/limit are wrapper-only parameters). The repo's `find-block` subcommand is the only line-scoping primitive.

### A.5 patch invocation strategy

- **Single-op, few payload lines:** pass the patch as an argv element: `hashline patch <file> "SWAP 4:d1:\n+  newline"` — newlines survive as a single argv element on both POSIX and Windows spawn-without-shell.
- **Multi-op / envelope:** `hashline patch <file> -`, stdin piped, write `*** Begin Patch\n[path#TAG]\n...ops...\n*** End Patch\n`, then `child.stdin.end()` (mirror pi-hledit `index.ts:263`). Always drain stdout+stderr to EOF before parsing (`main.rs:87-117` wraps stdout in a 1 MiB BufWriter and flushes before `process::exit`).
- **`--dry-run`** is exposed to the host agent as a parameter on the edit tools so the model can preview before applying.
- **Always branch on exit code FIRST.** On exit 0, `stdout` is a valid payload (parse `--json` if requested). On exit 1, stdout is empty and stderr carries the diagnostic (`Error: ...\nHint: ...` in pretty mode; a single-line JSON object `{"kind","error","hint","command"}` when `--json` was requested — `output.rs:52-87`). Never try to JSON-parse stdout on failure.

### A.6 Exit-code / stderr taxonomy (contract both wrappers implement)

| Condition | exit | stdout | stderr |
|---|---|---|---|
| success (incl. `*** Abort` no-op) | 0 | data | — |
| stale anchor | 1 | (empty) | `Error: line N content changed since last read in <path> (expected hash X, got Y)` + Hint (re-read) |
| empty / garbage-only patch | 1 | (empty) | `Error: patch produced no edits — ...` / `EMPTY_PATCH` |
| binary file / missing file / I/O | 1 | (empty) | `Error: ...` + Hint |
| ambiguous hash / hash not found | 1 | (empty) | `Error: ...` + Hint |

There is **no dedicated stale/no-op code**; exit 1 is "any HashlineError" (`main.rs:92-107`). The wrapper maps exit 1 + stderr text to a structured tool error with a `kind` field (see D.2) and appends the "re-read" teaching text to the model-facing message.

### A.7 What the Rust side needs to expose

**Nothing new today.** `crates/core/src/mcp.rs` already exposes read/patch/write/find_block/remove_file/rename_file as JSON-RPC tools (verified in `tool_list()`, lines 100-179, and `call_tool`, lines 639-720, each also accepted with a `hashline_` prefix), and the CLI exposes the same operations. The wrappers depend only on the CLI contract in A.3/A.5. **No Rust changes are required for the initial release.**

Documented in D.4 as optional future work:
- `hashline grep` subcommand (does NOT exist today — verified Commands enum, cli.rs:18-29). The OpenCode `hashline_grep` tool must spawn `rg` itself and re-hash lines by piping through the binary, spawn the sibling `ffs` binary, or defer (see C.7).
- `hashline read --limit/--offset` (does NOT exist today) — wrappers slice in TS instead.

---

## B. Package 1: `integration/pi-hashline`

pi-coding-agent extension. The `pi` runtime loads `index.ts` directly (no compile step; `package.json` `"pi": {"extensions": ["./index.ts"]}`). TypeScript is run through pi's Bun/Node loader, so source ships uncompiled — mirror pi-hledit's `"type": "module"` + `"main": "index.ts"` shape (pi-hledit has `"type": "module"`; pi-hashline-edit does not — either works, but **use `"type": "module"`** like the newer pi-hledit package).

### B.1 `package.json`

```jsonc
{
  "name": "hashline-pi",
  "version": "0.1.0",
  "description": "Hashline read/edit override for pi-coding-agent — thin wrapper that shells out to the hashline binary",
  "type": "module",
  "main": "index.ts",
  "license": "MIT",
  "files": ["index.ts", "src", "prompts", "README.md", "LICENSE"],
  "publishConfig": { "access": "public" },
  "pi": {
    "extensions": ["./index.ts"],
    "image": "https://raw.githubusercontent.com/<owner>/hashline/main/integration/pi-hashline/docs/demo.png"
  },
  "scripts": {
    "test": "npm run typecheck && npm run contract && npm run lint",
    "contract": "rm -rf .tmp-tests && tsc -p tsconfig.contract.json && node --test .tmp-tests/test/*.test.js",
    "typecheck": "tsc --noEmit",
    "lint": "biome check .",
    "format": "biome format --write ."
  },
  "dependencies": {
    "@earendil-works/pi-tui": "^0.79.9",
    "typebox": "^1.0.55"
  },
  "peerDependencies": {
    "@earendil-works/pi-coding-agent": "^0.79.9",
    "@earendil-works/pi-ai": ">=0.74.0"
  },
  "devDependencies": {
    "@biomejs/biome": "^2.5.2",
    "@earendil-works/pi-coding-agent": "^0.79.9",
    "@types/node": "^22.0.0",
    "typescript": "^6.0.3",
    "vitest": "^3.0.0"
  },
  "engines": { "node": ">=18" }
}
```

Notes (verified against the references):
- **Peer deps confirmed from the report:** pi-hashline-edit declares peers `@earendil-works/pi-ai >=0.74.0`, `@earendil-works/pi-coding-agent >=0.74.0`, `@earendil-works/pi-tui "*"`, `@sinclair/typebox "*"`; the newer thin-wrapper pi-hledit declares deps `@earendil-works/pi-tui ^0.79.9` + `typebox ^1.0.55` and peer `@earendil-works/pi-coding-agent ^0.79.9`. The plan follows the **pi-hledit shape** (that is the thin-wrapper precedent): `typebox` and `pi-tui` as real dependencies, `pi-coding-agent` and `pi-ai` as peers. npm latest pi-coding-agent is 0.84.1; pinning the peer floor at `^0.79.9` guarantees schema compatibility with what the SDK exports.
- **Typebox identity:** current pi-coding-agent@0.84.1 ships bare `typebox@1.3.7` (its npm-shrinkwrap). Import `Type, { type Static }` **from `"typebox"`** (the sinclairzx81 rename of `@sinclair/typebox`), matching pi-hledit. Do NOT import from `@sinclair/typebox` unless you add it as a `*` peer.
- Biome (dev-dep) is used for lint; contract tests use `node:test` + `tsc -p tsconfig.contract.json` (the pi-hledit strategy), which suits a thin wrapper — unit-test arg translation + stdout parsing, not the live binary. (Vitest is also present per the task's requested deps; see B.10 for the recommended split.)

### B.2 `tsconfig.json` / `tsconfig.contract.json`

```jsonc
// tsconfig.json
{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "esModuleInterop": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true
  },
  "include": ["index.ts", "src", "test"]
}
```

```jsonc
// tsconfig.contract.json — compiles entry + tests to .tmp-tests, run with node --test
{
  "compilerOptions": {
    "noEmit": false,
    "outDir": ".tmp-tests",
    "rootDir": ".",
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["index.ts", "test/**/*.ts"]
}
```

### B.3 File structure

```
integration/pi-hashline/
├── package.json
├── tsconfig.json
├── tsconfig.contract.json
├── biome.json                 # biome preset "recommended"; disable style.noNonNullAssertion,
│                              # suspicious.noConsole, correctness.noUnused*, formatter per ref
├── index.ts                   # default export factory: registerReadTool/registerEditTool/opt-in grep
│                              # + session_start config warnings (mirror pi-hashline-edit/index.ts:7-28)
├── README.md
├── prompts/
│   ├── read.md                # promptSnippet + promptGuidelines for read
│   └── edit.md                # op docs + example + rules (port of pi-hashline-edit/prompts/edit.md,
│                              #  rewritten to binary-native N:hh anchors)
├── src/
│   ├── config.ts              # parse ~/.pi/agent/hashline.json (hashLength/grep/replaceText → now
│   │                          #   mostly advisory; binary owns hashes) + HASHLINE_BIN override
│   ├── hashline.ts            # resolveHashlineBin(env) + runHashline(args, stdin?, ctx, signal)
│   │                          #   → {stdout, stderr, exitCode} (port of pi-hledit runHledit, index.ts:232-265)
│   ├── read.ts                # registerReadTool(pi) → hashline read <path> --json, render N:hh|content
│   ├── edit.ts                # registerEditTool(pi) → translate {path, edits:[{op,pos,end,lines}]}
│   │                          #   → hashline patch, envelope via stdin; map exit 1 → stale teaching error
│   ├── edit-args.ts           # pure arg translation: edits[] → patch ops; exported for contract tests
│   ├── grep.ts                # OPTIONAL / DEFERRED: no binary grep today; see B.9
│   ├── result.ts              # AgentToolResult builders: {content:[{type:"text",text}], details, isError}
│   └── render.ts              # (optional TUI polish) pi-tui Markdown/Text wrappers; renderDiff try/catch
└── test/
    └── contract.test.ts       # node:test, stub ExtensionAPI/ExtensionContext/Theme/Component
```

**Explicitly NOT ported** (lives in the binary): `src/hashline/hash.ts`, `parse.ts`, `apply.ts`, `format.ts`, `merge.ts`, `read-snapshot.ts`, `fs-write.ts`, `noop-loop-guard.ts`, `edit-diff.ts` core. The package is ~200-400 lines, matching pi-hledit's single 1090-line `index.ts`.

### B.4 Entry point — `index.ts`

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerEditTool } from "./src/edit";
import { registerReadTool } from "./src/read";
import { getGrepEnabled, getConfigWarnings } from "./src/config";

export default function (pi: ExtensionAPI): void {
  registerReadTool(pi);
  registerEditTool(pi);
  if (getGrepEnabled()) {
    // registerGrepTool(pi); // optional, B.9
  }

  pi.on("session_start", async (_event, ctx) => {
    const warnings = getConfigWarnings();
    if (warnings.length > 0) {
      ctx.ui.notify(`hashline.json config warnings:\n${warnings.join("\n")}`, "warning");
    }
    if (process.env.PI_HASHLINE_DEBUG === "1" || process.env.PI_HASHLINE_DEBUG === "true") {
      ctx.ui.notify("Hashline mode active", "info");
    }
  });
}
```

Mirror of `.tmp/pi-hashline-edit/index.ts:7-28`. `ExtensionAPI` and `ExtensionFactory` (`types.d.ts:1104`) are the real types; the default export is a plain `(pi: ExtensionAPI) => void`. `ctx.ui.notify(message, kind: "info" | "warning" | "error")` is confirmed available on `ExtensionContext`.

### B.5 Tool registration — overriding built-ins (name-collision precedence)

- **Override, don't add.** Register tools named exactly `"read"` and `"edit"`. Verified precedence: `AgentSession._refreshToolRegistry` seeds base tools into a Map keyed by name, then applies extension tools with `definitionRegistry.set(tool.definition.name, ...)` — **last registration wins** (`agent-session.js:1963-1974, 1990-1997`). So `pi.registerTool({ name: "read" })` and `name: "edit"}` replace the built-ins of the same name. Across multiple extensions, `getAllRegisteredTools()` keeps the FIRST registration per name (`runner.js:284-293`), so our read/edit win if loaded earlier.
- **Active-tool default set** is `["read","bash","edit","write"]` (`agent-session.js:2045-2046`); at initial build `includeAllExtensionTools: true` (`agent-session.js:158-160`) pushes extension tools into the active set. The opt-in `grep` tool becomes active automatically when registered.
- **`renderShell: "default"` is mandatory on the overriding edit tool** (`edit.ts:545`) or it inherits the built-in's `renderShell: "self"` and loses the shared background shell.
- Parameters use `TypeBox` (`Type.Object / Type.String / Type.Integer({minimum:1}) / Type.Optional / Type.Union / Type.Boolean`). `prepareArguments` is available as a dialect-convergence shim (`edit.ts:534-541`) — optional for the first release (see D.6).
- `ToolDefinition` shape (confirmed via DeepWiki on badlogic/pi-mono): `{ name, label, description, promptSnippet?, promptGuidelines?, parameters, renderShell?, prepareArguments?, execute(toolCallId, params, signal, onUpdate, ctx): Promise<AgentToolResult<T>>, renderCall?, renderResult? }`. `AgentToolResult<T>` = `{ content: (TextContent | ImageContent)[], details: T, usage?, addedToolNames?, terminate? }`. Errors are signaled by **throwing** from `execute` (the pi-native contract; pi catches and reports with isError) OR by returning `{ content, details, isError: true }` — both work; this plan uses the throw-free `{ isError: true }` result to keep the model-facing `content` text structured (see B.7).

### B.6 Read tool — `src/read.ts`

```ts
export function registerReadTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "read",
    label: "Read",
    description: READ_DESC,                       // loaded from prompts/read.md via prompt-loader
    promptSnippet: READ_PROMPT_SNIPPET,
    promptGuidelines: READ_PROMPT_GUIDELINES,
    parameters: Type.Object({
      path: Type.String({ description: "Path to the file to read (relative or absolute)" }),
      offset: Type.Optional(Type.Integer({ minimum: 1, description: "Line number to start from (1-indexed)" })),
      limit:  Type.Optional(Type.Integer({ minimum: 1, description: "Max lines to read" })),
      raw:    Type.Optional(Type.Boolean({ description: "Return plain text without anchors (cheaper)" })),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const file = resolveToCwd(params.path, ctx.cwd);        // expand ~, join cwd (path-utils.ts)
      const { stdout, stderr, exitCode } = await runHashline(
        ["read", file, "--json"], undefined, ctx, signal,
      );
      if (exitCode !== 0) return buildBinaryError(stderr, file, params.path);
      const parsed = parseReadJson(stdout);                   // {path,hash,lines}
      const preview = formatReadPreview(parsed, { offset: params.offset, limit: params.limit, raw: params.raw });
      return { content: [{ type: "text", text: preview.text }],
               details: { truncation: preview.truncation, nextOffset: preview.nextOffset } };
    },
  });
}
```

- **Render format decision:** render the binary's native `N:hh|content` (exactly what `hashline read` prints), with a leading `[<path>#<4hex>]` header line. This keeps read output byte-identical to the CLI and keeps edit anchors textually identical to what `hashline patch` parses. (pi-hashline-edit uses `LINE#HASH:content` with its own nibble alphabet — we deliberately do NOT.)
- `raw` returns plain lines (no hashes, no header) to save tokens.
- `offset`/`limit` are wrapper-side slices of the whole-file JSON (the binary has no pagination flags — verified cli.rs:33-39). Emit `details.nextOffset` when truncated so the model knows to continue.
- Errors from `hashline read` (missing/binary file → exit 1) are surfaced as `Error: ...` content with `isError: true`, mirroring `pi-hashline-edit/src/read.ts:162-207` but sourced from the binary's stderr rather than TS `fs` checks. Binary/image passthrough to the built-in `createReadTool(cwd)` (`pi-hashline-edit/src/read.ts:198-207`) is optional v1.1 polish; v0.1.0 relies on the binary's own `BINARY_FILE`/`INVALID_UTF8` errors.

### B.7 Edit tool — `src/edit.ts`

Schema (mirror pi-hashline-edit `src/edit.ts:56-170`, but anchors are binary-native `N:hh`):

```ts
const anchor = Type.String({ description: "anchor N:hh from read output" });

const replaceEdit = Type.Object({
  op: literalString("replace", { description: "replace one line at pos, or inclusive pos..end, with lines" }),
  pos: anchor,
  end: Type.Optional(anchor),                       // inclusive end of range
  lines: Type.Array(Type.String()),
}, { additionalProperties: false });

const appendEdit  = Type.Object({
  op: literalString("append"),
  pos: Type.Optional(anchor),                       // omit = EOF
  lines: Type.Array(Type.String()),
}, { additionalProperties: false });

const prependEdit = Type.Object({
  op: literalString("prepend"),
  pos: Type.Optional(anchor),                       // omit = BOF
  lines: Type.Array(Type.String()),
}, { additionalProperties: false });

const deleteEdit = Type.Object({
  op: literalString("delete"),
  pos: anchor,
  end: Type.Optional(anchor),                       // inclusive range delete
}, { additionalProperties: false });

export const editToolSchema = Type.Object({
  path: Type.String(),
  edits: Type.Array(Type.Union([replaceEdit, appendEdit, prependEdit, deleteEdit])),
}, { additionalProperties: false });
```

**Op → patch-string translation** (pure function in `src/edit-args.ts`, unit-tested):

| edit op | patch op | generated patch text |
|---|---|---|
| `replace {pos:"N:hh", lines}` (no end) | `SWAP N:hh:` | `SWAP N:hh:\n+ <lines...>` |
| `replace {pos, end:"M:aa", lines}` | `SWAP N..M:` | `SWAP N:hh M:aa:\n+ <lines...>` |
| `append {pos:"N:hh", lines}` | `INS.POST N:hh:` | `INS.POST N:hh:\n+ <lines...>` |
| `append {}` (no pos) | `INS.TAIL:` | `INS.TAIL:\n+ <lines...>` |
| `prepend {pos:"N:hh", lines}` | `INS.PRE N:hh:` | `INS.PRE N:hh:\n+ <lines...>` |
| `prepend {}` (no pos) | `INS.HEAD:` | `INS.HEAD:\n+ <lines...>` |
| `replace` with empty `lines` | `DEL N:hh` / `DEL N..M` | delete |
| `delete {pos}` / `{pos, end}` | `DEL N:hh` / `DEL N..M` | delete single line / range |
| `replace_text {oldText,newText}` | see below | only when a unique single-line match exists |

**`replace_text` (op in the reference's dialect, task-requested):** the binary has NO text-match/sed op. The wrapper implements it as a **pure translation**: re-read the file (`hashline read <path> --json`), find every line whose `content` contains/equals `oldText`. If exactly one line matches, translate to `SWAP N:hh:\n+ <newText>` (or `DEL` if `newText` is empty). If zero or multiple matches, return a structured error `replace_text matched N lines (need exactly 1) — use replace with an N:hh anchor from read` with `kind: "ambiguous_hash"`/`"hash_not_found"`. Never apply a heuristic on multiple matches. This keeps the invariant that all semantic validation happens in the binary.

Payload lines that begin with `+` must be escaped as `++`; lines beginning with `-` should be emitted as `+-` to suppress the bare-`-` warning (`patch_format.rs:53-56`, AGENTS.md pitfall). Multi-op edits are wrapped in `*** Begin Patch`/`*** End Patch` and piped via stdin (`hashline patch <file> -`); single-op edits may use argv.

`execute` pipeline:

```ts
async execute(_toolCallId, params, signal, _onUpdate, ctx) {
  const file = resolveToCwd(params.path, ctx.cwd);
  const patchText = buildPatchText(params.edits);          // edit-args.ts; may need a read for replace_text
  const { stdout, stderr, exitCode } = await runHashline(
    ["patch", file, "-"], patchText, ctx, signal,          // stdin form
  );
  if (exitCode !== 0) {
    // Stale-anchor / error: surface binary's message + teaching re-read text
    return { content: [{ type: "text", text: formatPatchError(stderr, file, params.path) }],
             details: { ok: false }, isError: true };
  }
  const { firstChangedLine } = parsePatchStdout(stdout, /*patch --json*/ false);
  // Chained edits: re-read the changed region for fresh anchors
  const fresh = await runHashline(["read", file, "--json"], undefined, ctx, signal);
  const anchors = fresh.exitCode === 0 ? renderAnchorBlock(parseReadJson(fresh.stdout)) : "";
  return { content: [{ type: "text", text: `Patch applied.\n${anchors}` }],
           details: { ok: true, firstChangedLine } };
}
```

- **Exit-1 mapping:** `hashline` treats a stale anchor as `HashlineError::StaleAnchor` → exit 1 → stderr `Error: line N content changed since last read in <file> (expected hash X, got Y)` + Hint. The wrapper returns that message verbatim plus a fixed teaching footer: `Re-read the file with \`read\` and retry with a fresh anchor.` **Recovery is delegated — the wrapper never re-implements the 3-way merge or remap logic.**
- **Chained edits:** after a successful patch, run `hashline read <file> --json` and append the re-rendered anchor block (an `--- Anchors ---` section) so the model can continue editing without a separate read call. This is the wrapper analog of pi-hashline-edit's `--- Anchors A-B ---` block (`edit-response.ts:62-136`) — the anchors come from the binary, not from TS computation.
- **No-op detection:** the binary handles `*** Abort` (exit 0, no output) and empty/garbage patches (exit 1, `EMPTY_PATCH`). The wrapper does not maintain its own no-op loop guard (binary authoritative). Optional [VERIFY] if the host needs one, track it in `result.ts`.

### B.8 Config — `src/config.ts`

- Port `parseHashlineConfig` from `.tmp/pi-hashline-edit/src/config.ts:39-157` (hashLength 2|3|4 default 2, grep bool default false, replaceText bool default true), load `join(getAgentDir(), "hashline.json")` once at module init with `readFileSync`; ENOENT → silent defaults, other failures → warnings, never throw.
- **hashLength semantics change:** the binary computes 2-hex hashes and the wrapper cannot re-hash. For v0.1.0: accept the field but treat values `3|4` as advisory; the anchors rendered come from the binary regardless. **[VERIFY]** Whether `hashline` will ever add an adaptive-length flag (`hashline-ecosystem-research.md:118-121` cites the idea) — if it lands, pass `--hash-length N` to read/patch and the config becomes functional again. For now document the limitation.
- Add `binary` field: `"binary": "/abs/path/to/hashline"` → used as `HASHLINE_BIN` equivalent (highest precedence, before env). This addresses the pi reference's reliance on PATH.
- `grep` → gate `registerGrepTool`.
- `replaceText` → gate the `replace_text` op translation (B.7).

### B.9 Grep tool — `src/grep.ts` (optional, DEFERRED by default)

**Deferred for v0.1.0.** The hashline binary has no grep subcommand (verified: cli.rs has read/patch/write/find-block/guide/serve/mcp/remove/rename only), and grep is intentionally out of hashline's scope (owned by the sibling `ffs` repo). If a `hashline grep` tool is wanted later, it spawns `ffs grep --json` and re-hashes matched lines via `hashline read --json` — see C.7 and D.4. Do not implement search inside hashline.

### B.10 Testing — vitest + mocked spawn

- `src/hashline.ts` exports `runHashline(args, stdin?, ctx, signal)`; tests inject a fake `runHashline` via a dependency-injection seam (`setRunnerForTests` or a `createHashlineRunner()` factory). No live binary in unit tests.
- `test/edit-args.test.ts`: op→patch-string translation table (all rows of B.7), `++`/`+-` escaping, multi-op envelope assembly.
- `test/read-format.test.ts`: `parseReadJson` + `formatReadPreview` against fixtures captured from the real binary (see E.8 golden fixtures), incl. offset/limit slicing and `details.nextOffset`.
- `test/error-mapping.test.ts`: exit-1 stderr samples (StaleAnchor pretty + JSON, EmptyPatch, missing file) → `formatPatchError` output.
- `test/contract.test.ts` (node:test via `tsconfig.contract.json`): stub `ExtensionAPI`/`ExtensionContext`/`Theme`/`Component` per pi-hledit's `test/contract.test.ts`, assert registered tool definitions (name `read`/`edit`, schema, renderShell `"default"`).
- **Integration smoke (manual, not CI):** build `hashline` with `cargo build -p hashline --release`; run `pi install .` from `integration/pi-hashline`, `/reload`, verify `/hashline-status`.

---

## C. Package 2: `integration/opencode-plugin`

OpenCode plugin package. Bun runtime (OpenCode's host is Bun); built with `bun build` + `tsc --emitDeclarationOnly`.

### C.1 API correctness — the `helper.*` namespace does NOT exist

The task brief mentions `helper.tool / helper.session.* / helper.user.* / helper.app.*`. **Verified against the installed SDK `C:/Users/ADMIN/.config/opencode/node_modules/@opencode-ai/plugin/dist/index.d.ts` (v1.4.6) and `dist/tool.d.ts`:** there is **no `helper.tool` / `helper.session` / `helper.user` / `helper.app` / `helper.project`** export. Do not design around it. (The `helper.*`/SDK-client facet language in some docs refers to `@opencode-ai/sdk` helpers such as `client.app.log()`; the references do NOT use them for the file tools.)

The real surface is:
- `import type { Plugin } from "@opencode-ai/plugin";`
- `import { tool } from "@opencode-ai/plugin";` — `tool()` returns a `ToolDefinition` plain object `{ description, args, execute }`; `tool.schema` IS zod (`dist/tool.js`: `tool.schema = z`). zod 4.1.8 is the bundled schema engine.
- Plugin signature: `type Plugin = (input: PluginInput, options?: PluginOptions) => Promise<Hooks>`; `PluginModule` may default-export (`export default plugin`) or named-export (`export const HashlinePlugin: Plugin`).
- Tools register under the `tool: { [key: string]: ToolDefinition }` key of the returned Hooks object (`index.d.ts:175-178`).
- `execute(args, context)` where `context: ToolContext` = `{ sessionID, messageID, agent, directory, worktree, abort, metadata({title,metadata}), ask(...) }` (`tool.d.ts:3-25`). Resolve paths via `context.directory || context.worktree`.
- Optional hooks: `"experimental.chat.system.transform"` (push into `output.system: string[]`), `"tool.execute.after"` (post-hoc re-render of built-in output), `"tool.definition"` (edit a tool's description/parameters). All confirmed in `index.d.ts:231-266,307-312`.
- `context.metadata({ title })` is the ONLY UI hook that shows array args in the tool title (paulp-o `src/index.ts:279`, `docs/opencode-tool-ui-title.md`); without it `hashline_edit` renders as just `⚙ hashline_edit [path=...]`.

### C.2 `package.json`

```jsonc
{
  "name": "@scope/hashline-opencode-plugin",
  "version": "0.1.0",
  "description": "Hashline plugin for OpenCode — thin wrapper that shells out to the hashline binary",
  "type": "module",
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": { "types": "./dist/index.d.ts", "import": "./dist/index.js", "default": "./dist/index.js" }
  },
  "files": ["dist", "README.md", "LICENSE"],
  "bin": {
    "hread": "./dist/cli/hread.js",
    "hedit": "./dist/cli/hedit.js"
  },
  "peerDependencies": {
    "@opencode-ai/plugin": ">=1.0.0"
  },
  "devDependencies": {
    "@opencode-ai/plugin": "^1.2.10",
    "@types/bun": "^1.3.4",
    "typescript": "^5.8.0"
  },
  "scripts": {
    "build": "bun build src/index.ts --outdir dist --format esm --sourcemap=linked --target=bun",
    "build:types": "tsc --emitDeclarationOnly",
    "build:cli": "bun build src/cli/hread.ts src/cli/hedit.ts --outdir dist/cli --format esm --target=bun",
    "clean": "rm -rf dist",
    "test": "bun test src/tests/",
    "typecheck": "tsc --noEmit",
    "build:all": "rm -rf dist && bun run build && bun run build:cli && bun run build:types",
    "prepublishOnly": "bun run build:all && bun run typecheck && bun test src/tests/"
  },
  "publishConfig": { "registry": "https://registry.npmjs.org/", "access": "public" }
}
```

- **peer version:** the references tested against 1.2.6 (tianhuil) and 1.2.11 (paulp-o); the installed SDK is 1.4.6. The `Plugin`/`tool`/`Hooks` contract is stable across these (verified). Use peer `">=1.0.0"` (paulp-o) and pin devDependency to the version you actually build against — **[VERIFY] decide `^1.2.10` vs `^1.4.6` at implementation time** by running `typecheck` against the local `bun.lock`. The task's requested name `@scope/hashline-opencode-plugin` is used; replace `@scope` with the real npm scope at publish.
- Follow paulp-o's script set. `bun build` with `--target=bun`; type declarations via `tsc --emitDeclarationOnly`.

### C.3 File structure

```
integration/opencode-plugin/
├── package.json
├── tsconfig.json               # target ES2022, module ESNext, moduleResolution bundler,
│                               # declaration + emitDeclarationOnly, outDir dist, rootDir src, strict
├── bun.lock
├── README.md                   # install via opencode.json plugin array + tools:{edit:false}
├── src/
│   ├── index.ts                # re-export entry: `export { default } from "./plugin";`
│   ├── plugin.ts               # `const plugin: Plugin = async (ctx) => ({ tool: {...},
│   │                           #   "experimental.chat.system.transform": ... }); export default plugin;`
│   ├── hashline-core.ts        # resolveHashlineBin + runHashline spawn helper; parse ReadResult /
│   │                           #   PatchResult / ErrorPayload JSON (the CLI contract A.3)
│   ├── hashline-apply.ts       # op-model → patch-string translator (buildPatchText / buildEditBatch).
│   │                           #   Pure translation ONLY — no hashing, no merge, no remap
│   ├── hashline-errors.ts      # formatMismatch / formatHashlineError; exit-code taxonomy (D.2);
│   │                           #   kind detection; install hint
│   ├── format.ts               # formatReadPreview (N:hh|content + [path#4hex]), offset/limit slice
│   ├── prompt.ts               # renderHashlineEditPrompt — example hashes regenerated by the binary
│   └── cli/                    # hread.ts, hedit.ts (thin bin wrappers, paulp-o style; optional)
├── test/                       # bun test
│   ├── binary.test.ts          # resolution order, spawn argv, stdout/stderr capture, abort
│   ├── edit.test.ts            # buildPatchText translation table, JSON parse, error mapping
│   ├── prompt.test.ts          # prompt renders, example anchors consistent with N:hh
│   └── e2e.test.ts             # optional; gated on real binary on PATH
├── dist/                       # bun build output (gitignored)
└── .npmrc                      # npm registry/access config
```

**Explicitly NOT ported** (all binary-owned): `hashline-core.ts`'s `computeLineHash`/`NIBBLE_STR`/`DICT`/`formatHashLines`/`parseTag`, `hashline-apply.ts`'s `applyHashlineEdits`/`collectEdits`/`dedup`/`validateAllHashes`/`sortEditsBottomUp`/`detectNoOp`, `hashline-strip.ts`, `grep-search.ts`'s `fsBasedSearch`, and opencode-hashlines' `lib/hashline.ts` + `lib/schema.ts`. Keep only: zod arg schemas, prompt text, binary-spawn glue, error rendering.

### C.4 Entry — `src/plugin.ts`

```ts
import type { Plugin } from "@opencode-ai/plugin";
import { tool } from "@opencode-ai/plugin";
import { hashlineReadTool } from "./hashline-apply";  // args + execute definitions, or separate files
import { renderHashlineEditPrompt } from "./prompt";

const plugin: Plugin = async (ctx) => {
  return {
    tool: {
      hashline_read: tool({ /* C.5 */ }),
      hashline_edit: tool({ /* C.6 */ }),
      // hashline_grep: tool({ /* C.7 */ }),   // deferred by default
    },
    "experimental.chat.system.transform": async (_input, output) => {
      output.system.push(renderHashlineEditPrompt());
    },
  };
};
export default plugin;
```

`src/index.ts` just re-exports: `export { default } from "./plugin";`.

### C.5 `hashline_read`

```ts
hashline_read: tool({
  description:
    "Read a file. Returns lines tagged as `N:hh|content` where hh is the 2-char content hash " +
    "(the hashline binary format). Pass these N:hh anchors to hashline_edit. Prefer this over read.",
  args: {
    path: tool.schema.string().describe("Path to the file to read"),
    offset: tool.schema.number().optional().describe("First line (1-based)"),
    limit:  tool.schema.number().optional().describe("Max lines (default all)"),
  },
  async execute(args, context) {
    const filePath = resolvePath(args.path, context.directory || context.worktree);
    const { stdout, stderr, exitCode } = await runHashline(["read", filePath, "--json"], undefined, context.abort);
    if (exitCode !== 0) return `Error: ${stderr.trim() || `hashline exited ${exitCode}`}`;
    return formatRead(parseReadJson(stdout), { offset: args.offset, limit: args.limit });
  },
}),
```

- Output format: `N:hh|content` lines (binary-native, per A.3 decision), prefixed with `[path#4hex]` (the 4-hex file tag is a cheap version marker the binary computes).
- `context.metadata({ title: "read <path>" })` optional polish.
- The binary has no `--offset/--limit`; the wrapper slices the whole-file JSON (binary-native content is authoritative; slicing is pure presentation).

### C.6 `hashline_edit`

```ts
hashline_edit: tool({
  description:
    "Edit a file using N:hh anchors from hashline_read. Operations are validated atomically by the " +
    "hashline binary; stale anchors are rejected with a mismatch error. Prefer this over edit.",
  args: {
    path: tool.schema.string().describe("Path to the file"),
    edits: tool.schema.array(tool.schema.discriminatedUnion("op", [
      tool.schema.object({
        op: tool.schema.literal("replace"),
        pos: tool.schema.string().describe("anchor N:hh"),
        end: tool.schema.string().optional().describe("inclusive end anchor N:hh"),
        lines: tool.schema.array(tool.schema.string()),
      }),
      tool.schema.object({
        op: tool.schema.literal("append"),
        pos: tool.schema.string().optional().describe("insert after this N:hh; omit = EOF"),
        lines: tool.schema.array(tool.schema.string()),
      }),
      tool.schema.object({
        op: tool.schema.literal("prepend"),
        pos: tool.schema.string().optional().describe("insert before this N:hh; omit = BOF"),
        lines: tool.schema.array(tool.schema.string()),
      }),
      tool.schema.object({
        op: tool.schema.literal("delete"),
        pos: tool.schema.string().describe("anchor N:hh to delete"),
        end: tool.schema.string().optional().describe("inclusive end anchor N:hh"),
      }),
    ])).describe("Edit operations; applied atomically bottom-up by the binary"),
  },
  async execute(args, context) {
    const filePath = resolvePath(args.path, context.directory || context.worktree);
    context.metadata({ title: buildEditTitle(args) });       // path + op summary (paulp-o index.ts:279)
    const patchText = buildPatchText(args.edits);            // hashline-apply.ts; shared translator
    const { stdout, stderr, exitCode } = await runHashline(
      ["patch", filePath, "-"], patchText, context.abort,    // stdin form; prefer over argv for multi-op
    );
    if (exitCode !== 0) {
      // binary owns mismatch rendering; return its message verbatim
      return formatMismatch(stderr, args.path);              // includes Error:/Hint: context as the binary emits it
    }
    return `Patch applied. Re-read with hashline_read for fresh anchors.`;
  },
}),
```

- **Atomic batch / validate-before-apply:** the binary's `parseAndValidate` + bottom-up apply (`commands/patch.rs:342-477`) already validates all hashes before writing; the wrapper does not need its own validate-all pass. The zod schema validates the shape client-side; semantic validation (hash correctness) is the binary's job.
- **Mismatch rendering:** do NOT reimplement the `±2 context / >>> marker` renderer from `.tmp/hashline-edit-opencode/src/lib/hashline-errors.ts`. The binary's stderr already contains the formatted `Error:`/`Hint:` (pretty mode) with the expected/actual hashes. Return it verbatim plus a retry hint. **[VERIFY] the Rust binary does not currently emit a `remaps` map in its error JSON** (error.rs `StaleAnchor` only has expected/actual; see D.4) — if a future `--json` error carries suggested positions, surface them.
- File-level ops (delete/move) live in the plugin layer (fs `unlink`/`rename`) or as patch ops. v0.1.0: expose `delete`/`move` via patch `REM`/`MV` strings rather than fs, keeping everything on the binary.
- `execute` returns `Promise<string>` (plain text). Both references return multi-line strings; never `{type:"text",text}` blocks in OpenCode.

### C.7 `hashline_grep` (OPTIONAL / DEFERRED — grep is NOT hashline's job)

**Decision: the hashline Rust binary adds NO grep subcommand and NO ffs dependency.** Grep/search is owned by the sibling [fast_file_search](https://github.com/quangdang46/fast_file_search) (`ffs`) repo; adding it to hashline would exceed scope. Wrappers that want a `hashline_grep` tool spawn the `ffs` binary and re-hash matches via `hashline read --json`; otherwise the tool is deferred (recommended for v0.1.0).

If a wrapper implements it:
1. Spawn `ffs grep <needle> --json [--root <dir>] [--limit N]` (ffs installed pinned to a rev; `HASHLINE_GREP_BIN` override).
2. Parse `{hits:[{path,line,text}],...}`; map each `line` → `N:hh` via `hashline read <file> --json` (hashes from the binary, never in TS).
3. Render `file` header + `N:hh: content`, usable directly in `hashline_edit`.

Defer if `ffs` is absent — never fall back to `rg` inside hashline.

### C.8 System-prompt hook + opencode.json config

- Keep `"experimental.chat.system.transform"` pushing a prompt that teaches `N:hh|content`, `N:hh` anchors, bottom-up apply, and mismatch→re-read recovery (port `.tmp/hashline-edit-opencode/src/lib/hashline-prompt.ts:81-268`). **The prompt's runtime-computed example hashes must be regenerated by the binary** (hash examples hardcoded from the TS reimplementation will NOT match) — the port should either embed static example anchors computed once against the real binary, or spawn the binary to compute them at session start.
- Install config (`opencode.json`):
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
Use the **`plugin` array** form (tianhuil `opencode.json:2-13`); the paulp-o README's `"plugins": { ... }` object form is a README artifact and NOT ecosystem-standard.

### C.9 Testing — bun test with a stub binary

- `src/hashline-core.ts` accepts an injectable `spawn` (or the tests set `process.env.HASHLINE_BIN` to a fixture stub that reads args and prints canned stdout/exit codes).
- `src/tests/binary.test.ts`: resolution order (env → PATH → fail), spawn argv correctness, stdout/stderr capture, abort signal plumbing.
- `src/tests/edit.test.ts`: `buildPatchText` translation table (reuse the same fixtures as pi-hashline to keep behavior identical); JSON parsing of exit-0 output; exit-1 → formatted error.
- `src/tests/prompt.test.ts`: prompt renders, example anchors consistent with the binary's format (`N:hh`).
- Optional e2e test (`src/tests/e2e.test.ts`): spawn the real `target/release/hashline` on a temp file, `read --json` → `patch` → verify. Gate behind an env flag so CI without the binary skips.

---

## D. Shared concerns

### D.1 Binary discovery & version check

- Shared helper (`src/hashline.ts` / `src/hashline-core.ts`): `resolveHashlineBin(): string` from `HASHLINE_BIN` env → PATH; on Windows resolve `hashline.exe` (append `.exe` via PATHEXT-style probe). Spawn with `{ cwd: ctx.directory || process.cwd(), stdio: ["pipe","pipe","pipe"] }` **without a shell** to avoid quoting/globbing.
- **Version check:** on first tool call (or `session_start` for pi, plugin init for OpenCode), run `hashline --version`; parse `hashline (\d+)\.(\d+)\.(\d+)`. If `< MIN_HASHLINE_VERSION` (start at `0.9.1`), emit a host notification (`ctx.ui.notify(..., "warning")` for pi; a banner in tool output for OpenCode) telling the user to upgrade. Never hard-fail on a missing/call-failing version probe — degrade to a warning. Also probe `hashline --help` to confirm the `read`/`patch` subcommands exist (sanity check for a PATH collision with a different tool named `hashline`).
- `MIN_HASHLINE_VERSION = "0.9.1"` in both packages; bump on contract changes (A.3).

### D.2 Error taxonomy (structured tool errors)

Both hosts get a `kind`-tagged error so the agent can react:

| host error `kind` | detection | message content |
|---|---|---|
| `stale_anchor` | exit 1 + stderr contains "changed since last read" or `STALE_ANCHOR` | binary `Error:` text + `Re-read and retry with fresh anchors.` |
| `noop` / `empty_patch` | exit 1 + "produced no edits" / `EMPTY_PATCH` | binary text + `Patch was empty — nothing to do.` |
| `ambiguous_hash` | exit 1 + "ambiguous" / "multiple matches" / `AMBIGUOUS_HASH` | binary text + `Re-read; use the exact N:hh.` |
| `out_of_range` | exit 1 + "line N out of range" | binary text |
| `binary_not_found` | spawn ENOENT | install hint (`hashline --help` / add to PATH / set HASHLINE_BIN) |
| `io` / `binary_file` | exit 1 + other | binary text verbatim |

Implementation: `formatHashlineError(stderr: string, exitCode: number): { text: string; kind: ErrorKind }` in each package's `src/result.ts` / `src/hashline-errors.ts`. Model-facing guidance always lives in the returned `text`; `kind` (and details) are host-side.

### D.3 Windows vs POSIX

- Spawn without `shell: true`. Pass patch text via **stdin** (`-`) for anything with newlines; argv for short single ops. This sidesteps Windows argv quoting entirely.
- `.exe` resolution: on Windows, prefer the path from `HASHLINE_BIN` if set; otherwise try `hashline` then `hashline.exe` on PATH.
- Read stdout/stderr as buffers then decode UTF-8 (avoid text-mode transforms mangling `\r`/`\n`; the binary emits `\n` only).
- CRLF source files: the binary normalizes to LF for hashing and output (`document.rs:60-70`); wrappers must not re-add CRLF handling (paulp-o/tianhuil both had BOM/CRLF layers — the binary owns it).
- Daemon/env interplay: `main.rs:31-68` auto-routes non-serve/mcp commands via `HASHLINE_SOCKET` (Unix) or `HASHLINE_URL` (HTTP) if set, and fails hard with `HASHLINE_NO_FALLBACK`. Spawned invocations inherit this transparently; output shapes are preserved via the `structuredContent{stdout,stderr,exit_code}` envelope so parsing is unaffected, but process semantics change (daemon returns immediately). On Windows only `HASHLINE_URL` applies. Consider unsetting these in the wrapper's child env (or document the behavior) to keep per-call spawns deterministic.

### D.4 The Rust side — current gap analysis

| Capability the wrappers want | Exists in binary today? | Action |
|---|---|---|
| `read` anchored output | Yes (`read --json`, text `N:hh|content`) | consume |
| `patch` (all ops) | Yes | consume |
| `write` / `remove` / `rename` / `find-block` | Yes | consume (pi edit may map `delete`→`REM`/`DEL`) |
| `read --limit/--offset` | **No** (cli.rs:33-39) | wrapper slices in TS; optional future Rust flag |
| `grep` / `hashline ripgrep` | **No — not in scope** | grep is owned by the sibling `ffs` repo; hashline adds no grep subcommand and no ffs dependency. A wrapper `hashline_grep` (optional, deferred) spawns `ffs grep --json` and re-hashes via `hashline read --json`. See C.7. |
| MCP `find_block` JSON output | MCP `find_block` returns text only (no `json` arg in schema, mcp.rs:144-154) | CLI `find-block --json` has the structured payload; wrappers use the CLI, not MCP |
| Machine-readable stale-anchor structured error (e.g. `remaps`) | Exit 1 + text/JSON on stderr (`output.rs:60-87`); **no `remaps` map** | surface text; [VERIFY] optional future: emit `{"error","hint","remaps":{expected:actual}}` on stderr in `--json` mode |

**Conclusion: nothing new is required for v0.1.0.** The documented CLI contract (A.3-A.6) is the dependency; future Rust work (grep subcommand, read pagination flags, structured remaps) is tracked as optional enhancements.

### D.5 Anchor format consistency guard (critical)

Both wrappers MUST render and parse `N:hh|content` / `N:hh` only. Add a shared invariant test fixture: a golden `hashline read --json` snapshot (captured from the release binary) that all parse/format tests assert against, so no wrapper drifts into `LINE#HASH` (pi-ref style) or `N:HH` (tianhuil style).

Also note the **shape difference between `read --json` and `patch --json`**: `read` lines use key `"n"`; `patch --json` success lines use key `"line"` and include a phantom trailing empty line when the file ends with `\n` (`patch.rs:101-124` filter `!l.is_empty() || final_text.ends_with('\n')`). Separate parsers per command; never round-trip `patch --json` lines into `read --json` shape.

### D.6 Dialect convergence (pi `edit` only, optional)

Pi's native edit dialect uses `{path, edits:[{oldText,newText}]}`, `file_path` alias, and JSON-string `edits`. The reference normalizes these in `prepareArguments` (`edit-normalize.ts:167-212`, `edit.ts:534-541`). For v0.1.0 the override edit tool publishes ONLY the hashline schema (models are steered by `promptSnippet`/`promptGuidelines`); dialect folding is a fast-follow if models regress. **[VERIFY] Watch real usage.** If folding lands, remember `prepareArguments` output must be plain enumerable data (Pi may `structuredClone` prepared args and drop non-enumerable props, `edit-normalize.ts:17-19`).

### D.7 Docs

- **Root `README.md`:** add the two packages to the ecosystem table (the repo already references pi/opencode integrations in `hashline-ecosystem-research.md`), with install lines:
  - pi: `pi install npm:hashline-pi` (or `pi install git:github.com/<owner>/hashline#integration/pi-hashline` — [VERIFY] `pi install` supports git pathspecs; the pi-hledit README documents `pi install git:github.com/<owner>/<repo>`).
  - opencode: add `"@scope/hashline-opencode-plugin"` to `opencode.json` `plugin` array + `agent.build.tools.edit: false`.
- **Per-package README.md:** setup (binary on PATH or `HASHLINE_BIN`), the `~/.pi/agent/hashline.json` schema, the `opencode.json` snippet, example agent prompts showing `read` → `N:hh` → `patch`.
- **AGENTS.md:** add a short "Editor integrations" note pointing at `integration/`, and correct the anchor-format inconsistency (AGENTS.md shows `12:ab3f` in prose but `N:hh|content` in samples) to the single binary-native format.

---

## E. Task breakdown (implementation checklist)

Effort: **S** ≤ 2h, **M** ≤ 1 day, **L** ≤ 2 days.

### Phase 0 — Shared foundations

| # | Task | File(s) | Effort |
|---|---|---|---|
| E.1 | Add `.gitignore` exception for `integration/` (currently `*.md` at line 9 and `.tmp*` at line 13 would silently swallow everything under `integration/`). | `.gitignore` | S |
| E.2 | Create `integration/` scaffolding + capture golden fixtures: build release binary (`cargo build -p hashline --release`), run `read` (text + `--json`, CRLF file, trailing-newline vs no-trailing-newline), `patch` (SWAP/DEL/INS.*/BLK, `--dry-run`, stale-hash exit 1, stdin `-` envelope, `*** Abort`), `find-block --json`, `remove/rename --json`, `read` missing/binary file. Store snapshots under `integration/fixtures/`. | `integration/fixtures/*.json`, `*.txt` | M |
| E.3 | Define the shared "CLI contract" doc (this plan's A.3-A.6 condensed) in `integration/CONTRACT.md` (or `.txt` per E.1). | `integration/CONTRACT.md` | S |

### Phase 1 — `integration/pi-hashline`

| # | Task | File(s) | Effort |
|---|---|---|---|
| E.4 | `package.json` (B.1), `tsconfig.json`, `tsconfig.contract.json`, `biome.json`, `.gitignore` in package | `integration/pi-hashline/package.json` etc. | S |
| E.5 | `src/config.ts`: port `parseHashlineConfig`, `getAgentDir()` load, `HASHLINE_BIN`/`binary` field; unit tests | `src/config.ts`, `test/config.test.ts` | M |
| E.6 | `src/hashline.ts`: `resolveHashlineBin`, `runHashline(args, stdin?, ctx, signal)` (port of pi-hledit runHledit), `parseReadJson`, `formatReadPreview`, `formatHashlineError`; unit tests incl. golden fixtures | `src/hashline.ts`, `test/hashline.test.ts` | M |
| E.7 | `src/read.ts`: `registerReadTool` (B.6) | `src/read.ts` | M |
| E.8 | `src/edit-args.ts`: op→patch translation table + envelope + `replace_text` resolution; unit tests | `src/edit-args.ts`, `test/edit-args.test.ts` | M |
| E.9 | `src/edit.ts`: `registerEditTool` with `renderShell:"default"`, exit-1 → teaching error, chained-edits fresh-anchor re-read (B.7) | `src/edit.ts` | L |
| E.10 | `src/result.ts` + `src/render.ts` (AgentToolResult builders, optional TUI polish) | `src/result.ts`, `src/render.ts` | S |
| E.11 | `src/grep.ts` (deferred by default; stub gate on config `grep: true`) | `src/grep.ts` | M |
| E.12 | `index.ts` factory (B.4) + `prompts/read.md`, `prompts/edit.md` (binary-native anchors) + `README.md` | `index.ts`, `prompts/`, `README.md` | M |
| E.13 | Contract tests: stub `ExtensionAPI`/`ExtensionContext`/`Theme`/`Component`, assert registered definitions (name, schema, renderShell) | `test/contract.test.ts` | M |
| E.14 | Install smoke: `cargo build -p hashline --release`, `pi install <path>`, `/reload`, `/hashline-status`, read+edit a temp file | manual | M |

### Phase 2 — `integration/opencode-plugin`

| # | Task | File(s) | Effort |
|---|---|---|---|
| E.15 | `package.json` (C.2), `tsconfig.json`, `.npmrc`, `bun.lock` (pin `@opencode-ai/plugin` — [VERIFY] `^1.2.10` vs `^1.4.6` at typecheck time) | `integration/opencode-plugin/package.json` etc. | S |
| E.16 | `src/hashline-core.ts`: `resolveHashlineBin` + spawn helper with injectable spawn seam; tests | `src/hashline-core.ts`, `test/binary.test.ts` | M |
| E.17 | `src/format.ts`: `formatRead` (N:hh\|content, `[path#4hex]` header, slice by offset/limit); `src/hashline-apply.ts`: `buildPatchText` (reuse E.8 logic) | `src/format.ts`, `src/hashline-apply.ts` | M |
| E.18 | `src/hashline-errors.ts` + `hashline_read`/`hashline_edit` tools in `src/plugin.ts` (`context.metadata({title})`, exit-1 → `formatMismatch`); tests | `src/hashline-errors.ts`, `src/plugin.ts`, `test/edit.test.ts` | L |
| E.19 | `src/grep.ts` (deferred by default; `ffs` + binary re-hash path if implemented) | `src/grep.ts` | M |
| E.20 | `src/prompt.ts`: port `renderHashlineEditPrompt`; regenerate example anchors against the real binary | `src/prompt.ts`, `test/prompt.test.ts` | M |
| E.21 | `src/index.ts` entry + wire `"experimental.chat.system.transform"` hook | `src/index.ts`, `src/plugin.ts` | S |
| E.22 | `src/cli/hread.ts` + `src/cli/hedit.ts` (optional bin wrappers) | `src/cli/*.ts` | S |
| E.23 | `README.md` with `opencode.json` `plugin` array + `agent.build.tools.edit:false` snippet + `HASHLINE_BIN`; optional e2e test gated on real binary | `README.md`, `test/e2e.test.ts` | M |

### Phase 3 — Docs, validation, Definition of Done

| # | Task | File(s) | Effort |
|---|---|---|---|
| E.24 | Root `README.md` ecosystem table + install lines; `AGENTS.md` integration note + anchor-format cleanup | `README.md`, `AGENTS.md` | M |
| E.25 | Cross-package consistency pass: same `buildPatchText`, same fixtures, same error taxonomy; run both test suites | — | M |

**Definition of Done:**
1. `cargo build -p hashline --release` succeeds; no Rust source changes.
2. `integration/pi-hashline`: `npm run typecheck`, `npm run lint`, `npm run contract` all pass; manual `pi install` smoke proves `read`/`edit` override works and a stale-anchor edit produces the binary's teaching error.
3. `integration/opencode-plugin`: `bun run build:all` and `bun test src/tests/` pass; manual opencode.json `plugin` install proves `hashline_read`/`hashline_edit` work and native `edit` is disabled.
4. Both packages contain **zero** TS hashing/merge code — verified by grep for `xxhash`, `NIBBLE_STR`, `DICT`, `applyHashlineEdits`, `computeLineHash`, `Bun.hash` (see D.5 guard).
5. Golden fixtures (E.2) are committed and all parse/format tests consume them.
6. Docs (E.24) land; `.gitignore` exception (E.1) lets them actually be tracked.
7. Optional items (grep tool, dialect folding, `remaps`) are explicitly marked deferred in each README, not silently absent.

---

## F. Risks & open questions

### Risks

- **Host API version drift.**
  - pi: `@earendil-works/pi-coding-agent` peers `^0.79.9` vs npm latest 0.84.1 — the `ExtensionAPI` surface (registerTool/on/ExtensionContext) is stable per research, but a wrong `typebox` import path breaks typecheck immediately. Mitigation: pin peer floor, import from `"typebox"`, run `npm run typecheck` against the declared version, and bump as needed.
  - OpenCode: references tested against `@opencode-ai/plugin` 1.2.6/1.2.11; installed SDK 1.4.6. The `Plugin`/`tool`/`Hooks` contract is unchanged (verified against 1.4.6), but pin devDep to the built-against version and re-verify `tool.schema`/`ToolContext` shape. **Highest drift risk: `helper.*` — it does not exist in the SDK; using it would fail at runtime.**
- **Binary not on PATH.** Wrappers hard-require `hashline`. Mitigation: `HASHLINE_BIN` override, `~/.hashline/` probe, graceful install-hint errors, `/hashline-status` command (pi) to diagnose.
- **Anchor-format drift.** Three incompatible formats exist across the ecosystem (`LINE#HASH` nibble-alphabet, `N:HH` hex, binary `N:hh|content`). Mitigation: D.5 invariant tests on golden fixtures; every test asserts binary-native output.
- **Hash-format divergence within hex.** `read --json` uses `{"n","hash","content"}`; `patch --json` success uses `{"line","hash","content"}` and appends a phantom trailing empty line (`patch.rs:101-124`). Mitigation: separate parsers per command; never round-trip `patch --json` lines into `read --json` shape.
- **`pi install` of an npm package from this monorepo.** The repo is Rust-first; publishing a TS package needs a package workflow. Mitigation for v0.1.0: install from a local path (`pi install <abs path to integration/pi-hashline>`) or a git URL; publish to npm only when CI is added.
- **OpenCode plugin registry / naming.** `@scope/hashline-opencode-plugin` needs a real scope; plugin installs are auto via the `plugin` array. Mitigation: document the local-path variant (`.opencode/node_modules/.../dist/index.js`) like paulp-o, plus the array form.
- **No binary grep / pagination today.** `hashline_grep` and read offset/limit are wrapper-side or deferred. Mitigation: explicitly defer; document `find-block` as the scope primitive.
- **MCP vs CLI contract split.** The MCP server and the CLI can drift (e.g. `find_block` MCP tool has no `json` arg while CLI `find-block --json` does; MCP `read` does not filter the trailing empty line while CLI `read` does). Wrappers depend on the CLI only; pin against golden fixtures, not MCP docs.
- **`*** Abort` no-op returns exit 0** (not 1). The wrapper must not treat "no output, exit 0" as a silent bug; document it (the pi `NoopLoopGuard` in the reference is not ported — the binary's Abort is authoritative).
- **Daemon env routing.** `HASHLINE_SOCKET`/`HASHLINE_URL` in the wrapper's env can route spawns to a daemon. Mitigation: document; optionally scrub these vars in the child env (see D.3).
- **Payload escapes.** Content lines starting with `+` must be `++`; `-` → `+-`. A translator that forgets this silently corrupts edits. Mitigation: table-driven tests in both packages (E.8/E.17).
- **Stale anchor on blank/symbol-only lines.** Blank and `}`-style lines use line-number-seeded hashes (`hash.rs:64-71`) so their hashes change when position changes — the wrapper must never cache hashes across edits; always re-read before patching.

### Open questions

- **[VERIFY]** pi peer minimums: is `@earendil-works/pi-ai` required as a direct peer, or is `pi-coding-agent` sufficient (it re-exports the types)? pi-hashline-edit lists both; pi-hledit lists only coding-agent. Check `node_modules/@earendil-works/pi-coding-agent` exports at implementation time.
- **[VERIFY]** OpenCode devDep pin: `^1.2.10` (build against a version whose `.d.ts` you control) vs `^1.4.6` (matches the local install). Decide by running `typecheck` with both.
- **[VERIFY]** Should pi-hashline override built-in `read`/`edit` (name collision) or register a distinct tool? Plan commits to **override** (matches pi-hashline-edit, best model adherence). Fallback: single `hashline` tool + prompt steering (pi-hledit model) if override proves fragile in the live session.
- **[VERIFY]** Does the model need `replace_text`/`oldText` dialect folding in pi? Deferred in part (D.6); the `replace_text` op is implemented as a single-match→SWAP translation (B.7), and full native-dialect folding is added if real usage shows the model emitting it.
- **[VERIFY]** Stale-anchor structured recovery: the Rust binary does not emit `remaps`/suggested-position JSON today. Optional future: extend `output.rs` error JSON with `{"error","hint","remaps":{...}}`. No blocker for v0.1.0.
- **[VERIFY]** `pi install` git-pathspec support (`pi install git:github.com/<owner>/hashline#integration/pi-hashline`).
- **GitHub workflows:** `.github/workflows/` exists. Adding npm-package CI (`bun`/`pnpm` for the two packages) is a follow-up, not in scope here.

---

## Appendix — Source-of-truth file map (what the implementer reads first)

| Contract | File | Notes |
|---|---|---|
| CLI commands + flags | `crates/core/src/cli.rs` | read has only `--json`/`--no-cache`; NO offset/limit; Commands has NO grep |
| read output | `crates/core/src/commands/read.rs` | `N:hh\|content`, `--json` shape `{"n","hash","content"}`, no phantom trailing line |
| patch resolution + exit semantics | `crates/core/src/commands/patch.rs` | `-`/`@path`/literal; `*** Abort` exit 0; `--json` success `{"line","hash","content"}` + phantom line; dry-run `diff` |
| patch grammar | `crates/core/src/tokenizer.rs`, `parser.rs`, `patch_format.rs`, `messages.rs` | op keywords, `..`/`.=` range, payload escapes, `*** Begin/End Patch` |
| hash format | `crates/core/src/hash.rs` | xxh32(seed 0) low byte → 2 hex; xxh3 top 16 bits → 4 hex; `trim_end`; symbol-only line-seeded |
| document normalization | `crates/core/src/document.rs` | LF-normalize, BOM-strip, trailing_newline, NUL binary check |
| error taxonomy | `crates/core/src/error.rs` | `StaleAnchor`, `EmptyPatch`, `BinaryFile`, ...; exit-1 mapping |
| stderr JSON errors | `crates/core/src/output.rs` | `{"kind","error","hint","command"}` on stderr only |
| MCP tools (reference contract) | `crates/core/src/mcp.rs` | 6 tools + `hashline_` aliases; result `{content:[{type:"text",text}]}` |
| pi API types | `.tmp/pi-types` → `.../extensions/types.d.ts` | `ExtensionAPI` (registerTool/registerCommand/on), `ToolDefinition`, `ExtensionContext`, `ExtensionFactory` |
| pi thin-wrapper precedent | `.tmp/pi-hledit/index.ts` | runHledit spawn (:232-265), `resolveHleditBin` (:228-230), batch translation, `HLEDIT_INSTALL_HINT` (:14-22) |
| pi full-reimpl precedent | `.tmp/pi-hashline-edit/` | schemas (`edit.ts:56-170`), read render (`read.ts:131-252`), config, prompts, error codes, Anchors block (`edit-response.ts:62-136`) |
| OpenCode SDK | `~/.config/opencode/node_modules/@opencode-ai/plugin/dist/index.d.ts`, `tool.d.ts` | `Plugin`, `PluginInput`, `Hooks.tool`, `tool()`, `tool.schema` (=zod), `ToolContext`, no `helper.*` |
| OpenCode precedents | `.tmp/hashline-edit-opencode/src/index.ts`, `.tmp/opencode-hashlines/src/hashline-plugin.ts` | tool registrations, `context.metadata`, system-prompt hook, `tool.execute.after` |
| Installers (MCP hosts) | `install.sh:378,394,406-429`; `install.ps1:396,401-408` | opencode → `~/.opencode.json` `mcpServers` `{command, args:["mcp"]}` only if file exists; **no pi host** |
