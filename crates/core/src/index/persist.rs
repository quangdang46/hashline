//! Binary persistence for the linehash index (.lhidx format).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;

use fixedbitset::FixedBitSet;

use super::token::{LineBitSet, TokenIndex};
use super::trigram::{Trigram, TrigramIndex};

const MAGIC: [u8; 4] = *b"LHIX";
const VERSION: u32 = 1;

/// Write a token index and trigram index to a binary .lhidx file.
pub fn write_index(
    path: &std::path::Path,
    content_hash: u64,
    token_index: &TokenIndex,
    tri_index: &TrigramIndex,
) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);

    writer.write_all(&MAGIC)?;
    write_u32(&mut writer, VERSION)?;
    write_u64(&mut writer, content_hash)?;
    write_u32(&mut writer, token_index.line_count() as u32)?;

    // Token entries
    let tokens: Vec<_> = token_index.tokens.iter().collect();
    write_u32(&mut writer, tokens.len() as u32)?;

    for (token, bitset) in &tokens {
        let token_bytes = token.as_bytes();
        let bitset_vec = bitset.as_slice().to_vec();
        write_u16(&mut writer, token_bytes.len() as u16)?;
        writer.write_all(token_bytes)?;
        write_u32(&mut writer, bitset_vec.len() as u32)?;
        for &word in &bitset_vec {
            write_u32(&mut writer, word)?;
        }
    }

    // Trigram entries
    let trigs: Vec<_> = tri_index.trigrams.iter().collect();
    write_u32(&mut writer, trigs.len() as u32)?;

    for (tri, bitset) in trigs {
        let bitset_vec = bitset.as_slice().to_vec();
        write_u32(&mut writer, bitset_vec.len() as u32)?;
        writer.write_all(&tri.0)?;
        for &word in &bitset_vec {
            write_u32(&mut writer, word)?;
        }
    }

    writer.flush()?;
    Ok(())
}

/// Read a token and trigram index from a binary .lhidx file.
pub fn read_index(path: &std::path::Path) -> std::io::Result<(TokenIndex, TrigramIndex, u64)> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);

    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Invalid magic bytes: {:?}", magic),
        ));
    }

    let version = read_u32(&mut reader)?;
    if version != VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unsupported index version: {}", version),
        ));
    }

    let content_hash = read_u64(&mut reader)?;
    let line_count = read_u32(&mut reader)? as usize;

    // Read token entries
    let token_count = read_u32(&mut reader)? as usize;
    let mut tokens: HashMap<Box<str>, Arc<LineBitSet>> = HashMap::with_capacity(token_count);

    for _ in 0..token_count {
        let token_len = read_u16(&mut reader)? as usize;
        let mut token_buf = vec![0u8; token_len];
        reader.read_exact(&mut token_buf)?;
        let token_str = String::from_utf8(token_buf)
            .unwrap_or_default()
            .into_boxed_str();

        let word_count = read_u32(&mut reader)? as usize;
        let mut words = vec![0u32; word_count];
        for slot in words.iter_mut() {
            *slot = read_u32(&mut reader)?;
        }
        let bitset = FixedBitSet::with_capacity_and_blocks(line_count, words.clone());
        tokens.insert(token_str, Arc::new(bitset));
    }

    // Read trigram entries
    let tri_count = read_u32(&mut reader)? as usize;
    let mut trigrams: HashMap<Trigram, Arc<LineBitSet>> = HashMap::with_capacity(tri_count);

    for _ in 0..tri_count {
        let word_count = read_u32(&mut reader)? as usize;
        let mut tri_bytes = [0u8; 3];
        reader.read_exact(&mut tri_bytes)?;

        let mut words = vec![0u32; word_count];
        for slot in words.iter_mut() {
            *slot = read_u32(&mut reader)?;
        }
        let bitset = FixedBitSet::with_capacity_and_blocks(line_count, words.clone());
        trigrams.insert(Trigram(tri_bytes), Arc::new(bitset));
    }

    let token_index = TokenIndex::from_map(tokens, line_count);
    let tri_index = TrigramIndex::from_map(trigrams, line_count);

    Ok((token_index, tri_index, content_hash))
}

// --- Helper functions ---

fn write_u32<W: Write>(w: &mut W, val: u32) -> std::io::Result<()> {
    w.write_all(&val.to_le_bytes())
}

fn write_u64<W: Write>(w: &mut W, val: u64) -> std::io::Result<()> {
    w.write_all(&val.to_le_bytes())
}

fn write_u16<W: Write>(w: &mut W, val: u16) -> std::io::Result<()> {
    w.write_all(&val.to_le_bytes())
}

fn read_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64<R: Read>(r: &mut R) -> std::io::Result<u64> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_u16<R: Read>(r: &mut R) -> std::io::Result<u16> {
    let mut buf = [0u8; 2];
    r.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn test_write_and_read_round_trip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.lhidx");

        let mut tokens: HashMap<Box<str>, Arc<LineBitSet>> = HashMap::new();
        let mut bs = FixedBitSet::with_capacity(10);
        bs.set(0, true);
        bs.set(2, true);
        tokens.insert("hello".into(), Arc::new(bs));

        let token_index = TokenIndex::from_map(tokens, 10);
        let tri_index = TrigramIndex::from_map(HashMap::new(), 10);

        write_index(&path, 0xCAFEBABE, &token_index, &tri_index).unwrap();

        let (tok_read, _tri_read, hash) = read_index(&path).unwrap();
        assert_eq!(hash, 0xCAFEBABE);
        assert_eq!(tok_read.token_count(), 1);
        assert!(tok_read.lookup("hello").is_some());
    }
}
