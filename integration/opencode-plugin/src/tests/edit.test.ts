/**
 * edit.test.ts — buildPatchText translation table, read rendering, and error
 * mapping. Pure functions; no live binary required. Golden fixtures from
 * integration/fixtures pin the binary-native shapes.
 */

import { describe, expect, test } from "bun:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import {
  buildPatchText,
  translateEdit,
  escapePayloadLine,
  buildEditTitle,
  type EditOperation,
} from "../hashline-apply";
import { formatRead } from "../format";
import { parseReadJson } from "../hashline-core";
import { formatHashlineError, mapErrorKind } from "../hashline-errors";

const here = dirname(fileURLToPath(import.meta.url));
const fixturesRoot = join(here, "..", "..", "..", "..", "integration", "fixtures");

describe("escapePayloadLine", () => {
  test("plain line gets a + prefix", () => {
    expect(escapePayloadLine("hello")).toBe("+hello");
  });
  test("empty line is a bare +", () => {
    expect(escapePayloadLine("")).toBe("+");
  });
  test("leading + is doubled (++ plus → literal +plus)", () => {
    expect(escapePayloadLine("+plus")).toBe("++plus");
  });
  test("leading - uses the +- escape (+- dash → literal -dash)", () => {
    expect(escapePayloadLine("-dash")).toBe("+--dash");
  });
  test("content that merely contains + or - is untouched", () => {
    expect(escapePayloadLine("a + b - c")).toBe("+a + b - c");
  });
});

describe("translateEdit — op to patch grammar", () => {
  test("replace single line → SWAP N:hh:", () => {
    expect(
      translateEdit({ op: "replace", pos: "5:da", lines: ["  const timeout = 3000;"] }),
    ).toEqual(["SWAP 5:da:\n+  const timeout = 3000;"]);
  });

  test("replace range → SWAP N:hh..M:aa: (both anchors in range)", () => {
    // The binary's range grammar is `N:hh..M:aa` — the start hash is
    // validated, the end hash is a hint. Verified against hashline 0.9.1.
    expect(
      translateEdit({
        op: "replace",
        pos: "4:28",
        end: "5:da",
        lines: ["x", "y"],
      }),
    ).toEqual(["SWAP 4:28..5:da:\n+x\n+y"]);
  });

  test("replace with empty lines → DEL", () => {
    expect(translateEdit({ op: "replace", pos: "5:da", lines: [] })).toEqual(["DEL 5:da"]);
  });

  test("append with pos → INS.POST N:hh:", () => {
    expect(
      translateEdit({ op: "append", pos: "5:da", lines: ["  const delay = 1000;"] }),
    ).toEqual(["INS.POST 5:da:\n+  const delay = 1000;"]);
  });

  test("append without pos → INS.TAIL:", () => {
    expect(translateEdit({ op: "append", lines: ["z"] })).toEqual(["INS.TAIL:\n+z"]);
  });

  test("prepend with pos → INS.PRE N:hh:", () => {
    expect(
      translateEdit({ op: "prepend", pos: "3:85", lines: ["// comment"] }),
    ).toEqual(["INS.PRE 3:85:\n+// comment"]);
  });

  test("prepend without pos → INS.HEAD:", () => {
    expect(translateEdit({ op: "prepend", lines: ["// header"] })).toEqual(["INS.HEAD:\n+// header"]);
  });

  test("delete single → DEL N:hh", () => {
    expect(translateEdit({ op: "delete", pos: "5:da" })).toEqual(["DEL 5:da"]);
  });

  test("delete range → DEL N:hh..M:aa", () => {
    expect(translateEdit({ op: "delete", pos: "8:b4", end: "12:f2" })).toEqual(["DEL 8:b4..12:f2"]);
  });
});

describe("buildPatchText", () => {
  test("single op is returned with trailing newline, no envelope", () => {
    const text = buildPatchText([{ op: "replace", pos: "5:da", lines: ["x"] }]);
    expect(text).toBe("SWAP 5:da:\n+x\n");
  });

  test("multi-op is wrapped in the Begin/End envelope", () => {
    const text = buildPatchText([
      { op: "delete", pos: "5:da" },
      { op: "replace", pos: "2:92", lines: ["a"] },
    ]);
    expect(text).toBe("*** Begin Patch\nDEL 5:da\nSWAP 2:92:\n+a\n*** End Patch\n");
  });

  test("empty edit list produces empty patch", () => {
    expect(buildPatchText([])).toBe("");
  });
});

