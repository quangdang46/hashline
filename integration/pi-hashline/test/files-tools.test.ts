import assert from "node:assert/strict";
import test from "node:test";
import { translateEdit, type HashlineEdit } from "../src/edit-args.js";

test("block ops translate to binary block syntax", () => {
  assert.deepEqual(
    translateEdit({ op: "replace_block", pos: 2, lines: ["a", "b"] }),
    ["SWAP.BLK 2:", "+a", "+b"],
  );
  assert.deepEqual(translateEdit({ op: "delete_block", pos: 3 }), [
    "DEL.BLK 3",
  ]);
  assert.deepEqual(
    translateEdit({ op: "insert_block_after", pos: 1, lines: ["x"] }),
    ["INS.BLK.POST 1:", "+x"],
  );
});

test("block ops reject invalid line numbers and missing lines", () => {
  assert.equal(
    translateEdit({ op: "replace_block", pos: 0, lines: ["a"] }),
    null,
  );
  assert.equal(
    translateEdit({ op: "replace_block", pos: 1.5, lines: ["a"] }),
    null,
  );
  assert.equal(translateEdit({ op: "replace_block", pos: 2 }), null);
  assert.equal(translateEdit({ op: "delete_block", pos: -1 }), null);
  assert.equal(translateEdit({ op: "delete_block" } as HashlineEdit), null);
  assert.equal(
    translateEdit({ op: "insert_block_after", pos: 4 }) as unknown,
    null,
  );
});
