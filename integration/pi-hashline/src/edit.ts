/**
 * Edit tool — overrides the built-in "edit".
 *
 * Translates {path, edits:[{op,pos,end,lines}]} into hashline patch strings,
 * pipes them to the binary via stdin, and surfaces the binary's diagnostics.
 * All semantic validation (staleness, hashes, no-op) is the binary's job —
 * this wrapper only maps exit codes to structured results.
 */

import type {
  ExtensionAPI,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type, type Static } from "typebox";
import { loadPrompt } from "./prompt-loader.js";
import { resolveToCwd } from "./path-utils.js";
import {
  runHashline,
  parseReadJson,
  parseCompactPatchOutput,
  formatReadLines,
  type ReadResult,
} from "./hashline.js";
import { buildPatchText, type HashlineEdit } from "./edit-args.js";
import {
  textResult,
  errorResult,
  formatHashlineError,
  binaryNotFoundError,
  type TextToolResult,
} from "./result.js";
import { getReplaceTextEnabled } from "./config.js";

const EDIT_DESC = loadPrompt(
  new URL("../prompts/edit.md", import.meta.url),
).trim();

const EDIT_PROMPT_SNIPPET = loadPrompt(
  new URL("../prompts/edit-snippet.md", import.meta.url),
).trim();

const EDIT_PROMPT_GUIDELINES = loadPrompt(
  new URL("../prompts/edit-guidelines.md", import.meta.url),
)
  .split("\n")
  .map((line) => line.trim())
  .filter((line) => line.startsWith("- "))
  .map((line) => line.slice(2));

const anchor = Type.String({
  description: "anchor N:hh from read output (e.g. 12:ab)",
});

const replaceEdit = Type.Object(
  {
    op: Type.Literal("replace", {
      description: "replace one line at pos, or inclusive pos..end, with lines",
    }),
    pos: anchor,
    end: Type.Optional(anchor),
    lines: Type.Array(Type.String()),
  },
  { additionalProperties: false },
);

const appendEdit = Type.Object(
  {
    op: Type.Literal("append"),
    pos: Type.Optional(anchor),
    lines: Type.Array(Type.String()),
  },
  { additionalProperties: false },
);

const prependEdit = Type.Object(
  {
    op: Type.Literal("prepend"),
    pos: Type.Optional(anchor),
    lines: Type.Array(Type.String()),
  },
  { additionalProperties: false },
);

const deleteEdit = Type.Object(
  {
    op: Type.Literal("delete"),
    pos: anchor,
    end: Type.Optional(anchor),
  },
  { additionalProperties: false },
);

const replaceTextEdit = Type.Object(
  {
    op: Type.Literal("replace_text"),
    oldText: Type.String({
      description: "exact text to replace; must be unique in the file",
    }),
    newText: Type.String({ description: "replacement text" }),
  },
  { additionalProperties: false },
);

export const editToolSchema = Type.Object(
  {
    path: Type.String(),
    edits: Type.Array(
      Type.Union([
        replaceEdit,
        appendEdit,
        prependEdit,
        deleteEdit,
        replaceTextEdit,
      ]),
      { description: "edit operations; applied atomically by the binary" },
    ),
  },
  { additionalProperties: false },
);

type EditToolParams = Static<typeof editToolSchema>;

/**
 * Resolve a replace_text edit by reading the file and finding a unique
 * matching line. Returns a replace edit targeting that line's N:hh anchor.
 * Zero or multiple matches → structured error (never a heuristic).
 */
async function resolveReplaceText(
  edit: HashlineEdit,
  file: string,
  ctx: ExtensionContext,
  signal: AbortSignal | undefined,
): Promise<
  { ok: true; edit: HashlineEdit } | { ok: false; kind: string; text: string }
