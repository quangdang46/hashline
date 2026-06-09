use std::io::Write;

use crate::cli::AnnotateCmd;
use crate::context::{CommandContext, OutputMode};
use crate::document::{LineView, SearchDocument};
use crate::error::HashlineError;
use crate::hash;
use crate::output;

pub fn run<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    cmd: AnnotateCmd,
) -> Result<(), HashlineError> {
    let search_doc = SearchDocument::load(&cmd.file)?;

    let lines = if cmd.regex {
        search_regex(&search_doc, &cmd.query, cmd.expect_one)?
    } else {
        search_literal(&search_doc, &cmd.query, cmd.expect_one)?
    };

    match ctx.output_mode() {
        OutputMode::Json => output::write_grep_json(ctx, &lines)?,
        OutputMode::Ndjson => output::print_line_views_ndjson(ctx.stdout(), &lines)?,
        OutputMode::Pretty => output::print_line_views(ctx.stdout(), &lines)?,
    }

    Ok(())
}

/// Perform a regex-based annotation search on the given `SearchDocument`.
///
/// Iterates through all lines, testing each against the compiled regex.
/// When `expect_one` is true, returns an error if the match count is not
/// exactly 1.
fn search_regex(
    search_doc: &SearchDocument,
    query: &str,
    expect_one: bool,
) -> Result<Vec<LineView>, HashlineError> {
    let re = regex::RegexBuilder::new(query)
        .case_insensitive(false)
        .build()
        .map_err(|e| HashlineError::InvalidPattern {
            pattern: query.to_string(),
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

        if re.is_match(line_content) {
            let fh = hash::full_hash(line_content);
            let sh = hash::short_from_full(fh);
            results.push(LineView {
                n: line_idx + 1,
                hash: hash::format_short_hash(sh),
                content: line_content.to_string(),
            });
        }
    }

    if expect_one && results.len() != 1 {
        let msg = if results.is_empty() {
            format!(
                "expected exactly 1 match for query '{}', but found 0",
                query
            )
        } else {
            format!(
                "expected exactly 1 match for query '{}', but found {}",
                query,
                results.len()
            )
        };
        return Err(HashlineError::InvalidPattern {
            pattern: query.to_string(),
            message: msg,
        });
    }

    Ok(results)
}

/// Perform a literal (memchr-based) annotation search on the given `SearchDocument`.
///
/// Reuses the memchr-based `grep_for_each` pattern for literal matching.
/// When `expect_one` is true, returns an error if the match count is not
/// exactly 1.
fn search_literal(
    search_doc: &SearchDocument,
    query: &str,
    expect_one: bool,
) -> Result<Vec<LineView>, HashlineError> {
    let mut results = Vec::new();

    search_doc.grep_for_each(query, false, |line_idx, content, short_hash| {
        results.push(LineView {
            n: line_idx + 1,
            hash: hash::format_short_hash(short_hash),
            content: content.to_string(),
        });
    });

    if expect_one && results.len() != 1 {
        let msg = if results.is_empty() {
            format!(
                "expected exactly 1 match for query '{}', but found 0",
                query
            )
        } else {
            format!(
                "expected exactly 1 match for query '{}', but found {}",
                query,
                results.len()
            )
        };
        return Err(HashlineError::InvalidPattern {
            pattern: query.to_string(),
            message: msg,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::SearchDocument;

    fn test_doc(content: &str) -> SearchDocument {
        SearchDocument::from_str(content)
    }

    #[test]
    fn annotate_literal_finds_matches() {
        let doc = test_doc("hello world\nfoo bar\nbaz hello\n");
        let results = search_literal(&doc, "hello", false).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "hello world");
        assert_eq!(results[1].content, "baz hello");
    }

    #[test]
    fn annotate_literal_no_matches_returns_empty() {
        let doc = test_doc("alpha\nbeta\n");
        let results = search_literal(&doc, "gamma", false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn annotate_regex_finds_matches() {
        let doc = test_doc("hello world\nfoo bar\nbaz hello\n");
        let results = search_regex(&doc, "h[a-z]+", false).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "hello world");
        assert_eq!(results[1].content, "baz hello");
    }

    #[test]
    fn annotate_regex_case_sensitive_by_default() {
        let doc = test_doc("Hello World\nfoo bar\nhello world\n");
        let results = search_regex(&doc, "hello", false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello world");
    }

    #[test]
    fn annotate_regex_invalid_pattern() {
        let doc = test_doc("hello\n");
        let err = search_regex(&doc, "(unclosed", false).unwrap_err();
        assert!(matches!(err, HashlineError::InvalidPattern { .. }));
    }

    #[test]
    fn annotate_literal_expect_one_ok() {
        let doc = test_doc("hello world\nfoo bar\n");
        let results = search_literal(&doc, "hello", true).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello world");
    }

    #[test]
    fn annotate_literal_expect_one_zero_matches() {
        let doc = test_doc("hello world\nfoo bar\n");
        let err = search_literal(&doc, "zzzz", true).unwrap_err();
        assert!(matches!(err, HashlineError::InvalidPattern { .. }));
        assert!(err.to_string().contains("found 0"));
    }

    #[test]
    fn annotate_literal_expect_one_multiple_matches() {
        let doc = test_doc("hello world\nfoo bar\nbaz hello\n");
        let err = search_literal(&doc, "hello", true).unwrap_err();
        assert!(matches!(err, HashlineError::InvalidPattern { .. }));
        assert!(err.to_string().contains("found 2"));
    }

    #[test]
    fn annotate_regex_expect_one_ok() {
        let doc = test_doc("hello world\nfoo bar\nbaz world\n");
        let results = search_regex(&doc, "^hello", true).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "hello world");
    }

    #[test]
    fn annotate_regex_expect_one_multiple_matches() {
        let doc = test_doc("hello world\nfoo bar\nhello again\n");
        let err = search_regex(&doc, "^hello", true).unwrap_err();
        assert!(matches!(err, HashlineError::InvalidPattern { .. }));
        assert!(err.to_string().contains("found 2"));
    }

    #[test]
    fn annotate_empty_file() {
        let doc = test_doc("");
        // Empty file has one line offset [0] — the empty line with content ""
        let results = search_regex(&doc, ".*", false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(&results[0].content, "");
    }
}
