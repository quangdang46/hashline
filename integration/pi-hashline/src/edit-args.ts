/**
 * Pure op→patch-string translation. Exported for contract tests.
 *
 * Translates the host edit dialect ({path, edits:[{op,pos,end,lines}]}) into
 * the hashline patch grammar. This is PURE translation — no hashing, no
 * staleness detection, no merge. All semantic validation happens in the binary.
 *
 * Patch grammar reference: integration/CONTRACT.md §2 (op keywords are
 * case-insensitive in the binary; ranges use `N..M`).
 */

export type HashlineEditOp =
  | "replace"
  | "append"
  | "prepend"
  | "delete"
  | "replace_text"
  | "replace_block"
  | "delete_block"
  | "insert_block_after";

export type HashlineEdit = {
  op: HashlineEditOp;
  /** Line number (1-based) for block ops; N:hh anchor otherwise. */
  /** N:hh anchor for line ops; 1-based line number (integer) for block ops. */
  pos?: string | number;
  end?: string;
  lines?: string[];
  oldText?: string;
  newText?: string;
};

export type HashlineEditRequest = {
  path: string;
  edits: HashlineEdit[];
};

export const BEGIN_PATCH = "*** Begin Patch";
export const END_PATCH = "*** End Patch";

const ANCHOR_RE = /^\d+:[0-9a-fA-F]{1,4}$/;

/** Validate an N:hh anchor (line number + 1-4 hex chars). */
export function isValidAnchor(value: string): boolean {
  return ANCHOR_RE.test(value);
}

/**
 * Escape a payload line into a hashline body row.
 *
 * Body rows are always `+` + payload. The leading `+` is the row marker; the
 * parser's escape tokens (`++` -> literal `+`, `+-` -> literal `-`,
 * patch_format.rs; AGENTS.md pitfall) mean a payload that itself starts with
 * `+` or `-` is written correctly by simply prefixing the marker — `+lead`
 * becomes `++lead`, `-lead` becomes `+-lead`, and a lone `-` payload would
 * never be emitted without the marker.
 */
export function escapePayloadLine(line: string): string {
  return `+${line}`;
}

/** Body rows for a list of payload lines. */
export function payloadRows(lines: string[]): string[] {
  return lines.map(escapePayloadLine);
}

/**
 * Translate one edit op into a patch snippet (may be multiple lines).
 * Returns null for a structurally invalid edit.
 */
export function translateEdit(edit: HashlineEdit): string[] | null {
  switch (edit.op) {
    case "replace": {
      const pos = edit.pos;
      if (typeof pos !== "string" || !isValidAnchor(pos)) {
        return null;
      }
      const lines = edit.lines ?? [];
      if (lines.length === 0) {
        // Replace-with-empty == delete.
        return edit.end ? [`DEL ${pos}..${edit.end}`] : [`DEL ${pos}`];
      }
      const header = edit.end ? `SWAP ${pos}..${edit.end}:` : `SWAP ${pos}:`;
      return [header, ...payloadRows(lines)];
    }
    case "delete": {
      const pos = edit.pos;
      if (typeof pos !== "string" || !isValidAnchor(pos)) {
        return null;
      }
      return edit.end ? [`DEL ${pos}..${edit.end}`] : [`DEL ${pos}`];
    }
    case "append": {
      const lines = edit.lines ?? [];
      const header = edit.pos ? `INS.POST ${edit.pos}:` : `INS.TAIL:`;
      return [header, ...payloadRows(lines)];
    }
    case "prepend": {
      const lines = edit.lines ?? [];
      const header = edit.pos ? `INS.PRE ${edit.pos}:` : `INS.HEAD:`;
      return [header, ...payloadRows(lines)];
    }
    case "replace_block": {
      // Block ops address a line number (1-based), not an N:hh anchor — the
      // binary locates the enclosing syntactic block via tree-sitter.
      const n = Number(edit.pos);
      if (!Number.isInteger(n) || n < 1 || edit.lines === undefined) {
        return null;
      }
      return [`SWAP.BLK ${n}:`, ...payloadRows(edit.lines)];
    }
    case "delete_block": {
      const n = Number(edit.pos);
      if (!Number.isInteger(n) || n < 1) {
        return null;
      }
      return [`DEL.BLK ${n}`];
    }
    case "insert_block_after": {
      const n = Number(edit.pos);
      if (!Number.isInteger(n) || n < 1 || edit.lines === undefined) {
        return null;
      }
      return [`INS.BLK.POST ${n}:`, ...payloadRows(edit.lines)];
    }
    case "replace_text": {
      // Never translatable without the file contents (needs a read to find
      // the unique matching line). Handled in edit.ts before this runs.
      return null;
    }
    default:
      return null;
  }
}

/**
 * Build the patch text for a multi-op envelope. Always wraps in
 * `*** Begin Patch` / `*** End Patch` and is piped via stdin
 * (`hashline patch <file> -`).
 *
 * Returns { ok: true, patch } on success, or { ok: false, error } describing
 * the first invalid edit.
 */
export function buildPatchText(
  request: HashlineEditRequest,
): { ok: true; patch: string } | { ok: false; error: string } {
  if (request.edits.length === 0) {
    return { ok: false, error: "edits must not be empty" };
  }

  const sections: string[] = [];
  for (let i = 0; i < request.edits.length; i++) {
    const edit = request.edits[i]!;
    const translated = translateEdit(edit);
    if (translated === null) {
      if (edit.op === "replace_text") {
        return {
          ok: false,
          error: `edit ${i}: op "replace_text" cannot be batched without a unique match; use a single replace with an N:hh anchor from read`,
        };
      }
      const detail =
        edit.op === "replace" || edit.op === "delete"
          ? ` (requires a valid N:hh "pos" anchor${edit.op === "replace" ? " or " : ""})`
          : "";
      return {
        ok: false,
        error: `edit ${i}: invalid edit for op "${edit.op}"${detail}`,
      };
    }
    sections.push(translated.join("\n"));
  }

  const patch = [BEGIN_PATCH, ...sections, END_PATCH].join("\n");
  return { ok: true, patch };
}

/**
 * Build a single-op argv patch (no envelope) for a one-line payload or short
 * single op. Returns the literal patch string or null when invalid.
 */
export function buildSingleOpPatch(edit: HashlineEdit): string | null {
  const translated = translateEdit(edit);
  return translated === null ? null : translated.join("\n");
}
