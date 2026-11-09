## hashline — Hash-Anchored File Editing (v0.9.22)

`hashline` is a file editing tool that uses content-hashed line anchors (`42:a3`) instead of fragile exact-text matching. Designed for agent-driven editing where concurrent changes are expected and edit safety is critical.

### Why It's Useful

- **Agent-first output:** Default output is compact, token-minimal, machine-readable — designed for AI agents, not humans
- **Stable anchors:** `line:hash` format survives nearby edits for content lines — line numbers shift but hashes stay valid
- **Concurrent-safe:** Detects stale anchors when content changed; fails explicitly instead of guessing
- **No merge conflicts:** Each edit is independent; no patch files needed
- **Works with any text:** Language-agnostic; no parsing required for line ops
- **xxh32 hashes:** 2-char (8-bit) short hashes — deterministic and fast, but collision-prone on large files; always keep the line number in the anchor

### The Anchor Format

Anchors are `line_number:content_hash` pairs like `3:a7`:

- **line_number**: 1-based line number (for human readability and disambiguation — the hash alone is only 8 bits)
- **content_hash**: 2-char lowercase hex, the low byte of xxh32 (seed 0) of the trimmed line content

Example output from `hashline read`:
```
src/main.rs#3a58
1:9b|fn main() {
2:89|    let name = "hashline";
3:a7|    let version = "0.9.0";
```

The `#3a58` file tag is a separate 4-hex-char xxh3-64 content hash of the whole file — it is a snapshot tag, **not** a line anchor.

**Hash stability caveat:** lines containing at least one letter or digit hash by content only, so their hash survives nearby edits. Symbol-only lines (`}`, `)`, `//`, blank) are *position-seeded* — their hash mixes in the line number, so it changes when lines are inserted/deleted above them. Prefer anchoring content lines when possible.

### Output Architecture

hashline uses **agent-first, token-minimal output** by default:

```
hashline CLI
├── DEFAULT  → Agent-native compact text (--verbose for human format)
├── --verbose → Human-readable format (full file after mutations)
└── --json   → Structured JSON output
```

**Default (compact) output examples:**

```bash
# read — full file with anchors (always full, already optimal)
hashline read src/main.rs
# src/main.rs#3a58
# 1:9b|fn main() {
# 2:89|    let name = "hashline";

# patch — status + changed lines only
hashline patch src/main.rs "SWAP 3:a7:
+    let version = \"0.9.1\";"
# OK src/main.rs#7f2a edits=1 changed=1
# ~3:b1|    let version = "0.9.1";

# write — status line only (no file re-read)
hashline write new.txt "hello"
# OK new.txt#a1b2 lines=1

# find-block — compact header + block lines
hashline find-block src/main.rs 2:89
# OK file=src/main.rs lang=Rust lines=5
# 1:9b|fn main() {
# 2:89|    let name = "hashline";
# 3:b1|    let version = "0.9.1";
# 4:3c|}
# 5:00|

# remove
hashline remove old.txt
# OK old.txt

# rename
hashline rename old.txt new.txt
# OK old.txt>new.txt
```

### Command Reference

| Command | Purpose |
|---------|---------|
| `hashline read <file>` | Show file with `[path#hash]` header + `line:hash|content` lines |
| `hashline read <file> --json` | Machine-readable JSON output |
| `hashline patch <file> <patch>` | Apply patch operations (see below) |
| `hashline find-block <file> <anchor>` | Find enclosing brace/indent block around anchor |
| `hashline write <file> <content>` | Create file (overwrite with `--force`) |
| `hashline remove <file>` | Delete a file |
| `hashline rename <old> <new>` | Rename/move a file (`--force` overwrites destination) |
| `hashline guide` | Interactive user guide (`--json` flag accepted) |
| `hashline serve` | Daemon mode over Unix socket or HTTP |
| `hashline mcp` | MCP server over stdio (6 tools) |
| `hashline update` | Self-update the binary from GitHub Releases (`--check` to only compare, `--json` for structured output, `--version <v>` for a specific release) |

