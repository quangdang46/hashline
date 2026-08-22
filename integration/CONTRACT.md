# hashline CLI Contract (for thin-wrapper integrations)

The single source of truth for hashes, staleness, and recovery is the **hashline Rust binary**
(`crates/core`). The `integration/pi-hashline` and `integration/opencode-plugin` packages are
pure subprocess glue — they spawn this binary and translate arguments/output. This document
pins the exact contract both wrappers depend on. Golden fixtures live in `integration/fixtures/`.

**Minimum binary version: `0.9.12`** (const `MIN_HASHLINE_VERSION` in both wrappers).

---

## 1. `read`

### Text mode (default)
```
[<path>#<4hex>]
N:hh|content
...
```
- `[path#HASH]` header: 4-hex file-content hash (top 16 bits of xxh3-64).
- Each line: `LINE:2hex|content`. `2hex` is the per-line short hash (xxh32 low byte,
  position-seeded for symbol-only lines — see `crates/core/src/hash.rs`).
- Lines are 1-based.

### `--json`
```json
{ "hash": "5db5", "path": "/abs/or/rel", "lines": [{"n":1,"hash":"9b","content":"..."}, ...] }
```
`lines` omit the trailing empty line of a newline-terminated file. `hash` is the file tag.

### Errors
- Missing file → exit 1, stderr `Error: I/O error: ...` / `Hint: ...`.
- Binary file → exit 1, `Error: file '...' appears to be binary ...`.
- Non-UTF-8 → exit 1, `InvalidUtf8`.

---

## 2. `patch`

### Invocation
```bash
hashline patch <file> <patch-string>
hashline patch <file> -                # patch string from stdin
hashline patch <file> '<patch>' --dry-run
hashline patch <file> '<patch>' --json
```

### Op syntax (anchor format `N:hh`)
| Op | Meaning |
|----|---------|
| `SWAP N:hh:` +body | Replace line N (hash-validated) with body |
| `SWAP N..M:hh:` +body | Replace range N..M with body |
| `DEL N:hh` | Delete line N |
| `DEL N..M` | Delete range |
| `INS.PRE N:hh:` / `INS.POST N:hh:` / `INS.HEAD:` / `INS.TAIL:` +body | Insert body |
| `SWAP.BLK N:` / `DEL.BLK N` / `INS.BLK.POST N:` | Block ops (language-aware) |
| `CUT N..M [@name]` / `PUT [@name] <N:` | Named-register move (0.9.1+) |
| `*** Begin Patch` / `*** End Patch` envelope | stdin multi-op (ignored if present) |

Body rows are `+TEXT`; `+` alone = blank; `++`→`+`, `+-`→`-` escapes.

### Exit codes (VERIFIED empirically against release binary >= 0.9.12)
### Default (compact) output — since 0.9.12 (BREAKING)
Success:
```text
OK <path>#<4hex> edits=<n> changed=<n>
~<line>:<hh>|content      # modified
+<line>:<hh>|content      # inserted
-<line>                   # deleted
```
`write`: `OK <path>#<hash> lines=<n>`. `remove`: `OK <path>`. `rename`: `OK <src>><dst>`.
`find-block`: `OK file=<p> lang=<l> lines=<n>` + block lines. Use `--verbose` for the old full-file dumps.
Errors (stderr, exit 1):
```text
ERR KIND key=val...
HINT <teaching hint>
```
`KIND` values match the `--json` kind list above (`STALE_ANCHOR file= line= expected= actual=`,
`EMPTY_PATCH reason=`, `HASH_NOT_FOUND hash= file=`, `AMBIGUOUS_HASH hash= count= lines= file=`,
`INVALID_ANCHOR anchor=`, `BINARY_FILE`, `IO`, ...). Legacy `Error:`/`Hint:` text is the
`--verbose` form; wrappers should accept both.
- `0` — success, no-op patch, or `*** Abort`. stdout = data; stderr = diagnostics.
- `1` — **any error**, including logical ones: stale anchor, empty patch, invalid anchor,
  ambiguous hash, infrastructure failure (I/O, invalid UTF-8, binary file).
  **Wrappers must treat exit 1 as "something failed" and classify stderr.** Accept both compact
  `ERR KIND ...`/`HINT ...` and verbose `Error:`/`Hint:` forms; do NOT assume exit 0 == recoverable.

### Stale anchor (the critical case — verbose form; compact is `ERR STALE_ANCHOR file= line= expected= actual=`)
```text
Error: line 2 content changed since last read in <path> (expected hash 5b, got 38)
Hint: re-read the file with `hashline read <file>`; ...
```

### `--json` errors
```json
{ "kind": "STALE_ANCHOR", "error": "line 2 content changed ...", "hint": "...", "command": "patch" }
```
`kind` values: `STALE_ANCHOR`, `STALE_FILE`, `NOOP_LOOP`, `EMPTY_PATCH`, `AMBIGUOUS_HASH`,
`HASH_NOT_FOUND`, `INVALID_ANCHOR`, `BLOCK_UNRESOLVED`, `BINARY_FILE`, `INVALID_UTF8`,
`FILE_NOT_FOUND`, `MISSING_SNAPSHOT_TAG`, `CANNOT_RECOVER`, `CLIPBOARD`, `IO`, `PATCH_FAILED`.

### `--dry-run`
Prints a unified-diff-like snippet to stdout (`@@ -- ++ @@` + `-old`/`+new` lines);
`--dry-run --json` emits `{success, file, dry_run, edits_applied, diff:[...]}`.

---

## 3. `write` / `remove` / `rename` / `find-block`

- `write <file> <content> [--force] [--json]` → JSON `{path, hash, lines:[...]}`.
- `remove <file> [--json]` → removes file.
- `rename <src> <dst> [--json]` → moves file.
- `find-block <file> <anchor> [--json]` → JSON `{file, line_count, language, block_lines:[{n,hash,content}]}`.

---

## 4. stdout/stderr contract (all commands)

- **stdout = data only** (file content, JSON, diff).
- **stderr = diagnostics** (warnings, `Error:`/`Hint:` on failure).
- `--json` errors also go to **stderr** as a single JSON object.

---

## 5. Anchor format (must match across read + patch)

`LINE:2hex` — 2 lowercase hex chars, xxh32 (low byte) of the line content
(`trim_end`), position-seeded (line number as xxh32 seed) **only for symbol-only lines**
(no alphanumerics). Content lines keep seed-0. See `crates/core/src/hash.rs`.

Wrappers must render and parse `N:hh|content` / `N:hh` **only** — never invent a different
anchor alphabet. A golden fixture (`integration/fixtures/read-json.json`) pins this.
