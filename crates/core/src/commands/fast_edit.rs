#![allow(clippy::redundant_pattern_matching, clippy::unnecessary_map_or, clippy::result_unit_err, clippy::unused_unit, clippy::let_unit_value)]
use std::io::{Read, Write};
use std::path::Path;
use memchr::memchr;
use tempfile::NamedTempFile;
use crate::context::{CommandContext, OutputMode};
use crate::document::Document;
use crate::error::HashlineError;
use crate::hash::{self, ShortHash};
use crate::output;

pub fn read_file(path: &Path) -> Result<String, HashlineError> {
    let mut content = String::new();
    let mut file = std::fs::File::open(path)?;
    file.read_to_string(&mut content)?;
    Ok(content)
}

pub fn atomic_write(path: &Path, content: &str) -> Result<(), HashlineError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    if let Ok(meta) = std::fs::metadata(path) { let _ = temp.as_file().set_permissions(meta.permissions()); }
    temp.write_all(content.as_bytes())?;
    temp.persist(path).map_err(|e| HashlineError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

fn seed_cache<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, content: &str) {
    if let Ok(doc) = Document::from_str(path, content) { ctx.modified_doc = Some(doc); }
}

pub fn fast_replace_line(content: &str, target_line: usize, expected_hash: ShortHash, new_content: &str) -> Result<(String, String), HashlineError> {
    let bytes = content.as_bytes();
    let mut line_start = 0; let mut current = 0;
    for _ in 0..target_line {
        match memchr(b'\n', &bytes[current..]) { Some(r) => { current += r + 1; line_start = current; } None => return Err(HashlineError::MutationIndexOutOfBounds { index: target_line, len: current + 1 }) }
    }
    let line_end = match memchr(b'\n', &bytes[current..]) { Some(r) => current + r, None => content.len() };
    let has_cr = line_end > line_start && bytes[line_end - 1] == b'\r';
    let he = if has_cr { line_end - 1 } else { line_end };
    let old = &content[line_start..he];
    let ah = hash::short_hash_value(old);
    if ah != expected_hash { return Err(HashlineError::StaleAnchor { anchor: format!("{}:{}", target_line+1, hash::format_short_hash(expected_hash)).into(), line: target_line+1, expected: hash::format_short_hash(expected_hash).into(), actual: hash::format_short_hash(ah).into(), path: "".into(), relocated_suffix: "".into() }); }
    let mut r = String::with_capacity(content.len() + new_content.len().saturating_sub(he - line_start));
    r.push_str(&content[..line_start]); r.push_str(new_content);
    if has_cr { r.push('\r'); r.push_str(&content[line_end..]); } else if line_end < content.len() { r.push_str(&content[line_end..]); }
    Ok((r, old.to_owned()))
}

pub fn fast_replace_range(content: &str, sl: usize, el: usize, esh: ShortHash, eeh: ShortHash, nc: &str) -> Result<(String, String, String), HashlineError> {
    let b = content.as_bytes(); let mut c = 0; let mut ls = 0;
    for _ in 0..sl { match memchr(b'\n', &b[c..]) { Some(r) => { c += r + 1; ls = c; } None => return Err(HashlineError::MutationIndexOutOfBounds { index: sl, len: c + 1 }) } }
    let rs = ls;
    for _ in sl..el { match memchr(b'\n', &b[c..]) { Some(r) => { c += r + 1; } None => break } }
    let re = match memchr(b'\n', &b[c..]) { Some(r) => c + r, None => content.len() };
    let se = memchr(b'\n', &b[rs..]).map_or(content.len(), |r| rs + r);
    let ss = if se > rs && b[se-1] == b'\r' { &content[rs..se-1] } else { &content[rs..se] };
    let ah1 = hash::short_hash_value(ss);
    if ah1 != esh { return Err(HashlineError::StaleAnchor { anchor: format!("{}:{}", sl+1, hash::format_short_hash(esh)).into(), line: sl+1, expected: hash::format_short_hash(esh).into(), actual: hash::format_short_hash(ah1).into(), path: "".into(), relocated_suffix: "".into() }); }
    let mut s = rs; let mut cnt = 0;
    while cnt < (el - sl) { if let Some(r) = memchr(b'\n', &b[s..]) { s += r + 1; cnt += 1; } else { break; } }
    let es = if re > s && b[re-1] == b'\r' { &content[s..re-1] } else { &content[s..re] };
    let ah2 = hash::short_hash_value(es);
    if ah2 != eeh { return Err(HashlineError::StaleAnchor { anchor: format!("{}:{}", el+1, hash::format_short_hash(eeh)).into(), line: el+1, expected: hash::format_short_hash(eeh).into(), actual: hash::format_short_hash(ah2).into(), path: "".into(), relocated_suffix: "".into() }); }
    let mut r = String::with_capacity(content.len() + nc.len().saturating_sub(re - rs));
    r.push_str(&content[..rs]); r.push_str(nc); if re < content.len() { r.push_str(&content[re..]); }
    Ok((r, ss.to_owned(), es.to_owned()))
}