### Patch Operations (hashline patch)

All editing goes through `hashline patch`. The `<patch>` argument is a string with operations in the format below. Use stdin (`hashline patch file - <<'EOF'`) for multi-op patches; `@/path/to/file` reads the patch from a file.

**Payload format:** the header line ends with `:` and payload lines follow on **subsequent lines**, each starting with `+` at column 1 (everything after `+` is literal content, including indentation). `+` alone inserts a blank line; `++x` inserts literal `+x`; `+-x` inserts literal `-x`. Bare `-` lines warn but are preserved.

**Inline payload (first line only):** the first payload line may also sit on the header line itself — `SWAP 3:a7:+    let version = "0.9.1";` is equivalent to putting `+    let version = "0.9.1";` on the next line. A space before the `+` is fine (`SWAP 3:a7: +x`). Works for `SWAP`, `SWAP.BLK`, and all `INS.*` ops, with or without a hash (`SWAP 3:+x`, `INS.TAIL:+x`). Inline text *without* a leading `+` is treated as a bare payload row (same auto-pipe warning as a bare next-line body) — it is never silently dropped. Ops with no payload (`DEL`, `DEL.BLK`, `CUT`, `PUT`, `REM`, `MV`) **reject** trailing junk on the header line. `SWAP N:` with no payload at all still deletes (documented below).

**Operations:**
| Op | Syntax | What it does |
|----|--------|-------------|
| **SWAP** | `SWAP N:hh:` | Replace line at anchor with payload |
| **SWAP** | `SWAP N..M:` or `SWAP N:hh..M:` | Replace inclusive range (hash, if given, validates only the start line) |
| **SWAP** | `SWAP N:` *(no payload)* | Empty-body SWAP **deletes** the target line(s) |
| **DEL** | `DEL N`, `DEL N:hh`, `DEL N..M` | Delete line/range (no payload, no trailing colon) |
| **INS.PRE** | `INS.PRE N:hh:` | Insert payload lines **before** anchor |
| **INS.POST** | `INS.POST N:hh:` | Insert payload lines **after** anchor |
| **INS** | `INS N:` | Shorthand for `INS.POST N:` |
| **INS.HEAD** | `INS.HEAD:` | Insert payload lines at top of file |
| **INS.TAIL** | `INS.TAIL:` | Append payload lines at end of file |
| **SWAP.BLK** | `SWAP.BLK N:` | Replace the block containing N (brace/indent/end heuristics — see below) |
| **DEL.BLK** | `DEL.BLK N` | Delete the block containing N (no payload) |
| **INS.BLK.PRE** | `INS.BLK.PRE N:` | Insert payload before the block at N |
| **INS.BLK.POST** | `INS.BLK.POST N:` | Insert payload after the block at N |
| **INS.BLK** | `INS.BLK N:` | Alias for `INS.BLK.POST N:` |
| **CUT** | `CUT N..M [@name]` | Delete lines N..M and capture them into a patch-local register |
| **PUT** | `PUT [@name] <N` or bare `PUT [@name]` | Paste a register's lines before original line N (bare PUT = file head). No payload. |

**Block ops are heuristics, not parsing.** `*.BLK` ops resolve the enclosing block by extension: brace-balanced `{}` for `.rs .js .ts .tsx .jsx .go .java .c .cpp .h .hpp .cs` (and unknown extensions, with indent fallback), indentation for `.py .verse` (brace pairs tried first, then indentation), `def/class/end` keywords for `.rb`. Results are best-effort — inspect with `find-block` first and prefer separate patches for interacting block edits.

