/// Multi-platter storage backend for pltzfs.
/// Each platter is independently PLTZ-compressed.
///
/// Platter layout:
///   0 — Inode table (metadata)
///   1 — Directory entries
///   2 — File data (structured, pattern-detected)
///   3 — File data (raw/incompressible)
///   4 — Free space bitmap

use pltz::codec::{encode, decode, RawCompressor};

const BLOCK_SIZE: usize = 512;
const NUM_PLATTERS: usize = 5;

pub const PLATTER_INODES: usize = 0;
pub const PLATTER_DIRS: usize = 1;
pub const PLATTER_DATA_STRUCTURED: usize = 2;
pub const PLATTER_DATA_RAW: usize = 3;
pub const PLATTER_BITMAP: usize = 4;

/// In-memory representation of the multi-platter storage.
#[derive(Debug)]
pub struct Storage {
    platters: [Vec<u8>; NUM_PLATTERS],
    total_blocks: u64,
}

impl Storage {
    pub fn new(total_blocks: u64) -> Self {
        let bitmap_size = ((total_blocks + 7) / 8) as usize;
        let mut platters = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        platters[PLATTER_BITMAP] = vec![0u8; bitmap_size]; // all free
        Storage { platters, total_blocks }
    }

    /// Read raw bytes from a platter.
    pub fn read_platter(&self, platter: usize) -> &[u8] {
        &self.platters[platter]
    }

    /// Write raw bytes to a platter (replaces entire content).
    pub fn write_platter(&mut self, platter: usize, data: Vec<u8>) {
        self.platters[platter] = data;
    }

    /// Append data to a platter, return the offset where it was written.
    pub fn append_platter(&mut self, platter: usize, data: &[u8]) -> u64 {
        let offset = self.platters[platter].len() as u64;
        self.platters[platter].extend_from_slice(data);
        offset
    }

    /// Allocate a block, returns block number or None if full.
    pub fn alloc_block(&mut self) -> Option<u64> {
        let bitmap = &mut self.platters[PLATTER_BITMAP];
        for (byte_idx, byte) in bitmap.iter_mut().enumerate() {
            if *byte != 0xFF {
                for bit in 0..8 {
                    if *byte & (1 << bit) == 0 {
                        *byte |= 1 << bit;
                        let block_num = (byte_idx * 8 + bit) as u64;
                        if block_num < self.total_blocks {
                            return Some(block_num);
                        }
                    }
                }
            }
        }
        None
    }

    /// Free a block.
    pub fn free_block(&mut self, block: u64) {
        let byte_idx = (block / 8) as usize;
        let bit = (block % 8) as u8;
        if byte_idx < self.platters[PLATTER_BITMAP].len() {
            self.platters[PLATTER_BITMAP][byte_idx] &= !(1 << bit);
        }
    }

    /// Serialize all platters to a single compressed blob.
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"PZFS");
        buf.extend_from_slice(&self.total_blocks.to_le_bytes());
        buf.extend_from_slice(&(NUM_PLATTERS as u16).to_le_bytes());

        for platter in &self.platters {
            let compressed = encode(platter, RawCompressor::Zlib);
            buf.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            buf.extend_from_slice(&compressed);
        }
        buf
    }

    /// Deserialize from compressed blob.
    pub fn deserialize(data: &[u8]) -> Result<Self, String> {
        if data.len() < 14 || &data[0..4] != b"PZFS" {
            return Err("Bad pltzfs magic".into());
        }
        let total_blocks = u64::from_le_bytes(data[4..12].try_into().unwrap());
        let num_platters = u16::from_le_bytes(data[12..14].try_into().unwrap()) as usize;
        if num_platters != NUM_PLATTERS {
            return Err(format!("Expected {} platters, got {}", NUM_PLATTERS, num_platters));
        }

        let mut platters = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut offset = 14;
        for i in 0..NUM_PLATTERS {
            let comp_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            platters[i] = decode(&data[offset..offset + comp_len])?;
            offset += comp_len;
        }

        Ok(Storage { platters, total_blocks })
    }

    /// Report compressed size of each platter.
    pub fn platter_stats(&self) -> Vec<(usize, usize)> {
        self.platters.iter().map(|p| {
            let compressed = encode(p, RawCompressor::Zlib);
            (p.len(), compressed.len())
        }).collect()
    }
}