pub fn fast_insert_line(content: &str, target_line: usize, new_content: &str) -> Result<String, HashlineError> {
    let bytes = content.as_bytes(); let mut current = 0;
    for _ in 0..=target_line { match memchr(b'\n', &bytes[current..]) { Some(r) => { current += r + 1; } None => { current = content.len(); break; } } }
    let sep = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut r = String::with_capacity(content.len() + new_content.len() + 2);
    r.push_str(&content[..current]); r.push_str(new_content); r.push_str(sep);
    r.push_str(&content[current..]);
    Ok(r)
}

pub fn fast_delete_lines(content: &str, start_line: usize, end_line: usize, expected_start_hash: ShortHash) -> Result<String, HashlineError> {
    let bytes = content.as_bytes(); let mut current = 0;
    for _ in 0..start_line { match memchr(b'\n', &bytes[current..]) { Some(r) => { current += r + 1; } None => return Err(HashlineError::MutationIndexOutOfBounds { index: start_line, len: current + 1 }) } }
    let ds = current;
    let le = memchr(b'\n', &bytes[current..]).map_or(content.len(), |r| current + r);
    let he = if le > current && bytes[le-1] == b'\r' { le - 1 } else { le };
    let ah = hash::short_hash_value(&content[current..he]);
    if ah != expected_start_hash { return Err(HashlineError::StaleAnchor { anchor: format!("{}:{}", start_line+1, hash::format_short_hash(expected_start_hash)).into(), line: start_line+1, expected: hash::format_short_hash(expected_start_hash).into(), actual: hash::format_short_hash(ah).into(), path: "".into(), relocated_suffix: "".into() }); }
    for _ in start_line..=end_line { match memchr(b'\n', &bytes[current..]) { Some(r) => { current += r + 1; } None => { current = content.len(); break; } } }
    let mut r = String::with_capacity(content.len());
    r.push_str(&content[..ds]); r.push_str(&content[current..]);
    Ok(r)
}

pub fn fast_swap_lines(content: &str, l1: usize, l2: usize, h1: ShortHash, h2: ShortHash) -> Result<String, HashlineError> {
    if l1 == l2 { return Err(HashlineError::PatchFailed { op_index: 0, reason: "source and target must resolve to different lines".into() }); }
    let b = content.as_bytes();
    let (s1, e1) = (0..l1).fold(Ok(0usize), |c: Result<usize,_>, _| c.and_then(|cur| match memchr(b'\n', &b[cur..]) { Some(r) => Ok(cur + r + 1), None => Err(HashlineError::MutationIndexOutOfBounds { index: l1, len: cur + 1 }) })).and_then(|cur| Ok((cur, memchr(b'\n', &b[cur..]).map_or(content.len(), |r| cur + r))))?;
    let (s2, e2) = (0..l2).fold(Ok(0usize), |c: Result<usize,_>, _| c.and_then(|cur| match memchr(b'\n', &b[cur..]) { Some(r) => Ok(cur + r + 1), None => Err(HashlineError::MutationIndexOutOfBounds { index: l2, len: cur + 1 }) })).and_then(|cur| Ok((cur, memchr(b'\n', &b[cur..]).map_or(content.len(), |r| cur + r))))?;
    let he1 = if e1 > s1 && b[e1-1] == b'\r' { e1-1 } else { e1 };
    let he2 = if e2 > s2 && b[e2-1] == b'\r' { e2-1 } else { e2 };
    if hash::short_hash_value(&content[s1..he1]) != h1 { return Err(HashlineError::StaleAnchor { anchor: format!("{}", l1+1).into(), line: l1+1, expected: hash::format_short_hash(h1).into(), actual: hash::format_short_hash(h1).into(), path: "".into(), relocated_suffix: "".into() }); }
    if hash::short_hash_value(&content[s2..he2]) != h2 { return Err(HashlineError::StaleAnchor { anchor: format!("{}", l2+1).into(), line: l2+1, expected: hash::format_short_hash(h2).into(), actual: hash::format_short_hash(h2).into(), path: "".into(), relocated_suffix: "".into() }); }
    let (line1, line2) = (&content[s1..e1], &content[s2..e2]);
    let mut r = String::with_capacity(content.len());
    if s1 < s2 { r.push_str(&content[..s1]); r.push_str(line2); r.push_str(&content[e1..s2]); r.push_str(line1); r.push_str(&content[e2..]); }
    else { r.push_str(&content[..s2]); r.push_str(line1); r.push_str(&content[e2..s1]); r.push_str(line2); r.push_str(&content[e1..]); }
    Ok(r)
}