**Section headers & envelopes:** a patch may contain `[path#hash]` section headers grouping ops per file (consecutive same-path sections merge; for a single-file `hashline patch` the bracketed path is not enforced). `*** Begin Patch` / `*** End Patch` envelope markers are accepted. `*** End of File` (apply_patch grammar) ends the current file's hunks — content after it is ignored with a warning until the next `[path]` section header. `*** Abort` discards the whole patch — the command then exits **1** with `ERR PATCH_REJECTED` (it looks like a failure; check the exit code, not just stdout). Two range syntaxes are accepted: `N..M` and `N.=M` (oh-my-pi compatible).

**File-level ops:** the parser recognizes `REM` (delete file) and `MV <dest>` (rename) inside a section, but `hashline patch` currently does **not** apply them — a patch containing only `REM`/`MV` fails with `ERR EMPTY_PATCH`. Use `hashline remove` / `hashline rename` instead.

**Example:**
```
SWAP 3:a7:
+    let version = "0.9.0";
+    println!("version: {}", version);
INS.POST 4:6c:
+    log::info!("done");
CUT 8..9 @tmp
PUT @tmp <12
```

### Typical Agent Workflow

1. **Read file with anchors:**
   ```bash
   hashline read src/main.rs
   ```

2. **Find scope (for block ops):**
   ```bash
   hashline find-block src/main.rs 12:2e
   ```

3. **Apply edit with fresh anchor:**
   ```bash
   hashline patch src/main.rs "SWAP 12:2e:
   +fn process_data(input: &str) -> String {
   +    let trimmed = input.trim().to_lowercase();
   +    format!(\"processed: {}\", trimmed)
   +}"
   ```

4. **Verify:**
   ```bash
   hashline read src/main.rs
   ```

5. **If stale anchor:** read file > get fresh anchors > retry. Never force, never bypass.

### Options

| Flag | Available on | Effect |
|------|-------------|--------|
| `--dry-run` | `patch` | Show what would change without modifying file |
| `--safe` | `patch`, `write` | Atomic temp-file + fsync (crash-safe, slower). **Default is a fast direct write — NOT atomic/crash-safe.** |
| `--emit-anchors` | `patch` | Append a fresh `[path#hash]` + `N:hh|content` listing after the summary so follow-up patches skip the re-read (also `HASHLINE_RETURN_ANCHORS=1`; in `--json` mode adds `updated_anchors`) |
| `--json` | `read`, `patch`, `find-block`, `write`, `remove`, `rename`, `guide`, `update` | Structured JSON output |
| `--verbose` | `patch`, `write`, `find-block`, `remove`, `rename` | Human-readable format (full file after mutations); conflicts with `--json` |
| `--no-cache` | `read` | Skip snapshot cache |
| `--pretty` | `find-block` | Formatted JSON output (with `--json`) |
| `--force` | `write`, `rename` | Overwrite existing destination file |
| `--detach` | `serve` | Fork to background as daemon |
| `--pid-file` | `serve` | Write PID to file |
| `--socket` | `serve` | Unix socket path (default ~/.hashline/daemon.sock) |
| `--http` | `serve` | HTTP port instead of Unix socket |
| `--proxy-to-daemon` | `mcp` | Proxy MCP requests to running daemon |
| `--check` | `update` | Only compare installed vs latest release; do not install |
| `--version <v>` | `update` | Install a specific release version instead of the latest |

**Environment variables** (boolean toggles take a truthy value — `1`, `true`, `yes`, `on`; unset, empty, `0`, or `false` mean off): `HASHLINE_RETURN_ANCHORS=1` (same as `--emit-anchors`), `HASHLINE_URL=http://host:17300` / `HASHLINE_SOCKET=~/.hashline/daemon.sock` (route CLI calls to a running daemon; `HASHLINE_URL` accepts `http(s)://host[:port][/path]` or bare `host[:port]`, default port 17300), `HASHLINE_NO_UPDATE_CHECK=1` (disable update notice), `HASHLINE_RELEASES_BASE_URL` (mirror/test override for release lookups), `HASHLINE_NO_FALLBACK=1` (do not fall back to local execution when the daemon is unreachable), `HASHLINE_LOG` / `HASHLINE_LOG_PATH` / `HASHLINE_LOG_MAX_BYTES` (tracing filter / log file path / rotation cap). State lives in `~/.hashline/` resolved via `USERPROFILE`→`HOME` on Windows, `HOME`→`USERPROFILE` elsewhere.

