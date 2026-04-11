//! Trigram index types for instant grep.
//!
//! This module defines the core data structures for a trigram-based inverted index,
//! inspired by Cursor's instant grep algorithm. The index maps 3-byte trigrams
//! (every 3-character sequence in a line) to posting lists that record which lines
//! contain each trigram and the context around each occurrence.
//!
//! # Trigram Decomposition
//!
//! Each line of content is decomposed into overlapping 3-byte trigrams:
//! ```
//! "hello" → ["hel", "ell", "llo"]
//! ```
//!
//! # Mask System
//!
//! - **LocMask**: 8-bit mask where bit `i` is set if the trigram starts at position `i % 8`
//! - **NextMask**: 8-bit bloom filter of characters immediately following the trigram,
//!   used to quickly reject false-positive matches during candidate filtering

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

/// 3-byte trigram stored as a 24-bit integer (first 3 bytes of the sequence).
pub type Trigram = u32;

/// 8-bit position mask: bit `i` is set if the trigram starts at position `i % 8`.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct LocMask(pub u8);

impl LocMask {
    /// Create a new LocMask with a single bit set at the given position.
    #[inline]
    pub fn new(pos: u8) -> Self {
        Self(1u8 << (pos % 8))
    }

    /// Check if this mask intersects with another (used for candidate filtering).
    #[inline]
    pub fn intersects(self, other: LocMask) -> bool {
        (self.0 & other.0) != 0
    }

    /// Check if this mask contains a specific position.
    #[inline]
    pub fn contains(self, pos: u8) -> bool {
        (self.0 & (1u8 << (pos % 8))) != 0
    }

    /// Union of two masks.
    #[inline]
    pub fn union(self, other: LocMask) -> LocMask {
        Self(self.0 | other.0)
    }

    /// Intersection of two masks.
    #[inline]
    pub fn intersection(self, other: LocMask) -> LocMask {
        Self(self.0 & other.0)
    }

    /// Returns true if the mask is empty (no bits set).
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Get the raw u8 value.
    #[inline]
    pub fn get(self) -> u8 {
        self.0
    }
}

/// 8-bit bloom filter mask for characters following a trigram.
///
/// This is a cheap bloom filter: bit `i` is set if any character with
/// value `c` where `c % 8 == i` follows this trigram somewhere in the line.
/// Used to quickly filter false-positive candidates.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct NextMask(pub u8);

impl NextMask {
    /// Create a new NextMask with a bit set for the given following character.
    #[inline]
    pub fn new(following_char: u8) -> Self {
        Self(1u8 << (following_char % 8))
    }

    /// Update the mask with a following character.
    #[inline]
    pub fn insert(&mut self, following_char: u8) {
        self.0 |= 1u8 << (following_char % 8);
    }

    /// Check if this mask could match another (bloom filter check).
    #[inline]
    pub fn could_match(self, other: NextMask) -> bool {
        // If either mask is empty, it could match anything
        if self.0 == 0 || other.0 == 0 {
            return true;
        }
        // Otherwise check bloom filter intersection
        (self.0 & other.0) != 0
    }

    /// Merge another mask into this one.
    #[inline]
    pub fn merge(&mut self, other: NextMask) {
        self.0 |= other.0;
    }

    /// Get the raw u8 value.
    #[inline]
    pub fn get(self) -> u8 {
        self.0
    }
}

/// A single posting: the location of a trigram occurrence in a specific line.
///
/// A posting records that a trigram appears in a particular line, along with
/// metadata (position and following character) used for candidate filtering.
#[derive(Clone, Debug)]
pub struct Posting {
    /// The 0-based line index where this trigram appears.
    pub line_idx: u32,
    /// Mask of positions within the line where this trigram starts.
    pub loc_mask: LocMask,
    /// Bloom filter of characters that follow this trigram in the line.
    pub next_mask: NextMask,
}

impl Posting {
    /// Create a new posting for a trigram at a specific position with a following char.
    #[inline]
    pub fn new(line_idx: u32, pos: u8, following_char: Option<u8>) -> Self {
        Self {
            line_idx,
            loc_mask: LocMask::new(pos),
            next_mask: following_char.map_or(NextMask::default(), NextMask::new),
        }
    }

    /// Update this posting's masks to include an additional occurrence.
    #[inline]
    pub fn add_occurrence(&mut self, pos: u8, following_char: Option<u8>) {
        self.loc_mask = self.loc_mask.union(LocMask::new(pos));
        if let Some(c) = following_char {
            self.next_mask.insert(c);
        }
    }
}

