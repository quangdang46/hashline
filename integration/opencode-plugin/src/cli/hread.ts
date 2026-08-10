#!/usr/bin/env bun
/**
 * hread — thin CLI wrapper over `hashline read` in the binary-native
 * `N:hh|content` format. All hashing is done by the hashline binary.
 *
 * Usage: hread <file> [--offset|-o <n>] [--limit|-l <n>] [--json] [--help|-h]
 */

import { parseArgs } from "node:util";
import {
  runHashline,
  parseReadJson,
  resolveHashlineBin,
} from "../hashline-core";
import { formatRead } from "../format";

const USAGE = `hread — hashline read wrapper

Usage:
  hread <file> [options]

Options:
  -o, --offset <n>   First line (1-based)
  -l, --limit <n>    Max lines to show
  -j, --json         Print raw hashline read --json payload
  -h, --help         Show this help
`;

async function main(): Promise<void> {
  const { values, positionals } = parseArgs({
    args: process.argv.slice(2),
    options: {
      offset: { type: "string", short: "o" },
      limit: { type: "string", short: "l" },
      json: { type: "boolean", short: "j" },
      help: { type: "boolean", short: "h" },
    },
    allowPositionals: true,
  });

  if (values.help || positionals.length === 0) {
    process.stdout.write(USAGE);
    process.exit(0);
  }

  const file = positionals[0]!;
  const offset = values.offset ? parseInt(values.offset, 10) : undefined;
  const limit = values.limit ? parseInt(values.limit, 10) : undefined;

  let result;
  try {
    result = await runHashline(["read", file, "--json"]);
  } catch {
    process.stderr.write(
      `Error: hashline binary not found. Install it (add to PATH) or set HASHLINE_BIN.\n`,
    );
    process.exit(2);
  }

  if (result.exitCode !== 0) {
    process.stderr.write(result.stderr);
    process.exit(result.exitCode);
  }

  if (values.json) {
    process.stdout.write(result.stdout);
    process.exit(0);
  }

  let parsed;
  try {
    parsed = parseReadJson(result.stdout);
  } catch {
    process.stderr.write(
      `Error: hashline read --json returned an unexpected payload\n`,
    );
    process.exit(2);
  }

  const view = formatRead(parsed, { offset, limit });
  process.stdout.write(view.text + "\n");
  process.exit(0);
}

void main();
