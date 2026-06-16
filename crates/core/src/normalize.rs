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
pub fn normalize_to_lf(text: &str) -> String {
    // Match: \r\n → \n, and \r alone → \n
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                out.push('\n');
                i += 2;
            } else {
                out.push('\n');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
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
        assert_eq!(restore_line_endings("a\nb\n", LineEnding::Crlf), "a\r\nb\r\n");
        assert_eq!(restore_line_endings("a\nb\n", LineEnding::Lf), "a\nb\n");
    }
}
