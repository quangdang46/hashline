/**
 * e2e.test.ts — live smoke against the real hashline binary.
 *
 * Skips when the binary is unreachable (CI without hashline). Requires the
 * binary on PATH or HASHLINE_BIN. Exercises read --json → formatRead →
 * buildPatchText → patch, and the stale-anchor error path.
 */

import { describe, expect, test } from "bun:test";
import { writeFile, rm, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runHashline } from "../hashline-core";
import { formatRead } from "../format";
import { buildPatchText } from "../hashline-apply";
import { formatHashlineError } from "../hashline-errors";

let binaryOk = false;
try {
  binaryOk = (await runHashline(["--version"])).exitCode === 0;
} catch {
  binaryOk = false;
}

const maybe = binaryOk ? describe : describe.skip;

maybe("e2e against the hashline binary", () => {
  test("read --json → formatRead → patch round trip", async () => {
    const dir = await mkdtemp(join(tmpdir(), "hl-e2e-"));
    const file = join(dir, "demo.rs");
    await writeFile(file, "fn main() {\n    let x = 1;\n}\n");

    const read = await runHashline(["read", file, "--json"]);
    expect(read.exitCode).toBe(0);
    const parsed = JSON.parse(read.stdout) as {
      lines: Array<{ n: number; hash: string; content: string }>;
    };
    const line2 = parsed.lines[1]!;
    expect(line2.content).toBe("    let x = 1;");

    const view = formatRead(parsed as never, {});
    expect(view.text).toContain("1:");
    expect(view.text).toContain(`2:${line2.hash}|    let x = 1;`);

    const patchText = buildPatchText([
      { op: "replace", pos: `2:${line2.hash}`, lines: ["    let x = 42;"] },
    ]);
    const patched = await runHashline(["patch", file, "-"], patchText, undefined, {
      cwd: dir,
    });
    expect(patched.exitCode).toBe(0);

    const verify = await runHashline(["read", file, "--json"]);
    const after = JSON.parse(verify.stdout) as {
      lines: Array<{ n: number; hash: string; content: string }>;
    };
    expect(after.lines[1]!.content).toBe("    let x = 42;");

    await rm(dir, { recursive: true, force: true });
  });

  test("stale anchor surfaces the binary's mismatch + kind stale_anchor", async () => {
    const dir = await mkdtemp(join(tmpdir(), "hl-e2e2-"));
    const file = join(dir, "demo.rs");
    await writeFile(file, "fn main() {\n    let x = 1;\n}\n");

    const patchText = buildPatchText([
      { op: "replace", pos: "2:ff", lines: ["bad"] },
    ]);
    const res = await runHashline(["patch", file, "-"], patchText, undefined, { cwd: dir });
    expect(res.exitCode).toBe(1);
    const fmt = formatHashlineError(res.stderr, res.exitCode);
    expect(fmt.kind).toBe("stale_anchor");
    expect(fmt.text).toMatch(/expected hash ff/i);

    await rm(dir, { recursive: true, force: true });
  });
});
