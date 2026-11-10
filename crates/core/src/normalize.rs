//! Text normalization helpers: line-ending detection, CRLF→LF normalization,
//! BOM stripping. The patcher uses these to canonicalize text to LF before
//! applying edits and to restore the original shape on write-back.

/// Line ending style.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    Crlf,
    /// Bare CR — the classic Mac OS line ending. `normalize_to_lf` already
    /// maps every `\r` to `\n`, so CR-only files round-trip through the
    /// LF-normalized representation; this variant lets write-back restore
    /// the original terminator instead of silently converting the file to LF.
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// Detect the **dominant** line ending style in `content`.
///
/// Every terminator is counted — `\n`, `\r\n`, and bare `\r` — and the style
/// with the most occurrences wins. Ties resolve to the style that appears
/// *first* in the file, which preserves the historical first-ending-wins
/// behavior for uniform files while no longer letting a single stray `\r\n`
/// (or `\r`) at the top of a mixed file hijack the whole write-back. Files
/// with no terminator at all default to LF.
///
/// Note this is a *canonicalization* choice, not per-line preservation: a
/// mixed-EOL file is rewritten with one dominant ending throughout (see the
/// `bd-eol-bom-u7h` design note). Per-line endings of untouched lines are
/// not preserved — tracking them through the edit engine would require a
/// line-level provenance map the patcher does not maintain.
pub fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
    let mut first = LineEnding::Lf;
    let mut seen = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    crlf += 1;
                    if !seen {
                        first = LineEnding::Crlf;
                        seen = true;
                    }
                    i += 2;
                } else {
                    cr += 1;
                    if !seen {
                        first = LineEnding::Cr;
                        seen = true;
                    }
                    i += 1;
                }
            }
            b'\n' => {
                lf += 1;
                if !seen {
                    first = LineEnding::Lf;
                    seen = true;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    // `first` doubles as the LF default when the file has no terminator.
    let mut best = first;
    let mut best_count = match best {
        LineEnding::Lf => lf,
        LineEnding::Crlf => crlf,
        LineEnding::Cr => cr,
    };
    // Strict `>` keeps the first-seen style on ties.
    for (ending, count) in [
        (LineEnding::Lf, lf),
        (LineEnding::Crlf, crlf),
        (LineEnding::Cr, cr),
    ] {
        if count > best_count {
            best = ending;
            best_count = count;
        }
    }
    best
}

/// `true` when `content` uses more than one line-ending style (`\n`,
/// `\r\n`, bare `\r`) with at least one terminator of each. Write-back
/// paths canonicalize to the *dominant* ending ([`detect_line_ending`]),
/// so a mixed-EOL file loses its per-line endings on rewrite — callers
/// surface this as a warning rather than silently rewriting
/// (bd-write-safety-batch-sz62). Per-line EOL provenance is not tracked
/// through the edit engine; preserving it would require a line-level map
/// the patcher does not maintain.
pub fn has_mixed_line_endings(content: &str) -> bool {
    let bytes = content.as_bytes();
    let (mut lf, mut crlf, mut cr) = (false, false, false);
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    crlf = true;
                    i += 2;
                } else {
                    cr = true;
                    i += 1;
                }
            }
            b'\n' => {
                lf = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    [lf, crlf, cr].iter().filter(|&&b| b).count() > 1
}

/// Probe raw file content for its BOM marker and dominant line-ending
/// style. Overwrite paths (`write --force`, `Editor::write`) use this to
/// avoid silently changing an existing file's encoding conventions.
/// Returns `("", LineEnding::Lf)` semantics are the caller's default for
/// missing/unreadable files — this function only inspects text it is given.
pub fn detect_file_style(content: &str) -> (String, LineEnding) {
    let bom = if content.starts_with('\u{FEFF}') {
        "\u{FEFF}".to_owned()
    } else {
        String::new()
    };
    (bom, detect_line_ending(content))
}

