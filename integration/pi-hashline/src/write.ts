/**
 * Write tool — overrides the built-in "write" so new files are created
 * through the hashline binary (snapshot-seeded from creation time, so the
 * first edit is stale-safe without a separate read).
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { resolveToCwd } from "./path-utils.js";
import {
  runHashline,
  formatReadLines,
  parseReadJson,
  type ReadResult,
} from "./hashline.js";
import {
  textResult,
  errorResult,
  formatHashlineError,
  binaryNotFoundError,
} from "./result.js";

const WRITE_DESC = [
  "Write a file: create a new file with content, or replace an existing file entirely (set force: true).",
  "Content is written verbatim — no anchors needed. The response returns N:hh|content lines",
  "(fresh anchors) so follow-up edit calls work immediately.",
  "",
  "Prefer edit for changing part of an existing file; use write only for new files or full rewrites.",
].join("\n");

export function registerWriteTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "write",
    label: "Write",
    description: WRITE_DESC,
    promptSnippet:
      "write creates/overwrites whole files via hashline; responses include fresh N:hh anchors.",
    promptGuidelines: [
      "Use write only for new files or full rewrites; use edit with N:hh anchors to change existing files partially.",
    ],
    parameters: Type.Object({
      path: Type.String({
        description: "Path to the file to write (relative or absolute)",
      }),
      content: Type.String({
        description: "Full file content to write (verbatim)",
      }),
      force: Type.Optional(
        Type.Boolean({
          description:
            "Overwrite the file if it already exists (default false)",
        }),
      ),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const file = resolveToCwd(params.path, ctx.cwd);
      const args = ["write", file, params.content];
      if (params.force) {
        args.push("--force");
      }
      const { stdout, stderr, exitCode } = await runHashline(
        [...args, "--json"],
        undefined,
        ctx,
        signal,
      );
      if (exitCode !== 0) {
        const err = stderr.trim()
          ? formatHashlineError(stderr, exitCode)
          : binaryNotFoundError(new Error("spawn failed"));
        return errorResult(err.text, { kind: err.kind });
      }
      let parsed: ReadResult | null;
      try {
        parsed = parseReadJson(stdout);
      } catch {
        parsed = null;
      }
      // Target-exists without --force is the common failure; surface it as a
      // teaching error even when the binary's message is terse.
      if (!params.force && !parsed) {
        // Binary emits {"kind":"ERROR","error":"target '...' already exists — use --force ..."}.
        if (stderr.includes("already exists")) {
          return errorResult(
            `${file} already exists. Re-read it and use edit with N:hh anchors for changes, or set force: true to replace it entirely.`,
            { kind: "io" },
          );
        }
        return errorResult(stderr.trim() || `failed to write ${file}`, {
          kind: "io",
        });
      }
      if (!parsed) {
        return textResult(`Wrote ${file}.`, { ok: true });
      }
      return textResult(
        `Wrote ${file} (#${parsed.hash}).\n--- Anchors ---\n${formatReadLines(parsed).join("\n")}`,
        { ok: true },
      );
    },
  });
}