describe("buildEditTitle", () => {
  test("summarizes ops for the UI title", () => {
    const title = buildEditTitle({
      path: "src/a.ts",
      edits: [
        { op: "replace", pos: "5:da", lines: ["x"] },
        { op: "append", lines: ["y"] },
        { op: "delete", pos: "8:b4", end: "12:f2" },
      ],
    });
    expect(title).toContain("src/a.ts");
    expect(title).toContain("repl 5:da");
    expect(title).toContain("app EOF");
    expect(title).toContain("del 8:b4..12");
  });
});

describe("formatRead — binary-native N:hh|content rendering", () => {
  const golden = parseReadJson(
    readFileSync(join(fixturesRoot, "read-json.json"), "utf8"),
  );

  test("renders [path#hash] header + N:hh|content lines", () => {
    const view = formatRead(golden);
    expect(view.text).toBe(
      "[C:/Users/ADMIN/AppData/Local/Temp/golden.rs#5db5]\n" +
        "1:9b|fn main() {\n" +
        "2:f8|    let x = 1;\n" +
        "3:d2|    println!(\"ok\");\n" +
        "4:88|}",
    );
    expect(view.truncated).toBe(false);
  });

  test("offset/limit slice with nextOffset when truncated", () => {
    const view = formatRead(golden, { offset: 2, limit: 2 });
    expect(view.text).toContain("2:f8|    let x = 1;");
    expect(view.text).toContain("3:d2|    println!(\"ok\");");
    expect(view.text).not.toContain("1:9b");
    expect(view.text).not.toContain("4:88");
    expect(view.startLine).toBe(2);
    expect(view.endLine).toBe(3);
    expect(view.nextOffset).toBe(4);
    expect(view.truncated).toBe(true);
  });

  test("no nextOffset when not truncated", () => {
    const view = formatRead(golden, { offset: 1, limit: 100 });
    expect(view.nextOffset).toBeUndefined();
    expect(view.truncated).toBe(false);
  });
});

describe("formatHashlineError — exit-1 taxonomy", () => {
  test("pretty-mode stale anchor is classified stale_anchor", () => {
    const stderr = readFileSync(join(fixturesRoot, "stale-error.txt"), "utf8");
    const fmt = formatHashlineError(stderr, 1);
    expect(fmt.kind).toBe("stale_anchor");
    expect(fmt.text).toContain("expected hash 5b, got 38");
  });

  test("structured --json STALE_ANCHOR", () => {
    const stderr =
      '{"kind":"STALE_ANCHOR","error":"line 2 content changed since last read in t.rs (expected hash aa, got ac)","hint":"re-read the file","command":null}';
    const fmt = formatHashlineError(stderr, 1);
    expect(fmt.kind).toBe("stale_anchor");
    expect(fmt.text).toContain("expected hash aa, got ac");
  });

  test("EMPTY_PATCH json maps to empty_patch", () => {
    const stderr =
      '{"kind":"EMPTY_PATCH","error":"patch produced no edits","hint":"verify the patch","command":null}';
    const fmt = formatHashlineError(stderr, 1);
    expect(fmt.kind).toBe("empty_patch");
    expect(fmt.text).toContain("patch produced no edits");
  });

  test("ambiguous pretty text maps to ambiguous_hash", () => {
    const fmt = formatHashlineError(
      "Error: ambiguous hash 5b — multiple lines match",
      1,
    );
    expect(fmt.kind).toBe("ambiguous_hash");
  });

  test("empty stderr falls back to io", () => {
    const fmt = formatHashlineError("", 1);
    expect(fmt.kind).toBe("io");
    expect(fmt.text).toContain("exited with code 1");
  });

  test("mapErrorKind covers the contract's kinds", () => {
    expect(mapErrorKind({ kind: "STALE_ANCHOR", error: "x" })).toBe("stale_anchor");
    expect(mapErrorKind({ kind: "NOOP_LOOP", error: "x" })).toBe("empty_patch");
    expect(mapErrorKind({ kind: "AMBIGUOUS_HASH", error: "x" })).toBe("ambiguous_hash");
    expect(mapErrorKind({ kind: "HASH_NOT_FOUND", error: "x" })).toBe("hash_not_found");
    expect(mapErrorKind({ kind: "BINARY_FILE", error: "x" })).toBe("io");
    expect(mapErrorKind({ kind: "UNKNOWN_KIND", error: "x" })).toBe("io");
  });
});

// Compile-time guard: the dialect type is exactly the four ops.
const _opCheck: EditOperation = { op: "replace", pos: "1:ab", lines: ["x"] };
void _opCheck;
