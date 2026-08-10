/**
 * Prompt loader.
 *
 * Prompts ship in prompts/*.md with binary-native `N:hh` anchors (2-hex, the
 * format the hashline binary computes and parses). No rewriting is needed for
 * hash length — the binary owns hash computation and always emits 2-hex, so
 * `hashLength` config is advisory only.
 *
 * When replaceText is disabled in config, strips the replace_text bullet from
 * prompts so the model never sees the op as an option.
 *
 * Resolution: first relative to the module (works when pi loads the extension
 * from node_modules), then relative to the package root (works when the
 * contract-test build runs the compiled output from `.tmp-tests/`, where tsc
 * does not copy the .md files).
 */

import { readFileSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { getReplaceTextEnabled } from "./config.js";

/**
 * Remove the `replace_text` op bullet and its inline `oldText`/`newText`
 * description from a prompt string. Matches the exact authored line in
 * prompts/edit.md; other prompt files that have no such line are returned
 * unchanged. The regex matches the leading `- ` bullet, the op name, and
 * everything to the end of the line, including a trailing newline if present.
 */
export function stripReplaceTextFromPrompt(text: string): string {
  return text.replace(/^- `replace_text`[^\n]*\n?/m, "");
}

function resolvePromptPath(name: string): string | undefined {
  const viaModule = resolve(
    dirname(fileURLToPath(import.meta.url)),
    "..",
    "prompts",
    name,
  );
  if (existsSync(viaModule)) {
    return viaModule;
  }
  const viaCwd = resolve(process.cwd(), "prompts", name);
  if (existsSync(viaCwd)) {
    return viaCwd;
  }
  return viaModule;
}

/** Read a prompt file, stripping replace_text content when disabled. */
export function loadPrompt(url: URL): string {
  const name = url.pathname.split("/").pop() ?? "";
  const path = resolvePromptPath(name);
  if (path === undefined) {
    throw new Error(`prompt file not found: prompts/${name}`);
  }
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    // Fall back to the import-meta-relative location (may still exist when
    // prompts are vendored next to the compiled module).
    text = readFileSync(url, "utf8");
  }
  if (!getReplaceTextEnabled()) {
    return stripReplaceTextFromPrompt(text);
  }
  return text;
}