/// Metadata about the indexed file, used for validity checking.
///
/// The index is considered stale if the file's mtime or content hash doesn't match.
#[derive(Clone, Debug)]
pub struct IndexMeta {
    /// Last modification time of the file (seconds since epoch).
    pub file_mtime: u64,
    /// Size of the file in bytes.
    pub file_size: u64,
    /// xxHash64 of the full file content for change detection.
    pub content_hash: u64,
    /// Number of lines in the indexed file.
    pub line_count: u32,
}

impl IndexMeta {
    /// Check if this metadata matches the given file stats.
    pub fn matches(&self, mtime: u64, size: u64, content_hash: u64, line_count: u32) -> bool {
        self.file_mtime == mtime
            && self.file_size == size
            && self.content_hash == content_hash
            && self.line_count == line_count
    }
}

/// The trigram inverted index.
///
/// Maps each unique trigram to a list of postings, where each posting
/// records which line(s) contain that trigram and metadata for filtering.
#[derive(Clone, Debug)]
pub struct TrigramIndex {
    /// The inverted index: trigram → list of postings.
    trigrams: HashMap<Trigram, Vec<Posting>>,
    /// Number of lines in the indexed document.
    pub line_count: usize,
}

impl Default for TrigramIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl TrigramIndex {
    /// Create a new empty trigram index.
    pub fn new() -> Self {
        Self {
            trigrams: HashMap::new(),
            line_count: 0,
        }
    }

    /// Get the number of unique trigrams in the index.
    pub fn trigram_count(&self) -> usize {
        self.trigrams.len()
    }

    /// Get the total number of postings across all trigrams.
    pub fn posting_count(&self) -> usize {
        self.trigrams.values().map(|v| v.len()).sum()
    }

    /// Get the postings for a specific trigram, if it exists.
    pub fn get(&self, trigram: Trigram) -> Option<&Vec<Posting>> {
        self.trigrams.get(&trigram)
    }

    /// Insert a posting for a trigram.
    /// If a posting for the same line already exists, merge the masks.
    pub fn insert(&mut self, trigram: Trigram, posting: Posting) {
        let postings = self.trigrams.entry(trigram).or_default();

        if let Some(existing) = postings.iter_mut().find(|p| p.line_idx == posting.line_idx) {
            existing.loc_mask = existing.loc_mask.union(posting.loc_mask);
            existing.next_mask.merge(posting.next_mask);
        } else {
            postings.push(posting);
        }
    }

    /// Clear all entries from the index.
    pub fn clear(&mut self) {
        self.trigrams.clear();
        self.line_count = 0;
    }

    /// Set the line count.
    pub fn set_line_count(&mut self, count: usize) {
        self.line_count = count;
    }

    /// Iterate over all trigrams and their postings.
    pub fn iter(&self) -> impl Iterator<Item = (&Trigram, &Vec<Posting>)> {
        self.trigrams.iter()
    }

    /// Get all trigrams sorted by their key.
    pub fn sorted_trigrams(&self) -> Vec<Trigram> {
        let mut trigrams: Vec<Trigram> = self.trigrams.keys().cloned().collect();
        trigrams.sort_unstable();
        trigrams
    }

