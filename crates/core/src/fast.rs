use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash::{self, ShortHash};
use crate::output;
use crate::receipt::{self, ChangeKind, LineChange};
use memchr::memchr;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::NamedTempFile;

/// Interpret common C-style escape sequences (e.g., \\n → newline).
pub fn interpret_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('0') => out.push('\0'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some(c) => {
                out.push('\\');
                out.push(c);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// If receipt or audit_log is needed, build and write/append the receipt.
#[allow(clippy::too_many_arguments)]
pub fn handle_receipt<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    op: &str,
    path: &Path,
    changes: Vec<LineChange>,
    before_bytes: &[u8],
    after_bytes: &[u8],
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    if !receipt_flag && audit_log.is_none() {
        return Ok(());
    }
    let r = receipt::build_receipt(op, path, changes, before_bytes, after_bytes);
    if let Some(log_path) = audit_log {
        if let Err(error) = receipt::append_to_audit_log(&r, log_path) {
            receipt::write_audit_warning(ctx, log_path, &error).map_err(HashlineError::from)?;
        }
    }
    if receipt_flag {
        return receipt::write_receipt(ctx, &r);
    }
    Ok(())
}

/// Get the old line content from a file at a given 0-indexed line.
pub fn get_line_content(content: &str, line: usize) -> Option<String> {
    content.lines().nth(line).map(|s| s.to_string())
}

/// Get multiple lines as Vec<String> from content at 0-indexed range [start..=end].
pub fn get_line_range(content: &str, start: usize, end: usize) -> Vec<String> {
    content
        .lines()
        .skip(start)
        .take(end - start + 1)
        .map(|s| s.to_string())
        .collect()
}

pub fn read_file(path: &Path) -> Result<String, HashlineError> {
    let mut content = String::new();
    let mut file = std::fs::File::open(path)?;
    file.read_to_string(&mut content)?;
    Ok(content)
}

pub fn atomic_write(path: &Path, content: &str) -> Result<(), HashlineError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(meta.permissions());
    }
    temp.write_all(content.as_bytes())?;
    temp.persist(path)
        .map_err(|e| HashlineError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

fn find_line_span_inner(bytes: &[u8], line: usize) -> Result<(usize, usize), HashlineError> {
    let mut current = 0usize;
    for _ in 0..line {
        match memchr(b'\n', &bytes[current..]) {
            Some(rel) => current += rel + 1,
            None => {
                return Err(HashlineError::MutationIndexOutOfBounds {
                    index: line,
                    len: current + 1,
                });
            }
        }
    }
    let start = current;
    let end = memchr(b'\n', &bytes[current..]).map_or(bytes.len(), |r| current + r);
    Ok((start, end))
}

fn check_guards(
    path: &Path,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
) -> Result<(), HashlineError> {
    if expect_mtime.is_none() && expect_inode.is_none() {
        return Ok(());
    }
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    if let Some(expected) = expect_mtime {
        let actual = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if actual != expected {
            return Err(HashlineError::StaleFile {
                path: path.display().to_string(),
            });
        }
    }
    #[cfg(unix)]
    if let Some(expected) = expect_inode {
        if meta.ino() as u64 != expected {
            return Err(HashlineError::StaleFile {
                path: path.display().to_string(),
            });
        }
    }
    Ok(())
}

pub fn fast_from_hash(content: &str, hash: ShortHash) -> Result<usize, HashlineError> {
    let bytes = content.as_bytes();
    let mut line_start = 0usize;
    let mut line_no = 0usize;
    let mut found = None;
    loop {
        let line_end = match memchr(b'\n', &bytes[line_start..]) {
            Some(rel) => line_start + rel,
            None => content.len(),
        };
        let he = if line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };
        if hash::short_hash_value(&content[line_start..he]) == hash {
            if let Some(prev) = found {
                return Err(HashlineError::AmbiguousHash {
                    hash: hash::format_short_hash(hash),
                    count: 2,
                    lines: format!("{}, {}", prev + 1, line_no + 1),
                    path: String::new(),
                });
            }
            found = Some(line_no);
        }
        if line_end >= content.len() {
            break;
        }
        line_start = line_end + 1;
        line_no += 1;
    }
    found.ok_or_else(|| HashlineError::HashNotFound {
        hash: hash::format_short_hash(hash),
        path: String::new(),
    })
}