pub fn fast_move_line(content: &str, source: usize, target: usize, hash: ShortHash, place_before: bool) -> Result<String, HashlineError> {
    if source == target { return Err(HashlineError::PatchFailed { op_index: 0, reason: "source and target must be different".into() }); }
    let b = content.as_bytes();
    let (ss, se) = (0..source).fold(Ok(0usize), |c: Result<usize,_>, _| c.and_then(|cur| match memchr(b'\n', &b[cur..]) { Some(r) => Ok(cur + r + 1), None => Err(HashlineError::MutationIndexOutOfBounds { index: source, len: cur + 1 }) })).and_then(|cur| Ok((cur, memchr(b'\n', &b[cur..]).map_or(content.len(), |r| cur + r))))?;
    let he = if se > ss && b[se-1] == b'\r' { se-1 } else { se };
    if hash::short_hash_value(&content[ss..he]) != hash { return Err(HashlineError::StaleAnchor { anchor: format!("{}", source+1).into(), line: source+1, expected: hash::format_short_hash(hash).into(), actual: hash::format_short_hash(hash).into(), path: "".into(), relocated_suffix: "".into() }); }
    let mut v: Vec<&str> = content.lines().collect();
    let line = v.remove(source);
    let adj = if source < target { target - 1 } else { target };
    v.insert(if place_before { adj } else { (adj + 1).min(v.len()) }, line);
    let sep = if content.contains("\r\n") { "\r\n" } else { "\n" };
    Ok(v.join(sep) + if content.ends_with('\n') { "\n" } else { "" })
}

pub fn fast_indent_lines(content: &str, start_line: usize, end_line: usize, hash: ShortHash, delta: isize) -> Result<String, HashlineError> {
    let b = content.as_bytes();
    let (ss, se) = (0..start_line).fold(Ok(0usize), |c: Result<usize,_>, _| c.and_then(|cur| match memchr(b'\n', &b[cur..]) { Some(r) => Ok(cur + r + 1), None => Err(HashlineError::MutationIndexOutOfBounds { index: start_line, len: cur + 1 }) })).and_then(|cur| Ok((cur, memchr(b'\n', &b[cur..]).map_or(content.len(), |r| cur + r))))?;
    let he = if se > ss && b[se-1] == b'\r' { se-1 } else { se };
    if hash::short_hash_value(&content[ss..he]) != hash { return Err(HashlineError::StaleAnchor { anchor: format!("{}", start_line+1).into(), line: start_line+1, expected: hash::format_short_hash(hash).into(), actual: hash::format_short_hash(hash).into(), path: "".into(), relocated_suffix: "".into() }); }
    let sep = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    for i in start_line..=end_line.min(lines.len().saturating_sub(1)) {
        if delta < 0 { for _ in 0..(-delta as usize) { if lines[i].starts_with(' ') { lines[i] = lines[i][1..].to_string(); } else { break; } } }
        else { let spaces = " ".repeat(delta as usize); lines[i] = format!("{}{}", spaces, lines[i]); }
    }
    Ok(lines.join(sep) + if content.ends_with('\n') { "\n" } else { "" })
}

// ===== Command handlers =====

/// Find a line by its short hash (raw hash anchor, no line number).
/// Returns 0-indexed line number if found.
pub fn fast_from_hash(content: &str, hash: ShortHash) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut line_start = 0usize;
    let mut line_no = 0usize;
    loop {
        let line_end = match memchr::memchr(b'\n', &bytes[line_start..]) {
            Some(rel) => line_start + rel,
            None => content.len(),
        };
        let he = if line_end > line_start && bytes[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };
        if hash::short_hash_value(&content[line_start..he]) == hash {
            return Some(line_no);
        }
        if line_end >= content.len() {
            break;
        }
        line_start = line_end + 1;
        line_no += 1;
    }
    None
}

