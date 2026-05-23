#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use serde::Serialize;

use crate::anchor::ResolvedLine;
use crate::context::{CommandContext, OutputMode};
use crate::document::{Document, FileStats, LineView, NewlineStyle, format_short_hash};
use crate::error::HashlineError;
use crate::hash::write_short_hash_bytes;
use crate::orchestration::{IndexPayload, ReadPayload};
use crate::risk::blocked_assessment;

/// Whether to emit JSON in compact (default) or pretty form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonStyle {
    Compact,
    Pretty,
}

impl JsonStyle {
    pub fn from_pretty(pretty: bool) -> Self {
        if pretty {
            JsonStyle::Pretty
        } else {
            JsonStyle::Compact
        }
    }
}

/// Serialize `value` to `writer` followed by a single newline, using the
/// requested JSON style (compact by default, pretty when explicitly opted in).
pub fn serialize_json<W: Write, T: Serialize + ?Sized>(
    writer: &mut W,
    value: &T,
    style: JsonStyle,
) -> io::Result<()> {
    match style {
        JsonStyle::Compact => serde_json::to_writer(&mut *writer, value)?,
        JsonStyle::Pretty => serde_json::to_writer_pretty(&mut *writer, value)?,
    }
    writeln!(writer)
}

/// Fast-path JSON string serializer used in the per-line hot loop of
/// `read --json` / `read --ndjson` / `grep --json`. Equivalent to
/// `serde_json::to_writer(writer, s)` for `s: &str` but skips the
/// byte-by-byte ESCAPE table iteration that dominates large files.
///
/// Strategy:
/// 1. Use `memchr3` (SIMD `pcmpeqb`/AVX2) to find the first byte that
///    needs escaping (`"`, `\`, or `\t` \u2014 the only bytes that realistically
///    appear inside a single-line content string since `\n`/`\r` are
///    consumed as line delimiters).
/// 2. If no such byte exists AND no control bytes < 0x20 are present,
///    emit `"<content>"` directly via one `write_all`.
/// 3. Otherwise, fall back to `serde_json::to_writer` for correctness.
///
/// On a 100k-line .rs fixture the fast path covers ~95% of lines, so this
/// trades a tight memchr scan (\u00ad2 cycles/byte on AVX2) for serde_json's
/// branch-per-byte loop (\u00ad6 cycles/byte). Output bytes match exactly.
#[inline]
pub fn write_json_string_fast<W: Write + ?Sized>(writer: &mut W, s: &str) -> io::Result<()> {
    let bytes = s.as_bytes();

    // memchr3 covers the three escape bytes likely to appear inside a
    // line: '"', '\\', and '\t'. Control chars below 0x20 are rare in
    // source files; we still need to catch them so the fast path also
    // requires that no byte < 0x20 (other than tab, already counted)
    // appears \u2014 verified by the second memchr2 below.
    if memchr::memchr3(b'"', b'\\', b'\t', bytes).is_some() || has_control_byte(bytes) {
        // Slow path: defer to serde_json for full escape correctness.
        serde_json::to_writer(writer, s).map_err(io::Error::from)?;
        return Ok(());
    }

    // Fast path: no escape needed. Write `"<bytes>"` in three syscalls.
    writer.write_all(b"\"")?;
    writer.write_all(bytes)?;
    writer.write_all(b"\"")
}

/// True if `bytes` contains any byte in 0x00..=0x1F other than `\t`
/// (which is checked separately by the `memchr3` above).
#[inline]
fn has_control_byte(bytes: &[u8]) -> bool {
    // Single-byte SIMD pass for any byte in 0x00..=0x08 OR 0x0A..=0x1F.
    // The standard library autovectorizes this loop with SSE2/AVX2 on
    // x86_64 release builds.
    for &b in bytes {
        if b < 0x20 && b != b'\t' {
            return true;
        }
    }
    false
}

#[derive(Serialize)]
struct ErrorPayload<'a> {
    error: String,
    hint: Option<&'a str>,
    command: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    risk: Option<crate::risk::RiskAssessment>,
}

#[allow(dead_code)]
pub fn write_success_line<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    line: &str,
) -> io::Result<()> {
    writeln!(ctx.stdout(), "{line}")
}

