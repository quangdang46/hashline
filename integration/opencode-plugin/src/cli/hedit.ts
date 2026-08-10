#!/usr/bin/env bun
/**
 * hedit — thin CLI wrapper over `hashline patch` using binary-native `N:hh`
 * anchors. All hash validation and merging is done by the hashline binary.
 *
 * Usage:
 *   hedit <file> --json '<edits-json>' [--dry-run]
 *   hedit <file> --help
 *
 * The edits JSON is an array of EditOperation objects matching the
 * hashline_edit tool schema:
 *   {"op":"replace","pos":"2:bf","lines":["x"]}
 *   {"op":"append","pos":"5:da","lines":["y"]}
 *   {"op":"prepend","pos":"3:85","lines":["z"]}
 *   {"op":"delete","pos":"2:bf"} | {"op":"delete","pos":"2:bf","end":"3:1b"}
 */

import { parseArgs } from "node:util";
import { runHashline } from "../hashline-core";
import { buildPatchText, type EditOperation } from "../hashline-apply";
import { formatHashlineError, RE_READ_HINT } from "../hashline-errors";

const USAGE = `hedit — hashline edit wrapper

Usage:
  hedit <file> --json '<edits-json>' [--dry-run] [--help]

Options:
  -j, --json <str>   JSON array of EditOperation objects
  -d, --dry-run      Preview the diff without applying
  -h, --help         Show this help
`;

async function main(): Promise<void> {
  const { values, positionals } = parseArgs({
    args: process.argv.slice(2),
    options: {
      json: { type: "string", short: "j" },
      "dry-run": { type: "boolean", short: "d" },
      help: { type: "boolean", short: "h" },
    },
    allowPositionals: true,
  });

  if (values.help || positionals.length === 0 || !values.json) {
    process.stdout.write(USAGE);
    process.exit(0);
  }

  const file = positionals[0]!;
  let edits: EditOperation[];
  try {
    const parsed = JSON.parse(values.json);
    if (!Array.isArray(parsed)) throw new Error("edits must be a JSON array");
    edits = parsed as EditOperation[];
  } catch (err) {
    process.stderr.write(
      `Error: invalid --json edits: ${(err as Error).message}\n`,
    );
    process.exit(2);
  }

  const patchText = buildPatchText(edits);
  const args = ["patch", file, "-"];
  if (values["dry-run"]) args.push("--dry-run", "--json");

  let result;
  try {
    result = await runHashline(args, patchText);
  } catch {
    process.stderr.write(
      `Error: hashline binary not found. Install it (add to PATH) or set HASHLINE_BIN.\n`,
    );
    process.exit(2);
  }

  if (result.exitCode !== 0) {
    const fmt = formatHashlineError(result.stderr, result.exitCode);
    const reRead =
      fmt.kind === "stale_anchor" ||
      fmt.kind === "ambiguous_hash" ||
      fmt.kind === "hash_not_found"
        ? `\n${RE_READ_HINT}`
        : "";
    const prefix = fmt.text.startsWith("Error:") ? "" : "Error: ";
    process.stderr.write(`${prefix}${fmt.text}${reRead}\n`);
    process.exit(1);
  }

  process.stdout.write(result.stdout);
  process.exit(0);
}

void main();
