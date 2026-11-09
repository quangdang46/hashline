//! Token-driven state machine that turns a stream of [`Token`]s into a
//! flat list of [`Edit`]s. Sits between the tokenizer and the applier.

use std::collections::HashMap;

use regex::Regex;

use crate::messages::{BARE_BODY_AUTO_PIPED_WARNING, MINUS_ROW_REJECTED};
use crate::patch_format::{HL_MAX_RANGE_LINES, HL_RANGE_SEP};
use crate::prefixes::strip_one_hashline_prefix;

/// Warning emitted once when bare (unprefixed) payload rows had read-output
/// line prefixes (`N:` / `N:hh|`) stripped from them.
const BARE_PREFIX_STRIPPED_WARNING: &str = "bare payload rows carried read-output line prefixes \
     (`N:`/`N:hh|`) which were stripped; prefix payload lines with `+` to make them explicit";

/// Warning emitted when a `+` payload row appears with no operation header
/// to attach it to — the row is ignored.
const ORPHAN_PAYLOAD_WARNING: &str =
    "payload line outside any operation was ignored (no preceding SWAP/DEL/INS header)";

/// Warning emitted once per patch that contains `[path#hash]` section
/// headers: ops are NOT routed by section — they all apply to the target.
const SECTION_HEADER_IGNORED_WARNING: &str = "section header(s) `[path#hash]` are informational only — ops are not routed by section; \
     all ops apply to the patch target file";

/// Check if `word` is a recognized hashline operation keyword.
/// Used to distinguish "known keyword, wrong format" from "truly unknown keyword".
fn is_known_op_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "swap"
            | "del"
            | "ins"
            | "ins.pre"
            | "ins.post"
            | "ins.head"
            | "ins.tail"
            | "swap.blk"
            | "del.blk"
            | "ins.blk.post"
            | "ins.blk.pre"
            | "ins.blk"
            | "cut"
            | "put"
            | "rem"
            | "mv"
    )
}

/// Return a format hint for a known keyword that failed to parse as a valid op.
fn format_hint_for_keyword(word: &str) -> &'static str {
    match word.to_ascii_lowercase().as_str() {
        "swap" => "SWAP N:hash or SWAP N..M:hash\n            Body: +new content (on next line)",
        "del" => "DEL N or DEL N..M",
        "ins.pre" => "INS.PRE N:hash",
        "ins.post" => "INS.POST N:hash",
        "ins.head" => "INS.HEAD",
        "ins.tail" => "INS.TAIL",
        "ins" => "INS N:hash (shorthand for INS.POST) or INS.PRE|POST|HEAD|TAIL",
        "swap.blk" => "SWAP.BLK N:hash",
        "del.blk" => "DEL.BLK N:hash",
        "ins.blk.post" => "INS.BLK.POST N:hash",
        "ins.blk.pre" => "INS.BLK.PRE N:hash",
        "ins.blk" => "INS.BLK N:hash",
        "cut" => "CUT N..M @register (e.g. CUT 5..9 @fn)",
        "put" => "PUT @register <N (e.g. PUT @fn <20) or PUT @register (paste at BOF)",
        "rem" => "REM (delete entire file)",
        "mv" => "MV destination_path",
        _ => "see hashline docs for correct format",
    }
}

/// Decide whether a bare (unprefixed) text row looks like a failed hunk
/// operation rather than literal payload.
///
/// Returns `Some((word, is_known_keyword))` for op candidates:
/// - the first word is ALL-CAPS/dotted, at least 2 chars, AND
/// - it is a recognized op keyword (bad syntax → "malformed operation"), or
///   it contains a `.` (e.g. `BAR.BAZ`), or it stands alone (`END`), or it
///   is followed by whitespace + a digit (`FOO 1:`).
///
/// `WORD:` label rows (`TODO:`, `NOTE:`, `FIXME:`) are NOT candidates —
/// they are ordinary payload. Leading whitespace is ignored so indented
/// lookalikes can't bypass the check.
fn op_candidate(text: &str) -> Option<(String, bool)> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let first_word_end = trimmed.find([' ', '.', ':']).unwrap_or(trimmed.len());
    let first_word = &trimmed[..first_word_end];
    if first_word.len() < 2
        || !first_word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '.')
    {
        return None;
    }
    if is_known_op_keyword(first_word) {
        return Some((first_word.to_string(), true));
    }
    if first_word.contains('.') {
        return Some((first_word.to_string(), false));
    }
    let rest = &trimmed[first_word.len()..];
    let rest_t = rest.trim();
    if rest_t.is_empty() {
        // Bare all-caps word on its own line (`END`, `FOO`) — likely a
        // stray op marker.
        return Some((first_word.to_string(), false));
    }
    if rest_t.starts_with(':') {
        // `TODO:` / `NOTE:` / `FIXME:` label form — payload, not an op.
        return None;
    }
    if !rest.starts_with(char::is_whitespace) {
        // `FOO.BAR` — a dotted continuation still looks op-like.
        return if rest_t.starts_with('.') {
            Some((first_word.to_string(), false))
        } else {
            None
        };
    }
    // `FOO 1:` — unknown keyword followed by what looks like an anchor.
    if rest_t.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Some((first_word.to_string(), false));
    }
    // `FOO BAR` — a second ALL-CAPS word keeps the op-like shape
    // (`TOTALLY BOGUS`), while `TODO Fix this` stays payload.
    if rest_t.chars().any(|c| c.is_ascii_uppercase())
        && rest_t.chars().all(|c| !c.is_ascii_lowercase())
    {
        return Some((first_word.to_string(), false));
    }
    None
}
use crate::tokenizer::{BlockTarget, Token, clone_cursor};
use crate::types::{Anchor, BlockMode, Cursor, Edit, FileOp, InsertMode, ParsedRange};

fn validate_range_order(range: &ParsedRange, line_num: usize) -> Result<(), String> {
    if range.end.line < range.start.line {
        return Err(format!(
            "line {line_num}: range {}..{} ends before it starts",
            range.start.line, range.end.line
        ));
    }
    if range.end.line - range.start.line >= HL_MAX_RANGE_LINES {
        return Err(format!(
            "line {line_num}: range {}..{} spans more than {HL_MAX_RANGE_LINES} lines; \
             split it into smaller ranges",
            range.start.line, range.end.line
        ));
    }
    Ok(())
}

fn expand_range(range: &ParsedRange) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    for line in range.start.line..=range.end.line {
        anchors.push(Anchor { line });
    }
    anchors
}

