use std::io::Write;

use crate::cli::GrepCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::{LineView, SearchDocument};
use crate::error::HashlineError;
use crate::hash;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: GrepCmd,
) -> Result<(), HashlineError> {
    let search_doc = SearchDocument::load(&cmd.file)?;
    let total_lines = search_doc.line_offsets.len();

    match ctx.output_mode() {
        OutputMode::Json => {
            if cmd.case_insensitive {
                let lines = grep_lines_regex(&search_doc, &cmd.pattern, true, cmd.invert)?;
                output::write_grep_json(ctx, &lines)?;
            } else {
                output::print_grep_json_streaming(
                    ctx.stdout(),
                    &search_doc,
                    &cmd.pattern,
                    cmd.invert,
                )?;
            }
        }
        OutputMode::Ndjson => {
            if cmd.case_insensitive {
                let lines = grep_lines_regex(&search_doc, &cmd.pattern, true, cmd.invert)?;
                output::print_line_views_ndjson(ctx.stdout(), &lines)?;
            } else {
                output::print_grep_ndjson_streaming(
                    ctx.stdout(),
                    &search_doc,
                    &cmd.pattern,
                    cmd.invert,
                )?;
            }
        }
        OutputMode::Pretty => {
            if cmd.case_insensitive {
                let lines = grep_lines_regex(&search_doc, &cmd.pattern, true, cmd.invert)?;
                output::print_line_views(ctx.stdout(), &lines)?;
            } else {
                output::print_grep_pretty_streaming(
                    ctx.stdout(),
                    &search_doc,
                    &cmd.pattern,
                    cmd.invert,
                    total_lines,
                )?;
            }
        }
    }
    Ok(())
}

/// Perform a regex-based grep on the given `SearchDocument`.
///
/// Iterates through all lines, testing each against the compiled regex.
/// Returns a `Vec<LineView>` with matching (or non-matching, when `invert` is
/// true) lines. Uses the `regex` crate with `case_insensitive` controlled by
/// the parameter.
fn grep_lines_regex(
    search_doc: &SearchDocument,
    pattern: &str,
    case_insensitive: bool,
    invert: bool,
) -> Result<Vec<LineView>, HashlineError> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| HashlineError::InvalidPattern {
            pattern: pattern.to_string(),
            message: e.to_string(),
        })?;

    let mut results = Vec::new();

    for (line_idx, &start) in search_doc.line_offsets.iter().enumerate() {
        let end = if line_idx + 1 < search_doc.line_offsets.len() {
            search_doc.line_offsets[line_idx + 1]
        } else {
            search_doc.content.len()
        };
        let line_end = if search_doc.trailing_newline
            && end > start
            && search_doc.content.as_bytes()[end.saturating_sub(1)] == b'\n'
        {
            end - 1
        } else {
            end.min(search_doc.content.len())
        };
        let line_content = search_doc.content[start..line_end]
            .strip_suffix('\r')
            .unwrap_or(&search_doc.content[start..line_end]);

        let is_match = re.is_match(line_content);
        let include = if invert { !is_match } else { is_match };

        if include {
            let fh = hash::full_hash(line_content);
            let sh = hash::short_from_full(fh);
            results.push(LineView {
                n: line_idx + 1,
                hash: hash::format_short_hash(sh),
                content: line_content.to_string(),
            });
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::SearchDocument;

    fn test_doc(content: &str) -> SearchDocument {
        SearchDocument::new(content)
    }

    #[test]
    fn grep_lines_regex_finds_matches() {
        let doc = test_doc("hello world\nfoo bar\nbaz hello\n");
        let results = grep_lines_regex(&doc, "hello", false, false).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "hello world");
        assert_eq!(results[1].content, "baz hello");
    }

    #[test]
    fn grep_lines_regex_invert_matches() {
        let doc = test_doc("hello world\nfoo bar\nbaz hello\n");
        let results = grep_lines_regex(&doc, "hello", false, true).unwrap();
        // Two non-matching lines: "foo bar" and the trailing empty line
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "foo bar");
        assert_eq!(results[1].content, "");
    }

    #[test]
    fn grep_lines_regex_case_insensitive() {
        let doc = test_doc("Hello World\nfoo bar\n");
        let results = grep_lines_regex(&doc, "hello", true, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Hello World");
    }

    #[test]
    fn grep_lines_regex_case_sensitive_no_match() {
        let doc = test_doc("Hello World\nfoo bar\n");
        let results = grep_lines_regex(&doc, "hello", false, false).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn grep_lines_regex_invalid_pattern() {
        let doc = test_doc("hello\n");
        let err = grep_lines_regex(&doc, "(unclosed", false, false).unwrap_err();
        assert!(matches!(err, HashlineError::InvalidPattern { .. }));
    }

    #[test]
    fn grep_lines_regex_empty_file() {
        let doc = test_doc("");
        let results = grep_lines_regex(&doc, ".*", false, false).unwrap();
        // Empty file has one line offset [0] but that's the "empty line" marker
        // — the actual line has content ""
        assert!(!results.is_empty());
        assert_eq!(&results[0].content, "");
    }

    #[test]
    fn grep_lines_regex_no_matches_returns_empty() {
        let doc = test_doc("alpha\nbeta\n");
        let results = grep_lines_regex(&doc, "gamma", false, false).unwrap();
        assert!(results.is_empty());
    }
}
