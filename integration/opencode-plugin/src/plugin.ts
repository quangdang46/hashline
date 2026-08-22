/**
 * plugin.ts — Hashline OpenCode plugin.
 *
 * Registers hashline_* tools that shell out to the hashline binary (>= 0.9.12).
 * No hashing/staleness/merge logic lives here.
 *
 * API surface (verified against @opencode-ai/plugin 1.4.6): `import { tool }
 * from "@opencode-ai/plugin"` returns a plain `{description, args, execute}`
 * ToolDefinition; `tool.schema` IS zod. The `helper.*` namespace does NOT
 * exist in the SDK. `execute(args, context)` receives a ToolContext with
 * `directory`, `worktree`, `abort`, and `metadata({title})`.
 */

import type { Plugin } from "@opencode-ai/plugin";
import { tool } from "@opencode-ai/plugin";
import { isAbsolute, resolve } from "node:path";
import {
  runHashline,
  parseReadJson,
  probeHashlineVersion,
  MIN_HASHLINE_VERSION,
} from "./hashline-core";
import { formatRead } from "./format";
import {
  buildPatchText,
  buildEditTitle,
  type EditOperation,
} from "./hashline-apply";
import {
  formatHashlineError,
  RE_READ_HINT,
  INSTALL_HINT,
} from "./hashline-errors";
import { renderHashlineEditPrompt } from "./prompt";

/** Resolve a path relative to the session base directory (directory || worktree). */
function resolvePath(p: string, baseDir: string): string {
  if (isAbsolute(p)) return p;
  return resolve(baseDir, p);
}

/** Get the effective base directory from plugin context. */
function getBaseDir(context: { directory: string; worktree: string }): string {
  return context.directory || context.worktree;
}

/**
 * Compare a `hashline X.Y.Z` banner against a minimum version string. Returns
 * true when the banner version is strictly older than the minimum.
 */
function isOlderThan(banner: string, minimum: string): boolean {
  const m = /hashline\s+(\d+)\.(\d+)\.(\d+)/.exec(banner);
  if (!m) return true; // unparseable banner — treat as unknown/old
  const parse = (s: string) => s.split(".").map((n) => parseInt(n, 10) || 0);
  const a = parse(`${m[1]}.${m[2]}.${m[3]}`);
  const b = parse(minimum);
  for (let i = 0; i < 3; i++) {
    if (a[i] !== b[i]) return a[i]! < b[i]!;
  }
  return false;
}