> {
  const { stdout, stderr, exitCode } = await runHashline(
    ["read", file, "--json"],
    undefined,
    ctx,
    signal,
  );
  if (exitCode !== 0) {
    return {
      ok: false,
      kind: "io",
      text: stderr.trim() || `hashline read exited ${exitCode}`,
    };
  }
  let parsed: ReadResult;
  try {
    parsed = parseReadJson(stdout);
  } catch (err) {
    return {
      ok: false,
      kind: "io",
      text: `failed to parse hashline read output: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
  const oldText = edit.oldText ?? "";
  const matches = parsed.lines.filter((l) => l.content.includes(oldText));
  if (matches.length === 0) {
    return {
      ok: false,
      kind: "hash_not_found",
      text: `replace_text matched 0 lines for "${oldText}" (need exactly 1) — use replace with an N:hh anchor from read`,
    };
  }
  if (matches.length > 1) {
    return {
      ok: false,
      kind: "ambiguous_hash",
      text: `replace_text matched ${matches.length} lines for "${oldText}" (need exactly 1) — use replace with an N:hh anchor from read`,
    };
  }
  const line = matches[0]!;
  const newText = edit.newText ?? "";
  if (newText.length === 0) {
    return { ok: true, edit: { op: "delete", pos: `${line.n}:${line.hash}` } };
  }
  return {
    ok: true,
    edit: {
      op: "replace",
      pos: `${line.n}:${line.hash}`,
      lines: newText.split("\n"),
    },
  };
}

export function registerEditTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "edit",
    label: "Edit",
    description: EDIT_DESC,
    promptSnippet: EDIT_PROMPT_SNIPPET,
    promptGuidelines: EDIT_PROMPT_GUIDELINES,
    parameters: editToolSchema,
    // Mandatory: the built-in edit tool uses renderShell:"self"; forcing
    // "default" keeps the shared background shell for the override.
    renderShell: "default",
    async execute(
      _toolCallId,
      params: EditToolParams,
      signal,
      _onUpdate,
      ctx,
    ): Promise<TextToolResult> {
      const file = resolveToCwd(params.path, ctx.cwd);

      // Resolve replace_text ops against a fresh read before translating.
      const resolvedEdits: HashlineEdit[] = [];
      for (const edit of params.edits) {
        if (edit.op === "replace_text") {
          if (!getReplaceTextEnabled()) {
            return errorResult(
              `The replace_text op is disabled in your hashline configuration (replaceText: false). Re-read the file and use replace/append/prepend with N:hh anchors instead.`,
              { kind: "io" },
            );
          }
          const resolved = await resolveReplaceText(edit, file, ctx, signal);
          if (!resolved.ok) {
            return errorResult(resolved.text, { kind: resolved.kind });
          }
          resolvedEdits.push(resolved.edit);
        } else {
          resolvedEdits.push(edit);
        }
      }

      const built = buildPatchText({ path: params.path, edits: resolvedEdits });
      if (!built.ok) {
        return errorResult(built.error, { kind: "io" });
      }

      const { stdout, stderr, exitCode } = await runHashline(
        ["patch", file, "-"],
        built.patch,
        ctx,
        signal,
      );
      if (exitCode !== 0) {
        const err = stderr.trim()
          ? formatHashlineError(stderr, exitCode)
          : binaryNotFoundError(new Error("spawn failed"));
        return errorResult(err.text, { kind: err.kind });
      }

      // Chained edits: re-read the changed file for fresh anchors so the
      // model can continue without a separate read call.
      let anchors = "";
      const fresh = await runHashline(
        ["read", file, "--json"],
        undefined,
        ctx,
        signal,
      );
      if (fresh.exitCode === 0) {
        try {
          const parsed = parseReadJson(fresh.stdout);
          anchors = `\n--- Anchors ---\n${formatReadLines(parsed).join("\n")}`;
        } catch {
          anchors = "";
        }
      }
      // Compact mode (0.9.12+): stdout carries the OK header + changed rows.
      // Fall back to raw stdout when the shape differs (older binaries).
      const compact = parseCompactPatchOutput(stdout);
      const applied = compact
        ? [
            `OK #${compact.fileHash} edits=${compact.editsApplied} changed=${compact.changedCount}`,
            ...compact.rows,
          ].join("\n")
        : stdout.trim();
      return textResult(
        `Patch applied.${applied ? `\n${applied}` : ""}${anchors}`,
        {
          ok: true,
        },
      );
    },
  });
}
