#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use memchr::{memchr, memchr2};
use memmap2::Mmap;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::LinehashError;
use crate::hash::{self, ShortHash};

/// Files with at least this many lines hash their lines in parallel
/// via rayon. Below this threshold the sequential single-pass path
/// is faster because rayon's scheduling overhead dominates.
const PARALLEL_HASH_LINE_THRESHOLD: usize = 20_000;

/// When hashing in parallel, group lines into chunks of this size before
/// dispatching to rayon. Per-task overhead is ~tens of microseconds, so
/// we amortize it by keeping each rayon task busy for ~milliseconds.
const PARALLEL_HASH_CHUNK_SIZE: usize = 2_048;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LineView {
    pub n: usize,
    pub hash: String,
    pub content: String,
}

pub type ShortHashIndex = Vec<Vec<usize>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewlineStyle {
    Lf,
    Crlf,
}

impl NewlineStyle {
    fn separator(self) -> &'static str {
        match self {
            NewlineStyle::Lf => "\n",
            NewlineStyle::Crlf => "\r\n",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMeta {
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    pub inode: u64,
    pub size: u64,
    pub change_secs: i64,
    pub change_nanos: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineRecord {
    pub content: String,
    pub full_hash: u32,
    pub short_hash: ShortHash,
}

/// Hard cap on the number of collision pairs surfaced through
/// [`FileStats::collision_pairs`]. The total pair count grows as O(N²)
/// inside a single short-hash bucket, so on files with many duplicate lines
/// it can balloon into the billions and dominate `stats` latency for no
/// downstream benefit. The total count is always reported via
/// [`FileStats::collision_pair_count`].
pub const COLLISION_PAIRS_SAMPLE_CAP: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FileStats {
    pub line_count: usize,
    pub unique_hashes: usize,
    pub collision_count: usize,
    /// A bounded sample of collision pairs (1-indexed line numbers) suitable
    /// for surfacing examples to the user. Capped at
    /// [`COLLISION_PAIRS_SAMPLE_CAP`] entries; see
    /// [`FileStats::collision_pair_count`] for the true total.
    pub collision_pairs: Vec<(usize, usize)>,
    /// True total number of unordered collision pairs across all short-hash
    /// buckets, computed in closed form (Σ |b|*(|b|-1)/2). May exceed
    /// `collision_pairs.len()`.
    pub collision_pair_count: u64,
    /// Set to `true` when [`FileStats::collision_pairs`] is a truncated
    /// sample (i.e. there are more collision pairs than the sample cap).
    pub collision_pairs_truncated: bool,
    pub estimated_read_tokens: usize,
    pub hash_length_advice: u8,
    pub suggested_context_n: usize,
    pub recommended_read_mode: &'static str,
    pub recommended_anchor_mode: &'static str,
    pub recommended_workflow: &'static str,
    pub warnings: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Document {
    pub path: PathBuf,
    pub newline: NewlineStyle,
    pub trailing_newline: bool,
    pub lines: Vec<LineRecord>,
    pub content_len: usize,
    pub file_meta: Option<FileMeta>,
    #[doc(hidden)]
    pub short_hash_index: Option<ShortHashIndex>,
}

#[derive(Clone, Debug)]
pub struct SearchDocument {
    pub path: PathBuf,
    pub content: String,
    pub newline: NewlineStyle,
    pub trailing_newline: bool,
    pub line_offsets: Vec<usize>,
}

impl SearchDocument {
    pub fn load(path: &Path) -> Result<SearchDocument, LinehashError> {
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        let path_string = path.display().to_string();

        if metadata.len() == 0 {
            return Ok(SearchDocument {
                path: path.to_path_buf(),
                content: String::new(),
                newline: NewlineStyle::Lf,
                trailing_newline: false,
                line_offsets: vec![0],
            });
        }

        let mmap = unsafe { Mmap::map(&file) }?;
        let bytes = &mmap[..];

        if memchr(0, &bytes[..bytes.len().min(8_000)]).is_some() {
            return Err(LinehashError::BinaryFile { path: path_string });
        }

        let content_owned = std::str::from_utf8(bytes)
            .map_err(|_| LinehashError::InvalidUtf8 {
                path: path_string.clone(),
            })?
            .to_owned();

        let (newline, trailing_newline, line_offsets) = parse_line_offsets(&content_owned);
        let path_buf = path.to_path_buf();
        drop(file);
        drop(mmap);
        let _ = metadata;

        Ok(SearchDocument {
            path: path_buf,
            content: content_owned,
            newline,
            trailing_newline,
            line_offsets,
        })
    }

    pub fn grep_lines(&self, pattern: &str, invert: bool) -> Vec<LineView> {
        let pattern_bytes = pattern.as_bytes();
        let pat_len = pattern_bytes.len();
        let mut results = Vec::new();

        for (line_idx, &start) in self.line_offsets.iter().enumerate() {
            let end = if line_idx + 1 < self.line_offsets.len() {
                self.line_offsets[line_idx + 1]
            } else {
                self.content.len()
            };
            let line_end = if self.trailing_newline
                && end > start
                && self.content.as_bytes()[end.saturating_sub(1)] == b'\n'
            {
                end - 1
            } else {
                end.min(self.content.len())
            };
            let line_content = self.content[start..line_end]
                .strip_suffix('\r')
                .unwrap_or(&self.content[start..line_end]);

            let is_match = if pat_len == 1 {
                memchr(pattern_bytes[0], line_content.as_bytes()).is_some()
            } else if pat_len <= line_content.len() {
                line_content
                    .as_bytes()
                    .windows(pat_len)
                    .any(|w| w == pattern_bytes)
            } else {
                false
            };

            let include = if invert { !is_match } else { is_match };
            if include {
                let full_hash = hash::full_hash(line_content);
                let short_hash = hash::short_from_full(full_hash);
                results.push(LineView {
                    n: line_idx + 1,
                    hash: hash::format_short_hash(short_hash),
                    content: line_content.to_string(),
                });
            }
        }

        results
    }
}

impl Document {
    pub fn load(path: &Path) -> Result<Document, LinehashError> {
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        let path_string = path.display().to_string();

        if metadata.len() == 0 {
            return Ok(Document {
                path: path.to_path_buf(),
                newline: NewlineStyle::Lf,
                trailing_newline: false,
                lines: Vec::new(),
                content_len: 0,
                file_meta: Some(FileMeta::from_metadata(&metadata)?),
                short_hash_index: None,
            });
        }

        let mmap = unsafe { Mmap::map(&file) }?;
        let bytes = &mmap[..];

        if memchr(0, &bytes[..bytes.len().min(8_000)]).is_some() {
            return Err(LinehashError::BinaryFile { path: path_string });
        }

        let content = std::str::from_utf8(bytes).map_err(|_| LinehashError::InvalidUtf8 {
            path: path_string.clone(),
        })?;

        let (newline, trailing_newline, lines, content_len) =
            parse_document_content(content, path)?;
        let file_meta = Some(FileMeta::from_metadata(&metadata)?);

        Ok(Document {
            path: path.to_path_buf(),
            newline,
            trailing_newline,
            lines,
            content_len,
            file_meta,
            short_hash_index: None,
        })
    }

    pub fn from_str(path: &Path, content: &str) -> Result<Document, LinehashError> {
        let (newline, trailing_newline, lines, content_len) =
            parse_document_content(content, path)?;

        Ok(Document {
            path: path.to_path_buf(),
            newline,
            trailing_newline,
            lines,
            content_len,
            file_meta: None,
            short_hash_index: None,
        })
    }

    pub fn build_index(&self) -> ShortHashIndex {
        let counts = count_short_hashes(&self.lines);
        build_index_from_counts(&self.lines, &counts)
    }

    /// Build and cache index, returning cached reference.
    /// Call this on a &mut Document to populate the cache for future calls.
    pub fn build_index_cached(doc: &mut Document) -> &ShortHashIndex {
        if doc.short_hash_index.is_none() {
            let counts = count_short_hashes(&doc.lines);
            doc.short_hash_index = Some(build_index_from_counts(&doc.lines, &counts));
        }
        doc.short_hash_index.as_ref().unwrap()
    }

    pub fn render(&self) -> Vec<u8> {
        if self.lines.is_empty() {
            return Vec::new();
        }

        let separator = self.newline.separator().as_bytes();
        let separator_count =
            self.lines.len().saturating_sub(1) + usize::from(self.trailing_newline);
        let mut rendered = Vec::with_capacity(self.content_len + separator.len() * separator_count);

        let mut first = true;
        for line in &self.lines {
            if !first {
                rendered.extend_from_slice(separator);
            }
            first = false;
            rendered.extend_from_slice(line.content.as_bytes());
        }

        if self.trailing_newline {
            rendered.extend_from_slice(separator);
        }

        rendered
    }

    pub fn compute_stats(&self) -> FileStats {
        let bucket_counts = count_short_hashes(&self.lines);
        let index = build_index_from_counts(&self.lines, &bucket_counts);
        let (mut collision_pairs, collision_pair_count) =
            collect_collision_pairs_sample(&index, COLLISION_PAIRS_SAMPLE_CAP);
        collision_pairs.sort_unstable();
        let collision_pairs_truncated = collision_pair_count > collision_pairs.len() as u64;
        let (unique_hashes, collision_count) = summarize_bucket_counts(&bucket_counts);

        let estimated_read_tokens = estimate_read_tokens(self);
        let hash_length_advice = recommend_hash_length(self);
        let suggested_context_n = suggest_context_n(self);
        let recommended_read_mode = recommend_read_mode(self, estimated_read_tokens);
        let recommended_anchor_mode =
            recommend_anchor_mode(self, collision_count, hash_length_advice);
        let recommended_workflow = recommend_workflow(self, estimated_read_tokens, collision_count);
        let warnings = collect_warnings(
            self,
            estimated_read_tokens,
            collision_count,
            hash_length_advice,
        );

        FileStats {
            line_count: self.len(),
            unique_hashes,
            collision_count,
            collision_pairs,
            collision_pair_count,
            collision_pairs_truncated,
            estimated_read_tokens,
            hash_length_advice,
            suggested_context_n,
            recommended_read_mode,
            recommended_anchor_mode,
            recommended_workflow,
            warnings,
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

impl FileMeta {
    fn from_metadata(metadata: &fs::Metadata) -> Result<Self, LinehashError> {
        let modified = metadata.modified()?;
        let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
        let (change_secs, change_nanos) = change_time_from_metadata(metadata);

        Ok(Self {
            mtime_secs: duration.as_secs() as i64,
            mtime_nanos: duration.subsec_nanos(),
            inode: inode_from_metadata(metadata),
            size: metadata.len(),
            change_secs,
            change_nanos,
        })
    }
}

pub fn read_file_meta(path: &Path) -> Result<FileMeta, LinehashError> {
    let metadata = fs::metadata(path)?;
    FileMeta::from_metadata(&metadata)
}

pub fn format_short_hash(short_hash: ShortHash) -> String {
    hash::format_short_hash(short_hash)
}

fn parse_document_content(
    content: &str,
    path: &Path,
) -> Result<(NewlineStyle, bool, Vec<LineRecord>, usize), LinehashError> {
    if content.is_empty() {
        return Ok((NewlineStyle::Lf, false, Vec::new(), 0));
    }

    let bytes = content.as_bytes();
    let trailing_newline = content.ends_with('\n');
    let estimated_line_count = memchr::memchr_iter(b'\n', bytes).count();

    if estimated_line_count >= PARALLEL_HASH_LINE_THRESHOLD {
        // Large file: scan line boundaries first, then hash chunks in
        // parallel via rayon. The (start, end) range vec adds an extra
        // allocation, but rayon parallelism on a non-trivial CPU workload
        // amortizes it many times over.
        parse_document_content_parallel(
            content,
            bytes,
            path,
            trailing_newline,
            estimated_line_count,
        )
    } else {
        // Small file: single sequential pass builds LineRecords inline,
        // avoiding the intermediate ranges Vec entirely. This path matches
        // the historical single-pass implementation and is the fastest
        // option when there is not enough work to parallelize.
        parse_document_content_sequential(
            content,
            bytes,
            path,
            trailing_newline,
            estimated_line_count,
        )
    }
}

fn parse_document_content_sequential(
    content: &str,
    bytes: &[u8],
    path: &Path,
    trailing_newline: bool,
    estimated_line_count: usize,
) -> Result<(NewlineStyle, bool, Vec<LineRecord>, usize), LinehashError> {
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_bare_cr = false;
    let mut newline = NewlineStyle::Lf;
    let mut lines = Vec::with_capacity(estimated_line_count + usize::from(!trailing_newline));
    let mut start = 0usize;
    let mut search_from = 0usize;
    let mut content_len = 0usize;

    while let Some(relative) = memchr2(b'\n', b'\r', &bytes[search_from..]) {
        let index = search_from + relative;
        match bytes[index] {
            b'\r' => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                    saw_crlf = true;
                    newline = NewlineStyle::Crlf;
                    let line = &content[start..index];
                    content_len += line.len();
                    lines.push(build_line_record(line));
                    search_from = index + 2;
                    start = search_from;
                } else {
                    saw_bare_cr = true;
                    search_from = index + 1;
                }
            }
            b'\n' => {
                saw_lf = true;
                let line = &content[start..index];
                content_len += line.len();
                lines.push(build_line_record(line));
                search_from = index + 1;
                start = search_from;
            }
            _ => unreachable!("memchr2 only returns requested bytes"),
        }
    }

    if saw_bare_cr || (saw_crlf && saw_lf) {
        return Err(LinehashError::MixedNewlines {
            path: path.display().to_string(),
        });
    }

    if !trailing_newline && start < content.len() {
        let line = &content[start..];
        content_len += line.len();
        lines.push(build_line_record(line));
    }

    Ok((newline, trailing_newline, lines, content_len))
}

fn parse_document_content_parallel(
    content: &str,
    bytes: &[u8],
    path: &Path,
    trailing_newline: bool,
    estimated_line_count: usize,
) -> Result<(NewlineStyle, bool, Vec<LineRecord>, usize), LinehashError> {
    // Phase 1: scan for line boundaries with memchr — fast, single pass,
    // no hashing here. We record (start, end) byte ranges so the hashing
    // phase can run in parallel without mutating shared state.
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_bare_cr = false;
    let mut newline = NewlineStyle::Lf;
    let mut ranges: Vec<(usize, usize)> =
        Vec::with_capacity(estimated_line_count + usize::from(!trailing_newline));
    let mut start = 0usize;
    let mut search_from = 0usize;

    while let Some(relative) = memchr2(b'\n', b'\r', &bytes[search_from..]) {
        let index = search_from + relative;
        match bytes[index] {
            b'\r' => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                    saw_crlf = true;
                    newline = NewlineStyle::Crlf;
                    ranges.push((start, index));
                    search_from = index + 2;
                    start = search_from;
                } else {
                    saw_bare_cr = true;
                    search_from = index + 1;
                }
            }
            b'\n' => {
                saw_lf = true;
                ranges.push((start, index));
                search_from = index + 1;
                start = search_from;
            }
            _ => unreachable!("memchr2 only returns requested bytes"),
        }
    }

    if saw_bare_cr || (saw_crlf && saw_lf) {
        return Err(LinehashError::MixedNewlines {
            path: path.display().to_string(),
        });
    }

    if !trailing_newline && start < content.len() {
        ranges.push((start, content.len()));
    }

    // Phase 2: hash each line in parallel. We dispatch in chunks of
    // PARALLEL_HASH_CHUNK_SIZE so per-task overhead is amortized — per-line
    // parallelism is too fine-grained for xxh32 to be worthwhile.
    let content_len: usize = ranges.iter().map(|(s, e)| e - s).sum();
    let lines: Vec<LineRecord> = ranges
        .par_chunks(PARALLEL_HASH_CHUNK_SIZE)
        .flat_map_iter(|chunk| {
            chunk
                .iter()
                .map(|&(s, e)| build_line_record(&content[s..e]))
                .collect::<Vec<_>>()
        })
        .collect();

    Ok((newline, trailing_newline, lines, content_len))
}

fn parse_line_offsets(content: &str) -> (NewlineStyle, bool, Vec<usize>) {
    if content.is_empty() {
        return (NewlineStyle::Lf, false, vec![0]);
    }

    let bytes = content.as_bytes();
    let mut saw_lf = false;
    let mut saw_crlf = false;
    let mut saw_bare_cr = false;
    let mut newline = NewlineStyle::Lf;
    let trailing_newline = content.ends_with('\n');
    let estimated_lines = memchr::memchr_iter(b'\n', bytes).count() + 1;
    let mut line_offsets = Vec::with_capacity(estimated_lines);
    line_offsets.push(0);
    let mut search_from = 0;

    while let Some(relative) = memchr2(b'\n', b'\r', &bytes[search_from..]) {
        let index = search_from + relative;
        match bytes[index] {
            b'\r' => {
                if index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                    saw_crlf = true;
                    newline = NewlineStyle::Crlf;
                    search_from = index + 2;
                    line_offsets.push(search_from);
                } else {
                    saw_bare_cr = true;
                    search_from = index + 1;
                }
            }
            b'\n' => {
                saw_lf = true;
                search_from = index + 1;
                line_offsets.push(search_from);
            }
            _ => unreachable!(),
        }
    }

    if saw_bare_cr || (saw_crlf && saw_lf) {
        return (NewlineStyle::Lf, trailing_newline, line_offsets);
    }

    (newline, trailing_newline, line_offsets)
}

fn build_line_record(content: &str) -> LineRecord {
    let full_hash = hash::full_hash(content);
    LineRecord {
        content: content.to_owned(),
        full_hash,
        short_hash: hash::short_from_full(full_hash),
    }
}

fn empty_index() -> ShortHashIndex {
    vec![Vec::new(); 256]
}

pub fn count_short_hashes(lines: &[LineRecord]) -> [usize; 256] {
    let mut counts = [0; 256];
    for line in lines {
        counts[line.short_hash as usize] += 1;
    }
    counts
}

pub fn build_index_from_counts(lines: &[LineRecord], counts: &[usize; 256]) -> ShortHashIndex {
    let mut index = empty_index();
    for (bucket, count) in counts.iter().enumerate() {
        if *count > 0 {
            index[bucket] = Vec::with_capacity(*count);
        }
    }

    for (line_index, line) in lines.iter().enumerate() {
        index[line.short_hash as usize].push(line_index);
    }

    index
}

fn summarize_bucket_counts(counts: &[usize; 256]) -> (usize, usize) {
    let mut unique_hashes = 0;
    let mut collision_count = 0;

    for count in counts {
        if *count == 0 {
            continue;
        }
        unique_hashes += 1;
        if *count >= 2 {
            collision_count += *count;
        }
    }

    (unique_hashes, collision_count)
}

/// Walk the short-hash index and return:
///
/// * the **true total** number of unordered collision pairs, computed via the
///   closed-form sum `Σ |b| * (|b| - 1) / 2` over each bucket. This is
///   `O(unique buckets)` and never materialises the cross-product.
/// * a bounded **sample** of those pairs (capped at `sample_cap`) for
///   surfacing examples to the user.
///
/// The previous implementation materialised every pair just to call `.len()`
/// on the result, which is O(N²) in the worst case (one bucket with all
/// lines) and made `stats` unusable on files with many duplicate lines
/// (e.g. a 500K-line file of repeated blanks took ~24 s; with this change
/// it returns in tens of milliseconds).
fn collect_collision_pairs_sample(
    index: &ShortHashIndex,
    sample_cap: usize,
) -> (Vec<(usize, usize)>, u64) {
    let mut total: u64 = 0;
    let mut sample: Vec<(usize, usize)> = Vec::new();

    for positions in index.iter().filter(|positions| positions.len() >= 2) {
        let n = positions.len() as u64;
        total = total.saturating_add(n * (n - 1) / 2);

        if sample.len() < sample_cap {
            'outer: for left in 0..positions.len() {
                for right in left + 1..positions.len() {
                    sample.push((positions[left] + 1, positions[right] + 1));
                    if sample.len() >= sample_cap {
                        break 'outer;
                    }
                }
            }
        }
    }

    (sample, total)
}

fn estimate_read_tokens(doc: &Document) -> usize {
    let anchor_overhead = doc.lines.len() * 8;
    (doc.content_len + anchor_overhead) / 4
}

fn recommend_hash_length(doc: &Document) -> u8 {
    let line_count = doc.len();
    for hash_len in [2_u8, 3, 4] {
        let buckets = 16_f64.powi(i32::from(hash_len));
        if collision_probability(line_count, buckets) < 0.01 {
            return hash_len;
        }
    }
    4
}

fn collision_probability(line_count: usize, buckets: f64) -> f64 {
    if line_count <= 1 {
        return 0.0;
    }

    let line_count = line_count as f64;
    1.0 - (-(line_count * (line_count - 1.0)) / (2.0 * buckets)).exp()
}

fn suggest_context_n(doc: &Document) -> usize {
    let markers = doc
        .lines
        .iter()
        .map(|line| line.content.as_str())
        .enumerate()
        .filter_map(|(index, content)| is_structure_marker(content).then_some(index + 1))
        .collect::<Vec<_>>();

    if markers.len() < 2 {
        return 5;
    }

    let mut gaps = markers
        .windows(2)
        .map(|window| window[1] - window[0])
        .collect::<Vec<_>>();
    gaps.sort_unstable();
    let median_gap = gaps[gaps.len() / 2];
    (median_gap / 2).clamp(3, 20)
}

fn recommend_read_mode(doc: &Document, estimated_read_tokens: usize) -> &'static str {
    if doc.is_empty() || (estimated_read_tokens <= 2_000 && doc.len() <= 400) {
        "read"
    } else if estimated_read_tokens <= 8_000 {
        "read --anchor <line:hash> --context N"
    } else {
        "index or read --anchor <line:hash> --context N"
    }
}

