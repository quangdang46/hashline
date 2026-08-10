import assert from "node:assert/strict";
import test from "node:test";
import { parseHashlineConfig } from "../src/config.js";

test("missing config file is silent defaults; invalid top-level value warns", () => {
  // A missing file never reaches parseHashlineConfig (loadConfig catches
  // ENOENT silently). Passing undefined directly is the "not an object" path.
  const { config, warnings } = parseHashlineConfig(undefined);
  assert.deepEqual(config, {
    hashLength: 2,
    grep: false,
    replaceText: true,
    binary: undefined,
  });
  assert.equal(warnings.length, 1);
  assert.match(warnings[0]!, /expected an object at top level/);
});

test("full valid config parses", () => {
  const { config, warnings } = parseHashlineConfig({
    hashLength: 4,
    grep: true,
    replaceText: false,
    binary: "/opt/hashline/bin/hashline",
  });
  assert.deepEqual(config, {
    hashLength: 4,
    grep: true,
    replaceText: false,
    binary: "/opt/hashline/bin/hashline",
  });
  assert.deepEqual(warnings, []);
});

test("invalid hashLength falls back to 2 with a warning", () => {
  const { config, warnings } = parseHashlineConfig({ hashLength: 7 });
  assert.equal(config.hashLength, 2);
  assert.equal(warnings.length, 1);
  assert.match(warnings[0]!, /"hashLength" must be 2, 3, or 4/);
});

test("non-boolean grep/replaceText fall back with warnings", () => {
  const { config, warnings } = parseHashlineConfig({
    grep: "yes",
    replaceText: 1,
  });
  assert.equal(config.grep, false);
  assert.equal(config.replaceText, true);
  assert.equal(warnings.length, 2);
});

test("non-object top level warns", () => {
  const { config, warnings } = parseHashlineConfig(42);
  assert.deepEqual(config, {
    hashLength: 2,
    grep: false,
    replaceText: true,
    binary: undefined,
  });
  assert.equal(warnings.length, 1);
});

test("empty binary string warns and is ignored", () => {
  const { config, warnings } = parseHashlineConfig({ binary: "" });
  assert.equal(config.binary, undefined);
  assert.equal(warnings.length, 1);
});
