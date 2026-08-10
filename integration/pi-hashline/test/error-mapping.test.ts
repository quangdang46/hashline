import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { formatHashlineError, binaryNotFoundError } from "../src/result.js";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "..", "..", "..", "fixtures");

test("stale anchor pretty stderr maps to stale_anchor with re-read hint", () => {
  const text = readFileSync(join(fixtures, "stale-error.txt"), "utf8");
  const err = formatHashlineError(text, 1);
  assert.equal(err.kind, "stale_anchor");
  assert.match(err.text, /changed since last read/);
  assert.match(err.text, /Re-read the file with `read`/);
});

test("stale anchor JSON stderr maps to stale_anchor", () => {
  const stderr = JSON.stringify({
    kind: "STALE_ANCHOR",
    error:
      "line 2 content changed since last read in s.rs (expected hash f8, got 50)",
    hint: "re-read the file with `hashline read <file>`",
    command: null,
  });
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "stale_anchor");
});

test("empty patch maps to empty_patch", () => {
  const stderr =
    "Error: patch produced no edits — input was empty or all operations were rejected\nHint: verify the patch contains a valid operation (SWAP/DEL/INS.*) and re-run";
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "empty_patch");
  assert.match(err.text, /nothing to do/);
});

test("missing file maps to io", () => {
  const stderr =
    "Error: I/O error: The system cannot find the file specified. (os error 2)\nHint: check the file path and permissions, then retry the command";
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "io");
  assert.match(err.text, /I\/O error/);
});

test("binary file maps to binary_file", () => {
  const stderr =
    "Error: file 'x.dat' appears to be binary and cannot be edited safely\nHint: hashline only supports UTF-8 text files";
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "binary_file");
});

test("out of range maps to out_of_range", () => {
  const stderr = "Error: line 99 out of range in x.rs (1..4)";
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "out_of_range");
});

test("ambiguous hash maps to ambiguous_hash", () => {
  const stderr = "Error: ambiguous hash aa in x.rs (multiple matches)";
  const err = formatHashlineError(stderr, 1);
  assert.equal(err.kind, "ambiguous_hash");
  assert.match(err.text, /exact N:hh/);
});

test("binary_not_found carries the install hint", () => {
  const err = binaryNotFoundError(new Error("ENOENT"));
  assert.equal(err.kind, "binary_not_found");
  assert.match(err.text, /Install the hashline CLI/);
  assert.match(err.text, /HASHLINE_BIN/);
});