/// Normalize every line ending to LF. Handles bare CR as well.
///
/// Byte-wise rather than `char`-wise: `'\r'`/`'\n'` are ASCII and can never
/// appear inside a multi-byte UTF-8 sequence, so copying bytes verbatim is
/// exact — and roughly an order of magnitude faster than decoding chars.
/// Files with no `'\r'` at all (the common case) skip the pass entirely.
pub fn normalize_to_lf(text: &str) -> String {
    let bytes = text.as_bytes();
    if memchr::memchr(b'\r', bytes).is_none() {
        return text.to_owned();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            out.push(b'\n');
            i += 1;
            // Skip the '\n' of a "\r\n" pair — it is consumed by the '\r'.
            if i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // SAFETY: `text` is valid UTF-8 and we only replace ASCII `'\r'` with
    // `'\n'` and delete ASCII `'\n'` bytes — neither byte can occur inside
    // a multi-byte sequence — so `out` is valid UTF-8.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Re-encode LF text with the requested line ending.
///
/// `text` must be LF-normalized (no `\r` bytes): every `\n` is a line
/// separator, so a wholesale replace is exact for all three styles.
pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_owned(),
        LineEnding::Crlf => text.replace('\n', "\r\n"),
        LineEnding::Cr => text.replace('\n', "\r"),
    }
}

/// Result of stripping a leading UTF-8 BOM.
pub struct BomResult {
    pub bom: String,
    pub text: String,
}