/// Fuzzy anchor resolution: check hash at exact line, if no match, scan ±3 lines.
/// Useful when lines shifted due to concurrent edits.
pub fn fast_fuzzy_resolve(content: &str, line_no: usize, hash: ShortHash) -> Option<usize> {
    let bytes = content.as_bytes();
    // Helper to check hash at a given line
    let check = |ln: usize| -> bool {
        let mut current = 0usize;
        for _ in 0..ln {
            match memchr::memchr(b'\n', &bytes[current..]) {
                Some(r) => current += r + 1,
                None => return false,
            }
        }
        let start = current;
        let end = memchr::memchr(b'\n', &bytes[current..]).map_or(content.len(), |r| current + r);
        let he = if end > start && bytes[end - 1] == b'\r' { end - 1 } else { end };
        hash::short_hash_value(&content[start..he]) == hash
    };
    // Try exact first
    if check(line_no) {
        return Some(line_no);
    }
    // Scan ±3 neighbors
    let max = content.lines().count().saturating_sub(1);
    let start = line_no.saturating_sub(3);
    let end = (line_no + 3).min(max);
    for attempt in start..=end {
        if attempt == line_no { continue; }
        if check(attempt) {
            return Some(attempt);
        }
    }
    None
}
pub fn run_fast_edit<W: Write, E: Write>(
    ctx: &mut CommandContext<'_, W, E>,
    path: &Path,
    target_line: usize,
    expected_hash: ShortHash,
    new_content: &str,
    dry_run: bool,
    expect_mtime: Option<i64>,
    expect_inode: Option<u64>,
) -> Result<(), HashlineError> {
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::MetadataExt;
        if let Some(expected) = expect_mtime {
            let actual = meta.modified().ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if actual != expected {
                return Err(HashlineError::StaleFile { path: path.display().to_string() });
            }
        }
        if let Some(expected) = expect_inode {
            let actual = meta.ino() as u64;
            if actual != expected {
                return Err(HashlineError::StaleFile { path: path.display().to_string() });
            }
        }
    }
    let content_str = read_file(path)?;
    let (nc, _old) = fast_replace_line(&content_str, target_line, expected_hash, new_content)?;
    if !dry_run {
        atomic_write(path, &nc)?;
        if let Ok(doc) = Document::from_str(path, &nc) { ctx.modified_doc = Some(doc); }
    }
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Edited line {}.", target_line + 1)).map_err(HashlineError::from)?;
    }
    Ok(())
}

pub fn run_fast_insert<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, target_line: usize, _: ShortHash, new_content: &str) -> Result<(), HashlineError> {
    let content = read_file(path)?;
    let nc = fast_insert_line(&content, target_line, new_content)?;
    atomic_write(path, &nc)?; seed_cache(ctx, path, &nc);
    if ctx.output_mode() == OutputMode::Pretty { output::write_success_line(ctx, &format!("Inserted line {}.", target_line + 2)).map_err(HashlineError::from)?; }
    Ok(())
}

pub fn run_fast_delete<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, start_line: usize, end_line: usize, expected_start_hash: ShortHash) -> Result<(), HashlineError> {
    let content = read_file(path)?;
    let nc = fast_delete_lines(&content, start_line, end_line, expected_start_hash)?;
    atomic_write(path, &nc)?; seed_cache(ctx, path, &nc);
    if ctx.output_mode() == OutputMode::Pretty {
        if start_line == end_line { output::write_success_line(ctx, &format!("Deleted line {}.", start_line + 1)).map_err(HashlineError::from)?; }
        else { output::write_success_line(ctx, &format!("Deleted lines {}-{}.", start_line + 1, end_line + 1)).map_err(HashlineError::from)?; }
    }
    Ok(())
}

pub fn run_fast_swap<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, line1: usize, line2: usize, hash1: ShortHash, hash2: ShortHash) -> Result<(), HashlineError> {
    let content = read_file(path)?;
    let nc = fast_swap_lines(&content, line1, line2, hash1, hash2)?;
    atomic_write(path, &nc)?; seed_cache(ctx, path, &nc);
    if ctx.output_mode() == OutputMode::Pretty { output::write_success_line(ctx, &format!("Swapped lines {} and {}.", line1 + 1, line2 + 1)).map_err(HashlineError::from)?; }
    Ok(())
}

pub fn run_fast_move<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, source: usize, target: usize, hash: ShortHash, place_before: bool) -> Result<(), HashlineError> {
    let content = read_file(path)?;
    let nc = fast_move_line(&content, source, target, hash, place_before)?;
    atomic_write(path, &nc)?; seed_cache(ctx, path, &nc);
    let adj_target = if source < target { target - 1 } else { target };
    let to_line = if place_before { adj_target } else { adj_target + 1 } + 1;
    if ctx.output_mode() == OutputMode::Pretty {
        output::write_success_line(ctx, &format!("Moved line {} to line {}.", source + 1, to_line))
            .map_err(HashlineError::from)?;
    }
    Ok(())
}
pub fn run_fast_indent<W: Write, E: Write>(ctx: &mut CommandContext<'_, W, E>, path: &Path, start_line: usize, end_line: usize, hash: ShortHash, amount: isize) -> Result<(), HashlineError> {
    let content = read_file(path)?;
    let nc = fast_indent_lines(&content, start_line, end_line, hash, amount)?;
    atomic_write(path, &nc)?; seed_cache(ctx, path, &nc);
    if ctx.output_mode() == OutputMode::Pretty { output::write_success_line(ctx, "Indented.").map_err(HashlineError::from)?; }
    Ok(())
}
