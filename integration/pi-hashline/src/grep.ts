/**
 * Optional grep tool — DEFERRED by default.
 *
 * The hashline binary has NO grep subcommand (verified: cli.rs Commands enum
 * has read/patch/write/find-block/guide/serve/mcp/remove/rename only), and
 * grep is out of hashline's scope (owned by the sibling `ffs` repo). This
 * module exists only so `index.ts` can gate a stub on `grep: true` config.
 *
 * When implemented, the tool must spawn `ffs grep --json` and re-hash matched
 * lines via `hashline read --json` — never reimplement search inside this
 * package. See integration/implementation-plan.md B.9 / C.7.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { errorResult } from "./result.js";

const GREP_DEFERRED =
  "grep is not implemented yet: the hashline binary has no grep subcommand. " +
  "Deferred for v0.1.0 (see integration/implementation-plan.md B.9). " +
  "Use `read` with offset/limit, or the sibling `ffs` tool.";

/**
 * Register a grep tool that reports the deferred state. Only reached when
 * config `grep: true`. A real implementation replaces this body.
 */
export function registerGrepTool(pi: ExtensionAPI): void {
  pi.registerTool({
    name: "grep",
    label: "Grep",
    description: "Search file contents (deferred in hashline-pi v0.1.0)",
    parameters: Type.Object({
      query: Type.String({ description: "Substring to search for" }),
      path: Type.Optional(
        Type.String({ description: "Directory or file to search" }),
      ),
    }),
    renderShell: "default",
    async execute() {
      return errorResult(GREP_DEFERRED, { kind: "io" });
    },
  });
}
