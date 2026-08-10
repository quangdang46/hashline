/**
 * hashline-apply.ts — op-model → patch-string translator.
 *
 * PURE TRANSLATION ONLY. This module maps the host edit dialect
 * ({op, pos, end, lines}) onto the hashline patch grammar. It does NO hashing,
 * NO staleness validation, and NO merge recovery — all of that lives in the
 * Rust binary and is authoritative. The generated patch text is handed to
 * `hashline patch <file> -` (stdin envelope form).
 *
 * Op → patch translation table (binary-native, verified against v0.9.1):
 *
 *   replace {pos:"N:hh", lines}        → SWAP N:hh:\n+ <lines...>
 *   replace {pos:"N:hh", end:"M:aa"}   → SWAP N:hh..M:\n+ <lines...>
 *   append  {pos:"N:hh", lines}        → INS.POST N:hh:\n+ <lines...>
 *   append  {} (no pos)                → INS.TAIL:\n+ <lines...>
 *   prepend {pos:"N:hh", lines}        → INS.PRE N:hh:\n+ <lines...>
 *   prepend {} (no pos)                → INS.HEAD:\n+ <lines...>
 *   replace {} with empty lines / delete {pos}        → DEL N:hh
 *   delete {pos, end:"M"}              → DEL N:hh..M
 *
 * Range note: the binary validates the hash only when it directly follows the
 * FIRST line number (`SWAP N:hh..M:` / `DEL N:hh..M`). A space-separated
 * `N:hh M:aa` form is NOT accepted. The end-of-range hash is a line-number
 * range hint, not a content hash; wrappers emit `N:hh..M` and let the binary
 * validate the start anchor.
 *
 * Payload escapes (patch_format.rs): a row `+TEXT` is a literal line; `++TEXT`
 * decodes to `+TEXT`; `+-TEXT` decodes to `-TEXT`; a lone `+` is a blank line.
 * `lines: []` on a replace is emitted as DEL.
 */

/** Edit operation dialect understood by the plugin tools. */
export interface EditOperation {
  op: "replace" | "append" | "prepend" | "delete";
  /** Anchor `N:hh` (from hashline_read output). Required for replace/delete, optional for append/prepend. */
  pos?: string;
  /** Inclusive end anchor `N:hh` for range operations. */
  end?: string;
  /** New content lines. Omit/empty for delete. */
  lines?: string[];
}

/** Escape a payload row per the patch grammar. */
export function escapePayloadLine(line: string): string {
  if (line.length === 0) return "+";
  if (line.startsWith("+")) return `++${line.slice(1)}`;
  if (line.startsWith("-")) return `+-${line}`;
  return `+${line}`;
}

/** Render the payload rows for a body edit (`SWAP`/`INS.*`). */
export function renderBody(anchor: string, lines: string[]): string {
  const rows = lines.map(escapePayloadLine).join("\n");
  return `${anchor}:\n${rows}`;
}

/** Translate a single edit op into one or more patch grammar lines. */
export function translateEdit(edit: EditOperation): string[] {
  const { op, pos, end, lines } = edit;
  const payload = lines ?? [];

  switch (op) {
    case "replace": {
      if (payload.length === 0) {
        // Delete semantics: `replace` with no payload lines → DEL
        const anchor = end ? `${pos}..${end}` : pos!;
        return [`DEL ${anchor}`];
      }
      const range = end ? `${pos}..${end}` : pos!;
      return [renderBody(`SWAP ${range}`, payload)];
    }
    case "append": {
      if (payload.length === 0) return [];
      const anchor = pos ? `INS.POST ${pos}` : "INS.TAIL";
      return [renderBody(anchor, payload)];
    }
    case "prepend": {
      if (payload.length === 0) return [];
      const anchor = pos ? `INS.PRE ${pos}` : "INS.HEAD";
      return [renderBody(anchor, payload)];
    }
    case "delete": {
      const anchor = end ? `${pos}..${end}` : pos!;
      return [`DEL ${anchor}`];
    }
  }
}

/**
 * Build the full patch string for a batch of edits. Multi-op batches are
 * wrapped in the `*** Begin Patch` / `*** End Patch` envelope and piped via
 * stdin (`hashline patch <file> -`).
 */
export function buildPatchText(edits: EditOperation[]): string {
  const ops = edits.flatMap((e) => translateEdit(e));
  if (ops.length === 0) return "";

  const body = ops.join("\n");
  if (ops.length === 1) {
    // Single-op patches may use argv; still safe to send via the envelope.
    return `${body}\n`;
  }
  return `*** Begin Patch\n${body}\n*** End Patch\n`;
}

/** Build a short UI title summarizing the edit batch (for context.metadata). */
export function buildEditTitle(args: { path: string; edits?: EditOperation[] }): string {
  const parts: string[] = [args.path];
  const ops: string[] = [];
  for (const e of args.edits ?? []) {
    if (e.op === "replace") {
      ops.push(e.lines && e.lines.length > 0 ? (e.end ? `repl ${e.pos}..${e.end}` : `repl ${e.pos}`) : (e.end ? `del ${e.pos}..${e.end}` : `del ${e.pos}`));
    } else if (e.op === "append") {
      ops.push(e.pos ? `app ${e.pos}` : "app EOF");
    } else if (e.op === "prepend") {
      ops.push(e.pos ? `prep ${e.pos}` : "prep BOF");
    } else if (e.op === "delete") {
      ops.push(e.end ? `del ${e.pos}..${e.end}` : `del ${e.pos}`);
    }
  }
  if (ops.length > 0) parts.push(ops.join(", "));
  return parts.join(" — ");
}