/// Strip a UTF-8 BOM if present.
pub fn strip_bom(content: &str) -> BomResult {
    if content.starts_with('\u{FEFF}') {
        let bom_len = '\u{FEFF}'.len_utf8();
        BomResult {
            bom: "\u{FEFF}".to_owned(),
            text: content[bom_len..].to_owned(),
        }
    } else {
        BomResult {
            bom: String::new(),
            text: content.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_lf() {
        assert_eq!(detect_line_ending("hello\nworld\n"), LineEnding::Lf);
    }

    #[test]
    fn test_detect_crlf() {
        assert_eq!(detect_line_ending("hello\r\nworld\r\n"), LineEnding::Crlf);
    }

    #[test]
    fn test_detect_default_lf() {
        assert_eq!(detect_line_ending("hello world"), LineEnding::Lf);
    }

    #[test]
    fn test_normalize_to_lf() {
        assert_eq!(normalize_to_lf("a\r\nb\nc\r"), "a\nb\nc\n");
    }

    #[test]
    fn test_strip_bom_present() {
        let result = strip_bom("\u{FEFF}hello\n");
        assert_eq!(result.bom, "\u{FEFF}");
        assert_eq!(result.text, "hello\n");
    }

    #[test]
    fn test_strip_bom_absent() {
        let result = strip_bom("hello\n");
        assert_eq!(result.bom, "");
        assert_eq!(result.text, "hello\n");
    }

    #[test]
    fn test_restore_line_endings() {
        assert_eq!(
            restore_line_endings("a\nb\n", LineEnding::Crlf),
            "a\r\nb\r\n"
        );
        assert_eq!(restore_line_endings("a\nb\n", LineEnding::Lf), "a\nb\n");
    }

    // -- bd-eol-bom-u7h ----------------------------------------------------

    #[test]
    fn test_detect_dominant_mixed_lf_first() {
        // CRLF appears first but LF is dominant (2 vs 1).
        assert_eq!(detect_line_ending("a\r\nb\nc\n"), LineEnding::Lf);
        // CRLF dominant (2 vs 1).
        assert_eq!(detect_line_ending("a\r\nb\r\nc\n"), LineEnding::Crlf);
    }

    #[test]
    fn test_detect_pure_cr() {
        assert_eq!(detect_line_ending("a\rb\rc\r"), LineEnding::Cr);
        assert_eq!(detect_line_ending("a\rb"), LineEnding::Cr);
        // A trailing lone '\r' counts as a CR terminator.
        assert_eq!(detect_line_ending("a\r"), LineEnding::Cr);
    }

    #[test]
    fn test_detect_tie_prefers_first_seen() {
        // One of each: the first terminator in the file wins the tie.
        assert_eq!(detect_line_ending("a\nb\r\nc\rd"), LineEnding::Lf);
        assert_eq!(detect_line_ending("a\r\nb\nc\rd"), LineEnding::Crlf);
        assert_eq!(detect_line_ending("a\rb\nc\r\nd"), LineEnding::Cr);
    }

    #[test]
    fn test_detect_no_terminator_defaults_lf() {
        assert_eq!(detect_line_ending(""), LineEnding::Lf);
        assert_eq!(detect_line_ending("no newlines here"), LineEnding::Lf);
    }

    #[test]
    fn test_restore_cr_roundtrip() {
        let original = "alpha\rbeta\r";
        let normalized = normalize_to_lf(original);
        assert_eq!(normalized, "alpha\nbeta\n");
        assert_eq!(restore_line_endings(&normalized, LineEnding::Cr), original);
        // And LF restore of the same normalized text.
        assert_eq!(
            restore_line_endings(&normalized, LineEnding::Lf),
            normalized
        );
    }

    #[test]
    fn test_detect_file_style() {
        let (bom, ending) = detect_file_style("\u{FEFF}a\r\nb\r\n");
        assert_eq!(bom, "\u{FEFF}");
        assert_eq!(ending, LineEnding::Crlf);

        let (bom2, ending2) = detect_file_style("a\rb\r");
        assert_eq!(bom2, "");
        assert_eq!(ending2, LineEnding::Cr);
    }

    #[test]
    fn test_normalize_utf8_multibyte_roundtrip() {
        let input = "# Brainrot MoneyPopUpManager — floating +$ UI on cash gains.\nline2\n";
        let normalized = normalize_to_lf(input);
        assert_eq!(normalized, input);
        // Em dash — is U+2014, encoded as E2 80 94 in UTF-8 (3 bytes)
        assert!(normalized.contains('—'));
    }

    #[test]
    fn test_normalize_utf8_multibyte_crlf() {
        let input = "# em dash — and arrow →\r\nsecond line\r\n";
        let normalized = normalize_to_lf(input);
        assert_eq!(normalized, "# em dash — and arrow →\nsecond line\n");
        assert!(normalized.contains('—'));
        assert!(normalized.contains('→'));
    }

    #[test]
    fn test_normalize_utf8_en_dash() {
        let input = "en dash – and em dash —\n";
        let normalized = normalize_to_lf(input);
        assert_eq!(normalized, input);
        assert!(normalized.contains('–'));
        assert!(normalized.contains('—'));
    }

    #[test]
    fn test_normalize_utf8_accented() {
        let input = "café résumé naïve façade\n";
        let normalized = normalize_to_lf(input);
        assert_eq!(normalized, input);
    }

    #[test]
    fn test_normalize_crlf_utf8_multibyte() {
        let input = "—\r\n—\r\n";
        let normalized = normalize_to_lf(input);
        assert_eq!(normalized, "—\n—\n");
    }
}

#[test]
fn test_edge_empty() {
    assert_eq!(normalize_to_lf(""), "");
}

#[test]
fn test_edge_only_newline() {
    assert_eq!(normalize_to_lf("\n"), "\n");
}

#[test]
fn test_edge_ascii_only() {
    assert_eq!(normalize_to_lf("hello\nworld\n"), "hello\nworld\n");
}

#[test]
fn test_edge_all_cr() {
    assert_eq!(normalize_to_lf("a\rb\rc\r"), "a\nb\nc\n");
}

#[test]
fn test_edge_all_crlf() {
    assert_eq!(normalize_to_lf("a\r\nb\r\nc\r\n"), "a\nb\nc\n");
}

#[test]
fn test_edge_mixed_endings() {
    assert_eq!(normalize_to_lf("a\r\nb\rc\r\nd"), "a\nb\nc\nd");
}

#[test]
fn test_edge_emoji_4byte() {
    let s = "hello 🚀 world\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_emoji_crlf() {
    let input = "hello 🚀 world\r\nline2\r\n";
    assert_eq!(normalize_to_lf(input), "hello 🚀 world\nline2\n");
}

#[test]
fn test_edge_japanese() {
    let s = "こんにちは世界\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_arabic() {
    let s = "مرحبا بالعالم\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_consecutive_cr_with_unicode() {
    assert_eq!(normalize_to_lf("—\r—\r—\r"), "—\n—\n—\n");
}

#[test]
fn test_edge_consecutive_crlf_with_unicode() {
    assert_eq!(normalize_to_lf("—\r\n—\r\n—\r\n"), "—\n—\n—\n");
}

#[test]
fn test_edge_mixed_endings_with_unicode() {
    assert_eq!(normalize_to_lf("é\r\n—\r🚀\n"), "é\n—\n🚀\n");
}

#[test]
fn test_edge_no_trailing_newline_unicode() {
    let s = "hello 🚀 world";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_no_trailing_newline_crlf() {
    let input = "hello 🚀 world\r\nline2";
    assert_eq!(normalize_to_lf(input), "hello 🚀 world\nline2");
}

#[test]
fn test_edge_only_crlf_no_text() {
    assert_eq!(normalize_to_lf("\r\n\r\n\r\n"), "\n\n\n");
}

#[test]
fn test_edge_zero_width_space() {
    let s = "a\u{200B}b\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_bidi_override() {
    let s = "test\u{202E}back\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_null_byte() {
    let s = "before\x00after\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_long_repeated_unicode() {
    let s = "x".repeat(100) + &"\u{2014}".repeat(50) + "\nline2\n";
    assert_eq!(normalize_to_lf(&s), s);
}

#[test]
fn test_edge_only_newlines() {
    assert_eq!(normalize_to_lf("\n\n\n\n"), "\n\n\n\n");
}

#[test]
fn test_edge_max_unicode_codepoint() {
    let s = "max \u{10FFFF} codepoint\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_snowman() {
    assert_eq!(normalize_to_lf("snowman ☃\nline2\n"), "snowman ☃\nline2\n");
}

#[test]
fn test_edge_music_symbols() {
    assert_eq!(normalize_to_lf("music ♭♯\nline2\n"), "music ♭♯\nline2\n");
}

#[test]
fn test_edge_math_symbols() {
    assert_eq!(normalize_to_lf("math ∞∂\nline2\n"), "math ∞∂\nline2\n");
}

#[test]
fn test_edge_box_drawing() {
    assert_eq!(normalize_to_lf("box ─│\nline2\n"), "box ─│\nline2\n");
}

#[test]
fn test_edge_crlf_restore_with_unicode() {
    // Verify that restore_line_endings works on normalized unicode text
    let original = "—\r\n—\r\n";
    let normalized = normalize_to_lf(original);
    let restored = restore_line_endings(&normalized, LineEnding::Crlf);
    assert_eq!(restored, original);
}

#[test]
fn test_edge_mixed_width_cjk_with_crlf() {
    let input = "中文\r\n日本語\r\n한국어\r\n";
    assert_eq!(normalize_to_lf(input), "中文\n日本語\n한국어\n");
}

#[test]
fn test_edge_surrogate_pair_not_in_crlf() {
    // U+1F600 = 😀 as UTF-8: F0 9F 98 80
    let s = "grinning 😀 face\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}

#[test]
fn test_edge_bom_preserved() {
    let s = "\u{FEFF}hello — world\nline2\n";
    assert_eq!(normalize_to_lf(s), s);
}
