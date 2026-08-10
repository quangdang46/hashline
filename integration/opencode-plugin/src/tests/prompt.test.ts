/**
 * prompt.test.ts — the system prompt renders, and its example anchors are
 * consistent with the binary-native `N:hh` format. When a real hashline
 * binary is reachable (HASHLINE_BIN or PATH), the embedded example hashes
 * are regenerated and compared against the binary's own `read --json` —
 * this is the D.5 anchor-format consistency guard.
 */

import { describe, expect, test } from "bun:test";
import { writeFile, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  renderHashlineEditPrompt,
  formatExampleLine,
  anchorRef,
  EXAMPLE_LINES,
  EXAMPLE_HASHES,
} from "../prompt";
import { runHashline } from "../hashline-core";

describe("renderHashlineEditPrompt", () => {
  test("advertises binary-native N:hh anchors, not LINE#HASH", () => {
    const prompt = renderHashlineEditPrompt();
    expect(prompt).toContain("N:hh|content");
    expect(prompt).toContain("hashline_read");
    expect(prompt).toContain("hashline_edit");
    expect(prompt).not.toContain("LINE#HASH");
  });

  test("example anchors are well-formed N:hh", () => {
    for (let i = 1; i <= EXAMPLE_LINES.length; i++) {
      expect(formatExampleLine(i)).toMatch(/^\d+:[0-9a-f]{2}\|/);
      expect(anchorRef(i)).toMatch(/^"\d+:[0-9a-f]{2}"$/);
    }
  });

  test("the prompt contains the full example file view", () => {
    const prompt = renderHashlineEditPrompt();
    for (let i = 1; i <= EXAMPLE_LINES.length; i++) {
      expect(prompt).toContain(formatExampleLine(i));
    }
  });

  test("example hashes are valid lowercase 2-hex", () => {
    for (const h of EXAMPLE_HASHES) {
      expect(h).toMatch(/^[0-9a-f]{2}$/);
    }
    expect(EXAMPLE_HASHES).toHaveLength(EXAMPLE_LINES.length);
  });
});

/**
 * Regenerate the example hashes against the real binary and assert they
 * match the embedded static values. Skips (does not fail) when the binary is
 * unavailable, so CI without hashline still passes.
 */
describe("example anchors vs the real binary (D.5 guard)", () => {
  test("embedded example hashes match hashline read --json", async () => {
    // Probe PATH; if the binary is unavailable, skip (don't fail CI without it).
    let probe: { exitCode: number } | null = null;
    try {
      probe = await runHashline(["--version"]);
    } catch {
      probe = null;
    }
    if (!probe || probe.exitCode !== 0) {
      console.warn(
        "hashline binary not available — skipping live anchor check",
      );
      return;
    }

    const content = EXAMPLE_LINES.join("\n") + "\n";
    const dir = await mkdtemp(join(tmpdir(), "hl-prompt-"));
    const filePath = join(dir, "Counter.tsx");
    await writeFile(filePath, content);

    const res = await runHashline(["read", filePath, "--json"]);
    if (res.exitCode !== 0) {
      throw new Error(`hashline read failed: ${res.stderr}`);
    }
    const parsed = JSON.parse(res.stdout) as {
      lines: Array<{ n: number; hash: string }>;
    };
    const lines = [...parsed.lines].sort((a, b) => a.n - b.n);
    expect(lines).toHaveLength(EXAMPLE_HASHES.length);
    for (let i = 0; i < EXAMPLE_HASHES.length; i++) {
      expect(lines[i]!.hash).toBe(EXAMPLE_HASHES[i]);
    }
  });
});
