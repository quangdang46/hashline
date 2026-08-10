import assert from "node:assert/strict";
import test from "node:test";
import {
  BEGIN_PATCH,
  END_PATCH,
  buildPatchText,
  translateEdit,
  payloadRows,
  escapePayloadLine,
  isValidAnchor,
} from "../src/edit-args.js";

test("validates N:hh anchors", () => {
  assert.equal(isValidAnchor("1:ab"), true);
  assert.equal(isValidAnchor("12:AB"), true);
  assert.equal(isValidAnchor("123:abc1"), true);
  assert.equal(isValidAnchor("12:abcde"), false); // > 4 hex
  assert.equal(isValidAnchor("1:ag"), false); // non-hex
  assert.equal(isValidAnchor("12:abc"), true); // 3-hex accepted (binary accepts 1-4)
  assert.equal(isValidAnchor("ab:12"), false);
  assert.equal(isValidAnchor(":ab"), false);
  assert.equal(isValidAnchor(""), false);
});

test("replace single line translates to SWAP with payload rows", () => {
  assert.deepEqual(
    translateEdit({ op: "replace", pos: "4:d1", lines: ["new line"] }),
    ["SWAP 4:d1:", "+new line"],
  );
});

test("replace range translates to SWAP with inclusive range", () => {
  assert.deepEqual(
    translateEdit({
      op: "replace",
      pos: "1:ab",
      end: "3:cd",
      lines: ["a", "b", "c"],
    }),
    ["SWAP 1:ab..3:cd:", "+a", "+b", "+c"],
  );
});

test("replace with empty lines translates to DEL", () => {
  assert.deepEqual(translateEdit({ op: "replace", pos: "4:d1", lines: [] }), [
    "DEL 4:d1",
  ]);
  assert.deepEqual(
    translateEdit({ op: "replace", pos: "1:ab", end: "3:cd", lines: [] }),
    ["DEL 1:ab..3:cd"],
  );
});

test("append translates to INS.POST; append without pos to INS.TAIL", () => {
  assert.deepEqual(translateEdit({ op: "append", pos: "4:d1", lines: ["x"] }), [
    "INS.POST 4:d1:",
    "+x",
  ]);
  assert.deepEqual(translateEdit({ op: "append", lines: ["x", "y"] }), [
    "INS.TAIL:",
    "+x",
    "+y",
  ]);
});

test("prepend translates to INS.PRE; prepend without pos to INS.HEAD", () => {
  assert.deepEqual(
    translateEdit({ op: "prepend", pos: "4:d1", lines: ["x"] }),
    ["INS.PRE 4:d1:", "+x"],
  );
  assert.deepEqual(translateEdit({ op: "prepend", lines: ["x"] }), [
    "INS.HEAD:",
    "+x",
  ]);
});

test("delete translates to DEL single or range", () => {
  assert.deepEqual(translateEdit({ op: "delete", pos: "4:d1" }), ["DEL 4:d1"]);
  assert.deepEqual(translateEdit({ op: "delete", pos: "1:ab", end: "3:cd" }), [
    "DEL 1:ab..3:cd",
  ]);
});

test("payload rows prefix every line with the + marker", () => {
  assert.deepEqual(payloadRows(["plain", "+plus", "-minus", "  indented"]), [
    "+plain",
    "++plus",
    "+-minus",
    "+  indented",
  ]);
  // The row marker IS the escape: `+`+literal "+lead" = "++lead" (parsed back
  // as a literal +), and `+`+literal "-lead" = "+-lead" (parsed back as a
  // literal -). Verified against the binary with patch_format escapes.
  assert.equal(escapePayloadLine("+lead"), "++lead");
  assert.equal(escapePayloadLine("-lead"), "+-lead");
  assert.equal(escapePayloadLine("plain"), "+plain");
});

test("replace_text is not translatable by translateEdit", () => {
  assert.equal(
    translateEdit({ op: "replace_text", oldText: "a", newText: "b" }),
    null,
  );
});

test("invalid edits translate to null", () => {
  assert.equal(translateEdit({ op: "replace", lines: ["x"] }), null); // missing pos
  assert.equal(
    translateEdit({ op: "replace", pos: "nope", lines: ["x"] }),
    null,
  );
  assert.equal(translateEdit({ op: "delete" }), null); // missing pos
  assert.equal(translateEdit({ op: "unknown" as never }), null);
});

test("buildPatchText assembles a multi-op envelope", () => {
  const built = buildPatchText({
    path: "a.rs",
    edits: [
      { op: "replace", pos: "1:ab", lines: ["one"] },
      { op: "delete", pos: "2:cd" },
      { op: "append", lines: ["tail"] },
    ],
  });
  assert.ok(built.ok);
  if (built.ok) {
    assert.equal(
      built.patch,
      [
        BEGIN_PATCH,
        "SWAP 1:ab:",
        "+one",
        "DEL 2:cd",
        "INS.TAIL:",
        "+tail",
        END_PATCH,
      ].join("\n"),
    );
  }
});

test("buildPatchText rejects empty edits", () => {
  const built = buildPatchText({ path: "a.rs", edits: [] });
  assert.equal(built.ok, false);
  if (!built.ok) {
    assert.match(built.error, /empty/);
  }
});

test("buildPatchText rejects invalid edits with an index", () => {
  const built = buildPatchText({
    path: "a.rs",
    edits: [{ op: "replace", lines: ["x"] }],
  });
  assert.equal(built.ok, false);
  if (!built.ok) {
    assert.match(built.error, /edit 0/);
  }
});

test("buildPatchText rejects replace_text with a teaching error", () => {
  const built = buildPatchText({
    path: "a.rs",
    edits: [{ op: "replace_text", oldText: "a", newText: "b" }],
  });
  assert.equal(built.ok, false);
  if (!built.ok) {
    assert.match(built.error, /N:hh anchor/);
  }
});
