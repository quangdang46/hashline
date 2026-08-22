/**
 * hashline-errors.ts — exit-code/stderr taxonomy mapping (plan D.2).
 *
 * The hashline binary is authoritative for failure semantics: exit 0 is a
 * valid payload (even for `*** Abort` no-ops); exit 1 is any logical error
 * with the diagnostic on stderr (pretty `Error: ...\nHint: ...` or a single
 * JSON object in `--json` mode). The wrapper never re-parses hashes or
 * re-derives staleness; it classifies the binary's stderr and adds model
 * teaching text.
 */

import { parseErrorPayload, type ErrorPayload } from "./hashline-core";

/** Structured error kinds the host can react to (plan D.2). */
export type ErrorKind =
  | "stale_anchor"
  | "empty_patch"
  | "ambiguous_hash"
  | "hash_not_found"
  | "out_of_range"
  | "invalid_anchor"
  | "binary_not_found"
  | "io";

export interface FormattedHashlineError {
  text: string;
  kind: ErrorKind;
}

const COMPACT_KIND_MAP: Record<string, ErrorKind> = {
  STALE_ANCHOR: "stale_anchor",
  STALE_FILE: "stale_anchor",
  EMPTY_PATCH: "empty_patch",
  NOOP_LOOP: "empty_patch",
  AMBIGUOUS_HASH: "ambiguous_hash",
  HASH_NOT_FOUND: "hash_not_found",
  INVALID_ANCHOR: "invalid_anchor",
  BLOCK_UNRESOLVED: "invalid_anchor",
};

/** Compact ERR line: `ERR KIND key=val...` (default stderr since binary 0.9.12). */
const COMPACT_ERR = /^ERR ([A-Z_]+)(.*)$/m;

/** Extract a single `key=value` pair from a compact ERR argument string. */
function errArg(args: string, key: string): string | undefined {
  const match = args.match(new RegExp(`(?:\\s|^)${key}=(\\S+)`));
  return match?.[1];
}

/** Substring signatures for pretty-mode stderr detection. */
const SIGNATURES: Array<[ErrorKind, RegExp]> = [
  ["stale_anchor", /changed since last read/i],
  ["empty_patch", /produced no edits/i],
  ["ambiguous_hash", /ambiguous|multiple matches/i],
  ["hash_not_found", /hash not found|not found for/i],
  ["out_of_range", /out of range/i],
  ["invalid_anchor", /invalid anchor|expected a line number/i],
];

/** Map the binary's structured `kind` to our host-facing kind. */
export function mapErrorKind(payload: ErrorPayload): ErrorKind {
  switch (payload.kind) {
    case "STALE_ANCHOR":
    case "STALE_FILE":
      return "stale_anchor";
    case "EMPTY_PATCH":
    case "NOOP_LOOP":
      return "empty_patch";
    case "AMBIGUOUS_HASH":
      return "ambiguous_hash";
    case "HASH_NOT_FOUND":
      return "hash_not_found";
    case "INVALID_ANCHOR":
    case "BLOCK_UNRESOLVED":
      return "invalid_anchor";
    case "MISSING_SNAPSHOT_TAG":
    case "CANNOT_RECOVER":
    case "FILE_NOT_FOUND":
    case "BINARY_FILE":
    case "INVALID_UTF8":
    case "CLIPBOARD":
    case "IO":
    case "PATCH_FAILED":
      return "io";
    default:
      return "io";
  }
}

/** Parse the binary's stderr into a structured error, or null when unparseable. */
export function parseStderr(stderr: string): ErrorPayload | null {
  return parseErrorPayload(stderr);
}

/**
 * Format an exit-1 stderr stream into the model-facing message plus a kind.
 * Always appends a re-read teaching line for stale-anchor/ambiguous cases.
 */
export function formatHashlineError(
  stderr: string,
  exitCode: number,
): FormattedHashlineError {
  const trimmed = stderr.trim();

  // Compact-mode stderr: `ERR KIND key=val...` + optional `HINT ...`.
  const compact = trimmed.match(COMPACT_ERR);
  if (compact) {
    const kindName = compact[1] as keyof typeof COMPACT_KIND_MAP;
    const args = compact[2] ?? "";
    const hintLine = trimmed
      .split("\n")
      .find((line) => line.startsWith("HINT "));
    const hint = hintLine?.slice("HINT ".length);
    const diag = hint
      ? `Error: ERR ${kindName}${args}\n${hint}`
      : `Error: ERR ${kindName}${args}`;
    const kind = COMPACT_KIND_MAP[kindName] ?? "io";
    const teaching =
      kind === "stale_anchor" || kind === "hash_not_found"
        ? `\n${RE_READ_HINT}`
        : kind === "ambiguous_hash"
          ? "\nRe-read; use the exact N:hh anchor."
          : "";
    return { text: `${diag}${teaching}`, kind };
  }


  // Pretty-mode stderr: `Error: ...` / `Hint: ...`.
  if (trimmed.length > 0) {
    for (const [kind, re] of SIGNATURES) {
      if (re.test(trimmed)) {
        return { text: trimmed, kind };
      }
    }
    return { text: trimmed, kind: "io" };
  }

  // No stderr at all.
  return {
    text: `Error: hashline exited with code ${exitCode} and produced no diagnostic`,
    kind: "io",
  };
}

/** Model-facing guidance appended for stale anchors. */
export const RE_READ_HINT =
  "Re-read the file with hashline_read and retry with a fresh N:hh anchor.";

/** Install hint when the binary cannot be spawned (ENOENT). */
export const INSTALL_HINT =
  "hashline binary not found. Install it (add to PATH) or set HASHLINE_BIN to the absolute path of the binary.";
