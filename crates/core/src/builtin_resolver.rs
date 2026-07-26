//! Built-in heuristic [`BlockResolver`] implementation.
//!
//! Uses brace-matching for C-family languages, indentation analysis for
//! Python/Verse, and `end`-keyword balancing for Ruby. Falls back to
//! indentation for unknown languages.
//!
//! This is the default resolver used by the CLI and the [`Editor`](crate::editor::Editor).
//! Consumers who want exact (tree-sitter / LSP) resolution can provide their own
//! [`BlockResolver`] implementation instead.

use crate::document::LineEntry;
use crate::error::HashlineError;
use crate::types::{BlockResolver, BlockResolverRequest, BlockSpan};

/// Default heuristic block resolver.
///
/// Duplicates and consolidates the inline block-resolution logic that was
/// previously duplicated across `commands::patch` and `commands::find_block`.
pub struct BuiltinBlockResolver;

impl BuiltinBlockResolver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BuiltinBlockResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockResolver for BuiltinBlockResolver {
    fn resolve(&self, request: &BlockResolverRequest) -> Option<BlockSpan> {
        let extension = std::path::Path::new(&request.path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let entries = text_to_entries(&request.text);
        let anchor_idx = request.line.wrapping_sub(1);
        if anchor_idx >= entries.len() {
            return None;
        }

        let result = match extension {
            "py" | "verse" => resolve_brace_block(&entries, anchor_idx, extension)
                .or_else(|| resolve_indent_block(&entries, anchor_idx)),
            "rb" => resolve_ruby_block(&entries, anchor_idx),
            "rs" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "c" | "cpp" | "h" | "hpp"
            | "cs" | "kt" | "kts" | "swift" | "scala" | "dart" | "zig" | "m" | "mm" => {
                resolve_brace_block(&entries, anchor_idx, extension)
            }
            _ => resolve_brace_block(&entries, anchor_idx, extension)
                .or_else(|| resolve_indent_block(&entries, anchor_idx)),
        };

        result.map(|(start, end)| BlockSpan { start, end })
    }
}

/// Convert text to line entries (for resolver use).
fn text_to_entries(text: &str) -> Vec<LineEntry> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|s| LineEntry {
            content: s.to_string(),
            short_hash: crate::hash::short_hash_value(s),
        })
        .collect()
}

/// Resolve a block by scanning braces `{}`.
fn resolve_brace_block(
    entries: &[LineEntry],
    anchor_idx: usize,
    ext: &str,
) -> Option<(usize, usize)> {
    let pairs = find_brace_pairs(entries, ext);
    pairs
        .iter()
        .filter(|(start, end)| *start <= anchor_idx && *end >= anchor_idx)
        .max_by_key(|(start, _)| *start)
        .copied()
}