### Self-Update

- `hashline update` fetches the latest GitHub Release for the platform, verifies the `.sha256` checksum, and atomically replaces the running binary.
- **Update notice:** once per 24h (cached in `~/.hashline/update-check.json`), successful interactive commands print `NOTE update available: latest=<v> installed=<v> (run \`hashline update\`)` to stderr — only when stderr is a terminal, so piped/agent output stays byte-identical.
- **Disable the notice:** set `HASHLINE_NO_UPDATE_CHECK=1`. **Mirror/test override:** set `HASHLINE_RELEASES_BASE_URL` to redirect all release lookups.

### Cutting a Release

Releases are driven by `cargo-release` (`release.toml`) and shipped by the tag
push — there is **no crates.io publish** (the `hashline` crate there is an
unrelated stale 0.2.1; distribution is GitHub Releases).

1. **Write the CHANGELOG section by hand** (`CHANGELOG.md`, newest first),
   including the `Thanks: @user` credit line. cargo-release does not generate
   it — the credits cannot be derived from commits.
2. **Preview:** `cargo release patch` (dry run is the default).
3. **Release:** `cargo release patch --execute`.

Steps 1–3 in practice:

```bash
# 1. edit CHANGELOG.md, commit it
# 2. preview: shows "Upgrading hashline from X to Y" and the tag to push
cargo release patch
# 3. bump version + Cargo.lock, commit `chore(release): bump to X.Y.Z`,
#    create `vX.Y.Z`, push main + tag. The tag triggers release.yml,
#    which builds linux/macos/windows binaries and publishes the Release.
cargo release patch --execute
```

Useful variants: `cargo release 0.9.22 --execute` (explicit version),
`cargo release patch --execute --no-push` (stop before the tag is pushed),
add `--no-confirm` for non-interactive runs.

Do **not** hand-edit the version in `crates/core/Cargo.toml` and tag manually —
`Cargo.lock` drift has broken `--locked` release builds before (see v0.9.17).
`cargo release` updates the manifest and lockfile together.

### Error Recovery

**Compact format (default):**
```
ERR STALE file=sample.rs line=3 expected=a7 actual=e7
HINT re-read the file with `hashline read <file>`; if the hash moved, use the reported line(s) and retry with a fresh qualified anchor
```

**Verbose format (`--verbose`):**
```
Error: line 3 content changed since last read in sample.rs (expected hash a7, got e7)
Hint: re-read the file with `hashline read <file>`; ...
```

**Fix:** `hashline read <file>` → use the new hash → retry.

**Exit codes:** `0` = success, `1` = any failure. A patch that is rejected
(e.g. an unknown or malformed operation, an `*** Abort` marker, or a parse
error) is applied atomically — nothing is written and the command exits `1`
with `ERR PATCH_REJECTED` on stderr, so a rejected edit is never mistaken for
a successful one. Note this is true for *rejected* patches; successful writes
are direct (non-atomic) unless `--safe` is passed.

### Common Pitfalls

- **Stale anchor:** Content changed since last read → re-run `hashline read`
- **File doesn't exist:** `hashline patch` fails with I/O error
- **Binary file:** hashline only works on UTF-8 text files
- **Inline payload needs the `+` sigil for literal text:** `SWAP 3:a7:+new` inserts `new`; `SWAP 3:a7:new` (no `+`) inserts `new` as a *bare* payload row with a warning. Only a truly empty body (`SWAP 3:`) deletes.
- **`-` rows generate a warning and are still preserved as content:** hashline patch uses `+` payload syntax; bare `-` items (e.g. Markdown list items) produce a warning but are preserved. Use the `+-` escape to suppress the warning: `+- list item` → `- list item`
- **Leading sigil is consumed:** The `+` payload prefix is stripped; if you need a literal leading `+`, double it: `++plus` → `+plus`. Same for bare body (no `+`) lines — they're treated as body content but may trigger a warning about missing `+` prefix
- **Boundary echo:** warning when payload first line duplicates the line above the target range
- **Hash collisions:** line hashes are only 8 bits — `hash 'ab' matches N lines` means the bare hash is ambiguous; always use the line-qualified `N:hh` form
- **Symbol-only anchors are position-dependent:** `}`, `)`, blank lines get position-seeded hashes that change when surrounding lines shift

