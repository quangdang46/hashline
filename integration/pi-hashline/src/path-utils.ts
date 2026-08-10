/**
 * Path resolution helpers. Pure path glue — no file I/O, no hashing.
 */

import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

/**
 * Resolve a user-supplied path against the pi session cwd.
 * Expands a leading `~` to the user's home directory. Relative paths are
 * resolved against `cwd`; absolute paths pass through.
 */
export function resolveToCwd(path: string, cwd: string): string {
  const trimmed = path.trim();
  const expanded = trimmed.startsWith("~/")
    ? join(homedir(), trimmed.slice(2))
    : trimmed === "~"
      ? homedir()
      : trimmed;
  return isAbsolute(expanded) ? expanded : resolve(cwd, expanded);
}