fn recommend_anchor_mode(
    doc: &Document,
    collision_count: usize,
    hash_length_advice: u8,
) -> &'static str {
    if doc.is_empty() || collision_count > 0 || doc.len() >= 200 || hash_length_advice > 2 {
        "qualified"
    } else {
        "bare-or-qualified"
    }
}

fn recommend_workflow(
    doc: &Document,
    estimated_read_tokens: usize,
    collision_count: usize,
) -> &'static str {
    if doc.is_empty() {
        "read-empty-file"
    } else if collision_count > 0 {
        "stats -> annotate/grep -> read --anchor --context -> edit/patch -> verify"
    } else if estimated_read_tokens > 8_000 {
        "index -> annotate/grep -> read --anchor --context -> edit/patch -> verify"
    } else {
        "read -> annotate/grep -> verify -> edit/patch -> verify"
    }
}

fn collect_warnings(
    doc: &Document,
    estimated_read_tokens: usize,
    collision_count: usize,
    hash_length_advice: u8,
) -> Vec<&'static str> {
    let mut warnings = Vec::new();

    if collision_count > 0 {
        warnings.push("short-hash collisions detected; prefer qualified anchors like 12:ab");
    }
    if hash_length_advice > 2 {
        warnings.push("2-char hashes may be cramped for this file; use stats and qualified anchors to avoid ambiguity");
    }
    if estimated_read_tokens > 8_000 {
        warnings.push("full read output will be expensive; orient with index/stats, then narrow with --anchor and --context");
    }
    if doc.len() > 2_000 {
        warnings.push("large file: prefer patch/find-block workflows over many tiny edits");
    }

    warnings
}

