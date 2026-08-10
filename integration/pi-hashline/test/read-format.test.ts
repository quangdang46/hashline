import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  formatReadLines,
  formatReadPreview,
  parseHashlineVersion,
  isVersionAtLeast,
  parseReadJson,
} from "../src/hashline.js";

const here = dirname(fileURLToPath(import.meta.url));
// Compiled tests run from .tmp-tests/test/*.test.js: `here` = package/.tmp-tests/test,
// so `..`x3 = the repo's integration/ dir, then fixtures/. Vitest running the
// same sources from test/*.test.ts lands on the identical path.
const fixtures = join(here, "..", "..", "..", "fixtures");

test("parseReadJson parses the golden fixture", () => {
  const text = readFileSync(join(fixtures, "read-json.json"), "utf8");
  const parsed = parseReadJson(text);
  assert.equal(parsed.hash, "5db5");
  assert.equal(parsed.path.endsWith("golden.rs"), true);
  assert.equal(parsed.lines.length, 4);
  assert.deepEqual(parsed.lines[1], {
    n: 2,
    hash: "f8",
    content: "    let x = 1;",
  });
});

test("formatReadLines renders binary-native N:hh|content", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const lines = formatReadLines(parsed);
  assert.deepEqual(lines, [
    "1:9b|fn main() {",
    "2:f8|    let x = 1;",
    '3:d2|    println!("ok");',
    "4:88|}",
  ]);
});

test("formatReadPreview matches the golden text fixture byte-for-byte", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const preview = formatReadPreview(parsed);
  const golden = readFileSync(join(fixtures, "read-text.txt"), "utf8");
  assert.equal(preview.text, golden.trim());
});

test("formatReadPreview slices by offset and limit", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const preview = formatReadPreview(parsed, { offset: 2, limit: 2 });
  assert.equal(
    preview.text,
    '[C:/Users/ADMIN/AppData/Local/Temp/golden.rs#5db5]\n2:f8|    let x = 1;\n3:d2|    println!("ok");',
  );
  assert.equal(preview.truncated, true);
  assert.equal(preview.nextOffset, 4);
});

test("formatReadPreview offset beyond EOF is advisory", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const preview = formatReadPreview(parsed, { offset: 99 });
  assert.match(preview.text, /beyond end of file \(4 lines total\)/);
  assert.equal(preview.truncated, false);
});

test("formatReadPreview raw mode omits anchors and header", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const preview = formatReadPreview(parsed, { raw: true });
  assert.equal(
    preview.text,
    'fn main() {\n    let x = 1;\n    println!("ok");\n}',
  );
});

test("empty file read renders [empty]", () => {
  const preview = formatReadPreview({ path: "x", hash: "abcd", lines: [] });
  assert.equal(preview.text, "[empty]");
});

test("parseHashlineVersion and version gate", () => {
  assert.deepEqual(parseHashlineVersion("hashline 0.9.1\n"), {
    major: 0,
    minor: 9,
    patch: 1,
  });
  assert.deepEqual(parseHashlineVersion("hashline v1.2.3"), {
    major: 1,
    minor: 2,
    patch: 3,
  });
  assert.equal(parseHashlineVersion("not a hashline"), null);
  assert.equal(
    isVersionAtLeast({ major: 0, minor: 9, patch: 1 }, "0.9.1"),
    true,
  );
  assert.equal(
    isVersionAtLeast({ major: 0, minor: 9, patch: 0 }, "0.9.1"),
    false,
  );
  assert.equal(
    isVersionAtLeast({ major: 1, minor: 0, patch: 0 }, "0.9.1"),
    true,
  );
});

test("golden fixture invariant: anchors are binary-native N:hh|content", () => {
  const parsed = parseReadJson(
    readFileSync(join(fixtures, "read-json.json"), "utf8"),
  );
  const lines = formatReadLines(parsed);
  for (const line of lines) {
    assert.match(line, /^\d+:[0-9a-f]{2}\|/, `binary-native anchor: ${line}`);
  }
  assert.match(lines[0]!, /^1:9b\|/);
  assert.doesNotMatch(lines[0]!, /#/); // no LINE#HASH style
});
