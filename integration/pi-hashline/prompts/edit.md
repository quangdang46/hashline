Patch a text file at `N:hh` anchors copied verbatim from the latest read output or the `--- Anchors ---` block of a previous edit.

Batch every change to a file into one `edit` call: all operations go in the `edits` array, every edit sets `op`, and all anchors must come from the same pre-edit read. The binary validates all anchors atomically before writing, so line numbers never shift between entries of the same call.

Ops:
- `replace` — replace the single line at `pos`, or the inclusive span `pos`..`end`. `lines` is the complete new content for the whole span; `lines: []` deletes it. Without `end`, exactly one line is replaced no matter how many entries `lines` has.
- `append` — insert `lines` after `pos`; omit `pos` to append at end of file.
- `prepend` — insert `lines` before `pos`; omit `pos` to insert at start of file.
- `delete` — delete the line at `pos`, or the inclusive span `pos`..`end`.
- `replace_text` — `{ "op": "replace_text", "oldText": ..., "newText": ... }` replaces one exact, unique occurrence and fails otherwise. Prefer anchors; use this only when uniqueness is certain. `oldText`/`newText` are invalid on any other op.

Example — single-line and span replace in one call:
```json
{ "path": "src/main.ts", "edits": [
  { "op": "replace", "pos": "12:ab", "lines": ["const x = 1;"] },
  { "op": "replace", "pos": "5:cd", "end": "8:ef", "lines": [
    "function greet(name) {",
    "  return `Hello, ${name}`;",
    "}"
  ] }
] }
```

Rules:
- `lines` is literal file content with exact indentation. Never include `N:hh|` prefixes, diff `+`/`-` markers, or a copy of a neighboring line — the `|content` part of an anchor is context for you, not payload, and repeating a boundary line duplicates it in the file.
- Anchors are opaque: copy them exactly, never compute, shift, or guess one.
- Edits in one call must not overlap or touch adjacent lines — merge such changes into a single edit.
- On a stale-anchor error, re-read the file to get current anchors before retrying. The changed-line rows and the --- Anchors --- block after a successful edit replace a re-read for nearby follow-up edits.