fn is_structure_marker(content: &str) -> bool {
    ["function ", "def ", "class ", "fn ", "impl "]
        .iter()
        .any(|marker| content.contains(marker))
}

#[cfg(unix)]
fn inode_from_metadata(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.ino()
}

#[cfg(not(unix))]
fn inode_from_metadata(_metadata: &fs::Metadata) -> u64 {
    0
}

#[cfg(unix)]
fn change_time_from_metadata(metadata: &fs::Metadata) -> (i64, u32) {
    use std::os::unix::fs::MetadataExt;

    (metadata.ctime(), metadata.ctime_nsec() as u32)
}

#[cfg(not(unix))]
fn change_time_from_metadata(_metadata: &fs::Metadata) -> (i64, u32) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::{Document, FileStats, NewlineStyle, format_short_hash};
    use crate::error::LinehashError;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn test_load_lf_simple() {
        let (_dir, path) = write_temp_file("alpha\nbeta\n");
        let document = Document::load(&path).unwrap();

        assert_eq!(document.newline, NewlineStyle::Lf);
        assert!(document.trailing_newline);
        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[0].content, "alpha");
        assert_eq!(document.lines[1].content, "beta");
    }

    #[test]
    fn test_load_crlf_simple() {
        let (_dir, path) = write_temp_file("alpha\r\nbeta\r\n");
        let document = Document::load(&path).unwrap();

        assert_eq!(document.newline, NewlineStyle::Crlf);
        assert!(document.trailing_newline);
        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[1].content, "beta");
    }

    #[test]
    fn test_load_single_line_no_trailing_newline() {
        let (_dir, path) = write_temp_file("alpha");
        let document = Document::load(&path).unwrap();

        assert_eq!(document.lines.len(), 1);
        assert_eq!(document.lines[0].content, "alpha");
        assert!(!document.trailing_newline);
    }

    #[test]
    fn test_load_single_line_with_trailing_newline() {
        let (_dir, path) = write_temp_file("alpha\n");
        let document = Document::load(&path).unwrap();

        assert_eq!(document.lines.len(), 1);
        assert_eq!(document.lines[0].content, "alpha");
        assert!(document.trailing_newline);
    }

    #[test]
    fn test_load_empty_file() {
        let (_dir, path) = write_temp_file("");
        let document = Document::load(&path).unwrap();

        assert!(document.lines.is_empty());
        assert!(!document.trailing_newline);
    }

    #[test]
    fn test_load_whitespace_only_lines() {
        let (_dir, path) = write_temp_file("  \n\t\n");
        let document = Document::load(&path).unwrap();

        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[0].content, "  ");
        assert_eq!(document.lines[1].content, "\t");
    }

    #[test]
    fn test_load_mixed_newlines_fails() {
        let (_dir, path) = write_temp_file("alpha\r\nbeta\n");
        let error = Document::load(&path).unwrap_err();

        assert!(matches!(error, LinehashError::MixedNewlines { .. }));
    }

    #[test]
    fn test_load_invalid_utf8_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invalid.txt");
        fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

        let error = Document::load(&path).unwrap_err();
        assert!(matches!(error, LinehashError::InvalidUtf8 { .. }));
    }

    #[test]
    fn test_load_binary_file_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("binary.bin");
        fs::write(&path, b"abc\0def").unwrap();

        let error = Document::load(&path).unwrap_err();
        assert!(matches!(error, LinehashError::BinaryFile { .. }));
    }

    #[test]
    fn test_binary_check_precedes_utf8_error_when_nul_is_present() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("binary-or-invalid.bin");
        let bytes = vec![0xff, 0x00, 0xfe];
        fs::write(&path, bytes).unwrap();

        let error = Document::load(&path).unwrap_err();
        assert!(matches!(error, LinehashError::BinaryFile { .. }));
    }

    #[test]
    fn test_binary_file_hint_matches_product_wording() {
        let error = LinehashError::BinaryFile {
            path: "demo.bin".to_owned(),
        };
        assert_eq!(
            error.hint(),
            Some("linehash only supports UTF-8 text files")
        );
    }

    #[test]
    fn test_render_lf_round_trip() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        assert_eq!(doc.render(), b"alpha\nbeta\n");
    }

    #[test]
    fn test_render_crlf_round_trip() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\r\nbeta\r\n").unwrap();
        assert_eq!(doc.render(), b"alpha\r\nbeta\r\n");
    }

    #[test]
    fn test_render_no_trailing_newline_preserved() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta").unwrap();
        assert_eq!(doc.render(), b"alpha\nbeta");
    }

    #[test]
    fn test_render_trailing_newline_preserved() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        assert_eq!(doc.render(), b"alpha\n");
    }

    #[test]
    fn test_render_empty_document_is_empty_bytes() {
        let doc = Document::from_str(Path::new("demo.txt"), "").unwrap();
        assert!(doc.render().is_empty());
    }

    #[test]
    fn test_line_order_matches_vector_positions() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        assert_eq!(doc.lines[0].content, "alpha");
        assert_eq!(doc.lines[1].content, "beta");
    }

    #[test]
    fn test_build_index_unique_hashes() {
        let doc = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let index = doc.build_index();
        let alpha_hash = doc.lines[0].short_hash as usize;
        let beta_hash = doc.lines[1].short_hash as usize;
        assert_eq!(index[alpha_hash], vec![0]);
        assert_eq!(index[beta_hash], vec![1]);
    }

    #[test]
    fn test_build_index_collision_has_multiple_entries() {
        let (first, second) = find_collision_pair().expect("collision pair should exist");
        let doc =
            Document::from_str(Path::new("demo.txt"), &format!("{first}\n{second}\n")).unwrap();
        let index = doc.build_index();
        let short = doc.lines[0].short_hash as usize;
        assert_eq!(index[short], vec![0, 1]);
    }

    #[test]
    fn test_empty_file_stats() {
        let document = Document::from_str(Path::new("demo.txt"), "").unwrap();
        let stats = document.compute_stats();
        assert_eq!(
            stats,
            FileStats {
                line_count: 0,
                unique_hashes: 0,
                collision_count: 0,
                collision_pairs: vec![],
                collision_pair_count: 0,
                collision_pairs_truncated: false,
                estimated_read_tokens: 0,
                hash_length_advice: 2,
                suggested_context_n: 5,
                recommended_read_mode: "read",
                recommended_anchor_mode: "qualified",
                recommended_workflow: "read-empty-file",
                warnings: vec![],
            }
        );
    }

    #[test]
    fn test_no_collisions_file_stats() {
        let document = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        let stats = document.compute_stats();
        assert_eq!(stats.line_count, 2);
        assert_eq!(stats.unique_hashes, 2);
        assert_eq!(stats.collision_count, 0);
        assert!(stats.collision_pairs.is_empty());
    }

    #[test]
    fn test_collision_count_and_pairs_correct() {
        let (first, second) = find_collision_pair().expect("collision pair should exist");
        let document = Document::from_str(
            Path::new("demo.txt"),
            &format!("{first}\n{second}\nunique\n"),
        )
        .unwrap();
        let stats = document.compute_stats();
        assert_eq!(stats.collision_count, 2);
        assert_eq!(stats.collision_pairs, vec![(1, 2)]);
    }

    #[test]
    fn test_token_estimate_proportional_to_size() {
        let short = Document::from_str(Path::new("demo.txt"), "a\n").unwrap();
        let long = Document::from_str(Path::new("demo.txt"), "a very long line indeed\n").unwrap();
        assert!(
            long.compute_stats().estimated_read_tokens
                > short.compute_stats().estimated_read_tokens
        );
    }

    #[test]
    fn test_hash_length_advice_2_for_small_file() {
        let document = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\n").unwrap();
        assert_eq!(document.compute_stats().hash_length_advice, 2);
    }

    #[test]
    fn test_hash_length_advice_4_for_medium_file() {
        let content = (0..200)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let document = Document::from_str(Path::new("demo.txt"), &content).unwrap();
        assert_eq!(document.compute_stats().hash_length_advice, 4);
    }

    #[test]
    fn test_context_suggestion_minimum_3_with_dense_markers() {
        let document =
            Document::from_str(Path::new("demo.txt"), "fn a\nfn b\nfn c\nfn d\n").unwrap();
        assert_eq!(document.compute_stats().suggested_context_n, 3);
    }

    #[test]
    fn test_context_suggestion_falls_back_to_5_without_markers() {
        let document = Document::from_str(Path::new("demo.txt"), "alpha\nbeta\ngamma\n").unwrap();
        assert_eq!(document.compute_stats().suggested_context_n, 5);
    }

    #[test]
    fn test_context_suggestion_capped_at_20() {
        let mut lines = (0..100).map(|i| format!("line-{i}")).collect::<Vec<_>>();
        lines.insert(0, String::from("fn a"));
        lines.push(String::from("fn b"));
        let document =
            Document::from_str(Path::new("demo.txt"), &(lines.join("\n") + "\n")).unwrap();
        assert_eq!(document.compute_stats().suggested_context_n, 20);
    }

    #[test]
    fn test_filemeta_captured() {
        let (_dir, path) = write_temp_file("alpha\n");
        let document = Document::load(&path).unwrap();

        let meta = document.file_meta.expect("metadata should be present");
        assert!(meta.mtime_secs > 0);
        #[cfg(unix)]
        assert!(meta.inode > 0);
    }

    #[test]
    fn test_short_hash_formatting_round_trip() {
        let document = Document::from_str(Path::new("demo.txt"), "alpha\n").unwrap();
        assert_eq!(format_short_hash(document.lines[0].short_hash).len(), 2);
    }

    fn write_temp_file(content: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("demo.txt");
        fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn find_collision_pair() -> Option<(String, String)> {
        for i in 0..10_000 {
            let left = format!("line-{i}");
            for j in (i + 1)..10_000 {
                let right = format!("line-{j}");
                let doc = Document::from_str(Path::new("demo.txt"), &format!("{left}\n{right}\n"))
                    .unwrap();
                if doc.lines[0].short_hash == doc.lines[1].short_hash {
                    return Some((left, right));
                }
            }
        }
        None
    }
}
