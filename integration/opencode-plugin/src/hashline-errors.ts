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
export function formatHashlineError(stderr: string, exitCode: number): FormattedHashlineError {
  const trimmed = stderr.trim();

  // Structured JSON error (binary invoked with --json).
  const payload = parseErrorPayload(stderr);
  if (payload) {
    const kind = mapErrorKind(payload);
    const hint = payload.hint ? `\n${payload.hint}` : "";
    return {
      text: `Error: ${payload.error}${hint}`,
      kind,
    };
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