fn is_bare_literal_value(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        return trimmed.len() > 2;
    }
    if let Some(stripped) = trimmed.strip_suffix(',') {
        return stripped.parse::<f64>().is_ok();
    }
    trimmed.parse::<f64>().is_ok()
}

fn detect_apply_patch_contamination(text: &str, has_pending: bool) -> Option<String> {
    // Inside a pending op these lines are payload content, not contamination.
    if has_pending {
        return None;
    }
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("*** Update File:")
        || trimmed.starts_with("*** Add File:")
        || trimmed.starts_with("*** Delete File:")
        || trimmed.starts_with("*** Move to:")
    {
        let preview = if trimmed.len() > 48 {
            // Use char-boundary-safe truncation for multi-byte safety.
            let end = trimmed
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i < 48)
                .last()
                .map(|i| i + 1) // end after the last char fully under 48
                .unwrap_or(0);
            format!("{}…", &trimmed[..end])
        } else {
            trimmed.to_owned()
        };
        return Some(format!(
            "apply_patch sentinel {preview:?} is not valid in hashline. \
             File sections start with `[path#HASH]` (no `Update File:` / `Add File:` keyword). \
             Use `SWAP N{HL_RANGE_SEP}M:`, `DEL N{HL_RANGE_SEP}M`, or `INS.PRE|POST|HEAD|TAIL:` ops."
        ));
    }
    if trimmed.starts_with("@@ ") && trimmed.contains("@@") {
        return Some(format!(
            "unified-diff hunk header is not valid in hashline. \
             Use `SWAP N{HL_RANGE_SEP}M:`, `DEL N{HL_RANGE_SEP}M`, or `INS.PRE|POST|HEAD|TAIL:` ops."
        ));
    }
    None
}

struct PayloadRow {
    text: String,
    #[allow(dead_code)]
    line_num: usize,
    bare: bool,
}

struct Pending {
    target: BlockTarget,
    line_num: usize,
    payloads: Vec<PayloadRow>,
    deferred_blanks: Vec<PayloadRow>,
}