pub fn fast_fuzzy_resolve(content: &str, line_no: usize, hash: ShortHash) -> Option<usize> {
    let bytes = content.as_bytes();
    let check = |ln: usize| -> bool {
        let mut current = 0usize;
        for _ in 0..ln {
            match memchr(b'\n', &bytes[current..]) {
                Some(r) => current += r + 1,
                None => return false,
            }
        }
        let start = current;
        let end = memchr(b'\n', &bytes[current..]).map_or(content.len(), |r| current + r);
        let he = if end > start && bytes[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        hash::short_hash_value(&content[start..he]) == hash
    };
    if check(line_no) {
        return Some(line_no);
    }
    let max = content.lines().count().saturating_sub(1);
    let start = line_no.saturating_sub(3);
    let end = (line_no + 3).min(max);
    for attempt in start..=end {
        if attempt == line_no {
            continue;
        }
        if check(attempt) {
            return Some(attempt);
        }
    }
    None
}

pub fn fast_replace_line(
    content: &str,
    target_line: usize,
    expected_hash: ShortHash,
    new_content: &str,
) -> Result<(String, String), HashlineError> {
    let bytes = content.as_bytes();
    let mut line_start = 0;
    let mut current = 0;
    for _ in 0..target_line {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => {
                current += r + 1;
                line_start = current;
            }
            None => {
                return Err(HashlineError::MutationIndexOutOfBounds {
                    index: target_line,
                    len: current + 1,
                });
            }
        }
    }
    let line_end = match memchr(b'\n', &bytes[current..]) {
        Some(r) => current + r,
        None => content.len(),
    };
    let has_cr = line_end > line_start && bytes[line_end - 1] == b'\r';
    let he = if has_cr { line_end - 1 } else { line_end };
    let old = &content[line_start..he];
    let ah = hash::short_hash_value(old);
    if ah != expected_hash {
        return Err(HashlineError::StaleAnchor {
            anchor: format!(
                "{}:{}",
                target_line + 1,
                hash::format_short_hash(expected_hash)
            )
            .into(),
            line: target_line + 1,
            expected: hash::format_short_hash(expected_hash).into(),
            actual: hash::format_short_hash(ah).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let mut r =
        String::with_capacity(content.len() + new_content.len().saturating_sub(he - line_start));
    r.push_str(&content[..line_start]);
    r.push_str(new_content);
    if has_cr {
        r.push('\r');
        r.push_str(&content[line_end..]);
    } else if line_end < content.len() {
        r.push_str(&content[line_end..]);
    }
    Ok((r, old.to_owned()))
}

pub fn fast_replace_range(
    content: &str,
    sl: usize,
    el: usize,
    esh: ShortHash,
    eeh: ShortHash,
    nc: &str,
) -> Result<(String, String, String), HashlineError> {
    let b = content.as_bytes();
    let mut c = 0;
    let mut ls = 0;
    for _ in 0..sl {
        match memchr(b'\n', &b[c..]) {
            Some(r) => {
                c += r + 1;
                ls = c;
            }
            None => {
                return Err(HashlineError::MutationIndexOutOfBounds {
                    index: sl,
                    len: c + 1,
                });
            }
        }
    }
    let rs = ls;
    for _ in sl..el {
        match memchr(b'\n', &b[c..]) {
            Some(r) => {
                c += r + 1;
            }
            None => break,
        }
    }
    let re = match memchr(b'\n', &b[c..]) {
        Some(r) => c + r,
        None => content.len(),
    };
    let se = memchr(b'\n', &b[rs..]).map_or(content.len(), |r| rs + r);
    let ss = if se > rs && b[se - 1] == b'\r' {
        &content[rs..se - 1]
    } else {
        &content[rs..se]
    };
    let ah1 = hash::short_hash_value(ss);
    if ah1 != esh {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}:{}", sl + 1, hash::format_short_hash(esh)).into(),
            line: sl + 1,
            expected: hash::format_short_hash(esh).into(),
            actual: hash::format_short_hash(ah1).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let mut s = rs;
    let mut cnt = 0;
    while cnt < (el - sl) {
        if let Some(r) = memchr(b'\n', &b[s..]) {
            s += r + 1;
            cnt += 1;
        } else {
            break;
        }
    }
    let es = if re > s && b[re - 1] == b'\r' {
        &content[s..re - 1]
    } else {
        &content[s..re]
    };
    let ah2 = hash::short_hash_value(es);
    if ah2 != eeh {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}:{}", el + 1, hash::format_short_hash(eeh)).into(),
            line: el + 1,
            expected: hash::format_short_hash(eeh).into(),
            actual: hash::format_short_hash(ah2).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let mut r = String::with_capacity(content.len() + nc.len().saturating_sub(re - rs));
    r.push_str(&content[..rs]);
    r.push_str(nc);
    if re < content.len() {
        r.push_str(&content[re..]);
    }
    Ok((r, ss.to_owned(), es.to_owned()))
}

pub fn fast_insert_line(
    content: &str,
    target_line: usize,
    new_content: &str,
) -> Result<String, HashlineError> {
    let bytes = content.as_bytes();
    let mut current = 0;
    for _ in 0..=target_line {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => {
                current += r + 1;
            }
            None => {
                current = content.len();
                break;
            }
        }
    }
    let sep = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut r = String::with_capacity(content.len() + new_content.len() + 2);
    r.push_str(&content[..current]);
    r.push_str(new_content);
    r.push_str(sep);
    r.push_str(&content[current..]);
    Ok(r)
}

/// Insert line BEFORE a target line.
pub fn fast_insert_line_before(
    content: &str,
    target_line: usize,
    new_content: &str,
) -> Result<String, HashlineError> {
    let bytes = content.as_bytes();
    let mut current = 0;
    for _ in 0..target_line {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => {
                current += r + 1;
            }
            None => {
                current = content.len();
                break;
            }
        }
    }
    let sep = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut r = String::with_capacity(content.len() + new_content.len() + 2);
    r.push_str(&content[..current]);
    r.push_str(new_content);
    r.push_str(sep);
    r.push_str(&content[current..]);
    Ok(r)
}

/// Find first line matching a content query string, return its 0-indexed line number.
pub fn fast_find_query(content: &str, query: &str) -> Result<usize, HashlineError> {
    let idx = content
        .find(query)
        .ok_or_else(|| HashlineError::QueryNotFound {
            query: query.to_string(),
            path: String::new(),
        })?;
    let bytes = content.as_bytes();
    let line_no = bytes[..idx].iter().filter(|&&b| b == b'\n').count();
    Ok(line_no)
}

pub fn fast_delete_lines(
    content: &str,
    start_line: usize,
    end_line: usize,
    expected_start_hash: ShortHash,
) -> Result<String, HashlineError> {
    let bytes = content.as_bytes();
    let mut current = 0;
    for _ in 0..start_line {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => {
                current += r + 1;
            }
            None => {
                return Err(HashlineError::MutationIndexOutOfBounds {
                    index: start_line,
                    len: current + 1,
                });
            }
        }
    }
    let ds = current;
    let le = memchr(b'\n', &bytes[current..]).map_or(content.len(), |r| current + r);
    let he = if le > current && bytes[le - 1] == b'\r' {
        le - 1
    } else {
        le
    };
    let ah = hash::short_hash_value(&content[current..he]);
    if ah != expected_start_hash {
        return Err(HashlineError::StaleAnchor {
            anchor: format!(
                "{}:{}",
                start_line + 1,
                hash::format_short_hash(expected_start_hash)
            )
            .into(),
            line: start_line + 1,
            expected: hash::format_short_hash(expected_start_hash).into(),
            actual: hash::format_short_hash(ah).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    for _ in start_line..=end_line {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => {
                current += r + 1;
            }
            None => {
                current = content.len();
                break;
            }
        }
    }
    let mut r = String::with_capacity(content.len());
    r.push_str(&content[..ds]);
    r.push_str(&content[current..]);
    Ok(r)
}

pub fn fast_swap_lines(
    content: &str,
    l1: usize,
    l2: usize,
    h1: ShortHash,
    h2: ShortHash,
) -> Result<String, HashlineError> {
    if l1 == l2 {
        return Err(HashlineError::PatchFailed {
            op_index: 0,
            reason: "source and target must resolve to different lines".into(),
        });
    }
    let b = content.as_bytes();
    let (s1, e1) = find_line_span_inner(b, l1)?;
    let (s2, e2) = find_line_span_inner(b, l2)?;
    let he1 = if e1 > s1 && b[e1 - 1] == b'\r' {
        e1 - 1
    } else {
        e1
    };
    let he2 = if e2 > s2 && b[e2 - 1] == b'\r' {
        e2 - 1
    } else {
        e2
    };
    if hash::short_hash_value(&content[s1..he1]) != h1 {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}", l1 + 1).into(),
            line: l1 + 1,
            expected: hash::format_short_hash(h1).into(),
            actual: hash::format_short_hash(h1).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    if hash::short_hash_value(&content[s2..he2]) != h2 {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}", l2 + 1).into(),
            line: l2 + 1,
            expected: hash::format_short_hash(h2).into(),
            actual: hash::format_short_hash(h2).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let (line1, line2) = (&content[s1..e1], &content[s2..e2]);
    let mut r = String::with_capacity(content.len());
    if s1 < s2 {
        r.push_str(&content[..s1]);
        r.push_str(line2);
        r.push_str(&content[e1..s2]);
        r.push_str(line1);
        r.push_str(&content[e2..]);
    } else {
        r.push_str(&content[..s2]);
        r.push_str(line1);
        r.push_str(&content[e2..s1]);
        r.push_str(line2);
        r.push_str(&content[e1..]);
    }
    Ok(r)
}

pub fn fast_move_line(
    content: &str,
    source: usize,
    target: usize,
    hash: ShortHash,
    place_before: bool,
) -> Result<String, HashlineError> {
    if source == target {
        return Err(HashlineError::PatchFailed {
            op_index: 0,
            reason: "source and target must be different".into(),
        });
    }
    let b = content.as_bytes();
    let (ss, se) = find_line_span_inner(b, source)?;
    let he = if se > ss && b[se - 1] == b'\r' {
        se - 1
    } else {
        se
    };
    if hash::short_hash_value(&content[ss..he]) != hash {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}", source + 1).into(),
            line: source + 1,
            expected: hash::format_short_hash(hash).into(),
            actual: hash::format_short_hash(hash).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let mut v: Vec<&str> = content.lines().collect();
    let line = v.remove(source);
    let adj = if source < target { target - 1 } else { target };
    v.insert(
        if place_before {
            adj
        } else {
            (adj + 1).min(v.len())
        },
        line,
    );
    let sep = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    Ok(v.join(sep) + if content.ends_with('\n') { "\n" } else { "" })
}

pub fn fast_indent_lines(
    content: &str,
    start_line: usize,
    end_line: usize,
    hash: ShortHash,
    delta: isize,
) -> Result<String, HashlineError> {
    let b = content.as_bytes();
    let (ss, se) = find_line_span_inner(b, start_line)?;
    let he = if se > ss && b[se - 1] == b'\r' {
        se - 1
    } else {
        se
    };
    if hash::short_hash_value(&content[ss..he]) != hash {
        return Err(HashlineError::StaleAnchor {
            anchor: format!("{}", start_line + 1).into(),
            line: start_line + 1,
            expected: hash::format_short_hash(hash).into(),
            actual: hash::format_short_hash(hash).into(),
            path: "".into(),
            relocated_suffix: "".into(),
        });
    }
    let sep = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    for i in start_line..=end_line.min(lines.len().saturating_sub(1)) {
        if delta < 0 {
            for _ in 0..(-delta as usize) {
                if lines[i].starts_with(' ') {
                    lines[i] = lines[i][1..].to_string();
                } else {
                    break;
                }
            }
        } else {
            let spaces = " ".repeat(delta as usize);
            lines[i] = format!("{}{}", spaces, lines[i]);
        }
    }
    Ok(lines.join(sep) + if content.ends_with('\n') { "\n" } else { "" })
}

// ===== Comprehensive command handlers =====
#[allow(clippy::too_many_arguments)]
pub fn run_fast_edit<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    target_line: usize,
    expected_hash: ShortHash,
    new_content: &str,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let content_to_use = if interpret_escapes_flag {
        interpret_escapes(new_content)
    } else {
        new_content.to_string()
    };
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let (nc, _old) = fast_replace_line(&raw, target_line, expected_hash, &content_to_use)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    // Emit receipt BEFORE write when receipt_flag is set (catch errors before mutating file)
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: target_line + 1,
                kind: ChangeKind::Modified,
                before: get_line_content(&raw, target_line),
                after: if interpret_escapes_flag {
                    content_to_use.lines().next().map(|s| s.to_string())
                } else {
                    get_line_content(&nc, target_line)
                },
            }];
            handle_receipt(ctx, "edit", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Audit-log after write (non-fatal error)
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: target_line + 1,
                kind: ChangeKind::Modified,
                before: get_line_content(&raw, target_line),
                after: if interpret_escapes_flag {
                    content_to_use.lines().next().map(|s| s.to_string())
                } else {
                    get_line_content(&nc, target_line)
                },
            }];
            handle_receipt(
                ctx,
                "edit",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if dry_run && ctx.output_mode() == OutputMode::Pretty {
        let before = get_line_content(&raw, target_line).unwrap_or_default();
        let after = if interpret_escapes_flag {
            content_to_use.clone()
        } else {
            get_line_content(&nc, target_line).unwrap_or_default()
        };
        output::write_success_line(ctx, &format!("Would change line {}:", target_line + 1))
            .map_err(HashlineError::from)?;
        output::write_success_line(ctx, &format!("  - {:?}", before))
            .map_err(HashlineError::from)?;
        output::write_success_line(ctx, &format!("  + {:?}", after))
            .map_err(HashlineError::from)?;
        output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)?;
    } else if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Edited line {}.", target_line + 1))
            .map_err(HashlineError::from)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_fast_insert<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    target_line: usize,
    _hash: ShortHash,
    new_content: &str,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let content_to_use = if interpret_escapes_flag {
        interpret_escapes(new_content)
    } else {
        new_content.to_string()
    };
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let nc = fast_insert_line(&raw, target_line, &content_to_use)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    // Emit receipt BEFORE write
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: target_line + 2,
                kind: ChangeKind::Inserted,
                before: None,
                after: Some(content_to_use.clone()),
            }];
            handle_receipt(ctx, "insert", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Audit-log after write
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: target_line + 2,
                kind: ChangeKind::Inserted,
                before: None,
                after: Some(content_to_use.clone()),
            }];
            handle_receipt(
                ctx,
                "insert",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Inserted line {}.", target_line + 2))
            .map_err(HashlineError::from)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_fast_delete<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    start_line: usize,
    end_line: usize,
    expected_start_hash: ShortHash,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    _interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let nc = fast_delete_lines(&raw, start_line, end_line, expected_start_hash)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    // Emit receipt BEFORE write
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let deleted_lines = get_line_range(&raw, start_line, end_line);
            let changes: Vec<LineChange> = if start_line == end_line {
                vec![LineChange {
                    line_no: start_line + 1,
                    kind: ChangeKind::Deleted,
                    before: deleted_lines.first().cloned(),
                    after: None,
                }]
            } else {
                deleted_lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| LineChange {
                        line_no: start_line + i + 1,
                        kind: ChangeKind::Deleted,
                        before: Some(l.clone()),
                        after: None,
                    })
                    .collect()
            };
            handle_receipt(ctx, "delete", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Audit-log after write
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let deleted_lines = get_line_range(&raw, start_line, end_line);
            let changes: Vec<LineChange> = if start_line == end_line {
                vec![LineChange {
                    line_no: start_line + 1,
                    kind: ChangeKind::Deleted,
                    before: deleted_lines.first().cloned(),
                    after: None,
                }]
            } else {
                deleted_lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| LineChange {
                        line_no: start_line + i + 1,
                        kind: ChangeKind::Deleted,
                        before: Some(l.clone()),
                        after: None,
                    })
                    .collect()
            };
            handle_receipt(
                ctx,
                "delete",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if dry_run && ctx.output_mode() == OutputMode::Pretty {
        let before_lines = get_line_range(&raw, start_line, end_line);
        if start_line == end_line {
            output::write_success_line(ctx, &format!("Would delete line {}:", start_line + 1))
                .map_err(HashlineError::from)?;
            output::write_success_line(
                ctx,
                &format!("  - {:?}", before_lines.first().unwrap_or(&String::new())),
            )
            .map_err(HashlineError::from)?;
        } else {
            output::write_success_line(
                ctx,
                &format!("Would delete lines {}-{}:", start_line + 1, end_line + 1),
            )
            .map_err(HashlineError::from)?;
        }
        output::write_success_line(ctx, "No file was written.").map_err(HashlineError::from)?;
    } else if ctx.output_mode() == OutputMode::Pretty {
        if start_line == end_line {
            output::write_success_line(ctx, &format!("Deleted line {}.", start_line + 1))
                .map_err(HashlineError::from)?;
        } else {
            output::write_success_line(
                ctx,
                &format!("Deleted lines {}-{}.", start_line + 1, end_line + 1),
            )
            .map_err(HashlineError::from)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_fast_swap<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    line1: usize,
    line2: usize,
    hash1: ShortHash,
    hash2: ShortHash,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    _interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let nc = fast_swap_lines(&raw, line1, line2, hash1, hash2)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    // Emit receipt BEFORE write
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![
                LineChange {
                    line_no: line1 + 1,
                    kind: ChangeKind::Modified,
                    before: get_line_content(&raw, line1),
                    after: get_line_content(&nc, line1),
                },
                LineChange {
                    line_no: line2 + 1,
                    kind: ChangeKind::Modified,
                    before: get_line_content(&raw, line2),
                    after: get_line_content(&nc, line2),
                },
            ];
            handle_receipt(ctx, "swap", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Audit-log after write
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![
                LineChange {
                    line_no: line1 + 1,
                    kind: ChangeKind::Modified,
                    before: get_line_content(&raw, line1),
                    after: get_line_content(&nc, line1),
                },
                LineChange {
                    line_no: line2 + 1,
                    kind: ChangeKind::Modified,
                    before: get_line_content(&raw, line2),
                    after: get_line_content(&nc, line2),
                },
            ];
            handle_receipt(
                ctx,
                "swap",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(
            ctx,
            &format!("Swapped lines {} and {}.", line1 + 1, line2 + 1),
        )
        .map_err(HashlineError::from)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_fast_move<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    source: usize,
    target: usize,
    hash: ShortHash,
    place_before: bool,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    _interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let nc = fast_move_line(&raw, source, target, hash, place_before)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    // Emit receipt BEFORE write
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let adj_target = if source < target { target - 1 } else { target };
            let to_line = if place_before {
                adj_target
            } else {
                adj_target + 1
            } + 1;
            let changes = vec![
                LineChange {
                    line_no: source + 1,
                    kind: ChangeKind::Deleted,
                    before: get_line_content(&raw, source),
                    after: None,
                },
                LineChange {
                    line_no: to_line,
                    kind: ChangeKind::Inserted,
                    before: None,
                    after: get_line_content(&nc, to_line - 1),
                },
            ];
            handle_receipt(ctx, "move", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Audit-log after write
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let adj_target = if source < target { target - 1 } else { target };
            let to_line = if place_before {
                adj_target
            } else {
                adj_target + 1
            } + 1;
            let changes = vec![
                LineChange {
                    line_no: source + 1,
                    kind: ChangeKind::Deleted,
                    before: get_line_content(&raw, source),
                    after: None,
                },
                LineChange {
                    line_no: to_line,
                    kind: ChangeKind::Inserted,
                    before: None,
                    after: get_line_content(&nc, to_line - 1),
                },
            ];
            handle_receipt(
                ctx,
                "move",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    let adj_target = if source < target { target - 1 } else { target };
    let to_line = if place_before {
        adj_target
    } else {
        adj_target + 1
    } + 1;
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(
            ctx,
            &format!("Moved line {} to line {}.", source + 1, to_line),
        )
        .map_err(HashlineError::from)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_fast_indent<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    start_line: usize,
    end_line: usize,
    hash: ShortHash,
    amount: isize,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    _interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    // Check mixed indent
    let ht = raw.lines().any(|l| l.starts_with('\t'));
    let hs = raw.lines().any(|l| l.starts_with(' '));
    if ht && hs {
        return Err(HashlineError::MixedIndentation {
            line_no: start_line + 1,
        });
    }
    // Check underflow for dedent
    if amount < 0 {
        let fl: Vec<&str> = raw.lines().collect();
        let max_idx = end_line.min(fl.len().saturating_sub(1));
        for (li, line) in fl.iter().enumerate().take(max_idx + 1).skip(start_line) {
            let lead = line.chars().take_while(|c| *c == ' ').count();
            if (lead as isize) < -amount {
                return Err(HashlineError::IndentUnderflow {
                    line_no: li + 1,
                    amount: (-amount) as usize,
                    available: lead,
                    kind: "spaces",
                });
            }
        }
    }
    let nc = fast_indent_lines(&raw, start_line, end_line, hash, amount)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    // Emit receipt BEFORE write
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = get_line_range(&raw, start_line, end_line)
                .iter()
                .enumerate()
                .map(|(i, l)| LineChange {
                    line_no: start_line + i + 1,
                    kind: ChangeKind::Modified,
                    before: Some(l.clone()),
                    after: get_line_content(&nc, start_line + i),
                })
                .collect();
            handle_receipt(ctx, "indent", path, changes, before, after, true, None)?;
        }
    }
    // Audit-log after write
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = get_line_range(&raw, start_line, end_line)
                .iter()
                .enumerate()
                .map(|(i, l)| LineChange {
                    line_no: start_line + i + 1,
                    kind: ChangeKind::Modified,
                    before: Some(l.clone()),
                    after: get_line_content(&nc, start_line + i),
                })
                .collect();
            handle_receipt(
                ctx,
                "indent",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    let by = if amount < 0 {
        format!("{}", -amount)
    } else {
        format!("{}", amount)
    };
    if ctx.output_mode() == OutputMode::Pretty {
        if start_line == end_line {
            output::write_success_line(
                ctx,
                &format!("Indented line {} by {} spaces.", start_line + 1, by),
            )
            .map_err(HashlineError::from)?;
        } else {
            output::write_success_line(
                ctx,
                &format!(
                    "Indented lines {}-{} by {} spaces.",
                    start_line + 1,
                    end_line + 1,
                    by
                ),
            )
            .map_err(HashlineError::from)?;
        }
    }
    Ok(())
}

// ===== Range edit handler (supports receipt/audit/interpret_escapes) =====
#[allow(clippy::too_many_arguments)]
pub fn run_fast_range_edit<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    start_line: usize,
    start_hash: ShortHash,
    end_line: usize,
    end_hash: ShortHash,
    new_content: &str,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let content_to_use = if interpret_escapes_flag {
        interpret_escapes(new_content)
    } else {
        new_content.to_string()
    };
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let (nc, _first_line, _last_line) = fast_replace_range(
        &raw,
        start_line,
        end_line,
        start_hash,
        end_hash,
        &content_to_use,
    )?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = make_range_changes(&raw, start_line, end_line, &content_to_use);
            handle_receipt(ctx, "edit", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = make_range_changes(&raw, start_line, end_line, &content_to_use);
            handle_receipt(
                ctx,
                "edit",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(
            ctx,
            &format!("Edited lines {}-{}.", start_line + 1, end_line + 1),
        )
        .map_err(HashlineError::from)?;
    }
    Ok(())
}

fn make_range_changes(raw: &str, start: usize, end: usize, new_content: &str) -> Vec<LineChange> {
    let before_lines: Vec<String> = raw
        .lines()
        .skip(start)
        .take(end - start + 1)
        .map(String::from)
        .collect();
    let after_lines: Vec<String> = new_content.lines().map(String::from).collect();
    let shared = before_lines.len().min(after_lines.len());
    let mut changes: Vec<LineChange> = (0..shared)
        .map(|i| LineChange {
            line_no: start + i + 1,
            kind: ChangeKind::Modified,
            before: before_lines.get(i).cloned(),
            after: after_lines.get(i).cloned(),
        })
        .collect();
    for i in shared..before_lines.len() {
        changes.push(LineChange {
            line_no: start + i + 1,
            kind: ChangeKind::Deleted,
            before: before_lines.get(i).cloned(),
            after: None,
        });
    }
    for i in shared..after_lines.len() {
        changes.push(LineChange {
            line_no: start + i + 1,
            kind: ChangeKind::Inserted,
            before: None,
            after: after_lines.get(i).cloned(),
        });
    }
    changes
}

// ===== Query edit handler (supports receipt/audit/interpret_escapes) =====
#[allow(clippy::too_many_arguments)]
pub fn run_fast_query_edit<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    query: &str,
    new_content: &str,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
    interpret_escapes_flag: bool,
    receipt_flag: bool,
    audit_log: Option<&Path>,
) -> Result<(), HashlineError> {
    check_guards(path, expect_mtime, expect_inode)?;
    let raw = read_file(path)?;
    let line_no = fast_find_query(&raw, query)?;
    let content_to_use = if interpret_escapes_flag {
        interpret_escapes(new_content)
    } else {
        new_content.to_string()
    };
    let bytes = raw.as_bytes();
    let mut current = 0;
    for _ in 0..line_no {
        match memchr(b'\n', &bytes[current..]) {
            Some(r) => current += r + 1,
            None => break,
        }
    }
    let line_end = match memchr(b'\n', &bytes[current..]) {
        Some(r) => current + r,
        None => raw.len(),
    };
    let he = if line_end > current && bytes[line_end - 1] == b'\r' {
        line_end - 1
    } else {
        line_end
    };
    let hash = hash::short_hash_value(&raw[current..he]);
    let before_bytes = if receipt_flag || audit_log.is_some() {
        Some(raw.as_bytes().to_vec())
    } else {
        None
    };
    let (nc, _) = fast_replace_line(&raw, line_no, hash, &content_to_use)?;
    let after_bytes = if receipt_flag || audit_log.is_some() {
        Some(nc.as_bytes().to_vec())
    } else {
        None
    };
    if receipt_flag {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: line_no + 1,
                kind: ChangeKind::Modified,
                before: get_line_content(&raw, line_no),
                after: get_line_content(&nc, line_no),
            }];
            handle_receipt(ctx, "edit", path, changes, before, after, true, None)?;
        }
    }
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) {
            ctx.modified_doc = Some(doc);
        }
    }
    if let Some(log_path) = audit_log {
        if let (Some(before), Some(after)) = (&before_bytes, &after_bytes) {
            let changes = vec![LineChange {
                line_no: line_no + 1,
                kind: ChangeKind::Modified,
                before: get_line_content(&raw, line_no),
                after: get_line_content(&nc, line_no),
            }];
            handle_receipt(
                ctx,
                "edit",
                path,
                changes,
                before,
                after,
                false,
                Some(log_path),
            )?;
        }
    }
    if receipt_flag {
        return Ok(());
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Edited line {}.", line_no + 1))
            .map_err(HashlineError::from)?;
    }
    Ok(())
}
