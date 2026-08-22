/**
 * Binary discovery + spawn glue for the hashline CLI.
 *
 * The wrapper NEVER reimplements hashing/staleness/merge — the Rust binary is
 * the single source of truth. This module only resolves the binary path,
 * spawns it (without a shell), and parses its documented output shapes.
 *
 * CLI contract: integration/CONTRACT.md (pinned to hashline >= 0.9.12).
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { getBinaryPath } from "./config.js";

export const MIN_HASHLINE_VERSION = "0.9.12";

export type HashlineRun = {
  stdout: string;
  stderr: string;
  exitCode: number;
};

export type ReadLine = {
  n: number;
  hash: string;
  content: string;
};

export type ReadResult = {
  path: string;
  hash: string;
  lines: ReadLine[];
};

const DEFAULT_HASHLINE_BIN = "hashline";

/**
 * Resolve the hashline binary path.
 * Order: config `binary` field (highest) -> HASHLINE_BIN env -> PATH.
 * Returns undefined when only the PATH default is available (spawn resolves it).
 */
export function resolveHashlineBin(
  env: NodeJS.ProcessEnv = process.env,
): string {
  const fromConfig = getBinaryPath();
  if (fromConfig) {
    return fromConfig;
  }
  const fromEnv = env.HASHLINE_BIN;
  if (fromEnv) {
    return fromEnv;
  }
  return DEFAULT_HASHLINE_BIN;
}

