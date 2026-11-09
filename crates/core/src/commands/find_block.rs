use std::io::Write;

use crate::cli::FindBlockCmd;
use crate::context::CommandContext;
use crate::document::FileContent;
use crate::error::HashlineError;
use crate::hash::{format_short_hash, write_short_hash_bytes};

/// Result JSON payload for find_block.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FindBlockPayload {
    pub file: String,
    /// Total lines in the file (real lines only — excludes the phantom
    /// trailing empty split element). Kept for consumers that need the
    /// file size; the compact `lines=` header reports `block_line_count`.
    pub line_count: usize,
    /// Number of lines in the emitted block — always equals
    /// `block_lines.len()`, so the compact `lines=N` header is consistent
    /// with the N `n:hash|content` rows that follow it.
    pub block_line_count: usize,
    pub language: Option<String>,
    pub block_lines: Vec<IndexLineView>,
    /// Non-fatal resolution warning (e.g. fuzzy anchor relocation).
    /// Surfaced in-band: compact output prints a `WARN` line and JSON
    /// includes this field, instead of stderr-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// A single display line inside the block payload.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct IndexLineView {
    pub n: usize,
    pub hash: String,
    pub content: String,
}

/// CLI entry point.
pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: FindBlockCmd,
) -> Result<(), HashlineError> {
    let fc = FileContent::load(&cmd.file)?;
    let entries = fc.lines_with_hashes();
    let payload = find_block_payload(&fc, &entries, &cmd.anchor)?;

    match ctx.output_mode() {
        crate::context::OutputMode::Compact => {
            // Agent-native: compact header + block lines. `lines=` counts the
            // block lines that follow (not the whole file); `file_lines=`
            // carries the file size. A `WARN` line surfaces relocation
            // warnings in-band (they used to be stderr-only).
            let lang = payload.language.as_deref().unwrap_or("Unknown");
            writeln!(
                ctx.stdout(),
                "OK file={} lang={} lines={} file_lines={}",
                payload.file,
                lang,
                payload.block_line_count,
                payload.line_count,
            )?;
            if let Some(ref warning) = payload.warning {
                writeln!(ctx.stdout(), "WARN {warning}")?;
            }
            for line in &payload.block_lines {
                writeln!(ctx.stdout(), "{}:{}|{}", line.n, line.hash, line.content)?;
            }
        }
        crate::context::OutputMode::Verbose => {
            // Human-readable: old format
            writeln!(
                ctx.stdout(),
                "File: {}  ({} lines)",
                payload.file,
                payload.line_count
            )?;
            if let Some(ref lang) = payload.language {
                writeln!(ctx.stdout(), "Language: {lang}")?;
            }
            if let Some(ref warning) = payload.warning {
                writeln!(ctx.stdout(), "Warning: {warning}")?;
            }
            let mut hash_buf = [0u8; 2];
            for line in &payload.block_lines {
                write_short_hash_bytes(
                    &mut hash_buf,
                    u8::from_str_radix(&line.hash, 16).unwrap_or(0),
                );
                let hash_str = std::str::from_utf8(&hash_buf).unwrap_or("??");
                writeln!(ctx.stdout(), "{}:{}|{}", line.n, hash_str, line.content)?;
            }
        }
        crate::context::OutputMode::Json => {
            crate::output::write_json_success(ctx, &payload)?;
        }
        crate::context::OutputMode::Ndjson => {
            // Same as compact
            let lang = payload.language.as_deref().unwrap_or("Unknown");
            writeln!(
                ctx.stdout(),
                "OK file={} lang={} lines={} file_lines={}",
                payload.file,
                lang,
                payload.block_line_count,
                payload.line_count,
            )?;
            if let Some(ref warning) = payload.warning {
                writeln!(ctx.stdout(), "WARN {warning}")?;
            }
            for line in &payload.block_lines {
                writeln!(ctx.stdout(), "{}:{}|{}", line.n, line.hash, line.content)?;
            }
        }
    }

    Ok(())
}