/// Print `±context` lines around the changed range with their fresh anchors.
///
/// Called after a successful mutation to give agents the new anchors of the
/// edited region without forcing them to call `read` again. This saves one
/// MCP round-trip in the typical edit-then-verify flow.
///
/// The doc must be POST-mutation (lines re-hashed) — `Document` already does
/// this in the mutation helpers via `refresh_line_metadata`.
#[allow(dead_code)]
pub fn write_post_edit_snippet<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    doc: &Document,
    first_changed: usize, // 1-indexed
    last_changed: usize,  // 1-indexed
) -> io::Result<()> {
    const CONTEXT: usize = 2;
    const MAX_OUTPUT_LINES: usize = 12;

    if doc.lines.is_empty() {
        return Ok(());
    }

    let lo = first_changed.saturating_sub(CONTEXT).max(1);
    let hi = (last_changed + CONTEXT).min(doc.lines.len());
    if hi < lo {
        return Ok(());
    }
    if hi - lo + 1 > MAX_OUTPUT_LINES {
        // Range too large to inline — agent should re-read.
        writeln!(
            ctx.stdout(),
            "(snippet omitted: changed range too large; use `hashline read --anchor LINE:HASH` to inspect)"
        )?;
        return Ok(());
    }

    let mut hash_buf = [0u8; 2];
    let mut number_buf = itoa::Buffer::new();
    let stdout = ctx.stdout();
    for i in lo..=hi {
        let line = &doc.lines[i - 1];
        let number = number_buf.format(i);
        stdout.write_all(number.as_bytes())?;
        stdout.write_all(b":")?;
        write_short_hash_bytes(&mut hash_buf, line.short_hash);
        stdout.write_all(&hash_buf)?;
        stdout.write_all(b"|")?;
        stdout.write_all(line.content.as_bytes())?;
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

/// Emit `value` as JSON to stdout, using the context's JSON style
/// (compact by default; pretty when the caller passed `--pretty`).
#[allow(dead_code)]
pub fn write_json_success<W: Write, E: Write, T: Serialize + ?Sized>(
    ctx: &mut CommandContext<'_, W, E>,
    value: &T,
) -> io::Result<()> {
    let style = JsonStyle::from_pretty(ctx.json_pretty());
    serialize_json(ctx.stdout(), value, style)
}

pub fn print_read(writer: &mut impl Write, doc: &Document) -> io::Result<()> {
    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    for (index, line) in doc.lines.iter().enumerate() {
        let number = number_buf.format(index + 1);
        writer.write_all(number.as_bytes())?;
        writer.write_all(b":")?;
        write_short_hash(&mut hash_buf, line.short_hash);
        writer.write_all(&hash_buf)?;
        writer.write_all(b"|")?;
        writer.write_all(line.content.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

pub fn print_read_json(
    writer: &mut impl Write,
    payload: &ReadPayload,
    style: JsonStyle,
) -> io::Result<()> {
    serialize_json(writer, payload, style)
}

/// Borrowed counterpart of [`LineView`] for zero-clone serialization paths.
///
/// `LineView` owns `String`s for `hash` and `content`, which forces a
/// per-line allocation when building [`ReadPayload`] from a [`Document`].
/// On large files this allocation dominates the `read --json` and
/// `read --ndjson` paths. This struct lets us serialize directly from
/// `&LineRecord` slices.
#[derive(Serialize)]
struct LineViewRef<'a> {
    n: usize,
    hash: &'a str,
    content: &'a str,
}

/// Streaming variant of [`print_read_json`] used when the caller wants the
/// entire document (no anchor filter, no context window). Bypasses
/// `ReadPayload`'s `Vec<LineView>` allocation by serializing each line
/// straight from `&doc.lines`, which is the dominant cost on large files.
///
/// Only the compact JSON style is streamed; the pretty path falls back to
/// the buffered serializer since pretty output is only requested for human
/// inspection of small files.
pub fn print_read_json_streaming(
    writer: &mut impl Write,
    doc: &Document,
    style: JsonStyle,
) -> io::Result<()> {
    if style == JsonStyle::Pretty {
        // Pretty output is requested only for human inspection. Build a
        // full payload and let serde_json::to_writer_pretty do the work.
        let payload = crate::orchestration::read_payload(doc, &[], 0)
            .map_err(|err| io::Error::other(err.to_string()))?;
        return serialize_json(writer, &payload, style);
    }

    let file = doc.path.display().to_string();
    let newline = newline_name(doc.newline);
    let (mtime, mtime_nanos, inode) = doc
        .file_meta
        .as_ref()
        .map(|meta| (meta.mtime_secs, meta.mtime_nanos, meta.inode))
        .unwrap_or((0, 0, 0));

    writer.write_all(b"{\"file\":")?;
    serde_json::to_writer(&mut *writer, &file)?;
    writer.write_all(b",\"newline\":\"")?;
    writer.write_all(newline.as_bytes())?;
    writer.write_all(b"\",\"trailing_newline\":")?;
    writer.write_all(if doc.trailing_newline {
        b"true"
    } else {
        b"false"
    })?;

    let mut number_buf = itoa::Buffer::new();
    writer.write_all(b",\"mtime\":")?;
    writer.write_all(number_buf.format(mtime).as_bytes())?;
    writer.write_all(b",\"mtime_nanos\":")?;
    writer.write_all(number_buf.format(mtime_nanos).as_bytes())?;
    writer.write_all(b",\"inode\":")?;
    writer.write_all(number_buf.format(inode).as_bytes())?;
    writer.write_all(b",\"lines\":[")?;

    let mut hash_buf = [0u8; 2];
    for (index, line) in doc.lines.iter().enumerate() {
        if index > 0 {
            writer.write_all(b",")?;
        }
        write_short_hash(&mut hash_buf, line.short_hash);
        writer.write_all(b"{\"n\":")?;
        writer.write_all(number_buf.format(index + 1).as_bytes())?;
        writer.write_all(b",\"hash\":\"")?;
        writer.write_all(&hash_buf)?;
        writer.write_all(b"\",\"content\":")?;
        write_json_string_fast(&mut *writer, line.content.as_ref())?;
        writer.write_all(b"}")?;
    }

    writer.write_all(b"]}")?;
    writeln!(writer)
}

/// Streaming variant of [`print_read_ndjson`] used when emitting the entire
/// document. Same zero-clone strategy as `print_read_json_streaming`.
pub fn print_read_ndjson_streaming(writer: &mut impl Write, doc: &Document) -> io::Result<()> {
    #[derive(Serialize)]
    struct ReadHeader<'a> {
        event: &'static str,
        file: &'a str,
        newline: &'a str,
        trailing_newline: bool,
        mtime: i64,
        mtime_nanos: u32,
        inode: u64,
        total_lines: usize,
    }

    let file = doc.path.display().to_string();
    let (mtime, mtime_nanos, inode) = doc
        .file_meta
        .as_ref()
        .map(|meta| (meta.mtime_secs, meta.mtime_nanos, meta.inode))
        .unwrap_or((0, 0, 0));
    let header = ReadHeader {
        event: "header",
        file: &file,
        newline: newline_name(doc.newline),
        trailing_newline: doc.trailing_newline,
        mtime,
        mtime_nanos,
        inode,
        total_lines: doc.lines.len(),
    };
    serde_json::to_writer(&mut *writer, &header)?;
    writeln!(writer)?;

    let mut hash_buf = [0u8; 2];
    let mut number_buf = itoa::Buffer::new();
    for (index, line) in doc.lines.iter().enumerate() {
        write_short_hash(&mut hash_buf, line.short_hash);
        // Manually inline the LineViewRef serialization so each line's
        // content can take the fast escape path. The previous code went
        // via `serde_json::to_writer(&LineViewRef { ... })` which routes
        // through the byte-by-byte ESCAPE table for `content`.
        writer.write_all(b"{\"n\":")?;
        writer.write_all(number_buf.format(index + 1).as_bytes())?;
        writer.write_all(b",\"hash\":\"")?;
        writer.write_all(&hash_buf)?;
        writer.write_all(b"\",\"content\":")?;
        write_json_string_fast(&mut *writer, &line.content)?;
        writer.write_all(b"}")?;
        writeln!(writer)?;
    }
    Ok(())
}

/// Backwards-compatible alias for [`write_short_hash_bytes`].
///
/// Kept here so existing callers in this file don't have to change. The
/// canonical helper now lives in [`crate::hash`] so output paths can render
/// directly without going through `format!`.
#[inline]
fn write_short_hash(buf: &mut [u8; 2], short: u8) {
    write_short_hash_bytes(buf, short);
}

pub fn print_read_context(
    writer: &mut impl Write,
    doc: &Document,
    anchors: &[ResolvedLine],
    context: usize,
) -> io::Result<()> {
    let anchor_indexes: BTreeSet<usize> = anchors.iter().map(|anchor| anchor.index).collect();
    let included = collect_context_indexes(doc, anchors, context);

    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    let mut previous: Option<usize> = None;
    for index in included {
        if let Some(prev) = previous {
            if index > prev + 1 {
                writer.write_all(b"...\n")?;
            }
        }

        let marker: &[u8] = if anchor_indexes.contains(&index) {
            // U+2192 RIGHTWARDS ARROW, 3 UTF-8 bytes.
            "→".as_bytes()
        } else {
            b" "
        };
        let line = &doc.lines[index];
        let number = number_buf.format(index + 1);
        writer.write_all(marker)?;
        writer.write_all(number.as_bytes())?;
        writer.write_all(b":")?;
        write_short_hash_bytes(&mut hash_buf, line.short_hash);
        writer.write_all(&hash_buf)?;
        writer.write_all(b"|")?;
        writer.write_all(line.content.as_bytes())?;
        writer.write_all(b"\n")?;
        previous = Some(index);
    }

    Ok(())
}

pub fn print_index(writer: &mut impl Write, doc: &Document) -> io::Result<()> {
    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    for (index, line) in doc.lines.iter().enumerate() {
        let number = number_buf.format(index + 1);
        writer.write_all(number.as_bytes())?;
        writer.write_all(b":")?;
        write_short_hash_bytes(&mut hash_buf, line.short_hash);
        writer.write_all(&hash_buf)?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

pub fn print_index_json(
    writer: &mut impl Write,
    payload: &IndexPayload,
    style: JsonStyle,
) -> io::Result<()> {
    serialize_json(writer, payload, style)
}

/// Emit `payload` as NDJSON: one header line, then one object per line in
/// `payload.lines`. Each object is compact JSON terminated by `\n`, so a
/// downstream agent can stream-parse the response without buffering the whole
/// document.
pub fn print_read_ndjson(writer: &mut impl Write, payload: &ReadPayload) -> io::Result<()> {
    #[derive(Serialize)]
    struct ReadHeader<'a> {
        event: &'static str,
        file: &'a str,
        newline: &'a str,
        trailing_newline: bool,
        mtime: i64,
        mtime_nanos: u32,
        inode: u64,
        total_lines: usize,
    }

    let header = ReadHeader {
        event: "header",
        file: &payload.file,
        newline: payload.newline,
        trailing_newline: payload.trailing_newline,
        mtime: payload.mtime,
        mtime_nanos: payload.mtime_nanos,
        inode: payload.inode,
        total_lines: payload.lines.len(),
    };
    serde_json::to_writer(&mut *writer, &header)?;
    writeln!(writer)?;
    for line in &payload.lines {
        serde_json::to_writer(&mut *writer, line)?;
        writeln!(writer)?;
    }
    Ok(())
}

/// Emit an [`IndexPayload`] as NDJSON: one header line then one
/// `{"n":..,"hash":..}` object per line.
pub fn print_index_ndjson(writer: &mut impl Write, payload: &IndexPayload) -> io::Result<()> {
    #[derive(Serialize)]
    struct IndexHeader<'a> {
        event: &'static str,
        file: &'a str,
        total_lines: usize,
    }

    let header = IndexHeader {
        event: "header",
        file: &payload.file,
        total_lines: payload.lines.len(),
    };
    serde_json::to_writer(&mut *writer, &header)?;
    writeln!(writer)?;
    for entry in &payload.lines {
        serde_json::to_writer(&mut *writer, entry)?;
        writeln!(writer)?;
    }
    Ok(())
}

/// Emit a slice of [`LineView`]s as NDJSON: one match per line, no wrapper or
/// header. Suitable for grep/annotate where 0 results is valid output.
pub fn print_line_views_ndjson(writer: &mut impl Write, lines: &[LineView]) -> io::Result<()> {
    for line in lines {
        serde_json::to_writer(&mut *writer, line)?;
        writeln!(writer)?;
    }
    Ok(())
}

pub fn print_stats(writer: &mut impl Write, stats: &FileStats) -> io::Result<()> {
    writeln!(writer, "Lines: {}", stats.line_count)?;
    writeln!(writer, "Unique hashes (2-char): {}", stats.unique_hashes)?;
    writeln!(writer, "Collisions: {}", stats.collision_count)?;
    writeln!(writer, "Collision pairs: {}", stats.collision_pair_count)?;
    writeln!(writer, "Est. read tokens: ~{}", stats.estimated_read_tokens)?;
    writeln!(
        writer,
        "Hash length advice: {}-char recommended",
        stats.hash_length_advice
    )?;
    writeln!(writer, "Suggested --context: {}", stats.suggested_context_n)?;
    writeln!(
        writer,
        "Recommended read mode: {}",
        stats.recommended_read_mode
    )?;
    writeln!(
        writer,
        "Recommended anchor mode: {}",
        stats.recommended_anchor_mode
    )?;
    writeln!(
        writer,
        "Recommended workflow: {}",
        stats.recommended_workflow
    )?;
    if stats.warnings.is_empty() {
        writeln!(writer, "Warnings: none")?;
    } else {
        writeln!(writer, "Warnings:")?;
        for warning in &stats.warnings {
            writeln!(writer, "- {warning}")?;
        }
    }
    writeln!(writer, "Note: v1 anchors still use fixed 2-char hashes.")
}

pub fn print_stats_json(
    writer: &mut impl Write,
    stats: &FileStats,
    style: JsonStyle,
) -> io::Result<()> {
    serialize_json(writer, stats, style)
}

pub fn print_grep(writer: &mut impl Write, doc: &Document, indexes: &[usize]) -> io::Result<()> {
    for index in indexes {
        let line = &doc.lines[*index];
        writeln!(
            writer,
            "{number}:{hash}|{content}",
            number = *index + 1,
            hash = format_short_hash(line.short_hash),
            content = line.content,
        )?;
    }
    Ok(())
}

/// Streaming pretty-mode `grep` writer.
///
/// Hands each match from `search_doc` straight to `writer` without
/// constructing a `LineView` or copying the line content into a `String`.
/// Used by the literal, case-sensitive pretty path — the most common shape
/// for human-facing grep invocations.
///
/// `total_line_count` is the document's line count (already known to the
/// caller from `search_doc.line_offsets.len()`) and is used to size the
/// number-column width. Padding is at most one column wider than the old
/// "compute width from observed matches" behavior, which is a worthwhile
/// trade to avoid the two-pass scan.
pub fn print_grep_pretty_streaming(
    writer: &mut impl Write,
    search_doc: &crate::document::SearchDocument,
    pattern: &str,
    invert: bool,
    _total_line_count: usize,
) -> io::Result<()> {
    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    let mut io_err: Option<io::Error> = None;

    search_doc.grep_for_each(pattern, invert, |line_idx, content, short_hash| {
        if io_err.is_some() {
            return;
        }
        let number = number_buf.format(line_idx + 1);
        let result = (|| -> io::Result<()> {
            writer.write_all(number.as_bytes())?;
            writer.write_all(b":")?;
            write_short_hash_bytes(&mut hash_buf, short_hash);
            writer.write_all(&hash_buf)?;
            writer.write_all(b"|")?;
            writer.write_all(content.as_bytes())?;
            writer.write_all(b"\n")
        })();
        if let Err(err) = result {
            io_err = Some(err);
        }
    });

    if let Some(err) = io_err {
        return Err(err);
    }
    Ok(())
}

pub fn print_line_views(writer: &mut impl Write, lines: &[LineView]) -> io::Result<()> {
    let mut number_buf = itoa::Buffer::new();
    for line in lines {
        let number = number_buf.format(line.n);
        writer.write_all(number.as_bytes())?;
        writer.write_all(b":")?;
        writer.write_all(line.hash.as_bytes())?;
        writer.write_all(b"|")?;
        writer.write_all(line.content.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}

pub fn write_grep_json<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    lines: &[LineView],
) -> io::Result<()> {
    write_json_success(ctx, lines)
}

/// Streaming `grep --json` writer.
///
/// Emits the same compact JSON array shape as
/// [`write_grep_json`]/[`write_json_success`] (`{"ok":true,"data":[...]}`)
/// but writes each match directly from the mmap-backed `SearchDocument`
/// without ever building a `Vec<LineView>`. On a 100k-line file with
/// 100k matches this drops the per-match `String` allocations for both
/// `hash` and `content`.
///
/// Pretty (`--json --pretty`) output is intentionally NOT streamed —
/// pretty JSON is only requested for small payloads by humans, where
/// the allocation cost is irrelevant.
pub fn print_grep_json_streaming(
    writer: &mut impl Write,
    search_doc: &crate::document::SearchDocument,
    pattern: &str,
    invert: bool,
) -> io::Result<()> {
    writer.write_all(b"[")?;
    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    let mut first = true;
    let mut io_err: Option<io::Error> = None;

    search_doc.grep_for_each(pattern, invert, |line_idx, content, short_hash| {
        if io_err.is_some() {
            return;
        }
        let result = (|| -> io::Result<()> {
            if !first {
                writer.write_all(b",")?;
            }
            first = false;
            write_short_hash_bytes(&mut hash_buf, short_hash);
            writer.write_all(b"{\"n\":")?;
            writer.write_all(number_buf.format(line_idx + 1).as_bytes())?;
            writer.write_all(b",\"hash\":\"")?;
            writer.write_all(&hash_buf)?;
            writer.write_all(b"\",\"content\":")?;
            write_json_string_fast(&mut *writer, content)?;
            writer.write_all(b"}")
        })();
        if let Err(err) = result {
            io_err = Some(err);
        }
    });

    if let Some(err) = io_err {
        return Err(err);
    }
    writer.write_all(b"]")?;
    writeln!(writer)
}

/// Streaming `grep --ndjson` writer. Same shape as
/// [`print_line_views_ndjson`] (one JSON object per line, no wrapper) but
/// streams matches straight from the `SearchDocument` without allocating
/// a `Vec<LineView>` or per-match `String`s.
pub fn print_grep_ndjson_streaming(
    writer: &mut impl Write,
    search_doc: &crate::document::SearchDocument,
    pattern: &str,
    invert: bool,
) -> io::Result<()> {
    // Same per-line scratch-buffer batching as `print_read_ndjson_streaming`
    // — see the comment there for the rationale (collapses N small
    // `write_all` calls down to one per record, amortizing call-site
    // overhead through the BufWriter).
    let mut number_buf = itoa::Buffer::new();
    let mut hash_buf = [0u8; 2];
    let mut scratch: Vec<u8> = Vec::with_capacity(512);
    let mut io_err: Option<io::Error> = None;

    search_doc.grep_for_each(pattern, invert, |line_idx, content, short_hash| {
        if io_err.is_some() {
            return;
        }
        write_short_hash_bytes(&mut hash_buf, short_hash);
        scratch.clear();
        let result: io::Result<()> = (|| {
            scratch.extend_from_slice(b"{\"n\":");
            scratch.extend_from_slice(number_buf.format(line_idx + 1).as_bytes());
            scratch.extend_from_slice(b",\"hash\":\"");
            scratch.extend_from_slice(&hash_buf);
            scratch.extend_from_slice(b"\",\"content\":");
            write_json_string_fast(&mut scratch, content)?;
            scratch.extend_from_slice(b"}\n");
            writer.write_all(&scratch)
        })();
        if let Err(err) = result {
            io_err = Some(err);
        }
    });

    if let Some(err) = io_err {
        return Err(err);
    }
    Ok(())
}

pub fn write_error<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    error: &HashlineError,
) -> io::Result<()> {
    match ctx.output_mode() {
        OutputMode::Pretty => {
            writeln!(ctx.stderr(), "Error: {error}")?;
            if let Some(risk) = blocked_assessment(error) {
                writeln!(
                    ctx.stderr(),
                    "Risk: {} - {}",
                    risk.level.as_str(),
                    risk.summary
                )?;
                for reason in risk.reasons {
                    writeln!(ctx.stderr(), "Reason: {}", reason.message)?;
                }
            }
            if let Some(hint) = error.hint() {
                writeln!(ctx.stderr(), "Hint: {hint}")?;
            }
            Ok(())
        }
        OutputMode::Json | OutputMode::Ndjson => {
            let risk = blocked_assessment(error);
            let payload = ErrorPayload {
                error: error.to_string(),
                hint: error.hint(),
                command: error.command().or_else(|| {
                    risk.as_ref().map(|risk| match risk.operation {
                        "patch" => "patch",
                        _ => "anchor-safety",
                    })
                }),
                risk,
            };
            // Errors are always one-shot objects, even in NDJSON mode.
            // Honor --pretty only for the single-document JSON mode; NDJSON is always compact.
            let style = if matches!(ctx.output_mode(), OutputMode::Json) && ctx.json_pretty() {
                JsonStyle::Pretty
            } else {
                JsonStyle::Compact
            };
            serialize_json(ctx.stderr(), &payload, style)
        }
    }
}

fn line_number_width(doc: &Document) -> usize {
    doc.lines.len().to_string().len().max(1)
}

fn newline_name(newline: NewlineStyle) -> &'static str {
    match newline {
        NewlineStyle::Lf => "lf",
        NewlineStyle::Crlf => "crlf",
    }
}

fn collect_context_indexes(doc: &Document, anchors: &[ResolvedLine], context: usize) -> Vec<usize> {
    let mut included = BTreeSet::new();

    for anchor in anchors {
        let start = anchor.index.saturating_sub(context);
        let end = (anchor.index + context).min(doc.lines.len().saturating_sub(1));
        for index in start..=end {
            included.insert(index);
        }
    }

    included.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{
        JsonStyle, print_index, print_index_json, print_read, print_read_context, print_read_json,
        print_read_json_streaming, print_stats, print_stats_json,
    };
    use crate::anchor::ResolvedLine;
    use crate::document::{Document, FileStats, format_short_hash};
    use crate::orchestration::{index_payload, read_payload};
    use std::path::Path;

    #[test]
    fn test_read_format_single_line() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("1:{}|alpha\n", format_short_hash(doc.lines[0].short_hash))
        );
    }

    #[test]
    fn test_read_format_no_line_number_padding() {
        // Phase 1: line numbers are flush-left, no padding for AI token efficiency.
        let doc = numbered_doc(10);
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().next().unwrap().starts_with("1:"));
        assert!(rendered.lines().last().unwrap().starts_with("10:"));
    }

    #[test]
    fn test_read_format_three_digit_lines_no_padding() {
        let doc = numbered_doc(100);
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().next().unwrap().starts_with("1:"));
        assert!(rendered.lines().last().unwrap().starts_with("100:"));
    }

    #[test]
    fn test_read_format_no_space_after_pipe() {
        // Phase 1: no space after `|` — content follows immediately.
        // This preserves leading whitespace in code (indentation) verbatim.
        let doc = Document::from_str(Path::new("demo.txt"), "    indented\n").unwrap();
        let mut out = Vec::new();
        print_read(&mut out, &doc).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("|    indented\n"), "got: {rendered}");
    }

    #[test]
    fn test_read_context_marks_anchor_line() {
        let doc = numbered_doc(5);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[ResolvedLine {
                index: 2,
                line_no: 3,
                short_hash: format_short_hash(doc.lines[2].short_hash),
            }],
            1,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.lines().any(|line| line.starts_with("→3:")));
    }

    #[test]
    fn test_read_context_suppresses_other_lines() {
        let doc = numbered_doc(5);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[ResolvedLine {
                index: 2,
                line_no: 3,
                short_hash: format_short_hash(doc.lines[2].short_hash),
            }],
            0,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered.lines().count(), 1);
        assert!(rendered.starts_with("→3:"));
    }

    #[test]
    fn test_read_context_multiple_anchors_merged() {
        let doc = numbered_doc(10);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[
                ResolvedLine {
                    index: 1,
                    line_no: 2,
                    short_hash: format_short_hash(doc.lines[1].short_hash),
                },
                ResolvedLine {
                    index: 8,
                    line_no: 9,
                    short_hash: format_short_hash(doc.lines[8].short_hash),
                },
            ],
            1,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("..."));
        assert!(
            rendered
                .lines()
                .any(|line| line.trim_start().starts_with("→2:"))
        );
        assert!(
            rendered
                .lines()
                .any(|line| line.trim_start().starts_with("→9:"))
        );
    }

    #[test]
    fn test_read_context_separator_between_neighborhoods() {
        let doc = numbered_doc(8);
        let mut out = Vec::new();
        print_read_context(
            &mut out,
            &doc,
            &[
                ResolvedLine {
                    index: 3,
                    line_no: 4,
                    short_hash: format_short_hash(doc.lines[3].short_hash),
                },
                ResolvedLine {
                    index: 4,
                    line_no: 5,
                    short_hash: format_short_hash(doc.lines[4].short_hash),
                },
            ],
            0,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered.lines().count(), 2);
        assert!(!rendered.contains("..."));
    }

    #[test]
    fn test_index_format_no_content() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let mut out = Vec::new();
        print_index(&mut out, &doc).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "1:{}\n2:{}\n",
                format_short_hash(doc.lines[0].short_hash),
                format_short_hash(doc.lines[1].short_hash)
            )
        );
    }

    #[test]
    fn test_stats_pretty_output_includes_summary_fields() {
        let stats = FileStats {
            line_count: 3,
            unique_hashes: 3,
            collision_count: 0,
            collision_pairs: vec![],
            collision_pair_count: 0,
            collision_pairs_truncated: false,
            estimated_read_tokens: 12,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "bare-or-qualified",
            recommended_workflow: "read -> annotate/grep -> verify -> edit/patch -> verify",
            warnings: vec![],
        };
        let mut out = Vec::new();
        print_stats(&mut out, &stats).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("Lines: 3"));
        assert!(rendered.contains("Unique hashes (2-char): 3"));
        assert!(rendered.contains("Hash length advice: 2-char recommended"));
        assert!(rendered.contains("Recommended read mode: read"));
        assert!(rendered.contains("Warnings: none"));
    }

    #[test]
    fn test_read_json_valid() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let mut out = Vec::new();
        let payload = read_payload(&doc, &[], 0).unwrap();
        print_read_json(&mut out, &payload, JsonStyle::Compact).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["file"], "demo.txt");
        assert_eq!(parsed["newline"], "lf");
        assert_eq!(parsed["lines"][0]["content"], "alpha");
        assert_eq!(
            parsed["lines"][1]["hash"],
            format_short_hash(doc.lines[1].short_hash)
        );
        // Compact JSON contains no indentation/newlines except the trailing one.
        let rendered = String::from_utf8(out).unwrap();
        let trimmed = rendered.trim_end_matches('\n');
        assert!(
            !trimmed.contains('\n'),
            "compact JSON should be single line, got: {trimmed:?}"
        );
    }

    #[test]
    fn test_read_json_pretty_emits_multiline() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let mut out = Vec::new();
        let payload = read_payload(&doc, &[], 0).unwrap();
        print_read_json(&mut out, &payload, JsonStyle::Pretty).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        // Pretty-printed JSON spans multiple lines with two-space indentation.
        assert!(rendered.contains("\n  \"file\""));
    }

    #[test]
    fn test_index_json_valid() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        let mut out = Vec::new();
        let payload = index_payload(&doc);
        print_index_json(&mut out, &payload, JsonStyle::Compact).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["file"], "demo.txt");
        assert_eq!(parsed["lines"][0]["n"], 1);
        assert_eq!(
            parsed["lines"][0]["hash"],
            format_short_hash(doc.lines[0].short_hash)
        );
        assert!(parsed["lines"][0].get("content").is_none());
    }

    #[test]
    fn test_stats_json_valid() {
        let stats = FileStats {
            line_count: 1,
            unique_hashes: 1,
            collision_count: 0,
            collision_pairs: vec![],
            collision_pair_count: 0,
            collision_pairs_truncated: false,
            estimated_read_tokens: 2,
            hash_length_advice: 2,
            suggested_context_n: 5,
            recommended_read_mode: "read",
            recommended_anchor_mode: "bare-or-qualified",
            recommended_workflow: "read -> annotate/grep -> verify -> edit/patch -> verify",
            warnings: vec![],
        };
        let mut out = Vec::new();
        print_stats_json(&mut out, &stats, JsonStyle::Compact).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["line_count"], 1);
        assert_eq!(parsed["hash_length_advice"], 2);
    }

    fn numbered_doc(count: usize) -> Document {
        let content = (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        Document::from_str(Path::new("demo.txt"), &content).unwrap()
    }

    #[test]
    fn streaming_read_json_matches_payload_json() {
        // The streaming path bypasses ReadPayload's Vec<LineView> allocation
        // and emits JSON byte-by-byte. It must produce semantically identical
        // JSON to the existing payload-based path or downstream consumers
        // (MCP clients, jq pipelines, ...) will break silently.
        let doc = numbered_doc(7);

        let mut streamed = Vec::new();
        print_read_json_streaming(&mut streamed, &doc, JsonStyle::Compact).unwrap();
        let streamed_val: serde_json::Value = serde_json::from_slice(&streamed).unwrap();

        let payload = read_payload(&doc, &[], 0).unwrap();
        let mut payload_bytes = Vec::new();
        print_read_json(&mut payload_bytes, &payload, JsonStyle::Compact).unwrap();
        let payload_val: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap();

        assert_eq!(streamed_val, payload_val);
    }

    #[test]
    fn streaming_read_json_escapes_special_characters() {
        // Quote, backslash, newline and unicode in line content must be
        // escaped per JSON, even though we hand-write the surrounding
        // wrapper instead of going through serde_json::to_writer for it.
        let content = "plain\nwith \"quote\" and \\ backslash\ntab\there and é\n";
        let doc = Document::from_str(Path::new("special.txt"), content).unwrap();

        let mut streamed = Vec::new();
        print_read_json_streaming(&mut streamed, &doc, JsonStyle::Compact).unwrap();
        let val: serde_json::Value = serde_json::from_slice(&streamed).unwrap();
        let lines = val["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["content"], "plain");
        assert_eq!(lines[1]["content"], "with \"quote\" and \\ backslash");
        assert_eq!(lines[2]["content"], "tab\there and é");
    }

    #[test]
    fn streaming_read_ndjson_matches_payload_ndjson() {
        use super::{print_read_ndjson, print_read_ndjson_streaming};

        let doc = numbered_doc(5);

        let mut streamed = Vec::new();
        print_read_ndjson_streaming(&mut streamed, &doc).unwrap();

        let payload = read_payload(&doc, &[], 0).unwrap();
        let mut payload_bytes = Vec::new();
        print_read_ndjson(&mut payload_bytes, &payload).unwrap();

        assert_eq!(streamed, payload_bytes);
    }
}

// ============================================================================
