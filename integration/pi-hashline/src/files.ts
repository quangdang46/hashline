/**
 * File management tools backed by the hashline binary: find-block, remove,
 * rename. Thin argv translation only — semantics live in the binary.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { resolveToCwd } from "./path-utils.js";
import { runHashline } from "./hashline.js";
import {
  textResult,
  errorResult,
  formatHashlineError,
  binaryNotFoundError,
} from "./result.js";

const FIND_BLOCK_DESC = [
  "Find the syntactic block (function, class, if/for body — tree-sitter aware) containing a line.",
  "Returns the block's lines with N:hh anchors; pair with edit replace_block/delete_block to",
  "rewrite or remove whole blocks.",
].join("\n");

export function registerFileTools(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "find_block",
    label: "Find Block",
    description: FIND_BLOCK_DESC,
    promptSnippet:
      "find_block shows the syntactic block around a line (N:hh anchors); use before replace_block edits.",
    promptGuidelines: [
      "Use find_block + edit {op: replace_block|delete_block} for whole-function or whole-block changes instead of line-by-line replaces.",
    ],
    parameters: Type.Object({
      path: Type.String({
        description: "Path to the file to inspect",
      }),
      pos: Type.Integer({
        description: "1-based line number inside the target block",
        minimum: 1,
      }),
      anchor: Type.String({
        description: "2-char hash of that line from read output (e.g. ab)",
      }),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const file = resolveToCwd(params.path, ctx.cwd);
      const anchor = `${params.pos}:${params.anchor}`;
      const { stdout, stderr, exitCode } = await runHashline(
        ["find-block", file, anchor, "--json"],
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
      let parsed: {
        file?: string;
        language?: string;
        line_count?: number;
        block_lines?: Array<{ n: number; hash: string; content: string }>;
      };
      try {
        parsed = JSON.parse(stdout);
      } catch (err) {
        return errorResult(
          `failed to parse find-block output: ${err instanceof Error ? err.message : String(err)}`,
          { kind: "io" },
        );
      }
      const rows = (parsed.block_lines ?? []).map(
        (l) => `${l.n}:${l.hash}|${l.content}`,
      );
      return textResult(
        `Block in ${file} (${parsed.language ?? "unknown"}, ${parsed.line_count ?? rows.length} lines total):\n${rows.join("\n") || "[empty]"}`,
        { ok: true },
      );
    },
  });

  pi.registerTool({
    name: "remove_file",
    label: "Remove File",
    description:
      "Delete a file via the hashline binary. Prefer this over bash rm so the deletion is explicit and auditable.",
    promptSnippet: "remove_file deletes a file (hashline-backed).",
    parameters: Type.Object({
      path: Type.String({ description: "Path to the file to delete" }),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const file = resolveToCwd(params.path, ctx.cwd);
      const { stdout, stderr, exitCode } = await runHashline(
        ["remove", file, "--json"],
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
      void stdout;
      return textResult(`Removed ${file}.`, { ok: true });
    },
  });

  pi.registerTool({
    name: "rename_file",
    label: "Rename File",
    description:
      "Move/rename a file via the hashline binary. Set force to overwrite an existing destination.",
    promptSnippet: "rename_file moves/renames a file (hashline-backed).",
    parameters: Type.Object({
      path: Type.String({ description: "Current file path" }),
      to: Type.String({
        description: "New path (may be in another directory)",
      }),
      force: Type.Optional(
        Type.Boolean({
          description:
            "Overwrite destination if it already exists (default false)",
        }),
      ),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const src = resolveToCwd(params.path, ctx.cwd);
      const dst = resolveToCwd(params.to, ctx.cwd);
      const args = ["rename", src, dst];
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
        if (stderr.includes("already exists")) {
          return errorResult(
            `${dst} already exists. Set force: true to overwrite it.`,
            { kind: "io" },
          );
        }
        return errorResult(err.text, { kind: err.kind });
      }
      void stdout;
      return textResult(`Renamed ${src} -> ${dst}.`, { ok: true });
    },
  });
}