/** Standard install locations probed on Windows when the binary is not on PATH. */
export function probeKnownLocations(
  env: NodeJS.ProcessEnv = process.env,
): string[] {
  const candidates: string[] = [];
  const exe = process.platform === "win32" ? "hashline.exe" : "hashline";
  const home = homedir();
  if (home) {
    candidates.push(join(home, ".hashline", exe));
  }
  const fromCwd = env.HASHLINE_CWD || process.cwd();
  candidates.push(join(fromCwd, exe));
  return candidates;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Spawn the hashline binary, feed optional stdin, and capture stdout/stderr to
 * EOF. Never uses a shell. Mirrors pi-hledit's runHledit (index.ts:232-265).
 */
export async function runHashline(
  args: string[],
  stdin: string | undefined,
  ctx: ExtensionContext,
  signal: AbortSignal | undefined,
): Promise<HashlineRun> {
  return runHashlineWithBin(args, stdin, ctx, signal, resolveHashlineBin());
}

export async function runHashlineWithBin(
  args: string[],
  stdin: string | undefined,
  ctx: ExtensionContext,
  signal: AbortSignal | undefined,
  bin: string,
): Promise<HashlineRun> {
  const cwd = ctx.cwd || process.cwd();
  // Probe known locations on Windows only: spawn("hashline.exe") on PATH is
  // resolved by the OS; a bare relative/unknown command is not.
  if (
    process.platform === "win32" &&
    !bin.includes("\\") &&
    !bin.includes("/") &&
    !bin.endsWith(".exe")
  ) {
    for (const candidate of probeKnownLocations(process.env)) {
      if (existsSync(candidate)) {
        bin = candidate;
        break;
      }
    }
  }

  return new Promise((resolve) => {
    const child = spawn(bin, args, {
      cwd,
      signal,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", (err) => {
      resolve({ stdout, stderr: err.message, exitCode: 1 });
    });
    child.on("close", (exitCode) => {
      resolve({ stdout, stderr, exitCode: exitCode ?? 1 });
    });
    child.stdin.on("error", (err: NodeJS.ErrnoException) => {
      if (err.code !== "EPIPE" && err.code !== "ERR_STREAM_DESTROYED") {
        stderr += `
stdin error: ${err.message}`;
      }
    });
    if (stdin !== undefined && stdin.length > 0) {
      child.stdin.end(stdin);
    } else {
      child.stdin.end();
    }
  });
}

/**
 * Parse `hashline read <file> --json` stdout.
 * Shape: { "path": string, "hash": "<4hex>", "lines": [{"n": number, "hash": "<2hex>", "content": string}] }
 * (commands/read.rs; CONTRACT.md §1.)
 */
export function parseReadJson(stdout: string): ReadResult {
  const parsed = JSON.parse(stdout) as unknown;
  if (!isRecord(parsed)) {
    throw new Error("hashline read --json: expected a JSON object on stdout");
  }
  if (typeof parsed.path !== "string") {
    throw new Error('hashline read --json: missing string field "path"');
  }
  const hash = typeof parsed.hash === "string" ? parsed.hash : "";
  const rawLines = parsed.lines;
  if (!Array.isArray(rawLines)) {
    throw new Error('hashline read --json: missing array field "lines"');
  }
  const lines: ReadLine[] = [];
  for (const item of rawLines) {
    if (
      !isRecord(item) ||
      typeof item.n !== "number" ||
      typeof item.content !== "string"
    ) {
      throw new Error("hashline read --json: malformed line entry");
    }
    lines.push({
      n: item.n,
      hash: typeof item.hash === "string" ? item.hash : "",
      content: item.content,
    });
  }
  return { path: parsed.path, hash, lines };
}

/** Split a ReadResult's lines into display rows `N:hh|content`. */
export function formatReadLines(parsed: ReadResult): string[] {
  return parsed.lines.map((l) => `${l.n}:${l.hash}|${l.content}`);
}

/**
 * Parse the compact (default, 0.9.12+) patch success output:
 * `OK <path>#<4hex> edits=<n> changed=<n>` followed by
 * `~N:hh|content` / `+N:hh|content` / `-N` changed-line rows.
 * Returns null when stdout is not compact patch output.
 */
export type CompactPatchResult = {
  fileHash: string;
  editsApplied: number;
  changedCount: number;
  rows: string[];
};

export function parseCompactPatchOutput(
  stdout: string,
): CompactPatchResult | null {
  const lines = stdout.split("\n").filter((line) => line.length > 0);
  const header = lines[0]?.match(
    /^OK (.+)#([0-9a-fA-F]{4}) edits=(\d+) changed=(\d+)$/,
  );
  if (!header) {
    return null;
  }
  return {
    // Path may contain '#'; keep everything before the final 4-hex tag.
    fileHash: header[2]!,
    editsApplied: Number(header[3]),
    changedCount: Number(header[4]),
    rows: lines.slice(1),
  };
}

/**
 * Render a ReadResult as the binary's native text form: a `[<path>#<4hex>]`
 * header followed by `N:hh|content` lines. Optional offset/limit slicing
 * (wrapper-side — the binary has no pagination flags).
 */
export function formatReadPreview(
  parsed: ReadResult,
  options: { offset?: number; limit?: number; raw?: boolean } = {},
): { text: string; truncated: boolean; nextOffset?: number } {
  const allLines = parsed.lines;
  const totalLines = allLines.length;
  const startIdx = Math.max(0, (options.offset ?? 1) - 1);
  const limit = options.limit;
  const endIdx = limit ? Math.min(startIdx + limit, totalLines) : totalLines;

  if (totalLines === 0) {
    return {
      text: "[empty]",
      truncated: false,
    };
  }
  if (startIdx >= totalLines) {
    return {
      text: `[offset ${options.offset ?? 1} is beyond end of file (${totalLines} lines total)]`,
      truncated: false,
    };
  }

  const slice = allLines.slice(startIdx, endIdx);
  const header = options.raw ? undefined : `[${parsed.path}#${parsed.hash}]`;
  const body = options.raw
    ? slice.map((l) => l.content).join("\n")
    : slice.map((l) => `${l.n}:${l.hash}|${l.content}`).join("\n");
  const text = header ? `${header}\n${body}` : body;
  const truncated = endIdx < totalLines;
  return {
    text,
    truncated,
    ...(truncated ? { nextOffset: endIdx + 1 } : {}),
  };
}

/**
 * Version probe: parse `hashline X.Y.Z` from `hashline --version` stdout.
 * Returns null when the output does not match (or the command failed).
 */
export function parseHashlineVersion(
  stdout: string,
): { major: number; minor: number; patch: number } | null {
  const match = stdout.trim().match(/^hashline\s+v?(\d+)\.(\d+)\.(\d+)/);
  if (!match) {
    return null;
  }
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
}

export function isVersionAtLeast(
  actual: { major: number; minor: number; patch: number },
  minimum: string,
): boolean {
  const [maj, min, pat] = minimum.split(".").map(Number);
  const atLeast = (got: number, want: number) => got >= want;
  return (
    atLeast(actual.major, maj ?? 0) &&
    (actual.major > (maj ?? 0) || atLeast(actual.minor, min ?? 0)) &&
    (actual.major > (maj ?? 0) ||
      actual.minor > (min ?? 0) ||
      atLeast(actual.patch, pat ?? 0))
  );
}
