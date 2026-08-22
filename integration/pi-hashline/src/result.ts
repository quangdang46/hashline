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
 * Accepts both the compact `ERR KIND key=val...` + `HINT ...` stderr (default
 * output mode since 0.9.12) and the legacy verbose `Error: ...`/`Hint: ...`
 * form. The binary's own diagnostics are authoritative; this only classifies
 * them and appends teaching text. Never re-implements hashing/merge/recovery.
 */
const COMPACT_ERR = /^ERR ([A-Z_]+)(.*)$/;

/** Extract a single `key=value` pair from a compact ERR argument string. */
function errArg(args: string, key: string): string | undefined {
  const match = args.match(new RegExp(`(?:\\s|^)${key}=(\\S+)`));
  return match?.[1];
}

export function formatHashlineError(
  stderr: string,
  exitCode: number,
): HashlineError {
  const text = stderr.trim();
  const lower = text.toLowerCase();

  // Compact mode (default since 0.9.12): first line is `ERR KIND key=val...`,
  // optional second line `HINT ...`. Kind names match the --json taxonomy.
  const compact = text.match(COMPACT_ERR);
  if (compact) {
    const kind = compact[1]!;
    const args = compact[2] ?? "";
    const hint = text
      .split("\n")
      .find((line) => line.startsWith("HINT "))
      ?.slice("HINT ".length);
    const diag = hint
      ? `ERR ${kind}${args}\nHINT: ${hint}`
      : `ERR ${kind}${args}`;
    switch (kind) {
      case "STALE_ANCHOR":
      case "STALE_FILE":
        return { kind: "stale_anchor", text: `${diag}\n${REREAD_HINT}` };
      case "EMPTY_PATCH": {
        const reason = errArg(args, "reason");
        return {
          kind: "empty_patch",
          text: `${diag}\nPatch was empty${reason ? `: ${reason}` : " — nothing to do."}`,
        };
      }
      case "AMBIGUOUS_HASH":
        return {
          kind: "ambiguous_hash",
          text: `${diag}\nRe-read; use the exact N:hh anchor.`,
        };
      case "HASH_NOT_FOUND":
        return { kind: "hash_not_found", text: `${diag}\n${REREAD_HINT}` };
      case "INVALID_ANCHOR":
        return { kind: "out_of_range", text: diag };
      case "BINARY_FILE":
        return { kind: "binary_file", text: diag };
      default:
        return { kind: "io", text: diag };
    }
  }

  // Verbose fallback (0.9.11- binaries or --verbose runs): legacy Error:/Hint: text.
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
