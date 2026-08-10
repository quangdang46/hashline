/**
 * Optional TUI render helpers. Kept deliberately small — the model-facing
 * content lives in plain text; these only wrap it for display in the pi UI.
 */

import { Markdown, Text, truncateToWidth } from "@earendil-works/pi-tui";
import { renderDiff } from "@earendil-works/pi-coding-agent";

/** Reusable Text component (pi CachedComponent pattern). */
export function reuseText(lastComponent: unknown): Text {
  return lastComponent instanceof Text ? lastComponent : new Text("", 0, 0);
}

/** Reusable Markdown component with the given theme. */
export function reuseMarkdown(
  lastComponent: unknown,
  theme: unknown,
): Markdown {
  if (lastComponent instanceof Markdown) {
    return lastComponent;
  }
  // Markdown requires a theme; renderDiff's native theme is created by the
  // caller when one is available. When absent, fall back to a plain Text.
  return new Markdown("", 0, 0, theme as never);
}

/** Render a diff string via pi's native renderDiff; raw fallback for tests. */
export function renderDiffLines(
  diffText: string | undefined,
  filePath: string | undefined,
): string[] {
  if (!diffText) {
    return [];
  }
  try {
    return renderDiff(diffText, { filePath }).split("\n");
  } catch {
    return diffText.split("\n");
  }
}

/** ANSI-safe width truncation of a single display line (tabs expanded). */
export function truncateDisplayLine(line: string, width: number): string {
  return truncateToWidth(line.replace(/\t/g, "   "), width, "…");
}