fn find_brace_pairs(entries: &[LineEntry], ext: &str) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    let line_comment: &[u8] = if ext == "py" || ext == "rb" {
        b"#"
    } else {
        b"//"
    };
    let use_block_comments = ext != "py" && ext != "rb";
    let mut in_block_comment = false;

    for (line_idx, entry) in entries.iter().enumerate() {
        let bytes = entry.content.as_bytes();
        let mut i = 0;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut prev_escape = false;

        while i < bytes.len() {
            if prev_escape {
                prev_escape = false;
                i += 1;
                continue;
            }
            if (in_single_quote || in_double_quote) && bytes[i] == b'\\' {
                prev_escape = true;
                i += 1;
                continue;
            }

            if in_block_comment {
                if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && bytes[i..].starts_with(line_comment) {
                break;
            }

            if use_block_comments
                && !in_single_quote
                && !in_double_quote
                && i + 1 < bytes.len()
                && bytes[i] == b'/'
                && bytes[i + 1] == b'*'
            {
                in_block_comment = true;
                i += 2;
                continue;
            }

            if in_single_quote && bytes[i] == b'\'' {
                in_single_quote = false;
                i += 1;
                continue;
            }
            if in_double_quote && bytes[i] == b'"' {
                in_double_quote = false;
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && bytes[i] == b'\'' {
                in_single_quote = true;
                i += 1;
                continue;
            }
            if !in_single_quote && !in_double_quote && bytes[i] == b'"' {
                in_double_quote = true;
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && !in_block_comment {
                if bytes[i] == b'{' {
                    stack.push(line_idx);
                } else if bytes[i] == b'}' {
                    if let Some(s) = stack.pop() {
                        pairs.push((s, line_idx));
                    }
                }
            }

            i += 1;
        }
    }

    pairs
}

/// Resolve a block by scanning indentation.
fn resolve_indent_block(entries: &[LineEntry], anchor_idx: usize) -> Option<(usize, usize)> {
    let anchor_indent = leading_ws(&entries[anchor_idx].content);

    if anchor_indent == 0 {
        let mut end = entries.len() - 1;
        for i in (anchor_idx + 1)..entries.len() {
            let trimmed = entries[i].content.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }
            if leading_ws(&entries[i].content) <= anchor_indent {
                end = i.saturating_sub(1);
                break;
            }
        }
        Some((anchor_idx, end))
    } else {
        let start = (0..anchor_idx).rev().find(|&i| {
            let t = entries[i].content.trim();
            !t.is_empty() && leading_ws(&entries[i].content) < anchor_indent
        })?;
        let start_indent = leading_ws(&entries[start].content);
        let mut end = entries.len() - 1;
        for i in (start + 1)..entries.len() {
            let trimmed = entries[i].content.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
                continue;
            }
            if leading_ws(&entries[i].content) <= start_indent {
                end = i.saturating_sub(1);
                break;
            }
        }
        Some((start, end))
    }
}

const RUBY_OPENERS: &[&str] = &[
    "def ", "class ", "module ", "do ", "do|", "if ", "unless ", "while ", "until ", "for ",
    "begin ", "case ",
];

/// Resolve a Ruby block by matching `def/class/if...end`.
fn resolve_ruby_block(entries: &[LineEntry], anchor_idx: usize) -> Option<(usize, usize)> {
    let mut depth: isize = 0;
    let start = (0..=anchor_idx).rev().find(|&i| {
        let trimmed = entries[i].content.trim();
        let end_count = if trimmed == "end" { 1 } else { 0 };
        let open_count = ruby_opener_count(trimmed);
        depth += end_count as isize;
        depth -= open_count as isize;
        open_count > 0 && depth <= 0
    })?;

    depth = 0;
    for i in start..entries.len() {
        let trimmed = entries[i].content.trim();
        let open_count = ruby_opener_count(trimmed);
        let end_count = if trimmed == "end" { 1 } else { 0 };
        depth += open_count as isize;
        depth -= end_count as isize;
        if i > start && depth <= 0 && trimmed == "end" {
            return Some((start, i));
        }
        if i == start && depth <= 0 {
            return Some((start, i));
        }
    }
    None
}

fn ruby_opener_count(trimmed: &str) -> usize {
    for opener in RUBY_OPENERS {
        if trimmed.starts_with(opener) {
            return 1;
        }
    }
    0
}

