/**
 * binary.test.ts — resolution order, spawn argv, stdout/stderr capture, and
 * abort plumbing for the spawn seam in src/hashline-core.ts. Uses an
 * injectable fake spawn; no live binary required.
 */

import { describe, expect, test } from "bun:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import {
  runHashline,
  resolveHashlineBin,
  parseReadJson,
  parseErrorPayload,
  setSpawnForTests,
  MIN_HASHLINE_VERSION,
  type SpawnSeamChild,
} from "../hashline-core";

const here = dirname(fileURLToPath(import.meta.url));
const fixturesRoot = join(
  here,
  "..",
  "..",
  "..",
  "..",
  "integration",
  "fixtures",
);

/**
 * Hand-rolled fake child that replays canned stdout/stderr and exit code
 * through real-ish event wiring. Tracks stdin written and kill calls.
 */
function makeChild(opts: {
  stdout?: string;
  stderr?: string;
  code?: number;
  spawnError?: Error;
}) {
  const { stdout = "", stderr = "", code = 0, spawnError } = opts;
  let closeCb: ((code: number | null) => void) | null = null;
  let errorCb: ((err: Error) => void) | null = null;
  let written = "";
  let killed = false;

  const stdoutDataCbs: Array<(d: string) => void> = [];
  const stderrDataCbs: Array<(d: string) => void> = [];

  const child: SpawnSeamChild = {
    stdout: {
      on: (event: string, cb: (d: string) => void) => {
        if (event === "data") stdoutDataCbs.push(cb);
      },
    } as unknown as NodeJS.ReadableStream,
    stderr: {
      on: (event: string, cb: (d: string) => void) => {
        if (event === "data") stderrDataCbs.push(cb);
      },
    } as unknown as NodeJS.ReadableStream,
    stdin: {
      write: (d: string) => {
        written += d;
        return true;
      },
      end: () => {},
    } as unknown as NodeJS.WritableStream,
    on: (event: string, cb: (...a: unknown[]) => void) => {
      if (event === "close") closeCb = cb as (code: number | null) => void;
      if (event === "error") errorCb = cb as (err: Error) => void;
      return child;
    },
    kill: () => {
      killed = true;
    },
  };

  setTimeout(() => {
    if (spawnError) {
      errorCb?.(spawnError);
      return;
    }
    for (const cb of stdoutDataCbs) cb(stdout);
    for (const cb of stderrDataCbs) cb(stderr);
    closeCb?.(code);
  }, 0);

  return { child, written: () => written, killed: () => killed };
}

describe("resolveHashlineBin", () => {
  test("HASHLINE_BIN env has highest precedence", () => {
    const old = process.env.HASHLINE_BIN;
    process.env.HASHLINE_BIN = "C:/custom/hashline.exe";
    expect(resolveHashlineBin()).toBe("C:/custom/hashline.exe");
    if (old === undefined) delete process.env.HASHLINE_BIN;
    else process.env.HASHLINE_BIN = old;
  });

  test("falls back to PATH name when env unset", () => {
    const old = process.env.HASHLINE_BIN;
    delete process.env.HASHLINE_BIN;
    expect(resolveHashlineBin()).toBe("hashline");
    if (old !== undefined) process.env.HASHLINE_BIN = old;
  });

  test("MIN_HASHLINE_VERSION is pinned to 0.9.1", () => {
    expect(MIN_HASHLINE_VERSION).toBe("0.9.1");
  });
});

describe("runHashline spawn seam", () => {
  test("passes argv verbatim and captures stdout/stderr/exitCode", async () => {
    const fake = makeChild({
      stdout: JSON.stringify({ hash: "5db5", path: "t.rs", lines: [] }),
      stderr: "diag",
      code: 0,
    });
    let seenArgs: string[] | null = null;
    setSpawnForTests((cmd, args) => {
      expect(cmd).toBe("hashline");
      seenArgs = args;
      return fake.child;
    });
    const res = await runHashline(["read", "t.rs", "--json"]);
    expect(seenArgs).toEqual(["read", "t.rs", "--json"]);
    expect(res.exitCode).toBe(0);
    expect(res.stdout).toContain("5db5");
    expect(res.stderr).toBe("diag");
    setSpawnForTests(null);
  });

  test("writes stdin payload and ends it (patch envelope form)", async () => {
    const fake = makeChild({ stdout: "", code: 0 });
    setSpawnForTests((_cmd, args) => {
      expect(args).toEqual(["patch", "t.rs", "-"]);
      return fake.child;
    });
    await runHashline(
      ["patch", "t.rs", "-"],
      "*** Begin Patch\n*** End Patch\n",
    );
    expect(fake.written()).toContain("*** Begin Patch");
    setSpawnForTests(null);
  });

  test("rejects when spawn fails (ENOENT / binary not found)", async () => {
    const fake = makeChild({ spawnError: new Error("spawn ENOENT") });
    setSpawnForTests(() => fake.child);
    await expect(runHashline(["read", "x"])).rejects.toThrow("spawn ENOENT");
    setSpawnForTests(null);
  });

  test("abort signal kills the child", async () => {
    const ac = new AbortController();
    const fake = makeChild({ stdout: "", code: 0 });
    setSpawnForTests(() => fake.child);
    const p = runHashline(["read", "x"], undefined, ac.signal);
    ac.abort();
    await p;
    expect(fake.killed()).toBe(true);
    setSpawnForTests(null);
  });
});

describe("JSON payload parsers", () => {
  test("parseReadJson parses the golden read --json fixture", () => {
    const raw = readFileSync(join(fixturesRoot, "read-json.json"), "utf8");
    const parsed = parseReadJson(raw);
    expect(parsed.hash).toBe("5db5");
    expect(parsed.lines).toHaveLength(4);
    expect(parsed.lines[1]).toEqual({
      n: 2,
      hash: "f8",
      content: "    let x = 1;",
    });
  });

  test("parseReadJson rejects a non-JSON / wrong-shape payload", () => {
    expect(() => parseReadJson("not json")).toThrow();
    expect(() => parseReadJson('{"foo":1}')).toThrow();
  });

  test("parseErrorPayload parses structured --json errors", () => {
    const p = parseErrorPayload(
      '{"kind":"STALE_ANCHOR","error":"line 2 content changed since last read in t.rs (expected hash aa, got ac)","hint":"re-read the file","command":null}',
    );
    expect(p?.kind).toBe("STALE_ANCHOR");
    expect(p?.error).toContain("expected hash aa");
  });

  test("parseErrorPayload returns null for pretty text or empty", () => {
    expect(
      parseErrorPayload("Error: line 2 content changed since last read"),
    ).toBeNull();
    expect(parseErrorPayload("")).toBeNull();
  });
});
