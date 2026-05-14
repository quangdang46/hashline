use std::cmp::Ordering;

pub const ZONE_SIZE_BYTES: usize = 8 * 1024;

/// A zone is a fixed-size byte segment of the file used for selective loading
/// and parallel processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub id: u32,
    pub byte_start: u64,
    pub byte_end: u64,
    pub line_start: usize,
    pub line_end: usize,
    pub content_hash: u32,
}

/// A map of zones covering the file bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneMap {
    zones: Vec<Zone>,
}

impl ZoneMap {
    /// Build a zone map from raw bytes and a sorted list of line byte offsets.
    pub fn from_bytes(bytes: &[u8], newline_offsets: &[usize]) -> Self {
        if bytes.is_empty() {
            return Self { zones: Vec::new() };
        }

        let total_bytes = bytes.len() as u64;
        let line_count = newline_offsets.len().saturating_sub(1);
        let zone_count = (total_bytes as usize).div_ceil(ZONE_SIZE_BYTES).max(1);

        let mut zones = Vec::with_capacity(zone_count);

        for zone_id in 0..zone_count {
            let byte_start_u64 = (zone_id as u64) * (ZONE_SIZE_BYTES as u64);
            let byte_end_u64 = ((zone_id as u64 + 1) * (ZONE_SIZE_BYTES as u64)).min(total_bytes);

            let line_start = newline_offsets
                .iter()
                .position(|&off| off >= byte_start_u64 as usize)
                .unwrap_or(line_count);

            let line_end = newline_offsets
                .iter()
                .position(|&off| off > byte_end_u64 as usize)
                .unwrap_or(line_count)
                .max(line_start + 1);

            let content_hash = xxhash_rust::xxh32::xxh32(
                &bytes[byte_start_u64 as usize..byte_end_u64 as usize],
                0,
            );

            zones.push(Zone {
                id: zone_id as u32,
                byte_start: byte_start_u64,
                byte_end: byte_end_u64,
                line_start,
                line_end,
                content_hash,
            });
        }

        Self { zones }
    }

    /// Returns the zone index for a given byte offset using binary search.
    pub fn zone_for_byte_offset(&self, offset: u64) -> usize {
        match self.zones.binary_search_by(|z| {
            if offset < z.byte_start {
                Ordering::Greater
            } else if offset >= z.byte_end {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        }) {
            Ok(idx) | Err(idx) => idx.min(self.zones.len().saturating_sub(1)),
        }
    }

    /// Returns the zone index that contains the given line number.
    pub fn zone_for_line(&self, line: usize) -> usize {
        match self.zones.binary_search_by(|z| {
            if line < z.line_start {
                Ordering::Greater
            } else if line >= z.line_end {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        }) {
            Ok(idx) | Err(idx) => idx.min(self.zones.len().saturating_sub(1)),
        }
    }

    pub fn len(&self) -> usize {
        self.zones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    pub fn as_slice(&self) -> &[Zone] {
        &self.zones
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zone_map_empty() {
        let map = ZoneMap::from_bytes(b"", &[]);
        assert!(map.is_empty());
    }

    #[test]
    fn test_zone_map_single_zone() {
        let content = b"hello world\n";
        let offsets = vec![0, 12];
        let map = ZoneMap::from_bytes(content, &offsets);
        assert_eq!(map.len(), 1);
        assert_eq!(map.zone_for_byte_offset(0), 0);
        assert_eq!(map.zone_for_line(0), 0);
    }

    #[test]
    fn test_zone_for_line_oob() {
        let content = b"line1\nline2\n";
        let offsets = vec![0, 6, 12];
        let map = ZoneMap::from_bytes(content, &offsets);
        assert_eq!(map.zone_for_line(999), 0);
    }
}
