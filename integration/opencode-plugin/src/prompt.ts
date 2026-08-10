/**
 * prompt.ts — system-prompt injection teaching the hashline workflow.
 *
 * Example anchors are EMBEDDED STATIC values computed once against the real
 * release binary (hashline 0.9.1) — see `src/tests/prompt.test.ts`, which
 * regenerates them against the binary (when present) and asserts consistency.
 * They MUST NOT be hand-computed: the hashing algorithm lives in the binary.
 *
 * The advertised format is the binary-native `N:hh|content` (read) and
 * `N:hh` (anchor) — NOT the pi ecosystem's `LINE#HASH` / nibble alphabet.
 */

// Example file shown in the prompt (src/Counter.tsx) with its real hashes.
// Regenerated against hashline 0.9.1 (read --json on the 15-line example).
export const EXAMPLE_LINES: string[] = [
  "import { useState } from 'react';", // 1
  "", // 2
  "export function Counter() {", // 3
  "  const [count, setCount] = useState(0);", // 4
  "  const timeout = 5000;", // 5
  "", // 6
  "  return (", // 7
  "    <div>", // 8
  "      <p>Count: {count}</p>", // 9
  "      <button onClick={() => setCount(c => c + 1)}>", // 10
  "        Increment", // 11
  "      </button>", // 12
  "    </div>", // 13
  "  );", // 14
  "}", // 15
];

/** Binary-computed 2-hex content hash per line (1-indexed). */
export const EXAMPLE_HASHES: string[] = [
  "d5", // 1
  "92", // 2
  "85", // 3
  "28", // 4
  "da", // 5
  "7e", // 6
  "21", // 7
  "b4", // 8
  "39", // 9
  "f6", // 10
  "b3", // 11
  "f2", // 12
  "6f", // 13
  "4e", // 14
  "75", // 15
];

/** Render `N:hh|content` for display. */
export function formatExampleLine(n: number): string {
  return `${n}:${EXAMPLE_HASHES[n - 1]}|${EXAMPLE_LINES[n - 1]}`;
}

/** Render a quoted `"N:hh"` anchor. */
export function anchorRef(n: number): string {
  return `"${n}:${EXAMPLE_HASHES[n - 1]}"`;
}

/** Render the full example file view with binary-native anchors. */
export function renderExampleView(): string {
  return EXAMPLE_LINES.map((_, i) => formatExampleLine(i + 1)).join("\n");
}

/** The full hashline system-prompt block pushed via `experimental.chat.system.transform`. */
export function renderHashlineEditPrompt(): string {
  const e5 = anchorRef(5);
  const e4 = anchorRef(4);
  const e3 = anchorRef(3);
  const e8 = anchorRef(8);
  const e12 = anchorRef(12);
  const e9 = anchorRef(9);
  const e11 = anchorRef(11);

  return `<hashline_edit>
You use \`hashline_read\` to view files and \`hashline_edit\` to modify them. Every line is tagged \`N:hh|content\` (N = 1-based line number, hh = 2-char content hash computed by the hashline binary). You reference lines by their \`N:hh\` anchor to make precise, collision-safe edits.

<workflow>
1. **Read first.** Always call \`hashline_read\` to obtain \`N:hh\` anchors before editing a file.
2. **Batch edits.** Collect all edits to a single file into one \`hashline_edit\` call.
3. **Re-read after edits.** Before making subsequent edits to the same file, call \`hashline_read\` again to get fresh anchors — anchors become stale the moment the file changes.
</workflow>

<format>
Each line from \`hashline_read\` looks like:
\`\`\`
${formatExampleLine(1)}
${formatExampleLine(2)}
${formatExampleLine(3)}
${formatExampleLine(4)}
${formatExampleLine(5)}
\`\`\`
The anchor ${e5} uniquely identifies line 5 by its content hash. Pass it to \`hashline_edit\` as \`pos\`/\`end\`.
</format>

<operations>
Each edit in the \`edits\` array has:
- **op**: \`"replace"\` | \`"append"\` | \`"prepend"\` | \`"delete"\`
- **pos**: \`"N:hh"\` — anchor of the line to anchor on
- **end**: \`"N:hh"\` — inclusive end anchor for range operations (optional)
- **lines**: new content lines (optional; omit/empty for delete)

**\`op: "replace"\`** — Replace one line or an inclusive range.
  - Single line: \`{ op: "replace", pos: ${e5}, lines: ["  const timeout = 3000;"] }\`
  - Range: \`{ op: "replace", pos: ${e4}, end: ${e5}, lines: ["..."] }\`

**\`op: "append"\`** — Insert lines after the anchor line.
  - \`{ op: "append", pos: ${e5}, lines: ["  const delay = 1000;"] }\`
  - Without \`pos\`: appends at end of file (EOF).

**\`op: "prepend"\`** — Insert lines before the anchor line.
  - \`{ op: "prepend", pos: ${e3}, lines: ["// Counter component"] }\`
  - Without \`pos\`: prepends at beginning of file (BOF).

**\`op: "delete"\`** — Delete a line or inclusive range by anchor.
  - \`{ op: "delete", pos: ${e5} }\`
  - \`{ op: "delete", pos: ${e8}, end: ${e12} }\`
</operations>

<rules>
- **Anchors are content hashes** — they are stable only while the line is unchanged. Never reuse anchors from an earlier read after any edit.
- **Minimize edit scope** — only include lines that actually change.
- **Anchor on structural boundaries** — prefer function signatures, imports, and declarations as anchors over generic code.
- **The hashline binary validates atomically** — all anchors in one \`hashline_edit\` call are checked before any write; a stale anchor rejects the whole batch.
</rules>

<recovery>
- **Anchor mismatch**: If \`hashline_edit\` returns a hash-mismatch error, the file changed since your last read. Call \`hashline_read\`, take fresh anchors, and retry.
- **No-op**: If an edit reports nothing applied, it was already in the desired state — do not loop.
</recovery>

## Examples

Given this file (\`src/Counter.tsx\`):
\`\`\`
${renderExampleView()}
\`\`\`

### 1. Single-line replace
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{ "op": "replace", "pos": ${e5}, "lines": ["  const timeout = 3000;"] }]
}
\`\`\`

### 2. Single-line delete
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{ "op": "delete", "pos": ${e5} }]
}
\`\`\`

### 3. Range delete
Delete lines 8–12 (the \`<div>\` through \`</button>\`):
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{ "op": "delete", "pos": ${e8}, "end": ${e12} }]
}
\`\`\`

### 4. Prepend a comment
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{ "op": "prepend", "pos": ${e3}, "lines": ["/** A simple counter component. */"] }]
}
\`\`\`

### 5. Append a line after line 9
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{ "op": "append", "pos": ${e9}, "lines": ["      <p>Current count: {count}</p>"] }]
}
\`\`\`

### 6. Replace a range with new JSX
\`\`\`json
{
  "path": "src/Counter.tsx",
  "edits": [{
    "op": "replace",
    "pos": ${e9},
    "end": ${e11},
    "lines": [
      "      <p>Current count: {count}</p>",
      "      <button onClick={() => setCount(c => c + 1)}>+1</button>",
      "      <button onClick={() => setCount(0)}>Reset</button>"
    ]
  }]
}
\`\`\`

<critical>
- **Always** call \`hashline_read\` before editing — never guess or invent hashes.
- **Batch** all edits to one file into a single \`hashline_edit\` call.
- **Re-read** with \`hashline_read\` before subsequent edits to the same file.
- **Never use anchors with invented hashes** — the binary rejects them with a mismatch error.
</critical>
</hashline_edit>`;
}
