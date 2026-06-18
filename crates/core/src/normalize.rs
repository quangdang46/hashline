//! Text normalization helpers: line-ending detection, CRLF→LF normalization,
//! BOM stripping. The patcher uses these to canonicalize text to LF before
//! applying edits and to restore the original shape on write-back.

/// Line ending style.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// Detect the first line ending style in `content`. Defaults to LF when
/// neither is present.
pub fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            return LineEnding::Crlf;
        }
        if bytes[i] == b'\n' {
            return LineEnding::Lf;
        }
        i += 1;
    }
    LineEnding::Lf
}

/// Normalize every line ending to LF. Handles bare CR as well.
///
/// Iterates by `char` (not byte) so multi-byte UTF-8 sequences like — (em dash,
/// U+2014, 3 bytes) pass through unchanged instead of being split and corrupted.
pub fn normalize_to_lf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next(); // skip the \n of \r\n
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Re-encode LF text with the requested line ending.
pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_owned(),
        LineEnding::Crlf => text.replace('\n', "\r\n"),
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