    /// Read an index from a binary file using memory mapping.
    ///
    pub fn read_from_file(path: &std::path::Path) -> std::io::Result<(MmapIndex, IndexMeta)> {
        let file = std::fs::File::open(path)?;
        let mmap = unsafe { memmap2::Mmap::map(&file)? };
        let mmap_bytes: Vec<u8> = mmap.to_vec();
        let cursor_data: &[u8] = &mmap_bytes;
        let mut cursor: std::io::Cursor<&[u8]> = std::io::Cursor::new(cursor_data);

        // Verify magic bytes
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"LHSI" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid magic bytes: {:?}", magic),
            ));
        }

        // Read version
        let version = read_u32(&mut cursor)?;
        if version != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unsupported index version: {}", version),
            ));
        }

        cursor.seek(SeekFrom::Current(4))?;

        let line_count = read_u32(&mut cursor)? as usize;
        let trigram_count = read_u32(&mut cursor)? as usize;
        let meta_offset = read_u64(&mut cursor)?;

        // Read metadata at the end
        cursor.seek(SeekFrom::Start(meta_offset))?;
        let file_mtime = read_u64(&mut cursor)?;
        let file_size = read_u64(&mut cursor)?;
        let content_hash = read_u64(&mut cursor)?;

        let meta = IndexMeta {
            file_mtime,
            file_size,
            content_hash,
            line_count: line_count as u32,
        };

        // Create a memory-mapped index view
        let mmap_index = MmapIndex::new(mmap_bytes, line_count, trigram_count);

        Ok((mmap_index, meta))
    }

    /// Write the index to a binary file.
    ///
    /// Format:
    /// - Header (24 bytes): magic, version, flags, line_count, trigram_count, meta_offset
    /// - Lookup table (sorted by trigram): trigram_key (4 bytes) + posting_offset (8 bytes)
    /// - Postings section: for each trigram, count + postings
    /// - Metadata section at meta_offset: mtime, size, content_hash, magic_end
    pub fn write_to_file(&self, path: &std::path::Path, meta: &IndexMeta) -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);

        // Collect sorted trigrams and compute offsets
        let mut trigrams: Vec<(Trigram, &Vec<Posting>)> =
            self.trigrams.iter().map(|(k, v)| (*k, v)).collect();
        trigrams.sort_by_key(|(k, _)| *k);
        let trigram_count = trigrams.len() as u32;

        // Calculate offsets
        let header_size: u64 = 24;
        let lookup_size: u64 = (trigram_count as u64) * 12; // 4 + 8 bytes per entry
        let lookup_offset = header_size;
        let postings_offset = lookup_offset + lookup_size;
        let meta_offset = postings_offset + self.compute_postings_size(&trigrams);

        writer.write_all(b"LHSI")?;
        write_u32(&mut writer, 1)?;
        write_u32(&mut writer, 0)?;
        write_u32(&mut writer, self.line_count as u32)?;
        write_u32(&mut writer, trigram_count)?;
        write_u64(&mut writer, meta_offset)?;

        let lookup_start = writer.stream_position()?;
        let mut lookup_entries: Vec<(Trigram, u64)> = Vec::with_capacity(trigrams.len());

        for &(trigram, _) in &trigrams {
            write_u32(&mut writer, trigram)?;
            write_u64(&mut writer, 0)?;
            lookup_entries.push((trigram, 0));
        }

        let mut posting_offsets: Vec<u64> = Vec::with_capacity(trigrams.len());
        for (_, postings) in &trigrams {
            let offset = writer.stream_position()?;
            posting_offsets.push(offset);

            write_u32(&mut writer, postings.len() as u32)?;
            for posting in postings.iter() {
                write_u32(&mut writer, posting.line_idx)?;
                writer.write_all(&[posting.loc_mask.get()])?;
                writer.write_all(&[posting.next_mask.get()])?;
                writer.write_all(&[0, 0])?;
            }
        }

        // Write metadata section
        writer.seek(SeekFrom::Start(meta_offset))?;
        write_u64(&mut writer, meta.file_mtime)?;
        write_u64(&mut writer, meta.file_size)?;
        write_u64(&mut writer, meta.content_hash)?;

        let mut pos = lookup_start;
        for (i, (_, _)) in trigrams.iter().enumerate() {
            // Update the offset at the correct position
            writer.seek(SeekFrom::Start(pos + 4))?;
            write_u64(&mut writer, posting_offsets[i])?;
            pos += 12;
        }

        writer.flush()?;
        Ok(())
    }

    /// Compute the total size of the postings section.
    fn compute_postings_size(&self, trigrams: &[(Trigram, &Vec<Posting>)]) -> u64 {
        let mut size: u64 = 0;
        for (_, postings) in trigrams {
            size += 4;
            size += (postings.len() as u64) * 6;
        }
        size
    }
}

/// Statistics and diagnostics for a TrigramIndex.
///
/// Provides a comprehensive view of index health, memory usage,
/// and search performance characteristics.
#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    /// Number of lines indexed.
    pub line_count: usize,
    /// Number of unique trigrams.
    pub unique_trigrams: usize,
    /// Total number of postings (trigram occurrences across all lines).
    pub total_postings: usize,
    /// Average postings per trigram.
    pub avg_postings_per_trigram: f64,
    /// Average postings per line.
    pub avg_postings_per_line: f64,
    /// Most frequent trigrams with their counts.
    pub top_trigrams: Vec<(Trigram, usize)>,
    /// Lines with the most trigrams (potential complexity hotspots).
    pub top_lines: Vec<(u32, usize)>,
    /// Memory usage estimate in bytes.
    pub estimated_memory_bytes: usize,
    /// Selectivity score: higher means more selective (fewer candidates).
    /// Range: 0.0 to 1.0, where 1.0 means highly selective.
    pub selectivity: f64,
}