### Patch Output (Compact)

```bash
hashline patch src/main.rs "SWAP 3:a7:
+    let version = \"0.9.1\";
INS.TAIL:
+    println!(\"done\");"
# OK src/main.rs#7f2a edits=2 changed=2
# ~3:b1|    let version = "0.9.1";
# +6:5e|    println!("done");
```

Prefix convention:
- `~` = modified line (content changed)
- `+` = inserted line (new)
- `-N` = deleted line (line N removed)

### JSON Output

`read`, `patch`, `find-block`, `write`, `remove`, `rename`, `guide`, and `update` all support `--json`:

```bash
hashline read sample.rs --json
# {"hash":"3a58","lines":[{"content":"fn main() {","hash":"9b","n":1},...],"path":"sample.rs"}

hashline patch sample.rs "SWAP 3:a7:
+replaced" --json
# {"success":true,"file":"sample.rs","hash":"7f2a","edits_applied":1,"changed":[{"type":"modified","line":3,"hash":"b1","content":"replaced"}]}
```

### MCP Server

`hashline mcp` runs a stdio JSON-RPC MCP server exposing exactly 6 tools: `read`, `patch`, `write`, `find_block`, `remove_file`, `rename_file`. The `initialize` handshake returns `instructions` with the authoritative usage rules (read→patch→verify, `return_updated_anchors` for chaining). `hashline mcp --proxy-to-daemon` forwards requests to a running `hashline serve` daemon. There are no other MCP tools — do not call `ensure_project`, `send_message`, or similar; they do not exist here.

### Rules for Agents (MANDATORY)

- **Always `hashline read <file>` before `hashline patch <file>`** — fresh anchors every time
- **Stale-anchor errors are safety signals, never bypass them**
- **hashline is NOT str_replace, NOT sed.** For fuzzy matching, use the `patch` tool
- **For multi-op patches, ALWAYS use stdin** (`hashline patch file - <<'EOF'`)
- **Payload may be inline or on following lines:** `SWAP 3:a7:+new` works, but multi-line payloads still need one `+` row per line after the header


## Beads Workflow Integration