const plugin: Plugin = async (ctx) => {
  const baseDir = ctx.directory || ctx.worktree;

  // D.1: version probe once on plugin init. Degrades to a warning banner in
  // tool output; never hard-fails on a missing/call-failing probe.
  let versionBanner: string | null = null;
  void (async () => {
    const banner = await probeHashlineVersion();
    if (banner === null) {
      versionBanner = `Warning: could not determine hashline version (expected >= ${MIN_HASHLINE_VERSION}). ${INSTALL_HINT}`;
    } else if (isOlderThan(banner, MIN_HASHLINE_VERSION)) {
      versionBanner = `Warning: hashline ${banner} is older than the minimum supported version ${MIN_HASHLINE_VERSION}. Upgrade hashline.`;
    }
  })();

  return {
    tool: {
      // ─── hashline_read ───────────────────────────────────────────────
      hashline_read: tool({
        description:
          "Read a file. Returns lines tagged as `N:hh|content` where hh is the 2-char content hash " +
          "(the hashline binary format). Pass these N:hh anchors to hashline_edit. " +
          "Prefer this over read for files you intend to edit.",
        args: {
          path: tool.schema
            .string()
            .describe("Path to the file to read (relative or absolute)"),
          offset: tool.schema
            .number()
            .optional()
            .describe("First line (1-based)"),
          limit: tool.schema
            .number()
            .optional()
            .describe("Max lines (default all)"),
        },
        async execute(args, context) {
          const filePath = resolvePath(args.path, getBaseDir(context));
          let result;
          try {
            result = await runHashline(
              ["read", filePath, "--json"],
              undefined,
              context.abort,
              { cwd: baseDir },
            );
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }
          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            return fmt.text.startsWith("Error:")
              ? fmt.text
              : `Error: ${fmt.text}`;
          }
          let parsed;
          try {
            parsed = parseReadJson(result.stdout);
          } catch {
            return `Error: hashline read --json returned an unexpected payload`;
          }
          const view = formatRead(parsed, {
            offset: args.offset,
            limit: args.limit,
          });
          const rangeNote =
            view.truncated || (args.offset ?? 1) > 1
              ? `(showing lines ${view.startLine}-${view.endLine} of ${parsed.lines.length} total)\n`
              : "";
          const banner = versionBanner ? `${versionBanner}\n` : "";
          return `${banner}${rangeNote}${view.text}`;
        },
      }),

      // ─── hashline_edit ───────────────────────────────────────────────
      hashline_edit: tool({
        description:
          "Edit a file using N:hh anchors from hashline_read. Operations are validated atomically by the " +
          "hashline binary; stale anchors are rejected with a mismatch error. Prefer this over edit.",
        args: {
          path: tool.schema.string().describe("Path to the file"),
          edits: tool.schema
            .array(
              tool.schema.discriminatedUnion("op", [
                tool.schema.object({
                  op: tool.schema.literal("replace"),
                  pos: tool.schema.string().describe("anchor N:hh"),
                  end: tool.schema
                    .string()
                    .optional()
                    .describe("inclusive end anchor N:hh"),
                  lines: tool.schema.array(tool.schema.string()).optional(),
                }),
                tool.schema.object({
                  op: tool.schema.literal("append"),
                  pos: tool.schema
                    .string()
                    .optional()
                    .describe("insert after this N:hh; omit = EOF"),
                  lines: tool.schema.array(tool.schema.string()).optional(),
                }),
                tool.schema.object({
                  op: tool.schema.literal("prepend"),
                  pos: tool.schema
                    .string()
                    .optional()
                    .describe("insert before this N:hh; omit = BOF"),
                  lines: tool.schema.array(tool.schema.string()).optional(),
                }),
                tool.schema.object({
                  op: tool.schema.literal("delete"),
                  pos: tool.schema.string().describe("anchor N:hh to delete"),
                  end: tool.schema
                    .string()
                    .optional()
                    .describe("inclusive end anchor N:hh"),
                }),
              ]),
            )
            .describe(
              "Edit operations; validated atomically by the hashline binary",
            ),
        },
        async execute(args, context) {
          const filePath = resolvePath(args.path, getBaseDir(context));
          context.metadata({ title: buildEditTitle(args) });

          const edits = (args.edits ?? []) as EditOperation[];
          if (edits.length === 0) {
            return "No edit operations provided. Pass a non-empty `edits` array.";
          }

          const patchText = buildPatchText(edits);
          let result;
          try {
            result = await runHashline(
              ["patch", filePath, "-"],
              patchText,
              context.abort,
              { cwd: baseDir },
            );
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }

          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            const reRead =
              fmt.kind === "stale_anchor" ||
              fmt.kind === "ambiguous_hash" ||
              fmt.kind === "hash_not_found"
                ? `\n${RE_READ_HINT}`
                : "";
            const prefix = fmt.text.startsWith("Error:") ? "" : "Error: ";
            return `${prefix}${fmt.text}${reRead}`;
          }

          // Compact mode (binary >= 0.9.12): stdout carries the OK header +
          // changed-line rows (~modified / +inserted / -deleted).
          const changedRows = result.stdout.trim();
          return `Patch applied to ${args.path}.${changedRows ? `\n${changedRows}` : ""}\nRe-read with hashline_read for fresh anchors.`;
        },
      }),

      // ─── hashline_write ──────────────────────────────────────────────
      hashline_write: tool({
        description:
          "Create a new file or fully replace an existing one via the hashline binary. " +
          "The response includes fresh N:hh anchors so follow-up hashline_edit calls work immediately. " +
          "Prefer hashline_edit for changing part of an existing file.",
        args: {
          path: tool.schema.string().describe("Path to the file"),
          content: tool.schema
            .string()
            .describe("Full file content, written verbatim"),
          force: tool.schema
            .boolean()
            .optional()
            .describe("Overwrite if the file already exists (default false)"),
        },
        async execute(args, context) {
          const filePath = resolvePath(args.path, getBaseDir(context));
          const argv = ["write", filePath, args.content];
          if (args.force) argv.push("--force");
          let result;
          try {
            result = await runHashline([...argv, "--json"], undefined, context.abort, {
              cwd: baseDir,
            });
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }
          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            if (!args.force && result.stderr.includes("already exists")) {
              return `Error: ${args.path} already exists. Re-read it and use hashline_edit with N:hh anchors for changes, or pass force: true to replace it entirely.`;
            }
            return fmt.text.startsWith("Error:")
              ? fmt.text
              : `Error: ${fmt.text}`;
          }
          try {
            const parsed = parseReadJson(result.stdout);
            const anchors = parsed.lines
              .map((l) => `${l.n}:${l.hash}|${l.content}`)
              .join("\n");
            return `Wrote ${args.path} (#${parsed.hash}).\n--- Anchors ---\n${anchors}`;
          } catch {
            return `Wrote ${args.path}.`;
          }
        },
      }),

      // ─── hashline_find_block ─────────────────────────────────────────
      hashline_find_block: tool({
        description:
          "Show the syntactic block (function/class/if body — tree-sitter aware) containing a line. " +
          "Returns the block's N:hh anchors; pair with hashline_edit replace_block/delete_block ops.",
        args: {
          path: tool.schema.string().describe("Path to the file"),
          pos: tool.schema
            .number()
            .describe("1-based line number inside the target block"),
          anchor: tool.schema
            .string()
            .describe("2-char hash of that line from hashline_read output"),
        },
        async execute(args, context) {
          const filePath = resolvePath(args.path, getBaseDir(context));
          let result;
          try {
            result = await runHashline(
              ["find-block", filePath, `${args.pos}:${args.anchor}`, "--json"],
              undefined,
              context.abort,
              { cwd: baseDir },
            );
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }
          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            return fmt.text.startsWith("Error:")
              ? fmt.text
              : `Error: ${fmt.text}`;
          }
          try {
            const parsed = JSON.parse(result.stdout) as {
              language?: string;
              line_count?: number;
              block_lines?: Array<{ n: number; hash: string; content: string }>;
            };
            const rows = (parsed.block_lines ?? [])
              .map((l) => `${l.n}:${l.hash}|${l.content}`)
              .join("\n");
            return `Block in ${args.path} (${parsed.language ?? "unknown"}, ${parsed.line_count ?? "?"} lines total):\n${rows || "[empty]"}`;
          } catch {
            return `Error: hashline find-block --json returned an unexpected payload`;
          }
        },
      }),

      // ─── hashline_remove_file ────────────────────────────────────────
      hashline_remove_file: tool({
        description:
          "Delete a file via the hashline binary. Prefer this over shell rm so deletions stay explicit.",
        args: {
          path: tool.schema.string().describe("Path to the file to delete"),
        },
        async execute(args, context) {
          const filePath = resolvePath(args.path, getBaseDir(context));
          let result;
          try {
            result = await runHashline(
              ["remove", filePath, "--json"],
              undefined,
              context.abort,
              { cwd: baseDir },
            );
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }
          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            return fmt.text.startsWith("Error:")
              ? fmt.text
              : `Error: ${fmt.text}`;
          }
          return `Removed ${args.path}.`;
        },
      }),

      // ─── hashline_rename_file ────────────────────────────────────────
      hashline_rename_file: tool({
        description:
          "Move/rename a file via the hashline binary. Pass force to overwrite an existing destination.",
        args: {
          path: tool.schema.string().describe("Current file path"),
          to: tool.schema.string().describe("New path"),
          force: tool.schema
            .boolean()
            .optional()
            .describe("Overwrite destination if it exists (default false)"),
        },
        async execute(args, context) {
          const src = resolvePath(args.path, getBaseDir(context));
          const dst = resolvePath(args.to, getBaseDir(context));
          const argv = ["rename", src, dst];
          if (args.force) argv.push("--force");
          let result;
          try {
            result = await runHashline([...argv, "--json"], undefined, context.abort, {
              cwd: baseDir,
            });
          } catch {
            return `Error: ${INSTALL_HINT}`;
          }
          if (result.exitCode !== 0) {
            const fmt = formatHashlineError(result.stderr, result.exitCode);
            if (!args.force && result.stderr.includes("already exists")) {
              return `Error: ${args.to} already exists. Pass force: true to overwrite it.`;
            }
            return fmt.text.startsWith("Error:")
              ? fmt.text
              : `Error: ${fmt.text}`;
          }
          return `Renamed ${args.path} -> ${args.to}.`;
        },
      }),
    },

    // ─── System-prompt injection ───────────────────────────────────────
    "experimental.chat.system.transform": async (_input, output) => {
      output.system.push(renderHashlineEditPrompt());
    },
  };
};

export default plugin;
export { plugin };
