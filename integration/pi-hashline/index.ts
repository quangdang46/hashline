/**
 * hashline-pi — read/edit override for pi-coding-agent.
 *
 * Thin wrapper: every tool shells out to the real `hashline` binary. No
 * hashing, staleness, or merge logic lives in this package.
 */

import type {
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { registerEditTool } from "./src/edit.js";
import { registerReadTool } from "./src/read.js";
import { registerGrepTool } from "./src/grep.js";
import { registerWriteTool } from "./src/write.js";
import { registerFileTools } from "./src/files.js";
import {
  getGrepEnabled,
  getConfigWarnings,
  getBinaryPath,
} from "./src/config.js";
import {
  runHashline,
  resolveHashlineBin,
  parseHashlineVersion,
  isVersionAtLeast,
  MIN_HASHLINE_VERSION,
} from "./src/hashline.js";
import { HASHLINE_INSTALL_HINT } from "./src/result.js";

async function versionStatus(ctx: ExtensionContext): Promise<string> {
  const bin = resolveHashlineBin();
  const run = await runHashline(["--version"], undefined, ctx, ctx.signal);
  if (run.exitCode !== 0) {
    return `hashline binary: ${bin} (failed: ${run.stderr.trim() || `exit ${run.exitCode}`})\n\n${HASHLINE_INSTALL_HINT}`;
  }
  const version = parseHashlineVersion(run.stdout);
  if (version === null) {
    return `hashline binary: ${bin}\n(unexpected --version output: ${JSON.stringify(run.stdout.trim())})`;
  }
  const ok = isVersionAtLeast(version, MIN_HASHLINE_VERSION);
  const versionText = `hashline binary: ${bin} (v${version.major}.${version.minor}.${version.patch})`;
  return ok
    ? `${versionText} — OK (>= ${MIN_HASHLINE_VERSION})`
    : `${versionText} — WARNING: expected >= ${MIN_HASHLINE_VERSION}; upgrade for the pinned CLI contract`;
}

export default function (pi: ExtensionAPI): void {
  registerReadTool(pi);
  registerEditTool(pi);
  registerWriteTool(pi);
  registerFileTools(pi);
  if (getGrepEnabled()) {
    registerGrepTool(pi);
  }

  pi.on("session_start", async (_event, ctx) => {
    const warnings = getConfigWarnings();
    if (warnings.length > 0) {
      ctx.ui.notify(
        `hashline.json config warnings:\n${warnings.join("\n")}`,
        "warning",
      );
    }

    const debugValue = process.env.PI_HASHLINE_DEBUG;
    if (debugValue === "1" || debugValue === "true") {
      ctx.ui.notify("Hashline mode active", "info");
    }
  });

  pi.registerCommand("hashline-status", {
    description: "Check the configured hashline binary and config",
    handler: async (_args: string, ctx: ExtensionCommandContext) => {
      const binary = getBinaryPath();
      const config = binary ? `\nconfig binary: ${binary}` : "";
      const status = await versionStatus(ctx);
      ctx.ui.notify(`${status}${config}`, "info");
    },
  });
}