This project uses [beads_rust](https://github.com/Dicklesworthstone/beads_rust) (`br`) for issue tracking. Issues are stored in `.beads/` and tracked in git.

**Important:** `br` is non-invasive—it NEVER executes git commands. After `br sync --flush-only`, you must manually run `git add .beads/ && git commit`.

### Essential Commands

```bash
# View issues (launches TUI - avoid in automated sessions)
bv

# CLI commands for agents (use these instead)
br ready              # Show issues ready to work (no blockers)
br list --status=open # All open issues
br show <id>          # Full issue details with dependencies
br create --title="..." --type=task --priority=2
br update <id> --status=in_progress
br close <id> --reason "Completed"
br close <id1> <id2>  # Close multiple issues at once
br sync --flush-only  # Export to JSONL (NO git operations)
```

### Workflow Pattern

1. **Start**: Run `br ready` to find actionable work
2. **Claim**: Use `br update <id> --status=in_progress`
3. **Work**: Implement the task
4. **Complete**: Use `br close <id>`
5. **Sync**: Run `br sync --flush-only` then manually commit

### Key Concepts

- **Dependencies**: Issues can block other issues. `br ready` shows only unblocked work.
- **Priority**: P0=critical, P1=high, P2=medium, P3=low, P4=backlog (use numbers, not words)
- **Types**: task, bug, feature, epic, question, docs
- **Blocking**: `br dep add <issue> <depends-on>` to add dependencies

### Session Protocol

**Before ending any session, run this checklist:**

```bash
git status              # Check what changed
git add <files>         # Stage code changes
br sync --flush-only    # Export beads to JSONL
git add .beads/         # Stage beads changes
git commit -m "..."     # Commit everything together
git push                # Push to remote
```

### Best Practices

- Check `br ready` at session start to find available work
- Update status as you work (in_progress → closed)
- Create new issues with `br create` when you discover tasks
- Use descriptive titles and set appropriate priority/type
- Always `br sync --flush-only && git add .beads/` before ending session

<!-- end-bv-agent-instructions -->

---

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Sync beads** - `br sync --flush-only` to export to JSONL
5. **Hand off** - Provide context for next session

### Commit Discipline

- **Always run checks before committing**: For any code change, run tests, linters, builds,,format and `ubs $(git diff --name-only --cached)` on staged files before creating a commit.
- **Only commit when checks pass**: Do not commit if tests, linters, builds, or UBS are failing, unless you are explicitly committing a known-broken state with a clear reason in the commit message and associated issue.
- **Treat every change as commit-ready**: Work as if any local change could be committed; keep changes small, coherent, and fully validated before `git commit`.

---

Note for Codex/GPT-5.2:

You constantly bother me and stop working with concerned questions that look similar to this:

```
Unexpected changes (need guidance)

- Working tree still shows edits I did not make in Cargo.toml, Cargo.lock, src/cli/commands/upgrade.rs, src/storage/sqlite.rs, tests/conformance.rs, tests/storage_deps.rs. Please advise whether to keep/commit/revert these before any further work. I did not touch them.

Next steps (pick one)

1. Decide how to handle the unrelated modified files above so we can resume cleanly.
2. Triage beads_rust-orko (clippy/cargo warnings) and beads_rust-ydqr (rustfmt failures).
3. If you want a full suite run later, fix conformance/clippy blockers and re‑run cargo test --all.
```

NEVER EVER DO THAT AGAIN. The answer is literally ALWAYS the same: those are changes created by the potentially dozen of other agents working on the project at the same time. This is not only a common occurence, it happens multiple times PER MINUTE. The way to deal with it is simple: you NEVER, under ANY CIRCUMSTANCE, stash, revert, overwrite, or otherwise disturb in ANY way the work of other agents. Just treat those changes identically to changes that you yourself made. Just fool yourself into thinking YOU made the changes and simply don't recall it for some reason.

## Legacy `bd` Workflow (Deprecated)

Historical docs may still mention `beads_viewer`/`bd` commands. For this repository, that workflow is deprecated.

Canonical issue workflow is:
- `br` for task state and dependency management
- `br sync --flush-only` for JSONL export (no git automation)
- `bv --robot-*` for triage/planning (never bare `bv`)

Do not run `bd`/`bd sync` for normal work. Only use legacy command names when reading old artifacts or translating historical instructions.

Quick translation from legacy docs:

| Legacy | Canonical |
|--------|-----------|
| `bd ready` | `br ready` |
| `bd list --status=open` | `br list --status open` |
| `bd show <id>` | `br show <id>` |
| `bd update <id> --status=in_progress` | `br update <id> --status in_progress` |
| `bd close <id>` | `br close <id> --reason "Completed"` |
| `bd sync` | `br sync --flush-only` + manual git add/commit/push |

---

## UBS Quick Reference for AI Agents

UBS stands for "Ultimate Bug Scanner": **The AI Coding Agent's Secret Weapon: Flagging Likely Bugs for Fixing Early On**

**Install:** `curl -sSL https://raw.githubusercontent.com/Dicklesworthstone/ultimate_bug_scanner/master/install.sh | bash`

**Golden Rule:** `ubs <changed-files>` before every commit. Exit 0 = safe. Exit >0 = fix & re-run.

**Commands:**
```bash
ubs file.ts file2.py                    # Specific files (< 1s) — USE THIS
ubs $(git diff --name-only --cached)    # Staged files — before commit
ubs --only=js,python src/               # Language filter (3-5x faster)
ubs --ci --fail-on-warning .            # CI mode — before PR
ubs --help                              # Full command reference
ubs sessions --entries 1                # Tail the latest install session log
ubs .                                   # Whole project (ignores things like .venv and node_modules automatically)
```

**Output Format:**
```
⚠️  Category (N errors)
    file.ts:42:5 – Issue description
    💡 Suggested fix
Exit code: 1
```
Parse: `file:line:col` → location | 💡 → how to fix | Exit 0/1 → pass/fail

**Fix Workflow:**
1. Read finding → category + fix suggestion
2. Navigate `file:line:col` → view context
3. Verify real issue (not false positive)
4. Fix root cause (not symptom)
5. Re-run `ubs <file>` → exit 0
6. Commit

**Speed Critical:** Scope to changed files. `ubs src/file.ts` (< 1s) vs `ubs .` (30s). Never full scan for small edits.

**Bug Severity:**
- **Critical** (always fix): Null safety, XSS/injection, async/await, memory leaks
- **Important** (production): Type narrowing, division-by-zero, resource leaks
- **Contextual** (judgment): TODO/FIXME, console logs

**Anti-Patterns:**
- ❌ Ignore findings → ✅ Investigate each
- ❌ Full scan per edit → ✅ Scope to file
- ❌ Fix symptom (`if (x) { x.y }`) → ✅ Root cause (`x?.y`)

---

## cass — Cross-Agent Session Search

`cass` indexes prior agent conversations (Claude Code, Codex, Cursor, Gemini, ChatGPT, etc.) so we can reuse solved problems.

**Rules:** Never run bare `cass` (TUI). Always use `--robot` or `--json`.

### Examples

```bash
cass health
cass search "async runtime" --robot --limit 5
cass view /path/to/session.jsonl -n 42 --json
cass expand /path/to/session.jsonl -n 42 -C 3 --json
cass capabilities --json
cass robot-docs guide
```

### Tips

- Use `--fields minimal` for lean output
- Filter by agent with `--agent`
- Use `--days N` to limit to recent history

stdout is data-only, stderr is diagnostics; exit code 0 means success.

Treat cass as a way to avoid re-solving problems other agents already handled.

---

Note for Codex/GPT-5.2:

You constantly bother me and stop working with concerned questions that look similar to this:

```
Unexpected changes (need guidance)

- Working tree still shows edits I did not make in Cargo.toml, Cargo.lock, src/cli/commands/upgrade.rs, src/storage/sqlite.rs, tests/conformance.rs, tests/storage_deps.rs. Please advise whether to keep/commit/revert these before any further work. I did not touch them.

Next steps (pick one)

1. Decide how to handle the unrelated modified files above so we can resume cleanly.
2. Triage beads_rust-orko (clippy/cargo warnings) and beads_rust-ydqr (rustfmt failures).
3. If you want a full suite run later, fix conformance/clippy blockers and re-run cargo test --all.
```

NEVER EVER DO THAT AGAIN. The answer is literally ALWAYS the same: those are changes created by the potentially dozen of other agents working on the project at the same time. This is not only a common occurrence, it happens multiple times PER MINUTE. The way to deal with it is simple: you NEVER, under ANY CIRCUMSTANCE, stash, revert, overwrite, or otherwise disturb in ANY way the work of other agents. Just treat those changes identically to changes that you yourself made. Just fool yourself into thinking YOU made the changes and simply don't recall it for some reason.

---



## Note on Built-in TODO Functionality

Also, if I ask you to explicitly use your built-in TODO functionality, don't complain about this and say you need to use beads. You can use built-in TODOs if I tell you specifically to do so. Always comply with such orders.