fn leading_ws(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// Use a [`BlockResolver`] to find the syntactic block around an anchor.
///
/// Returns `(language_name, start_index, end_index)` where indices are
/// 0-based (same as `LineEntry` indexing).
///
/// This is the core logic extracted from `commands::find_block::find_block_boundaries`,
/// now driven by an injected resolver instead of duplicated inline heuristic code.
pub fn resolve_block_boundaries(
    entries: &[LineEntry],
    anchor_index: usize,
    path: &std::path::Path,
    resolver: &dyn BlockResolver,
) -> Result<(Option<String>, usize, usize), HashlineError> {
    let text: String = entries
        .iter()
        .map(|e| e.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let request = BlockResolverRequest {
        path: path.to_string_lossy().to_string(),
        text,
        line: anchor_index + 1,
    };

    let span = resolver
        .resolve(&request)
        .ok_or_else(|| HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        })?;

    if span.start >= entries.len() || span.end >= entries.len() || span.start > span.end {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = language_for_extension(extension);

    Ok((language, span.start, span.end))
}

fn language_for_extension(ext: &str) -> Option<String> {
    match ext {
        "rs" => Some("Rust".into()),
        "py" => Some("Python".into()),
        "js" => Some("JavaScript".into()),
        "ts" => Some("TypeScript".into()),
        "tsx" => Some("TSX".into()),
        "jsx" => Some("JSX".into()),
        "go" => Some("Go".into()),
        "rb" => Some("Ruby".into()),
        "verse" => Some("Verse".into()),
        "java" => Some("Java".into()),
        "c" => Some("C".into()),
        "cpp" | "hpp" => Some("C++".into()),
        "h" => Some("C/C++ Header".into()),
        "cs" => Some("C#".into()),
        "kt" | "kts" => Some("Kotlin".into()),
        "swift" => Some("Swift".into()),
        "scala" => Some("Scala".into()),
        "dart" => Some("Dart".into()),
        "zig" => Some("Zig".into()),
        "m" => Some("Objective-C".into()),
        "mm" => Some("Objective-C++".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries_from(text: &str) -> Vec<LineEntry> {
        if text.is_empty() {
            return Vec::new();
        }
        text.split('\n')
            .map(|s| LineEntry {
                content: s.to_string(),
                short_hash: crate::hash::short_hash_value(s),
            })
            .collect()
    }

    #[test]
    fn test_brace_pairs_simple() {
        let e = entries_from(
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n",
        );
        let inner = resolve_brace_block(&e, 2, "rs");
        assert_eq!(inner, Some((2, 4)));
        let outer = resolve_brace_block(&e, 0, "rs");
        assert_eq!(outer, Some((0, 5)));
    }

    #[test]
    fn test_indent_block_python_body() {
        let e = entries_from(
            "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n",
        );
        // Anchor on "    if True:" (index 2) — scans backward to "def hello():" (0)
        let actual = resolve_indent_block(&e, 2);
        assert!(actual.is_some());
        let (s, e_idx) = actual.unwrap();
        assert_eq!(s, 0, "should start at 'def hello():'");
        // The trailing \n creates an empty entry at index 5; since no
        // de-indent is found before it, end stays at entries.len()-1.
        assert_eq!(e_idx, 5, "should include trailing empty entry");
    }

    #[test]
    fn test_indent_block_python_deeply_nested() {
        let e = entries_from(
            "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n",
        );
        // Anchor on "        print('ok')" (index 3) — scans backward to "    if True:" (2)
        let actual = resolve_indent_block(&e, 3);
        assert!(actual.is_some());
        let (s, e_idx) = actual.unwrap();
        assert_eq!(s, 2, "should start at '    if True:'");
        assert_eq!(e_idx, 3, "should end at '        print(\"ok\")'");
    }

    #[test]
    fn test_ruby_block() {
        let e = entries_from("def hello\n  x = 1\n  if true\n    puts 'ok'\n  end\nend\n");
        let actual = resolve_ruby_block(&e, 1);
        assert!(actual.is_some());
        let (s, e_idx) = actual.unwrap();
        assert_eq!(s, 0);
        assert_eq!(e_idx, 5);
    }

    #[test]
    fn test_builtin_resolver_basic() {
        let resolver = BuiltinBlockResolver;
        let request = BlockResolverRequest {
            path: "test.rs".into(),
            text: "fn a() {}\nfn b() {\n    let x = 1;\n}".into(),
            line: 3,
        };
        let span = resolver.resolve(&request).unwrap();
        assert_eq!(span.start, 1);
        assert_eq!(span.end, 3);
    }

    #[test]
    fn test_resolver_unknown_language_fallback() {
        let resolver = BuiltinBlockResolver;
        let request = BlockResolverRequest {
            path: "test.txt".into(),
            text: "header\n    body\n    more\nfooter\n".into(),
            line: 2,
        };
        let span = resolver.resolve(&request);
        assert!(span.is_some());
    }

    #[test]
    fn test_resolver_out_of_range_returns_none() {
        let resolver = BuiltinBlockResolver;
        let request = BlockResolverRequest {
            path: "test.rs".into(),
            text: "fn a() {}".into(),
            line: 999,
        };
        assert!(resolver.resolve(&request).is_none());
    }
}
