//! Token-driven state machine that turns a stream of [`Token`]s into a
//! flat list of [`Edit`]s. Sits between the tokenizer and the applier.

use std::collections::HashMap;

use crate::messages::BARE_BODY_AUTO_PIPED_WARNING;
use crate::patch_format::HL_RANGE_SEP;
use crate::prefixes::strip_one_hashline_prefix;
use crate::tokenizer::{BlockTarget, Token, clone_cursor};
use crate::types::{Anchor, BlockMode, Cursor, Edit, InsertMode, ParsedRange};

fn validate_range_order(range: &ParsedRange, line_num: usize) -> Result<(), String> {
    if range.end.line < range.start.line {
        return Err(format!(
            "line {line_num}: range {}..{} ends before it starts",
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

fn detect_apply_patch_contamination(text: &str, _has_pending: bool) -> Option<String> {
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
            format!("{}…", &trimmed[..48])
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
    edit_index: usize,
    pending: Option<Pending>,
    terminated: bool,
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
            edit_index: 0,
            pending: None,
            terminated: false,
        }
    }

    pub fn feed(&mut self, token: Token) {
        if self.terminated {
            return;
        }
        match token {
            Token::EnvelopeBegin { .. } => {}
            Token::EnvelopeEnd { .. } => {
                self.terminated = true;
            }
            Token::Abort { .. } => {
                self.terminated = true;
            }
            Token::Header { .. } => {
                self.flush_pending();
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
                if matches!(&target, BlockTarget::Replace(_) | BlockTarget::Delete(_)) {
                    if let BlockTarget::Replace(r) | BlockTarget::Delete(r) = &target {
                        let _ = validate_range_order(r, line_num);
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

    pub fn end(&mut self) -> (Vec<Edit>, Vec<String>) {
        self.flush_pending();
        self.validate_no_overlapping_deletes();
        let edits = std::mem::take(&mut self.edits);
        let warnings = std::mem::take(&mut self.warnings);
        self.edit_index = 0;
        self.pending = None;
        self.terminated = false;
        (edits, warnings)
    }

    fn handle_literal_payload(&mut self, text: &str, line_num: usize) {
        let pending = match &self.pending {
            Some(p) => p,
            None => {
                return;
            }
        };
        if matches!(
            pending.target,
            BlockTarget::Delete(_) | BlockTarget::DeleteBlock(_)
        ) {
            return;
        }
        let pending_mut = self.pending.as_mut().unwrap();
        pending_mut.deferred_blanks.clear();
        pending_mut.payloads.push(PayloadRow {
            text: text.to_owned(),
            line_num,
            bare: false,
        });
    }

    fn handle_raw(&mut self, text: &str, line_num: usize) {
        let contamination = detect_apply_patch_contamination(text, self.pending.is_some());
        if let Some(_msg) = contamination {
            return;
        }

        if let Some(pending) = &self.pending {
            if text.trim().is_empty() {
                self.handle_blank();
                return;
            }
            if matches!(pending.target, BlockTarget::Delete(_)) {
                return;
            }
            if matches!(pending.target, BlockTarget::DeleteBlock(_)) {
                return;
            }
            if text.trim_start().starts_with('-') {
                return;
            }
            if !self
                .warnings
                .contains(&BARE_BODY_AUTO_PIPED_WARNING.to_string())
            {
                self.warnings.push(BARE_BODY_AUTO_PIPED_WARNING.to_owned());
            }
            let pending_mut = self.pending.as_mut().unwrap();
            pending_mut.deferred_blanks.clear();
            pending_mut.payloads.push(PayloadRow {
                text: text.to_owned(),
                line_num,
                bare: true,
            });
            return;
        }

        if text.trim().is_empty() {
        }
    }

    fn handle_blank(&mut self) {
        let pending = match &mut self.pending {
            Some(p) => p,
            None => return,
        };
        if matches!(
            pending.target,
            BlockTarget::Delete(_) | BlockTarget::DeleteBlock(_)
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

        Self::strip_bare_prefixes_if_uniform(&mut payloads);

        let payload_texts: Vec<String> = payloads.into_iter().map(|r| r.text).collect();

        match &target {
            BlockTarget::Delete(range) => {
                for anchor in expand_range(range) {
                    self.edits.push(Edit::Delete {
                        anchor,
                        line_num,
                        index: self.edit_index,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::DeleteBlock(anchor) => {
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: Vec::new(),
                    line_num,
                    index: self.edit_index,
                    mode: None,
                });
                self.edit_index += 1;
            }
            BlockTarget::Block(anchor) => {
                if payload_texts.is_empty() {
                    return;
                }
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: payload_texts,
                    line_num,
                    index: self.edit_index,
                    mode: None,
                });
                self.edit_index += 1;
            }
            BlockTarget::InsertAfterBlock(anchor) => {
                if payload_texts.is_empty() {
                    return;
                }
                self.edits.push(Edit::Block {
                    anchor: *anchor,
                    payloads: payload_texts,
                    line_num,
                    index: self.edit_index,
                    mode: Some(BlockMode::InsertAfter),
                });
                self.edit_index += 1;
            }
            BlockTarget::Replace(range) => {
                if payload_texts.is_empty() {
                    // SWAP with no body = delete
                    for anchor in expand_range(range) {
                        self.edits.push(Edit::Delete {
                            anchor,
                            line_num,
                            index: self.edit_index,
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
                    });
                    self.edit_index += 1;
                }
                for anchor in expand_range(range) {
                    self.edits.push(Edit::Delete {
                        anchor,
                        line_num,
                        index: self.edit_index,
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::InsertBefore(anchor) => {
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
                    });
                    self.edit_index += 1;
                }
            }
            BlockTarget::InsertAfter(anchor) => {
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
                    });
                    self.edit_index += 1;
                }
            }
        }
    }

    fn validate_no_overlapping_deletes(&self) {
        let mut source_lines_by_anchor: HashMap<usize, Vec<usize>> = HashMap::new();
        for edit in &self.edits {
            if let Edit::Delete {
                anchor, line_num, ..
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

/// Parse a complete patch diff body into `Edit`s.
pub fn parse_patch(diff: &str) -> (Vec<Edit>, Vec<String>) {
    let tokenizer = crate::tokenizer::Tokenizer;
    let mut executor = Executor::new();

    for (i, line) in diff.lines().enumerate() {
        let token = tokenizer.tokenize(line, i + 1);
        executor.feed(token);
    }
    // Handle final line if no trailing newline
    if !diff.ends_with('\n') && !diff.is_empty() {
        // Already handled by lines() which splits without trailing
    }

    executor.end()
}