impl TrigramIndex {
    /// Compute detailed statistics about this index.
    pub fn stats(&self) -> IndexStats {
        let line_count = self.line_count;
        let unique_trigrams = self.trigrams.len();
        let total_postings: usize = self.trigrams.values().map(|v| v.len()).sum();

        let avg_postings_per_trigram = if unique_trigrams > 0 {
            total_postings as f64 / unique_trigrams as f64
        } else {
            0.0
        };

        let avg_postings_per_line = if line_count > 0 {
            total_postings as f64 / line_count as f64
        } else {
            0.0
        };

        let mut top_trigrams: Vec<_> = self.trigrams.iter().map(|(&t, p)| (t, p.len())).collect();
        top_trigrams.sort_by(|a, b| b.1.cmp(&a.1));
        top_trigrams.truncate(10);

        let mut line_posting_counts: HashMap<u32, usize> = HashMap::new();
        for postings in self.trigrams.values() {
            for posting in postings {
                *line_posting_counts.entry(posting.line_idx).or_insert(0) += 1;
            }
        }
        let mut top_lines: Vec<_> = line_posting_counts.into_iter().collect();
        top_lines.sort_by(|a, b| b.1.cmp(&a.1));
        top_lines.truncate(10);

        let estimated_memory_bytes = std::mem::size_of::<TrigramIndex>()
            + (unique_trigrams * std::mem::size_of::<(Trigram, Vec<Posting>)>())
            + (total_postings * std::mem::size_of::<Posting>());

        let selectivity = if line_count > 0 && total_postings > 0 {
            let avg_candidates = total_postings as f64 / unique_trigrams as f64;
            let expected_selectivity = 1.0 - (avg_candidates / line_count as f64);
            expected_selectivity.clamp(0.0, 1.0)
        } else {
            0.0
        };

        IndexStats {
            line_count,
            unique_trigrams,
            total_postings,
            avg_postings_per_trigram,
            avg_postings_per_line,
            top_trigrams,
            top_lines,
            estimated_memory_bytes,
            selectivity,
        }
    }
}

fn write_u32<W: std::io::Write>(writer: &mut W, val: u32) -> std::io::Result<()> {
    writer.write_all(&val.to_le_bytes())
}

fn write_u64<W: std::io::Write>(writer: &mut W, val: u64) -> std::io::Result<()> {
    writer.write_all(&val.to_le_bytes())
}

fn read_u32(cursor: &mut std::io::Cursor<&[u8]>) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(cursor: &mut std::io::Cursor<&[u8]>) -> std::io::Result<u64> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

pub struct MmapIndex {
    data: Vec<u8>,
    line_count: usize,
    trigram_count: usize,
}

impl MmapIndex {
    pub fn new(data: Vec<u8>, line_count: usize, trigram_count: usize) -> Self {
        Self {
            data,
            line_count,
            trigram_count,
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_count
    }

    pub fn trigram_count(&self) -> usize {
        self.trigram_count
    }

    pub fn is_empty(&self) -> bool {
        self.trigram_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loc_mask() {
        let m = LocMask::new(0);
        assert!(m.contains(0));
        assert!(m.contains(8));
        assert!(!m.contains(1));

        let m2 = LocMask::new(1);
        assert!(m2.contains(1));
        assert!(m2.contains(9));

        // Intersection
        assert!(m.intersects(m));
        assert!(!m.intersects(m2));
    }

    #[test]
    fn test_next_mask() {
        let m1 = NextMask::new(b'a');
        let m2 = NextMask::new(b'i');

        assert!(m1.could_match(m2));

        let m3 = NextMask::new(b'b');
        assert!(!m1.could_match(m3));

        let empty = NextMask::default();
        assert!(empty.could_match(m1));
        assert!(m1.could_match(empty));
    }

    #[test]
    fn test_posting() {
        let mut p = Posting::new(5, 3, Some(b'x'));
        assert_eq!(p.line_idx, 5);
        assert!(p.loc_mask.contains(3));
        assert!(p.next_mask.get() != 0);

        p.add_occurrence(10, Some(b'y'));
        assert!(p.loc_mask.contains(10));
        assert!(p.loc_mask.contains(3));
    }

    #[test]
    fn test_trigram_index() {
        let mut index = TrigramIndex::new();
        index.set_line_count(10);

        index.insert(0x68656C, Posting::new(0, 0, Some(b'l')));
        index.insert(0x68656C, Posting::new(1, 2, Some(b'o')));
        index.insert(0x6C6C6F, Posting::new(0, 2, Some(b'\n')));

        assert_eq!(index.trigram_count(), 2);

        let hel = index.get(0x68656C).unwrap();
        assert_eq!(hel.len(), 2);
    }

    #[test]
    fn test_index_meta_matches() {
        let meta = IndexMeta {
            file_mtime: 1000,
            file_size: 500,
            content_hash: 0xDEADBEEF,
            line_count: 10,
        };

        assert!(meta.matches(1000, 500, 0xDEADBEEF, 10));
        assert!(!meta.matches(1001, 500, 0xDEADBEEF, 10));
        assert!(!meta.matches(1000, 501, 0xDEADBEEF, 10));
        assert!(!meta.matches(1000, 500, 0xCAFEBABE, 10));
        assert!(!meta.matches(1000, 500, 0xDEADBEEF, 11));
    }
}
