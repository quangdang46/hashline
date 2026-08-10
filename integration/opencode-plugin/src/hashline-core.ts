/**
 * hashline-core.ts — binary discovery + subprocess glue for the hashline CLI.
 *
 * This package is a THIN WRAPPER: it never reimplements hashing, staleness
 * detection, or merge recovery. The hashline Rust binary (`crates/core`,
 * v0.9.1) is the single source of truth for hashes and edit validation.
 * Everything here is pure subprocess plumbing: resolve the binary, spawn it,
 * and parse its stdout/JSON payloads.
 *
 * CLI contract pinned by `integration/CONTRACT.md` and the golden fixtures
 * in `integration/fixtures/`. If the contract shape changes, bump
 * `MIN_HASHLINE_VERSION`.
 */

import { spawn } from "node:child_process";

/** Minimum hashline binary version this package is compatible with. */
export const MIN_HASHLINE_VERSION = "0.9.1";

/** Result of a spawned hashline invocation. */
export interface HashlineRunResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

/** Minimal child-process shape the seam exposes (a subset of child_process.ChildProcess). */
export interface SpawnSeamChild {
  stdout: NodeJS.ReadableStream | null;
  stderr: NodeJS.ReadableStream | null;
  stdin: NodeJS.WritableStream | null;
  on: (event: string, cb: (...args: unknown[]) => void) => void;
  kill?: (signal?: NodeJS.Signals | number) => void;
}

/**
 * Injectable spawn seam. Defaults to `child_process.spawn`; tests inject a
 * fake (or set HASHLINE_BIN to a fixture stub binary).
 */
export type SpawnFn = (
  cmd: string,
  args: string[],
  options: { cwd?: string; env?: NodeJS.ProcessEnv },
) => SpawnSeamChild;

let overriddenSpawn: SpawnFn | null = null;

/** Test-only: inject a fake spawn implementation. */
export function setSpawnForTests(fn: SpawnFn | null): void {
  overriddenSpawn = fn;
}

function defaultSpawn(
  cmd: string,
  args: string[],
  options: { cwd?: string; env?: NodeJS.ProcessEnv },
): SpawnSeamChild {
  const child = spawn(cmd, args, {
    cwd: options.cwd,
    env: options.env,
    stdio: ["pipe", "pipe", "pipe"],
    shell: false,
  });
  return child as unknown as SpawnSeamChild;
}

/**
 * Spawn the hashline binary with `args`, optionally writing `stdinText` to
 * stdin (the `patch <file> -` envelope form), and drain stdout+stderr to EOF.
 * Resolves with the captured streams and exit code. Rejects only when the
 * binary could not be spawned at all (e.g. ENOENT — binary not on PATH).
 */
export function runHashline(
  args: string[],
  stdinText?: string,
  signal?: AbortSignal,
  opts?: { cwd?: string; env?: NodeJS.ProcessEnv; hashlineBin?: string },
): Promise<HashlineRunResult> {
  const bin = opts?.hashlineBin ?? resolveHashlineBin();
  const spawnFn = overriddenSpawn ?? defaultSpawn;

  return new Promise<HashlineRunResult>((resolve, reject) => {
    const child = spawnFn(bin, args, {
      cwd: opts?.cwd ?? process.cwd(),
      env: opts?.env ?? process.env,
    });

    let stdout = "";
    let stderr = "";

    child.stdout?.on("data", (chunk: Buffer | string) => {
      stdout += chunk.toString("utf8");
    });
    child.stderr?.on("data", (chunk: Buffer | string) => {
      stderr += chunk.toString("utf8");
    });

    let settled = false;
    const done = (code: number | null, err?: Error) => {
      if (settled) return;
      settled = true;
      if (err) {
        reject(err);
        return;
      }
      resolve({ stdout, stderr, exitCode: code ?? -1 });
    };

    child.on("close", (code: unknown) =>
      done(typeof code === "number" ? code : null),
    );
    child.on("error", (err: unknown) => done(null, err as Error));

    if (stdinText !== undefined) {
      child.stdin?.write(stdinText);
    }
    child.stdin?.end();

    if (signal) {
      if (signal.aborted) {
        child.kill?.();
      } else {
        signal.addEventListener("abort", () => child.kill?.(), { once: true });
      }
    }
  });
}

/** Resolve the hashline binary: HASHLINE_BIN env → PATH → Windows probes. */
export function resolveHashlineBin(): string {
  const fromEnv = process.env.HASHLINE_BIN;
  if (fromEnv && fromEnv.trim().length > 0) {
    return fromEnv.trim();
  }
  return "hashline";
}

/**
 * Probe `hashline --version` and return the full version banner (e.g.
 * `hashline 0.9.1`), or null when the binary is missing/unparseable. Never
 * throws — callers degrade to a warning.
 */
export async function probeHashlineVersion(opts?: {
  signal?: AbortSignal;
}): Promise<string | null> {
  try {
    const res = await runHashline(["--version"], undefined, opts?.signal);
    const m = /hashline\s+\d+\.\d+\.\d+/.exec(res.stdout.trim());
    return m ? m[0] : null;
  } catch {
    return null;
  }
}

// ─── JSON payload parsers (CLI contract A.3) ────────────────────────────────

/** `read --json` payload. `n` is 1-based; no phantom trailing line. */
export interface ReadLine {
  n: number;
  hash: string;
  content: string;
}

export interface ReadResult {
  path: string;
  hash: string;
  lines: ReadLine[];
}

export function parseReadJson(raw: string): ReadResult {
  const parsed = JSON.parse(raw) as ReadResult;
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    typeof parsed.path !== "string" ||
    typeof parsed.hash !== "string" ||
    !Array.isArray(parsed.lines)
  ) {
    throw new Error("hashline read --json returned an unexpected shape");
  }
  return parsed;
}

/** `patch --json` success payload. Lines use key `line` and include a phantom trailing empty line when the file ends with `\n`. */
export interface PatchLine {
  line: number;
  hash: string;
  content: string;
}

export interface PatchSuccessResult {
  success: boolean;
  file: string;
  edits_applied: number;
  lines: PatchLine[];
}

/** `patch --json --dry-run` payload. */
export interface PatchDryRunResult {
  success: boolean;
  file: string;
  dry_run: true;
  edits_applied: number;
  diff: string[];
}

/** Binary's structured error payload (stderr, `--json` mode). */
export interface ErrorPayload {
  kind: string;
  error: string;
  hint?: string;
  command?: string | null;
}

export function parseErrorPayload(stderr: string): ErrorPayload | null {
  const trimmed = stderr.trim();
  if (trimmed.length === 0) return null;
  try {
    const parsed = JSON.parse(trimmed) as ErrorPayload;
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof parsed.kind === "string"
    ) {
      return parsed;
    }
    return null;
  } catch {
    return null;
  }
}
