/**
 * Hashline configuration — loads ~/.pi/agent/hashline.json once at module init.
 *
 * Schema: {
 *   "hashLength": 2 | 3 | 4,   // advisory for the wrapper (the binary owns hashes)
 *   "grep": boolean,            // gate the optional grep tool
 *   "replaceText": boolean,     // gate the replace_text op translation
 *   "binary": "/abs/path/to/hashline"  // HASHLINE_BIN equivalent, highest precedence
 * }
 * Defaults: hashLength=2, grep=false, replaceText=true, binary=undefined.
 * Any field that fails validation falls back to its default; loading errors
 * are collected as warnings, never thrown.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

// ─── Types ───────────────────────────────────────────────────────────────

export type HashlineConfig = {
  hashLength: 2 | 3 | 4;
  grep: boolean;
  replaceText: boolean;
  binary: string | undefined;
};

/**
 * Supported hash length range. Single source of truth for every site that
 * must agree on it: config validation and the display-prefix rejection regexes.
 * NOTE: the binary always computes 2-hex per-line hashes; 3|4 are accepted but
 * advisory — anchors rendered by this wrapper come from the binary regardless.
 * (See integration/implementation-plan.md B.8.)
 */
export const HASH_LENGTH_MIN = 2;
export const HASH_LENGTH_MAX = 4;

// ─── Pure parse function (exported for unit tests) ───────────────────────

/**
 * Parse and validate a raw JSON value into a HashlineConfig.
 * All invalid fields fall back to defaults; errors are collected as warnings.
 */
export function parseHashlineConfig(raw: unknown): {
  config: HashlineConfig;
  warnings: string[];
} {
  const warnings: string[] = [];
  let hashLength: 2 | 3 | 4 = 2;
  let grep = false;
  let replaceText = true;
  let binary: string | undefined;

  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    warnings.push(
      `hashline.json: expected an object at top level, got ${JSON.stringify(raw)}. Using defaults.`,
    );
    return { config: { hashLength, grep, replaceText, binary }, warnings };
  }

  const obj = raw as Record<string, unknown>;

  // Validate hashLength
  if ("hashLength" in obj) {
    const hl = obj.hashLength;
    if (hl === 2 || hl === 3 || hl === 4) {
      hashLength = hl;
    } else {
      warnings.push(
        `hashline.json: "hashLength" must be 2, 3, or 4; got ${JSON.stringify(hl)}. Using default (2).`,
      );
    }
  }

  // Validate grep
  if ("grep" in obj) {
    const g = obj.grep;
    if (typeof g === "boolean") {
      grep = g;
    } else {
      warnings.push(
        `hashline.json: "grep" must be a boolean; got ${JSON.stringify(g)}. Using default (false).`,
      );
    }
  }

  // Validate replaceText
  if ("replaceText" in obj) {
    const rt = obj.replaceText;
    if (typeof rt === "boolean") {
      replaceText = rt;
    } else {
      warnings.push(
        `hashline.json: "replaceText" must be a boolean; got ${JSON.stringify(rt)}. Using default (true).`,
      );
    }
  }

  // Validate binary
  if ("binary" in obj) {
    const b = obj.binary;
    if (typeof b === "string" && b.length > 0) {
      binary = b;
    } else {
      warnings.push(
        `hashline.json: "binary" must be a non-empty string path; got ${JSON.stringify(b)}. Ignoring.`,
      );
    }
  }

  return { config: { hashLength, grep, replaceText, binary }, warnings };
}

// ─── Module-level singleton ──────────────────────────────────────────────

let _hashLength: 2 | 3 | 4 = 2;
let _grep = false;
let _replaceText = true;
let _binary: string | undefined;
let _warnings: string[] = [];

function loadConfig(): void {
  const configPath = join(getAgentDir(), "hashline.json");
  let raw: unknown;
  try {
    const text = readFileSync(configPath, "utf8");
    raw = JSON.parse(text);
  } catch (err: unknown) {
    // File not found is the common path — no warning, just use defaults.
    if (
      typeof err === "object" &&
      err !== null &&
      (err as NodeJS.ErrnoException).code !== "ENOENT"
    ) {
      _warnings = [
        `hashline.json: failed to load (${(err as Error).message}). Using defaults.`,
      ];
    }
    return;
  }
  const { config, warnings } = parseHashlineConfig(raw);
  _hashLength = config.hashLength;
  _grep = config.grep;
  _replaceText = config.replaceText;
  _binary = config.binary;
  _warnings = warnings;
}

// Load once at module init.
loadConfig();

// ─── Public API ─────────────────────────────────────────────────────────

export function getHashLength(): number {
  return _hashLength;
}

export function getGrepEnabled(): boolean {
  return _grep;
}

export function getReplaceTextEnabled(): boolean {
  return _replaceText;
}

export function getBinaryPath(): string | undefined {
  return _binary;
}

export function getConfigWarnings(): string[] {
  return _warnings;
}

// ─── Test helpers (not for production use) ──────────────────────────────

/** @internal */
export function __setHashLengthForTests(n: 2 | 3 | 4): void {
  _hashLength = n;
}

/** @internal */
export function __setGrepEnabledForTests(v: boolean): void {
  _grep = v;
}

/** @internal */
export function __setReplaceTextEnabledForTests(v: boolean): void {
  _replaceText = v;
}

/** @internal */
export function __setBinaryPathForTests(v: string | undefined): void {
  _binary = v;
}

/** @internal */
export function __resetConfigForTests(): void {
  _hashLength = 2;
  _grep = false;
  _replaceText = true;
  _binary = undefined;
  _warnings = [];
}
