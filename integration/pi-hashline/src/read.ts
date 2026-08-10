/**
 * Read tool — overrides the built-in "read" so every line renders as
 * binary-native `N:hh|content` anchors the edit tool can consume.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { loadPrompt } from "./prompt-loader.js";
import { resolveToCwd } from "./path-utils.js";
import {
  runHashline,
  parseReadJson,
  formatReadPreview,
  type ReadResult,
} from "./hashline.js";
import {
  textResult,
  errorResult,
  formatHashlineError,
  binaryNotFoundError,
} from "./result.js";

const READ_DESC = loadPrompt(
  new URL("../prompts/read.md", import.meta.url),
).trim();

const READ_PROMPT_SNIPPET = loadPrompt(
  new URL("../prompts/read-snippet.md", import.meta.url),
).trim();

const READ_PROMPT_GUIDELINES = loadPrompt(
  new URL("../prompts/read-guidelines.md", import.meta.url),
)
  .split("\n")
  .map((line) => line.trim())
  .filter((line) => line.startsWith("- "))
  .map((line) => line.slice(2));

export function registerReadTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "read",
    label: "Read",
    description: READ_DESC,
    promptSnippet: READ_PROMPT_SNIPPET,
    promptGuidelines: READ_PROMPT_GUIDELINES,
    parameters: Type.Object({
      path: Type.String({
        description: "Path to the file to read (relative or absolute)",
      }),
      offset: Type.Optional(
        Type.Integer({
          minimum: 1,
          description: "Line number to start from (1-indexed)",
        }),
      ),
      limit: Type.Optional(
        Type.Integer({
          minimum: 1,
          description: "Max lines to read",
        }),
      ),
      raw: Type.Optional(
        Type.Boolean({
          description: "Return plain text without anchors (cheaper)",
        }),
      ),
    }),
    renderShell: "default",
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const file = resolveToCwd(params.path, ctx.cwd);
      const { stdout, stderr, exitCode } = await runHashline(
        ["read", file, "--json"],
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
      let parsed: ReadResult;
      try {
        parsed = parseReadJson(stdout);
      } catch (err) {
        return errorResult(
          `failed to parse hashline read output: ${err instanceof Error ? err.message : String(err)}`,
          { kind: "io" },
        );
      }
      const preview = formatReadPreview(parsed, {
        offset: params.offset,
        limit: params.limit,
        raw: params.raw,
      });
      return textResult(preview.text, {
        truncation: preview.truncated,
        ...(preview.nextOffset !== undefined
          ? { nextOffset: preview.nextOffset }
          : {}),
      });
    },
  });
}
