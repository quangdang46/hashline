/**
 * AgentToolResult builders + error taxonomy.
 *
 * Errors are signaled by returning `{ isError: true, ... }` (never thrown) so
 * the model-facing `content` text stays structured and the host-side `kind`
 * field lets the agent react to the failure class.
 */

export type HashlineErrorKind =
  | "stale_anchor"
  | "empty_patch"
  | "ambiguous_hash"
  | "hash_not_found"
  | "out_of_range"
  | "binary_not_found"
  | "io"
  | "binary_file";

export type HashlineError = {
  kind: HashlineErrorKind;
  text: string;
};

export type TextToolResult = {
  content: Array<{ type: "text"; text: string }>;
  details: Record<string, unknown>;
  isError?: boolean;
};

export function textResult(
  text: string,
  details: Record<string, unknown> = {},
  isError = false,
): TextToolResult {
  return {
    content: [{ type: "text", text }],
    details: { ok: !isError, ...details },
    ...(isError ? { isError: true } : {}),
  };
}

export function errorResult(
  text: string,
  details: Record<string, unknown> = {},
): TextToolResult {
  return textResult(text, details, true);
}

const REREAD_HINT =
  "\nRe-read the file with `read` and retry with a fresh anchor.";

/**
 * Map a failed hashline invocation (exit != 0) to a structured tool error.
 * The binary's own stderr is authoritative; this only classifies it and appends
 * teaching text. Never re-implements hashing/merge/recovery.
 */
export function formatHashlineError(
  stderr: string,
  exitCode: number,
): HashlineError {
  const text = stderr.trim();
  const lower = text.toLowerCase();

  if (exitCode === 1) {
    if (
      lower.includes("changed since last read") ||
      lower.includes("stale_anchor") ||
      lower.includes("stale anchor")
    ) {
      return { kind: "stale_anchor", text: `${text}\n${REREAD_HINT}` };
    }
    if (
      lower.includes("produced no edits") ||
      lower.includes("empty_patch") ||
      lower.includes("empty patch")
    ) {
      return {
        kind: "empty_patch",
        text: `${text}\nPatch was empty — nothing to do.`,
      };
    }
    if (
      lower.includes("ambiguous") ||
      lower.includes("multiple matches") ||
      lower.includes("ambiguous_hash")
    ) {
      return {
        kind: "ambiguous_hash",
        text: `${text}\nRe-read; use the exact N:hh anchor.`,
      };
    }
    if (
      lower.includes("out of range") ||
      lower.includes("line out of range") ||
      lower.includes("out_of_range")
    ) {
      return { kind: "out_of_range", text };
    }
    if (
      lower.includes("appears to be binary") ||
      lower.includes("binary_file") ||
      lower.includes("binary file")
    ) {
      return { kind: "binary_file", text };
    }
    return {
      kind: "io",
      text: text || `hashline exited with code ${exitCode}`,
    };
  }

  // Any other non-zero exit (infrastructure failures).
  return { kind: "io", text: text || `hashline exited with code ${exitCode}` };
}

export const HASHLINE_INSTALL_HINT = `Install the hashline CLI first:
  see https://github.com/quangdang46/hashline#install

Then make sure the binary is on PATH for pi, or set:
  export HASHLINE_BIN="/path/to/hashline"

Or add to ~/.pi/agent/hashline.json:
  { "binary": "/path/to/hashline" }`;

/**
 * Structured error for a spawn failure (binary not found / not executable).
 */
export function binaryNotFoundError(err: Error): HashlineError {
  return {
    kind: "binary_not_found",
    text: `failed to run hashline: ${err.message}\n\n${HASHLINE_INSTALL_HINT}`,
  };
}