/// Core logic: resolve anchor, detect language, find block.
pub fn find_block_payload(
    fc: &FileContent,
    entries: &[crate::document::LineEntry],
    anchor_str: &str,
) -> Result<FindBlockPayload, HashlineError> {
    let parsed = crate::anchor::parse_anchor(anchor_str)?;
    let resolved = resolve_find_anchor(&parsed, anchor_str, fc)?;
    let anchor_index = resolved.index;
    let warning = resolved.warning;

    let (language, block_start, block_end) =
        find_block_boundaries(entries, anchor_index, &fc.path)?;

    // `entries` can contain one phantom trailing element: `split('\n')` on a
    // newline-terminated file yields a final "" that is NOT a real line, and
    // `patch` rejects anchors on it. Clamp the block to real lines only so
    // emitted `n:hash|content` rows are always addressable.
    let real_lines = fc.len();
    let block_end = block_end.min(real_lines.saturating_sub(1));
    if block_start >= real_lines || block_start > block_end {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    let block_lines: Vec<IndexLineView> = (block_start..=block_end)
        .map(|i| IndexLineView {
            n: i + 1,
            hash: format_short_hash(entries[i].short_hash),
            content: entries[i].content.clone(),
        })
        .collect();
    let block_line_count = block_lines.len();

    Ok(FindBlockPayload {
        file: fc.path.display().to_string(),
        line_count: real_lines,
        block_line_count,
        language,
        block_lines,
        warning,
    })
}

/// Resolve the anchor for find-block, with a disambiguation fallback for
/// all-digit anchors.
///
/// `parse_anchor` treats a bare all-digit string as a line number
/// (`BlockAnchor`) so `find-block 53` means "block at line 53". That makes
/// the 100 all-digit short hashes (`00`–`99`) unexpressible as bare anchors.
/// When the "line number" is past the end of the file, retry the same two
/// characters as a hex short hash — `find-block 53` on a short file then
/// resolves hash `0x53` instead of erroring. In-range digits keep their
/// line-number meaning (the documented find-block UX); the hash remains
/// expressible via a qualified `N:hh` anchor.
fn resolve_find_anchor(
    parsed: &crate::anchor::Anchor,
    anchor_str: &str,
    fc: &FileContent,
) -> Result<crate::anchor::ResolvedLine, HashlineError> {
    let out_of_range = |resolved: &crate::anchor::ResolvedLine| {
        // A bare line number past the last REAL line is out of range —
        // including the phantom trailing entry (`entries` is one longer
        // than `fc.len()` on newline-terminated files), which `patch`
        // rejects anyway.
        matches!(parsed, crate::anchor::Anchor::BlockAnchor { .. }) && resolved.index >= fc.len()
    };
    match crate::anchor::resolve(parsed, fc) {
        Ok(resolved) if !out_of_range(&resolved) => Ok(resolved),
        Ok(_) | Err(_) => {
            if let crate::anchor::Anchor::BlockAnchor { line } = *parsed {
                let digits = anchor_str.trim();
                if line > fc.len()
                    && digits.len() == 2
                    && digits.bytes().all(|b| b.is_ascii_digit())
                {
                    if let Ok(short) = u8::from_str_radix(digits, 16) {
                        return crate::anchor::resolve(&crate::anchor::Anchor::Hash { short }, fc);
                    }
                }
            }
            // No hash fallback applies: surface the original outcome. For the
            // Ok-on-phantom case there is no stored error, so re-resolve to
            // reproduce it (it will fail the same way downstream).
            crate::anchor::resolve(parsed, fc)
        }
    }
}

/// Resolve the structural block boundaries containing the given anchor line.
///
/// Delegates to the canonical resolver in `builtin_resolver` — the same
/// engine `patch`'s block ops and the library Editor use — so all three
/// paths agree on the span for a given file.
pub fn find_block_boundaries(
    entries: &[crate::document::LineEntry],
    anchor_index: usize,
    path: &std::path::Path,
) -> Result<(Option<String>, usize, usize), HashlineError> {
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    let resolved = crate::builtin_resolver::resolve_span(entries, anchor_index, path);
    let (block_start, block_end) = match resolved {
        Some(span) => span,
        // Verse historically fell back to the whole file when no block
        // structure was found; preserve that last-resort behavior here.
        None if extension == "verse" => (0, entries.len().saturating_sub(1)),
        None => {
            return Err(HashlineError::UnbalancedBlock {
                line_no: anchor_index + 1,
            });
        }
    };

    if block_start >= entries.len() || block_end >= entries.len() || block_start > block_end {
        return Err(HashlineError::UnbalancedBlock {
            line_no: anchor_index + 1,
        });
    }

    // `language` is `Some("Unknown")` for extensions without a mapping —
    // matching the historical output shape.
    let language = Some(
        crate::builtin_resolver::language_for_extension(extension)
            .unwrap_or_else(|| "Unknown".into()),
    );

    Ok((language, block_start, block_end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_fc(content: &str, name: &str) -> (FileContent, Vec<crate::document::LineEntry>) {
        let fc = FileContent {
            path: Path::new(name).to_path_buf(),
            raw: content.to_string(),
            normalized: content.to_string(),
            bom: String::new(),
            newline: crate::document::NewlineStyle::Lf,
            trailing_newline: content.ends_with('\n'),
            hash: "0000".into(),
        };
        let entries = fc.lines_with_hashes();
        (fc, entries)
    }

    fn anchor_for(line: usize, entries: &[crate::document::LineEntry]) -> String {
        let hash = format_short_hash(entries[line.saturating_sub(1)].short_hash);
        format!("{line}:{hash}")
    }

    #[test]
    fn test_brace_pairs_simple() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let (_, entries) = make_fc(content, "test.rs");
        let pairs = crate::builtin_resolver::brace_pairs_for_ext(&entries, "rs");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], (2, 4));
        assert_eq!(pairs[1], (0, 5));
    }

    #[test]
    fn test_find_brace_block_resolve() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let (_, entries) = make_fc(content, "test.rs");
        let (s, e) =
            crate::builtin_resolver::resolve_span(&entries, 2, Path::new("test.rs")).unwrap();
        assert_eq!(s, 2);
        assert_eq!(e, 4);
    }

    #[test]
    fn test_brace_block_rust_function() {
        let content =
            "fn hello() {\n    let x = 1;\n    if true {\n        println!(\"ok\");\n    }\n}\n";
        let (fc, entries) = make_fc(content, "test.rs");
        let payload = find_block_payload(&fc, &entries, &anchor_for(3, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Rust"));
        assert_eq!(payload.block_lines.len(), 3);
        assert!(payload.block_lines[0].content.contains("if true"));
    }

    #[test]
    fn test_python_block() {
        let content = "def hello():\n    x = 1\n    if True:\n        print('ok')\n    return x\n";
        let (fc, entries) = make_fc(content, "test.py");
        let payload = find_block_payload(&fc, &entries, &anchor_for(4, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Python"));
        assert_eq!(payload.block_lines.len(), 2);
        assert!(payload.block_lines[0].content.contains("if True"));
    }

    #[test]
    fn test_ruby_block() {
        let content = "def hello\n  x = 1\n  if true\n    puts 'ok'\n  end\nend\n";
        let (fc, entries) = make_fc(content, "test.rb");
        let payload = find_block_payload(&fc, &entries, &anchor_for(2, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Ruby"));
        assert_eq!(payload.block_lines.len(), 6);
    }

    #[test]
    fn test_error_missing_anchor() {
        let (fc, entries) = make_fc("fn a() {}\n", "test.rs");
        let result = find_block_payload(&fc, &entries, "99:ff");
        assert!(result.is_err());
    }

    #[test]
    fn test_brace_detection_skips_strings() {
        let content = "fn hello() {\n    let s = \"{\";\n}\n";
        let (fc, entries) = make_fc(content, "test.rs");
        let payload = find_block_payload(&fc, &entries, &anchor_for(2, &entries)).unwrap();
        assert_eq!(payload.block_lines.len(), 3);
        assert_eq!(payload.block_lines[2].content, "}");
    }
    #[test]

    fn test_brace_detection_skips_javascript_template_literals() {
        let content = "function greet(name) {\n  return `Hello, ${name}!`;\n}\n";
        let (fc, entries) = make_fc(content, "test.js");
        let payload = find_block_payload(&fc, &entries, &anchor_for(2, &entries)).unwrap();
        assert_eq!(payload.block_lines.len(), 3);
        assert_eq!(payload.block_lines[0].content, "function greet(name) {");
        assert_eq!(payload.block_lines[2].content, "}");
    }

    #[test]
    fn test_verse_top_level_using() {
        let content = "using { /Verse.org/Simulation }\nusing { /Fortnite.com/Devices }\n\nbase_x := class():\n    x:int = 0\n";
        let (fc, entries) = make_fc(content, "test.verse");
        // Anchor on the top-level `using` should not fail.
        let payload = find_block_payload(&fc, &entries, &anchor_for(1, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Verse"));
        assert!(!payload.block_lines.is_empty());
    }

    #[test]
    fn test_verse_class_body() {
        let content = "using { /Verse.org/Simulation }\n\nsignal_device := class(creative_device):\n    var Count:int = 0\n    OnBegin<override>()<suspends>:void=\n        Count += 1\n        Print(\"ok\")\n";
        let (fc, entries) = make_fc(content, "test.verse");
        // Anchor inside the class body (the var Count line) should resolve
        // to the enclosing class block.
        let payload = find_block_payload(&fc, &entries, &anchor_for(4, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Verse"));
        assert!(
            payload
                .block_lines
                .iter()
                .any(|l| l.content.starts_with("signal_device"))
        );
    }

    #[test]
    fn test_go_block() {
        let content = "func main() {\n\tif true {\n\t\tfmt.Println(\"ok\")\n\t}\n}\n";
        let (fc, entries) = make_fc(content, "test.go");
        let payload = find_block_payload(&fc, &entries, &anchor_for(3, &entries)).unwrap();
        assert_eq!(payload.language.as_deref(), Some("Go"));
        assert_eq!(payload.block_lines.len(), 3);
        assert!(payload.block_lines[0].content.contains("if true"));
    }

    /// Regression: line_count must match fc.len(), not entries.len(), so files
    /// with a trailing newline do not report an off-by-one count (Issue #90).
    #[test]
    fn test_line_count_no_trailing_newline() {
        let content = "line1\nline2\nline3";
        let (fc, entries) = make_fc(content, "test.txt");
        let payload = find_block_payload(&fc, &entries, &anchor_for(1, &entries)).unwrap();
        assert!(!fc.trailing_newline);
        assert_eq!(fc.len(), 3);
        // When there is no trailing newline, entries and fc.len() happen to
        // agree, but we assert the semantic field regardless.
        assert_eq!(payload.line_count, fc.len());
        assert_eq!(payload.line_count, 3);
    }

    #[test]
    fn test_line_count_with_trailing_newline() {
        let content = "line1\nline2\nline3\n";
        let (fc, entries) = make_fc(content, "test.txt");
        let payload = find_block_payload(&fc, &entries, &anchor_for(1, &entries)).unwrap();
        assert!(fc.trailing_newline);
        assert_eq!(fc.len(), 3);
        // entries.len() would be 4 here (includes the empty split tail).
        // The fix ensures line_count comes from fc.len().
        assert_eq!(payload.line_count, fc.len());
        assert_eq!(payload.line_count, 3);
    }

    // -- bd-find-block-quality-vff regressions ----------------------------

    /// The phantom trailing "" split element must never appear in
    /// `block_lines` — `patch` rejects anchors on it. A Python block that
    /// runs to EOF previously included it (end = entries.len()-1).
    #[test]
    fn test_no_phantom_trailing_line_in_block() {
        let content = "def f():\n    pass\n";
        let (fc, entries) = make_fc(content, "test.py");
        assert_eq!(entries.len(), fc.len() + 1); // phantom present in entries
        let payload = find_block_payload(&fc, &entries, &anchor_for(1, &entries)).unwrap();
        assert_eq!(payload.block_lines.len(), 2);
        assert_eq!(payload.block_lines[0].n, 1);
        assert_eq!(payload.block_lines[1].n, 2);
        assert!(
            payload
                .block_lines
                .iter()
                .all(|l| !l.content.is_empty() || l.n <= fc.len())
        );
        assert_eq!(payload.block_line_count, payload.block_lines.len());
    }

    /// `lines=` semantics: header count is the BLOCK line count, not the
    /// file line count; `line_count`/`file_lines` keep the file size.
    #[test]
    fn test_block_line_count_differs_from_file_line_count() {
        let content = "fn a() {\n}\nfn b() {\n}\n";
        let (fc, entries) = make_fc(content, "test.rs");
        let payload = find_block_payload(&fc, &entries, &anchor_for(1, &entries)).unwrap();
        assert_eq!(payload.line_count, 4);
        assert_eq!(payload.block_line_count, 2);
        assert_eq!(payload.block_lines.len(), 2);
    }

    /// Fuzzy relocation inside the radius surfaces a warning in-band via
    /// the payload (previously stderr-only).
    #[test]
    fn test_relocation_warning_in_payload() {
        let content = "alpha\nbeta\ngamma\n";
        let (fc, entries) = make_fc(content, "test.txt");
        // Hash of line 2 used with line number 1 → distance 1 relocation.
        let hash = format_short_hash(entries[1].short_hash);
        let anchor = format!("1:{hash}");
        let payload = find_block_payload(&fc, &entries, &anchor).unwrap();
        let warning = payload.warning.expect("relocation must warn in-band");
        assert!(warning.contains("relocated"), "warning: {warning}");
    }

    /// Fuzzy relocation beyond the radius now fails closed (StaleAnchor)
    /// instead of returning OK for a block 100s of lines away.
    #[test]
    fn test_far_relocation_fails_closed() {
        let content = (1..=12)
            .map(|i| format!("unique findblock content {i} quux"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let (fc, entries) = make_fc(&content, "test.txt");
        let hash = format_short_hash(entries[0].short_hash); // line 1
        let anchor = format!("10:{hash}"); // distance 9 > radius 3
        let err = find_block_payload(&fc, &entries, &anchor).unwrap_err();
        assert!(matches!(err, HashlineError::StaleAnchor { .. }));
    }

    /// All-digit hashes (00–99) were unexpressible as bare anchors: `53`
    /// parsed as line 53. When the "line number" is past EOF we now fall
    /// back to interpreting the two digits as a hex short hash.
    #[test]
    fn test_digit_anchor_falls_back_to_hash_past_eof() {
        // Fixed 60-line file; find a line whose short hash is an all-digit
        // two-char string whose DECIMAL value exceeds 60 (i.e. "61"–"99") —
        // past EOF as a line number, so the digit→hash fallback applies.
        let content = (1..=60)
            .map(|i| format!("digit fallback probe line {i} waldo"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let fc = FileContent {
            path: Path::new("probe.txt").to_path_buf(),
            raw: content.clone(),
            normalized: content,
            bom: String::new(),
            newline: crate::document::NewlineStyle::Lf,
            trailing_newline: true,
            hash: "0000".into(),
        };
        let entries = fc.lines_with_hashes();
        let (line_no, hash_str) = entries
            .iter()
            .enumerate()
            .take(fc.len())
            .find_map(|(idx, e)| {
                let h = format_short_hash(e.short_hash);
                // Unique across ALL entries (incl. the phantom tail, which
                // hash resolution also scans) to avoid AmbiguousHash.
                let unique = entries
                    .iter()
                    .filter(|x| x.short_hash == e.short_hash)
                    .count()
                    == 1;
                (unique
                    && h.len() == 2
                    && h.bytes().all(|b| b.is_ascii_digit())
                    && h.parse::<usize>().unwrap() > fc.len())
                .then(|| (idx + 1, h))
            })
            .expect("fixture should contain an all-digit hash past EOF");

        let payload = find_block_payload(&fc, &entries, &hash_str).unwrap();
        // The block must contain the line carrying that hash.
        assert!(
            payload.block_lines.iter().any(|l| l.n == line_no),
            "hash {hash_str} should resolve to line {line_no}; got {:?}",
            payload.block_lines.iter().map(|l| l.n).collect::<Vec<_>>()
        );
    }

    /// Same fallback on a tiny file: pick a two-digit hash absent from the
    /// file, past EOF as a line number. The error must be a hash-resolution
    /// error (HashNotFound etc.), proving the digits were tried as a hash —
    /// not InvalidAnchor, which is what bare "53" produced before.
    #[test]
    fn test_digit_anchor_past_eof_missing_hash() {
        let (fc, entries) = make_fc("a\nb\nc\n", "tiny.txt");
        let present: std::collections::HashSet<u8> = entries.iter().map(|e| e.short_hash).collect();
        let absent = (0u8..=99)
            .map(|v| format!("{v:02x}"))
            .find(|h| {
                h.bytes().all(|b| b.is_ascii_digit())
                    && !present.contains(&u8::from_str_radix(h, 16).unwrap())
            })
            .expect("some all-digit hash is absent");
        let err = find_block_payload(&fc, &entries, &absent).unwrap_err();
        assert!(
            matches!(
                err,
                HashlineError::HashNotFound { .. }
                    | HashlineError::StaleAnchor { .. }
                    | HashlineError::AmbiguousHash { .. }
            ),
            "expected hash-resolution error for {absent}, got {err:?}"
        );
    }

    /// In-range digits keep line-number semantics (documented find-block UX).
    #[test]
    fn test_digit_anchor_in_range_stays_line_number() {
        let content = "one\ntwo\nthree\nfour\nfive\n";
        let (fc, entries) = make_fc(content, "test.txt");
        let payload = find_block_payload(&fc, &entries, "2").unwrap();
        assert!(payload.block_lines.iter().any(|l| l.n == 2));
        assert!(payload.warning.is_none());
    }
}
