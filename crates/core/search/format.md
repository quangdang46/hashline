# Binary Index Format Specification

## Overview

The trigram index is persisted to a binary file with a specific on-disk format optimized for memory-mapped access and efficient lookup.

## File Layout

```
┌─────────────────────────────────────┐
│           HEADER (24 bytes)          │
├─────────────────────────────────────┤
│        LOOKUP TABLE (variable)       │
│   [trigram_key:u32, offset:u64] × N │
├─────────────────────────────────────┤
│      POSTINGS SECTION (variable)      │
│   [count:u32, postings × count]     │
├─────────────────────────────────────┤
│      METADATA SECTION (32 bytes)     │
└─────────────────────────────────────┘
```

## Header (24 bytes)

| Offset | Size | Field         | Description                    |
|--------|------|---------------|--------------------------------|
| 0      | 4    | magic         | `LHSI` (LineHash Search Index) |
| 4      | 4    | version       | Format version (1)             |
| 8      | 4    | flags         | Reserved for future use         |
| 12     | 4    | line_count    | Number of lines indexed        |
| 16     | 4    | trigram_count | Number of unique trigrams      |
| 20     | 8    | meta_offset   | Offset to metadata section     |

## Lookup Table (12 bytes per entry)

The lookup table contains one entry per unique trigram, sorted by trigram key for binary search.

| Offset | Size | Field   | Description                        |
|--------|------|---------|------------------------------------|
| 0      | 4    | key     | The trigram value (u32)            |
| 4      | 8    | offset  | Offset to this trigram's postings  |

Entries are sorted by `key` in ascending order to enable binary search O(log N).

## Postings Section (6 bytes per posting)

For each trigram in the lookup table:

| Offset | Size | Field      | Description                              |
|--------|------|------------|------------------------------------------|
| 0      | 4    | line_idx  | 0-based line index (u32)                 |
| 4      | 1    | loc_mask  | Position mask (which positions)          |
| 5      | 1    | next_mask | Bloom filter of following characters     |

Padding: 2 bytes after each posting (reserved, set to 0).

Format per trigram:
```
[count: u32]                 ; number of postings for this trigram
[posting: 6 bytes] × count   ; the actual postings
```

## Metadata Section (32 bytes)

Located at `meta_offset` from beginning of file:

| Offset | Size | Field         | Description                       |
|--------|------|---------------|-----------------------------------|
| 0      | 8    | file_mtime    | File modification time (u64)      |
| 8      | 8    | file_size     | File size in bytes (u64)          |
| 16     | 8    | content_hash  | xxHash64 of full content (u64)    |
| 24     | 4    | line_count    | Number of lines (u32)             |
| 28     | 4    | magic_end    | End magic `0xDEADBEEF` (u32)      |

## Magic Values

- Header magic: `LHSI` (0x4C485349)
- Metadata end magic: `0xDEADBEEF` (0xDEADBEEFCAFEBABE as u64 in earlier format)

## Version History

### Version 1 (current)
- Initial format
- Fixed 24-byte header
- Variable-length lookup table and postings
- 32-byte metadata footer

## Access Patterns

### Memory-Mapped Access

The file is designed for memory-mapping:
1. Read header to get `meta_offset`
2. Memory-map the entire file
3. Binary search lookup table for trigram
4. Jump to posting offset
5. Read postings sequentially

### Random Access

For incremental index updates:
1. Read metadata to validate file hasn't changed
2. If valid, use memory-mapped lookup
3. If stale, rebuild entire index

## Example File

Small example with 2 trigrams, 3 lines:

```
Header:
  magic:     LHSI
  version:   1
  flags:     0
  line_count: 3
  trigram_count: 2
  meta_offset: 56

Lookup Table (sorted by trigram):
  [0x68656C, 24]  -> "hel" trigram
  [0x6C6C6F, 36]  -> "llo" trigram

Postings:
  For 0x68656C:
    count: 2
    posting[0]: line_idx=0, loc_mask=0x01, next_mask=0x04
    posting[1]: line_idx=1, loc_mask=0x04, next_mask=0x08

  For 0x6C6C6F:
    count: 1
    posting[0]: line_idx=0, loc_mask=0x04, next_mask=0x08

Metadata:
  file_mtime:   1699999999
  file_size:    1024
  content_hash: 0xABCD1234...
  line_count:   3
  magic_end:    0xDEADBEEF
```
