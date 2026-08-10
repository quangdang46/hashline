/**
 * format.ts — render `hashline read --json` output in the binary-native
 * `N:hh|content` format, prefixed with a `[path#4hex]` header line.
 *
 * This is PURE PRESENTATION. The hashes come from the binary; offset/limit
 * slicing happens here (the binary has no pagination flags). The rendered
 * text is byte-identical to what `hashline read <file>` prints (minus the
 * path normalization), so anchors copied out of this view are exactly what
 * `hashline patch` parses.
 */

import type { ReadResult } from "./hashline-core";

/** Options for rendering a read result. */
export interface ReadFormatOptions {
  offset?: number;
  limit?: number;
}

export interface ReadFormatResult {
  text: string;
  /** True when output was truncated by `limit`. */
  truncated: boolean;
  /** 1-based first line shown. */
  startLine: number;
  /** 1-based last line shown (inclusive). */
  endLine: number;
  /** Next offset to continue from, when truncated. */
  nextOffset?: number;
}

function anchorLine(n: number, hash: string, content: string): string {
  return `${n}:${hash}|${content}`;
}

/** Slice + render a read result in binary-native `N:hh|content` form. */
export function formatRead(
  result: ReadResult,
  opts: ReadFormatOptions = {},
): ReadFormatResult {
  const offset = opts.offset && opts.offset >= 1 ? Math.floor(opts.offset) : 1;
  const limit = opts.limit && opts.limit >= 1 ? Math.floor(opts.limit) : Number.POSITIVE_INFINITY;

  const startIdx = offset - 1;
  const sliced = result.lines.slice(startIdx, startIdx + limit);
  const total = result.lines.length;

  const header = `[${result.path}#${result.hash}]`;
  const body = sliced.map((l) => anchorLine(l.n, l.hash, l.content)).join("\n");
  const text = body.length > 0 ? `${header}\n${body}` : header;

  const startLine = sliced.length > 0 ? sliced[0]!.n : offset;
  const endLine = sliced.length > 0 ? sliced[sliced.length - 1]!.n : offset - 1;
  const truncated = offset + limit <= total; // more lines remain below
  const nextOffset = truncated ? offset + sliced.length : undefined;

  return { text, truncated, startLine, endLine, nextOffset };
}