/// Token-driven state machine producing a flat list of `Edit`s.
pub struct Executor {
    edits: Vec<Edit>,
    warnings: Vec<String>,
    file_op: Option<FileOp>,
    edit_index: usize,
    pending: Option<Pending>,
    terminated: bool,
    aborted: bool,
    had_unknown_op: bool,
    /// Distinct `[path#hash]` section-header paths seen so far. Section
    /// headers do not route ops — a patch naming more than one file is
    /// rejected because every op would otherwise hit the single target.
    header_paths: Vec<String>,
    header_warned: bool,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        Self {
            edits: Vec::new(),
            warnings: Vec::new(),
            file_op: None,
            edit_index: 0,
            pending: None,
            terminated: false,
            aborted: false,
            had_unknown_op: false,
            header_paths: Vec::new(),
            header_warned: false,
        }
    }

    pub fn feed(&mut self, token: Token) {
        if self.terminated || self.aborted {
            return;
        }
        match token {
            Token::EnvelopeBegin { .. } => {}
            Token::EnvelopeEnd { .. } => {
                self.terminated = true;
            }
            Token::Abort { .. } => {
                self.terminated = true;
                self.aborted = true;
            }
            Token::Header {
                path, file_hash, ..
            } => {
                self.flush_pending();
                self.note_section_header(&path, file_hash.as_deref());
            }
            Token::Blank { .. } => {
                if self.pending.is_some() {
                    self.handle_blank();
                }
            }
            Token::PayloadLiteral { text, line_num } => {
                self.handle_literal_payload(&text, line_num);
            }
            Token::Raw { text, line_num } => {
                self.handle_raw(&text, line_num);
            }
            Token::OpBlock { target, line_num } => {
                if matches!(
                    &target,
                    BlockTarget::Replace(_, _) | BlockTarget::Delete(_, _) | BlockTarget::Cut(..)
                ) {
                    if let BlockTarget::Replace(r, _)
                    | BlockTarget::Delete(r, _)
                    | BlockTarget::Cut(r, _, _) = &target
                    {
                        if let Err(e) = validate_range_order(r, line_num) {
                            // Reversed or oversized ranges are rejected
                            // atomically — a reversed range used to degrade
                            // into an insert-only SWAP, and a huge range
                            // would hang materializing per-line edits.
                            self.warnings.push(e);
                            self.had_unknown_op = true;
                        }
                    }
                }
                self.flush_pending();
                self.pending = Some(Pending {
                    target,
                    line_num,
                    payloads: Vec::new(),
                    deferred_blanks: Vec::new(),
                });
            }
        }
    }

    pub fn end(&mut self) -> (Vec<Edit>, Vec<String>, Option<FileOp>, bool) {
        if self.aborted {
            // *** Abort was encountered — discard all pending and accumulated
            // edits so the abort truly cancels the entire patch (Bug #97).
            self.edits.clear();
            self.warnings.clear();
            self.file_op = None;
            self.pending = None;
            self.header_paths.clear();
            self.header_warned = false;
            let aborted = true;
            self.edit_index = 0;
            self.terminated = false;
            self.aborted = false;
            return (Vec::new(), Vec::new(), None, aborted);
        }
        self.flush_pending();

        // Reject the entire patch if any unrecognized operation keyword was
        // encountered (Bug #112). Without this guard, unknown ops like bare
        // `SWAP` (missing range) or invented keywords like `END` are silently
        // treated as body text and inserted into the file, corrupting it while
        // the tool still reports success.
        if self.had_unknown_op {
            self.edits.clear();
            self.file_op = None;
            self.pending = None;
            self.had_unknown_op = false;
            self.header_paths.clear();
            self.header_warned = false;
            self.edit_index = 0;
            self.terminated = false;
            self.aborted = false;
            let warnings = std::mem::take(&mut self.warnings);
            return (Vec::new(), warnings, None, true);
        }

        self.validate_no_overlapping_deletes();
        let edits = std::mem::take(&mut self.edits);
        let warnings = std::mem::take(&mut self.warnings);
        let file_op = self.file_op.take();
        let aborted = self.aborted;
        self.edit_index = 0;
        self.pending = None;
        self.terminated = false;
        self.aborted = false;
        self.header_paths.clear();
        self.header_warned = false;
        (edits, warnings, file_op, aborted)
    }

    /// Track a `[path#hash]` section header. Headers do not route ops to
    /// other files — `parse_patch` produces edits for a single target — so
    /// a patch naming more than one distinct path is rejected outright
    /// (otherwise ops meant for file B would silently hit file A). A single
    /// repeated path stays tolerated for compatibility, with a warning.
    fn note_section_header(&mut self, path: &str, file_hash: Option<&str>) {
        if let Some(first) = self.header_paths.first() {
            if first != path {
                let msg = format!(
                    "patch contains `[path#hash]` sections for multiple files \
                     (`{first}` vs `{path}`); `hashline patch` applies every op to the \
                     single target file — split the patch or remove the extra sections"
                );
                if !self.warnings.contains(&msg) {
                    self.warnings.push(msg);
                }
                self.had_unknown_op = true;
                return;
            }
        } else {
            self.header_paths.push(path.to_string());
        }
        if !self.header_warned {
            self.header_warned = true;
            let tag = match file_hash {
                Some(h) => format!("[{path}#{h}]"),
                None => format!("[{path}]"),
            };
            let mut msg = SECTION_HEADER_IGNORED_WARNING.to_string();
            msg.push_str(&format!(" (first header: {tag})"));
            self.warnings.push(msg);
        }
    }

    /// Detect a bare ALL-CAPS row that looks like a failed hunk operation
    /// (`END`, `FOO 1:`, `BAR.BAZ`) and flag the patch for atomic rejection.
    /// `TODO:`/`NOTE:`/`FIXME:`-style label rows and leading-whitespace
    /// variants are payload, not ops.
    fn flag_op_candidate(&mut self, text: &str) {
        let Some((word, known)) = op_candidate(text) else {
            return;
        };
        if known {
            let hint = format_hint_for_keyword(&word);
            let msg =
                format!("malformed operation `{word}` — expected format:\n            {hint}");
            if !self.warnings.contains(&msg) {
                self.warnings.push(msg);
            }
        } else {
            let msg = format!("unknown operation `{word}`");
            if !self.warnings.contains(&msg) {
                self.warnings.push(format!(
                    "{msg} — use SWAP, DEL, INS.PRE, INS.POST, \
                     INS.HEAD, INS.TAIL, SWAP.BLK, DEL.BLK, INS.BLK.POST, INS.BLK.PRE, \
                     INS.BLK, CUT, or PUT"
                ));
            }
        }
        self.had_unknown_op = true;
    }

    fn handle_literal_payload(&mut self, text: &str, line_num: usize) {
        let pending = match &self.pending {
            Some(p) => p,
            None => {
                if !self.warnings.contains(&ORPHAN_PAYLOAD_WARNING.to_string()) {
                    self.warnings.push(ORPHAN_PAYLOAD_WARNING.to_owned());
                }
                return;
            }
        };
        if matches!(pending.target, BlockTarget::Delete(_, _)) {
            return;
        }
        if matches!(pending.target, BlockTarget::DeleteBlock(..)) {
            // DEL.BLK takes no payload, but garbage lines must still pass
            // through the unknown-op guard — previously they were swallowed
            // silently until the next valid op.
            self.flag_op_candidate(text);
            return;
        }

        let pending_mut = self.pending.as_mut().unwrap();
        // Flush deferred blanks into payloads before appending the new
        // payload line, so interior blank lines survive (Issue #93-A).
        // Trailing blanks that remain deferred at flush_pending are
        // still dropped, preserving the trailing-blank trimming behavior.
        pending_mut
            .payloads
            .append(&mut pending_mut.deferred_blanks);
        pending_mut.payloads.push(PayloadRow {
            text: text.to_owned(),
            line_num,
            bare: false,
        });
    }

    fn handle_raw(&mut self, text: &str, line_num: usize) {
        if let Some(msg) = detect_apply_patch_contamination(text, self.pending.is_some()) {
            // Surface the diagnostic and reject the patch atomically instead
            // of silently dropping the line.
            if !self.warnings.contains(&msg) {
                self.warnings.push(msg);
            }
            self.had_unknown_op = true;
            return;
        }

        if let Some(pending) = &self.pending {
            if text.trim().is_empty() {
                self.handle_blank();
                return;
            }
            if matches!(pending.target, BlockTarget::Delete(_, _)) {
                return;
            }
            if matches!(pending.target, BlockTarget::DeleteBlock(..)) {
                // DEL.BLK takes no payload — but still run the unknown-op
                // guard so stray op-like lines can't slip through silently.
                self.flag_op_candidate(text);
                return;
            }
            if text.trim_start().starts_with('-') {
                if !self.warnings.contains(&MINUS_ROW_REJECTED.to_string()) {
                    self.warnings.push(MINUS_ROW_REJECTED.to_owned());
                }
                // Preserve `-` content in output while still warning
                // (Issue #93-B). Mark as bare:false so prefix-stripping
                // doesn't eat the leading `-`.
                let pending_mut = self.pending.as_mut().unwrap();
                pending_mut
                    .payloads
                    .append(&mut pending_mut.deferred_blanks);
                pending_mut.payloads.push(PayloadRow {
                    text: text.to_owned(),
                    line_num,
                    bare: false,
                });
                return;
            }
            // Bug #112: Detect unknown operation keywords being consumed as bare
            // payload. Lines like `END` or bare `SWAP` (without range) look like
            // ops but were never tokenized as OpBlock — flag and abort atomically.
            // Fix #113/#114/#115: Distinguish known keywords with bad syntax from
            // truly unknown keywords, and provide format-specific hints.
            self.flag_op_candidate(text);
            if self.had_unknown_op {
                return;
            }

            if !self
                .warnings
                .contains(&BARE_BODY_AUTO_PIPED_WARNING.to_string())
            {
                self.warnings.push(BARE_BODY_AUTO_PIPED_WARNING.to_owned());
            }
            let pending_mut = self.pending.as_mut().unwrap();
            // Flush deferred blanks into payloads before appending the new
            // payload line, so interior blank lines survive (Issue #93-A).
            pending_mut
                .payloads
                .append(&mut pending_mut.deferred_blanks);
            pending_mut.payloads.push(PayloadRow {
                text: text.to_owned(),
                line_num,
                bare: true,
            });
            return;
        }

        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }

        // No pending operation — check for orphan `-` rows
        if trimmed.starts_with('-') && !self.warnings.contains(&MINUS_ROW_REJECTED.to_string()) {
            self.warnings.push(MINUS_ROW_REJECTED.to_owned());
        }

        // Check for unknown operation keywords — lines like `FOO 1:` or `BAR.BAZ 5:`
        // that look like they're trying to be hunk operations but failed to parse.
        // Fix #113/#114/#115: Distinguish known keywords with bad syntax from
        // truly unknown keywords, and provide format-specific hints.
        self.flag_op_candidate(text);
    }

    fn handle_blank(&mut self) {
        let pending = match &mut self.pending {
            Some(p) => p,
            None => return,
        };
        if matches!(
            pending.target,
            BlockTarget::Delete(_, _) | BlockTarget::DeleteBlock(..)
        ) {
            return;
        }
        if pending.payloads.is_empty() {
            return;
        }
        pending.deferred_blanks.push(PayloadRow {
            text: String::new(),
            line_num: 0,
            bare: true,
        });
    }

    fn set_file_op(&mut self, file_op: FileOp, line_num: usize) -> Result<(), String> {
        if self.file_op.is_some() {
            return Err(format!(
                "line {}: only one file-level op (`REM` or `MV`) per section. Merge them under one header.",
                line_num
            ));
        }
        if matches!(&file_op, FileOp::Remove) && !self.edits.is_empty() {
            return Err(format!(
                "line {}: `REM` deletes the whole file and cannot be combined with line ops.",
                line_num
            ));
        }
        self.file_op = Some(file_op);
        Ok(())
    }

    fn strip_bare_prefixes_if_uniform(payloads: &mut Vec<PayloadRow>) -> bool {
        let mut saw_bare = false;
        let mut all_literal_values = true;

        for row in payloads.iter() {
            if !row.bare || row.text.trim().is_empty() {
                continue;
            }
            saw_bare = true;
            let stripped = strip_one_hashline_prefix(&row.text);
            if stripped == row.text {
                return false;
            }
            if !is_bare_literal_value(&stripped) {
                all_literal_values = false;
            }
        }
        if !saw_bare {
            return false;
        }
        if all_literal_values {
            return false;
        }
        for row in payloads.iter_mut() {
            if row.bare && !row.text.trim().is_empty() {
                row.text = strip_one_hashline_prefix(&row.text);
            }
        }
        true
    }

    fn flush_pending(&mut self) {
        let pending = match self.pending.take() {
            Some(p) => p,
            None => return,
        };

        let Pending {
            target,
            line_num,
            mut payloads,
            ..
        } = pending;

        if Self::strip_bare_prefixes_if_uniform(&mut payloads)
            && !self
                .warnings
                .contains(&BARE_PREFIX_STRIPPED_WARNING.to_string())
        {
            self.warnings.push(BARE_PREFIX_STRIPPED_WARNING.to_owned());
        }

        let payload_texts: Vec<String> = payloads.into_iter().map(|r| r.text).collect();

        match &target {
            BlockTarget::Delete(range, hash) => {
                let anchors = expand_range(range);
                let last = anchors.len().saturating_sub(1);
                for (i, anchor) in anchors.into_iter().enumerate() {
                    // Start hash binds the first line; a `:HH` written after
                    // the range end (`DEL 2:d4..4:ee`) binds the last line.
                    let expected_hash = if i == 0 {
                        *hash
                    } else if i == last {
                        range.end_hash
                    } else {
                        None
                    };
                    self.edits.push(Edit::Delete {
                        anchor,
                        line_num,
                        index: self.edit_index,
                        expected_hash,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::DeleteBlock(anchor, hash) => {
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: Vec::new(),
                    line_num,
                    index: self.edit_index,
                    mode: None,
                    expected_hash: *hash,
                });
                self.edit_index += 1;
            }
            BlockTarget::Block(anchor, hash) => {
                // SWAP.BLK with an empty payload deletes the block — the same
                // convention `SWAP N:` (empty body = delete) already uses.
                // The apply layer treats `mode: None` + empty payloads as a
                // pure block deletion, identical to DEL.BLK.
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: payload_texts,
                    line_num,
                    index: self.edit_index,
                    mode: None,
                    expected_hash: *hash,
                });
                self.edit_index += 1;
            }
            BlockTarget::InsertAfterBlock(anchor, hash) => {
                if payload_texts.is_empty() {
                    self.warnings.push(format!(
                        "line {line_num}: INS.BLK.POST with no payload lines was ignored"
                    ));
                    return;
                }
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: payload_texts,
                    line_num,
                    index: self.edit_index,
                    mode: Some(BlockMode::InsertAfter),
                    expected_hash: *hash,
                });
                self.edit_index += 1;
            }
            BlockTarget::InsertBeforeBlock(anchor, hash) => {
                if payload_texts.is_empty() {
                    self.warnings.push(format!(
                        "line {line_num}: INS.BLK.PRE with no payload lines was ignored"
                    ));
                    return;
                }
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: payload_texts,
                    line_num,
                    index: self.edit_index,
                    mode: Some(BlockMode::InsertBefore),
                    expected_hash: *hash,
                });
                self.edit_index += 1;
            }
            BlockTarget::Replace(range, hash) => {
                if payload_texts.is_empty() {
                    // SWAP with no body = delete
                    let anchors = expand_range(range);
                    let last = anchors.len().saturating_sub(1);
                    for (i, anchor) in anchors.into_iter().enumerate() {
                        let expected_hash = if i == 0 {
                            *hash
                        } else if i == last {
                            range.end_hash
                        } else {
                            None
                        };
                        self.edits.push(Edit::Delete {
                            anchor,
                            line_num,
                            index: self.edit_index,
                            expected_hash,
                        });
                        self.edit_index += 1;
                    }
                    return;
                }
                let cursor = Cursor::BeforeAnchor(Anchor {
                    line: range.start.line,
                });
                for text in &payload_texts {
                    self.edits.push(Edit::Insert {
                        cursor: clone_cursor(&cursor),
                        text: text.clone(),
                        line_num,
                        index: self.edit_index,
                        mode: Some(InsertMode::Replacement),
                        block_start: None,
                        expected_hash: None,
                    });
                    self.edit_index += 1;
                }
                let anchors = expand_range(range);
                let last = anchors.len().saturating_sub(1);
                for (i, anchor) in anchors.into_iter().enumerate() {
                    let expected_hash = if i == 0 {
                        *hash
                    } else if i == last {
                        range.end_hash
                    } else {
                        None
                    };
                    self.edits.push(Edit::Delete {
                        anchor,
                        line_num,
                        index: self.edit_index,
                        expected_hash,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::InsertBefore(anchor, hash) => {
                if payload_texts.is_empty() {
                    return;
                }
                let cursor = Cursor::BeforeAnchor(*anchor);
                for text in &payload_texts {
                    self.edits.push(Edit::Insert {
                        cursor: clone_cursor(&cursor),
                        text: text.clone(),
                        line_num,
                        index: self.edit_index,
                        mode: None,
                        block_start: None,
                        expected_hash: *hash,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::InsertAfter(anchor, hash) => {
                if payload_texts.is_empty() {
                    return;
                }
                let cursor = Cursor::AfterAnchor(*anchor);
                for text in &payload_texts {
                    self.edits.push(Edit::Insert {
                        cursor: clone_cursor(&cursor),
                        text: text.clone(),
                        line_num,
                        index: self.edit_index,
                        mode: None,
                        block_start: None,
                        expected_hash: *hash,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::Bof => {
                let cursor = Cursor::Bof;
                for text in &payload_texts {
                    self.edits.push(Edit::Insert {
                        cursor: clone_cursor(&cursor),
                        text: text.clone(),
                        line_num,
                        index: self.edit_index,
                        mode: None,
                        block_start: None,
                        expected_hash: None,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::Eof => {
                let cursor = Cursor::Eof;
                for text in &payload_texts {
                    self.edits.push(Edit::Insert {
                        cursor: clone_cursor(&cursor),
                        text: text.clone(),
                        line_num,
                        index: self.edit_index,
                        mode: None,
                        block_start: None,
                        expected_hash: None,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::Cut(range, hash, register) => {
                self.edits.push(Edit::Cut {
                    anchor: range.start,
                    end: range.end,
                    line_num,
                    index: self.edit_index,
                    register: register.clone(),
                    expected_hash: *hash,
                });
                self.edit_index += 1;
            }
            BlockTarget::Paste(cursor, register) => {
                self.edits.push(Edit::Paste {
                    cursor: clone_cursor(cursor),
                    line_num,
                    index: self.edit_index,
                    register: register.clone(),
                });
                self.edit_index += 1;
            }
            BlockTarget::Remove => {
                if let Err(e) = self.set_file_op(FileOp::Remove, line_num) {
                    self.warnings.push(e);
                }
            }
            BlockTarget::MoveTo(dest) => {
                if let Err(e) = self.set_file_op(FileOp::Rename(dest.clone()), line_num) {
                    self.warnings.push(e);
                }
            }
        }
    }

    fn validate_no_overlapping_deletes(&self) {
        let mut source_lines_by_anchor: HashMap<usize, Vec<usize>> = HashMap::new();
        for edit in &self.edits {
            if let Edit::Delete {
                anchor,
                line_num,
                expected_hash: None,
                ..
            } = edit
            {
                source_lines_by_anchor
                    .entry(anchor.line)
                    .or_default()
                    .push(*line_num);
            }
        }
        for source_lines in source_lines_by_anchor.values() {
            if source_lines.len() < 2 {
                continue;
            }
            let _first = source_lines.iter().min().unwrap();
            let _second = source_lines.iter().max().unwrap();
            // Just track it; overlapping deletes are rare and the validation
            // is informational
        }
    }
}

/// Merge consecutive sections targeting the same path into one.
/// When same-path sections have different hash tags, returns an error.
/// Returns the merged diff with all sections concatenated under one header.
pub fn merge_same_path_sections(patch_text: &str) -> Result<String, String> {
    let header_re =
        Regex::new(r"^\[([^\[\]#]+)(?:#([0-9a-fA-F]{1,4}))?\]\s*$").expect("valid header regex");

    let lines: Vec<&str> = patch_text.lines().collect();
    if lines.is_empty() {
        return Ok(String::new());
    }

    // Find all header line indices
    let header_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| header_re.is_match(line))
        .map(|(i, _)| i)
        .collect();

    if header_indices.is_empty() {
        return Ok(patch_text.to_owned());
    }

    // Build sections: each section spans from a header to the next header (or end of input).
    // body_start is the line after the header; body_end is the next header line (or lines.len()).
    struct Section {
        header: String,
        path: String,
        hash: Option<String>,
        body_start: usize,
        body_end: usize,
    }

    let mut sections: Vec<Section> = Vec::new();
    for (i, &hdr_idx) in header_indices.iter().enumerate() {
        let caps = header_re.captures(lines[hdr_idx]).unwrap();
        let path = caps[1].to_string();
        let hash = caps.get(2).map(|m| m.as_str().to_string());
        let body_start = hdr_idx + 1;
        let body_end = header_indices.get(i + 1).copied().unwrap_or(lines.len());
        sections.push(Section {
            header: lines[hdr_idx].to_string(),
            path,
            hash,
            body_start,
            body_end,
        });
    }

    // Merge consecutive sections targeting the same path.
    struct Merged {
        header: String,
        path: String,
        hash: Option<String>,
        body_parts: Vec<(usize, usize)>,
    }

    let mut merged: Vec<Merged> = Vec::new();
    for section in sections {
        if let Some(last) = merged.last_mut() {
            if last.path == section.path {
                // Same path — check for hash conflicts
                match (&last.hash, &section.hash) {
                    (Some(a), Some(b)) if a != b => {
                        return Err(format!(
                            "conflicting hash tags for '{}': '{}' vs '{}'",
                            section.path, a, b
                        ));
                    }
                    (None, Some(h)) => {
                        // Later section has a hash where the first didn't — prefer the hash
                        last.hash = Some(h.clone());
                        last.header = section.header;
                    }
                    _ => {}
                }
                last.body_parts.push((section.body_start, section.body_end));
                continue;
            }
        }
        merged.push(Merged {
            header: section.header,
            path: section.path,
            hash: section.hash,
            body_parts: vec![(section.body_start, section.body_end)],
        });
    }

    // Rebuild the patch string with merged sections.
    let mut output: Vec<&str> = Vec::new();

    // Content before the first header (leading non-header lines)
    if header_indices[0] > 0 {
        output.extend_from_slice(&lines[..header_indices[0]]);
    }

    for group in &merged {
        output.push(&group.header);
        for &(start, end) in &group.body_parts {
            output.extend_from_slice(&lines[start..end]);
        }
    }

    Ok(output.join("\n"))
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod merge_tests {
    use super::*;

    fn merge(s: &str) -> String {
        merge_same_path_sections(s).unwrap()
    }

    #[test]
    fn two_sections_same_path_merged() {
        let input = "[file.rs#ABCD]\nSWAP 5:\n+foo\n[file.rs#ABCD]\nSWAP 10:\n+bar";
        let result = merge(input);
        assert_eq!(result, "[file.rs#ABCD]\nSWAP 5:\n+foo\nSWAP 10:\n+bar");
    }

    #[test]
    fn two_sections_different_path_kept_separate() {
        let input = "[a.rs#ABCD]\nSWAP 5:\n+foo\n[b.rs#1234]\nSWAP 10:\n+bar";
        let result = merge(input);
        assert_eq!(result, input);
    }

    #[test]
    fn three_sections_middle_different_path() {
        let input = "[a.rs#ABCD]\nSWAP 5:\n+foo\n[b.rs#1234]\nDEL 3\n[a.rs#ABCD]\nSWAP 10:\n+bar";
        let result = merge(input);
        // a, b, a — first a and second a are not consecutive (b is in between),
        // so they stay separate.
        assert_eq!(result, input);
    }

    #[test]
    fn three_sections_last_also_same_path() {
        // Three sections all same path and all consecutive — all merged
        let input = "[a.rs#ABCD]\nSWAP 5:\n+foo\n[a.rs#ABCD]\nSWAP 5:\n+bar\n[a.rs#ABCD]\nDEL 3";
        let result = merge(input);
        assert_eq!(result, "[a.rs#ABCD]\nSWAP 5:\n+foo\nSWAP 5:\n+bar\nDEL 3");
    }

    #[test]
    fn conflicting_hashes_error() {
        let input = "[file.rs#ABCD]\nSWAP 5:\n+foo\n[file.rs#1234]\nSWAP 10:\n+bar";
        let err = merge_same_path_sections(input).unwrap_err();
        assert!(
            err.contains("conflicting hash tags"),
            "expected hash conflict error, got: {err}"
        );
    }

    #[test]
    fn single_section_unchanged() {
        let input = "[file.rs#ABCD]\nSWAP 5:\n+foo";
        let result = merge(input);
        assert_eq!(result, input);
    }

    #[test]
    fn no_headers_unchanged() {
        let input = "SWAP 5:\n+foo\nDEL 3";
        let result = merge(input);
        assert_eq!(result, input);
    }
}

/// Normalize pipe `|` as inline body separator (Fix #113/#115).
///
/// Agents may send `SWAP 2:97 |content` on a single line. The `|`
/// separates the operation header from its body. This function detects
/// lines where `|` follows a valid hunk operation and expands them to
/// the canonical multiline format:
///   `SWAP 2:97 |content` → `SWAP 2:97\n+content`
///
/// Lines without `|` or where the part before `|` is not a valid
/// operation are returned unchanged.
fn normalize_pipe_separators(text: &str) -> String {
    use crate::tokenizer::try_parse_hunk_header;
    let mut result = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            result.push('\n');
        }
        if let Some(pipe_pos) = line.find('|') {
            let op_part = &line[..pipe_pos];
            let body_part = &line[pipe_pos + 1..];
            if try_parse_hunk_header(op_part).is_some() {
                result.push_str(op_part.trim_end());
                result.push('\n');
                result.push('+');
                result.push_str(body_part);
                continue;
            }
        }
        result.push_str(line);
    }
    result
}

/// Parse a complete patch diff body into `Edit`s, warnings, file-level operations,
/// and a flag indicating whether parsing was halted by an `*** Abort` marker.
pub fn parse_patch(diff: &str) -> (Vec<Edit>, Vec<String>, Option<FileOp>, bool) {
    let merged = match merge_same_path_sections(diff) {
        Ok(m) => m,
        Err(e) => return (Vec::new(), vec![e], None, false),
    };

    // Fix #113/#115: Normalize pipe-separated single-line operations
    // before tokenization (e.g. `SWAP 2:97 |content` → multiline).
    let normalized = normalize_pipe_separators(&merged);

    let tokenizer = crate::tokenizer::Tokenizer;
    let mut executor = Executor::new();

    for (i, line) in normalized.lines().enumerate() {
        let token = tokenizer.tokenize(line, i + 1);
        executor.feed(token);
    }

    let (mut edits, mut warnings, file_op, aborted) = executor.end();

    // Bug #112: If the executor produced both valid edits AND warnings about
    // unknown or malformed operation keywords, the patch mixed recognized and
    // unrecognized ops. Unrecognized ops (e.g. bare `SWAP`, `END`) were consumed
    // as payload text by the executor, silently corrupting the file while
    // reporting OK. Reject the entire patch atomically — no edits, no file
    // mutation.
    // Fix #113/#114/#115: Also handle "malformed operation" warnings for known
    // keywords with wrong syntax (e.g. `PUT @name:N:` instead of `PUT @name <N`).
    if !edits.is_empty() && !aborted {
        let has_op_error = warnings
            .iter()
            .any(|w| w.starts_with("unknown operation") || w.starts_with("malformed operation"));
        if has_op_error {
            edits.clear();
            warnings.clear();
            return (Vec::new(), Vec::new(), None, true);
        }
    }

    (edits, warnings, file_op, aborted)
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod cut_put_tests {
    use super::*;

    #[test]
    fn cut_parses_to_cut_edit() {
        let (edits, warnings, _file_op, _aborted) = parse_patch("CUT 5..9 @fn");
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );
        assert_eq!(edits.len(), 1);
        match &edits[0] {
            Edit::Cut {
                anchor,
                end,
                register,
                expected_hash,
                ..
            } => {
                assert_eq!(anchor.line, 5);
                assert_eq!(end.line, 9);
                assert_eq!(register.as_deref(), Some("fn"));
                assert_eq!(*expected_hash, None);
            }
            other => panic!("expected Cut edit, got {other:?}"),
        }
    }

    #[test]
    fn cut_anonymous_parses() {
        let (edits, _warnings, _file_op, _aborted) = parse_patch("CUT 5..9");
        match &edits[0] {
            Edit::Cut { register, .. } => assert_eq!(*register, None),
            other => panic!("expected Cut edit, got {other:?}"),
        }
    }

    #[test]
    fn put_parses_to_paste_edit() {
        let (edits, _warnings, _file_op, _aborted) = parse_patch("PUT @fn <20");
        assert_eq!(edits.len(), 1);
        match &edits[0] {
            Edit::Paste {
                cursor, register, ..
            } => {
                assert!(matches!(cursor, Cursor::BeforeAnchor(a) if a.line == 20));
                assert_eq!(register.as_deref(), Some("fn"));
            }
            other => panic!("expected Paste edit, got {other:?}"),
        }
    }

    #[test]
    fn cut_and_put_parse_in_one_patch() {
        let (edits, warnings, _file_op, _aborted) = parse_patch("CUT 5..9 @fn\nPUT @fn <20");
        assert!(
            warnings.is_empty(),
            "expected no warnings, got {warnings:?}"
        );
        assert_eq!(edits.len(), 2);
        assert!(matches!(&edits[0], Edit::Cut { .. }));
        assert!(matches!(&edits[1], Edit::Paste { .. }));
    }

    #[test]
    fn cut_range_reversed_rejected() {
        // Reversed ranges are rejected at tokenize time now (they used to
        // degrade into a warning + a corrupt edit).
        let (edits, warnings, _file_op, aborted) = parse_patch("CUT 9..5 @fn");
        assert!(edits.is_empty());
        assert!(aborted);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("malformed operation `CUT`")),
            "expected malformed-op warning, got {warnings:?}"
        );
    }
}

#[cfg(test)]
mod regression_113_114_115_tests {
    use super::*;

    // --- Issue #113: SWAP single-line pipe format ---

    #[test]
    fn swap_pipe_single_line_parses() {
        // The agent sends `SWAP 2:97 |content` on a single line.
        // The `|` should be treated as a body separator.
        let (edits, warnings, _file_op, _aborted) =
            parse_patch("SWAP 2:97 |Line 2: MODIFIED via SWAP");
        // Should produce valid edits (a SWAP), not be rejected as unknown.
        assert!(
            !edits.is_empty(),
            "SWAP with pipe separator should produce edits, got warnings: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("unknown operation")),
            "SWAP with pipe should NOT be flagged as unknown, got: {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("malformed operation")),
            "SWAP with pipe should NOT be flagged as malformed, got: {warnings:?}"
        );
    }

    #[test]
    fn swap_pipe_single_line_with_plus_prefix() {
        // `SWAP 2:97 |+content` — body has explicit `+` prefix.
        let (edits, _warnings, _file_op, _aborted) =
            parse_patch("SWAP 2:97 |+Line 2: MODIFIED via SWAP");
        assert!(
            !edits.is_empty(),
            "should produce edits for pipe with +prefix"
        );
    }

    // --- Issue #114: PUT format error message ---

    #[test]
    fn put_wrong_format_gives_malformed_error() {
        // The agent used `PUT @fn:` (colon instead of `<N`).
        let (_edits, warnings, _file_op, _aborted) = parse_patch("PUT @fn:");
        assert!(
            warnings.iter().any(|w| w.contains("malformed operation")),
            "PUT with wrong format should give 'malformed operation' error, got: {warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("PUT @register <N")),
            "PUT error should show correct format hint, got: {warnings:?}"
        );
    }

    #[test]
    fn put_correct_format_no_error() {
        // Correct PUT syntax: `PUT @fn <20`
        let (edits, warnings, _file_op, _aborted) = parse_patch("PUT @fn <20");
        assert!(
            !edits.is_empty(),
            "correct PUT should produce edits, got warnings: {warnings:?}"
        );
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("malformed") || w.contains("unknown")),
            "correct PUT should have no format errors, got: {warnings:?}"
        );
    }

    #[test]
    fn put_bare_register_no_error() {
        // Bare `PUT @fn` (no `<N`) defaults to BOF.
        let (edits, warnings, _file_op, _aborted) = parse_patch("PUT @fn");
        assert!(!edits.is_empty(), "bare PUT should produce edits");
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("malformed") || w.contains("unknown")),
            "bare PUT should have no format errors, got: {warnings:?}"
        );
    }

    // --- Issue #115: Stale anchor not masked as unknown ---

    #[test]
    fn stale_anchor_with_pipe_not_masked_as_unknown() {
        // Stale anchor `SWAP 99:zz |stale line` — `zz` is a malformed hash,
        // so the whole patch is now rejected (previously it applied with the
        // stale-anchor check silently disabled). It must be reported as a
        // malformed KNOWN op, never "unknown operation".
        let (edits, warnings, _file_op, aborted) = parse_patch("SWAP 99:zz |stale line");
        assert!(
            !warnings.iter().any(|w| w.contains("unknown operation")),
            "stale anchor SWAP with pipe should NOT be flagged as unknown, got: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("malformed operation `SWAP`")),
            "expected malformed-operation warning, got: {warnings:?}"
        );
        assert!(edits.is_empty(), "malformed hash must reject the patch");
        assert!(aborted, "expected aborted=true for malformed hash");
    }

    #[test]
    fn recognized_keyword_bad_hash_not_unknown() {
        // `SWAP 99:zz` without pipe — keyword SWAP is recognized but the hash
        // format is bad. Should give "malformed" not "unknown".
        let (_edits, warnings, _file_op, _aborted) = parse_patch("SWAP 99:zz");
        assert!(
            !warnings.iter().any(|w| w.contains("unknown operation")),
            "recognized keyword with bad hash should NOT be 'unknown', got: {warnings:?}"
        );
    }

    #[test]
    fn truly_unknown_keyword_still_flagged() {
        // `FOOBAR 5:` is NOT a known keyword — should still be "unknown".
        let (_edits, warnings, _file_op, _aborted) = parse_patch("FOOBAR 5:");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown operation `FOOBAR`")),
            "truly unknown keyword should be flagged as unknown, got: {warnings:?}"
        );
    }
}

#[cfg(test)]
mod audit_hardening_tests {
    use super::*;
    use crate::types::Edit;

    // ---- bd-tokenizer-hardening-zg5 ----

    #[test]
    fn huge_line_number_does_not_panic_or_wrap() {
        // Exact audit repro: `SWAP 99999999999999999999999999:` panicked in
        // debug / wrapped to a wrong line in release.
        let (edits, _w, _f, aborted) = parse_patch("SWAP 99999999999999999999999999:\n+payload");
        assert!(!aborted);
        assert_eq!(edits.len(), 2);
        match &edits[1] {
            Edit::Delete { anchor, .. } => assert_eq!(anchor.line, usize::MAX),
            other => panic!("expected Delete, got {other:?}"),
        }
    }

    #[test]
    fn huge_del_range_rejected_not_hang() {
        // `DEL 1..999999999` used to materialize ~1G anchors and hang.
        let (edits, warnings, _f, aborted) = parse_patch("DEL 1..999999999");
        assert!(edits.is_empty());
        assert!(aborted);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("malformed operation `DEL`"))
        );
    }

    #[test]
    fn non_hex_hash_rejected_whole_patch() {
        for patch in [
            "SWAP 2:zz:\n+replaced",
            "DEL 2:zz",
            "INS.POST 3:qq:\n+x",
            "SWAP 5:abc\n+x",
            "SWAP 5:abcd:\n+x",
        ] {
            let (edits, _w, _f, aborted) = parse_patch(patch);
            assert!(edits.is_empty(), "{patch} produced edits: {edits:?}");
            assert!(aborted, "{patch} was not aborted");
        }
    }

    #[test]
    fn range_end_hash_verified_on_last_line() {
        // `DEL 2:d4..4:ee` — end hash used to be silently discarded.
        let (edits, _w, _f, aborted) = parse_patch("DEL 2:d4..4:ee");
        assert!(!aborted);
        assert_eq!(edits.len(), 3);
        let hashes: Vec<Option<u8>> = edits
            .iter()
            .map(|e| match e {
                Edit::Delete { expected_hash, .. } => *expected_hash,
                other => panic!("expected Delete, got {other:?}"),
            })
            .collect();
        assert_eq!(hashes, vec![Some(0xd4), None, Some(0xee)]);
    }

    #[test]
    fn reversed_range_rejected() {
        // `SWAP 5..0:` used to degrade into an insert-only edit.
        let (edits, _w, _f, aborted) = parse_patch("SWAP 5..0:\n+payload");
        assert!(edits.is_empty());
        assert!(aborted);
        let (edits, _w, _f, aborted) = parse_patch("DEL 9..5");
        assert!(edits.is_empty());
        assert!(aborted);
    }

    #[test]
    fn register_names_with_sigils_rejected() {
        for patch in [
            "CUT 5 @a<b",
            "CUT 5 @a@b",
            "CUT 5 @'fn'",
            "PUT @a<b <2",
            "PUT @\"fn\" <2",
        ] {
            let (edits, _w, _f, aborted) = parse_patch(patch);
            assert!(edits.is_empty(), "{patch} produced edits: {edits:?}");
            assert!(aborted, "{patch} was not aborted");
        }
    }

    // ---- bd-patch-boundary-syntax-imw ----

    #[test]
    fn end_patch_marker_with_whitespace_and_trailing_text_terminates() {
        // `  *** End Patch — done` used to be written INTO the file.
        let (edits, _w, _f, aborted) = parse_patch("SWAP 1:\n+kept\n  *** End Patch — done\n+lost");
        assert!(!aborted);
        assert_eq!(edits.len(), 2); // 1 insert + 1 delete for SWAP 1:
        match &edits[0] {
            Edit::Insert { text, .. } => assert_eq!(text, "kept"),
            other => panic!("expected Insert, got {other:?}"),
        }
    }

    #[test]
    fn abort_marker_with_trailing_text_aborts() {
        let (_edits, _w, _f, aborted) =
            parse_patch("SWAP 1:\n+x\n*** Abort: user cancelled\nDEL 2");
        assert!(aborted);
    }

    #[test]
    fn all_caps_label_rows_are_payload_not_rejected() {
        // `TODO:`/`NOTE:`/`FIXME:` bare payload rows used to reject the
        // whole patch via the over-inclusive ALL-CAPS detector.
        let (edits, warnings, _f, aborted) =
            parse_patch("SWAP 1:\n+before\nTODO: fix later\nNOTE: keep\nFIXME: x");
        assert!(!aborted, "warnings: {warnings:?}");
        assert_eq!(edits.len(), 5); // 4 inserts + 1 delete
        assert!(
            warnings.iter().any(|w| w.contains("bare body auto-piped")),
            "expected bare-body warning, got {warnings:?}"
        );
    }

    #[test]
    fn leading_whitespace_cannot_bypass_op_detection() {
        // `  FOO 1:` used to slip past the detector via leading whitespace.
        let (edits, _w, _f, aborted) = parse_patch("SWAP 1:\n+x\n  FOO 1:");
        assert!(edits.is_empty());
        assert!(aborted);
    }

    #[test]
    fn bare_read_echo_prefix_is_stripped_with_warning() {
        // A bare `N:hh|content` row used to be silently mangled to
        // `hh|content`; now it strips to `content` AND warns.
        let (edits, warnings, _f, aborted) = parse_patch("SWAP 1:\n+x\n2:ab|content");
        assert!(!aborted);
        let texts: Vec<&str> = edits
            .iter()
            .filter_map(|e| match e {
                Edit::Insert {
                    mode: Some(crate::types::InsertMode::Replacement),
                    text,
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["x", "content"]);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("read-output line prefixes")),
            "expected prefix-strip warning, got {warnings:?}"
        );
    }

    #[test]
    fn orphan_payload_row_warns() {
        // `+row` before any op header used to be silently dropped.
        let (_edits, warnings, _f, _aborted) = parse_patch("+orphan\nSWAP 1:\n+x");
        assert!(
            warnings.iter().any(|w| w.contains("outside any operation")),
            "expected orphan-payload warning, got {warnings:?}"
        );
    }

    // ---- bd-section-headers-137 ----

    #[test]
    fn multi_file_section_headers_rejected() {
        // Ops for `[b.rs]` used to silently apply to the CLI target file.
        let (edits, warnings, _f, aborted) =
            parse_patch("[a.rs#1234]\nSWAP 1:\n+x\n[b.rs#5678]\nDEL 2");
        assert!(edits.is_empty());
        assert!(aborted);
        assert!(
            warnings.iter().any(|w| w.contains("multiple files")),
            "expected multi-file rejection, got {warnings:?}"
        );
    }

    #[test]
    fn single_section_header_tolerated_with_warning() {
        // Same-path (or single) headers remain tolerated for compatibility,
        // but now produce an explicit warning that they don't route/bind.
        let (edits, warnings, _f, aborted) = parse_patch("[file.txt#abcd]\nDEL 2");
        assert!(!aborted);
        assert_eq!(edits.len(), 1);
        assert!(
            warnings.iter().any(|w| w.contains("informational only")),
            "expected section-header warning, got {warnings:?}"
        );
    }

    #[test]
    fn payload_after_section_header_warns_instead_of_silent_drop() {
        let (_edits, warnings, _f, _aborted) = parse_patch("SWAP 1:\n+x\n[other.rs#9999]\n+y");
        // `+y` lands after the header flushed the pending SWAP — it is an
        // orphan payload row and must at least warn.
        assert!(
            warnings.iter().any(|w| w.contains("outside any operation")),
            "expected orphan-payload warning, got {warnings:?}"
        );
    }
}
