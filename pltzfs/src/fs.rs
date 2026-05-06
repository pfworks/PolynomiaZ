/// pltzfs: a compressed filesystem using PLTZ multi-platter storage.
///
/// Operations: create, read, write, mkdir, readdir, stat, unlink.

use crate::inode::Inode;
use crate::dir::DirEntry;
use crate::storage::*;
use std::collections::HashMap;

const ROOT_INO: u64 = 1;

pub struct PltzFs {
    storage: Storage,
    inodes: HashMap<u64, Inode>,
    dirs: HashMap<u64, Vec<DirEntry>>,  // ino -> entries
    file_data: HashMap<u64, Vec<u8>>,   // ino -> content
    next_ino: u64,
}

impl PltzFs {
    /// Create a new empty filesystem.
    pub fn new(total_blocks: u64) -> Self {
        let storage = Storage::new(total_blocks);
        let mut inodes = HashMap::new();
        let mut dirs = HashMap::new();

        // Create root directory
        let root = Inode::new_dir(ROOT_INO, 0, 0);
        inodes.insert(ROOT_INO, root);
        dirs.insert(ROOT_INO, vec![
            DirEntry { ino: ROOT_INO, name: ".".into() },
            DirEntry { ino: ROOT_INO, name: "..".into() },
        ]);

        PltzFs {
            storage,
            inodes,
            dirs,
            file_data: HashMap::new(),
            next_ino: 2,
        }
    }

    /// Create a file in the given directory.
    pub fn create_file(&mut self, parent_ino: u64, name: &str, data: &[u8]) -> Result<u64, String> {
        if !self.inodes.get(&parent_ino).map_or(false, |i| i.is_dir()) {
            return Err("Parent is not a directory".into());
        }

        let ino = self.next_ino;
        self.next_ino += 1;

        let mut inode = Inode::new_file(ino, 0, 0);
        inode.size = data.len() as u64;
        inode.blocks = ((data.len() + 511) / 512) as u64;

        // Allocate blocks
        for _ in 0..inode.blocks {
            let block = self.storage.alloc_block().ok_or("No space")?;
            inode.block_addrs.push(block);
        }

        self.inodes.insert(ino, inode);
        self.file_data.insert(ino, data.to_vec());
        self.dirs.get_mut(&parent_ino).unwrap().push(DirEntry { ino, name: name.into() });

        Ok(ino)
    }

    /// Create a subdirectory.
    pub fn mkdir(&mut self, parent_ino: u64, name: &str) -> Result<u64, String> {
        if !self.inodes.get(&parent_ino).map_or(false, |i| i.is_dir()) {
            return Err("Parent is not a directory".into());
        }

        let ino = self.next_ino;
        self.next_ino += 1;

        let inode = Inode::new_dir(ino, 0, 0);
        self.inodes.insert(ino, inode);
        self.dirs.insert(ino, vec![
            DirEntry { ino, name: ".".into() },
            DirEntry { ino: parent_ino, name: "..".into() },
        ]);
        self.dirs.get_mut(&parent_ino).unwrap().push(DirEntry { ino, name: name.into() });

        Ok(ino)
    }

    /// Read file contents.
    pub fn read_file(&self, ino: u64) -> Result<&[u8], String> {
        self.file_data.get(&ino).map(|v| v.as_slice()).ok_or("File not found".into())
    }

    /// List directory entries.
    pub fn readdir(&self, ino: u64) -> Result<&[DirEntry], String> {
        self.dirs.get(&ino).map(|v| v.as_slice()).ok_or("Not a directory".into())
    }

    /// Get inode metadata.
    pub fn stat(&self, ino: u64) -> Option<&Inode> {
        self.inodes.get(&ino)
    }

    /// Look up a name in a directory, return its inode number.
    pub fn lookup(&self, parent_ino: u64, name: &str) -> Option<u64> {
        self.dirs.get(&parent_ino)?
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.ino)
    }

    /// Flush all data to platters and return compressed size.
    pub fn sync(&mut self) -> usize {
        // Serialize inodes to platter 0
        let mut inode_data = Vec::new();
        let mut inos: Vec<u64> = self.inodes.keys().copied().collect();
        inos.sort();
        for &ino in &inos {
            inode_data.extend_from_slice(&self.inodes[&ino].to_bytes());
        }
        self.storage.write_platter(PLATTER_INODES, inode_data);

        // Serialize directories to platter 1
        let mut dir_data = Vec::new();
        for &ino in &inos {
            if let Some(entries) = self.dirs.get(&ino) {
                for entry in entries {
                    dir_data.extend_from_slice(&entry.to_bytes());
                }
            }
        }
        self.storage.write_platter(PLATTER_DIRS, dir_data);

        // Serialize file data to platter 2/3
        let mut structured = Vec::new();
        let mut raw = Vec::new();
        for &ino in &inos {
            if let Some(data) = self.file_data.get(&ino) {
                // Simple heuristic: if PLTZ compresses it well, it's structured
                let compressed = pltz::codec::encode(data, pltz::codec::RawCompressor::None);
                if compressed.len() < data.len() * 3 / 4 {
                    structured.extend_from_slice(data);
                } else {
                    raw.extend_from_slice(data);
                }
            }
        }
        self.storage.write_platter(PLATTER_DATA_STRUCTURED, structured);
        self.storage.write_platter(PLATTER_DATA_RAW, raw);

        // Return total compressed size
        self.storage.serialize().len()
    }

    /// Report per-platter compression stats.
    pub fn stats(&self) -> Vec<(&str, usize, usize)> {
        let names = ["inodes", "dirs", "data_structured", "data_raw", "bitmap"];
        self.storage.platter_stats().into_iter().enumerate()
            .map(|(i, (raw, comp))| (names[i], raw, comp))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut fs = PltzFs::new(1024);

        // Create files
        let f1 = fs.create_file(ROOT_INO, "ramp.bin", &(0..=255u8).collect::<Vec<_>>()).unwrap();
        let f2 = fs.create_file(ROOT_INO, "zeros.bin", &[0u8; 1024]).unwrap();
        let f3 = fs.create_file(ROOT_INO, "hello.txt", b"Hello, pltzfs!").unwrap();

        // Read back
        assert_eq!(fs.read_file(f1).unwrap(), &(0..=255u8).collect::<Vec<_>>());
        assert_eq!(fs.read_file(f2).unwrap(), &[0u8; 1024]);
        assert_eq!(fs.read_file(f3).unwrap(), b"Hello, pltzfs!");

        // Directory listing
        let entries = fs.readdir(ROOT_INO).unwrap();
        assert_eq!(entries.len(), 5); // . + .. + 3 files

        // Lookup
        assert_eq!(fs.lookup(ROOT_INO, "ramp.bin"), Some(f1));
        assert_eq!(fs.lookup(ROOT_INO, "nonexistent"), None);

        // Mkdir
        let dir_ino = fs.mkdir(ROOT_INO, "subdir").unwrap();
        let f4 = fs.create_file(dir_ino, "nested.bin", &[0xAB; 512]).unwrap();
        assert_eq!(fs.read_file(f4).unwrap(), &[0xAB; 512]);

        // Sync and check compression
        let total_size = fs.sync();
        let raw_size: usize = 256 + 1024 + 14 + 512; // sum of file data
        println!("Raw data: {}B, Compressed FS image: {}B", raw_size, total_size);
        assert!(total_size < raw_size, "FS should compress: {} >= {}", total_size, raw_size);
    }

    #[test]
    fn test_platter_stats() {
        let mut fs = PltzFs::new(4096);

        // Create structured data (should compress well on platter 2)
        for i in 0..100 {
            let data: Vec<u8> = (0..=255u8).cycle().skip(i).take(256).collect();
            fs.create_file(ROOT_INO, &format!("file_{:03}.bin", i), &data).unwrap();
        }

        fs.sync();
        let stats = fs.stats();
        println!("Platter stats:");
        for (name, raw, comp) in &stats {
            let ratio = if *raw > 0 { *comp as f64 / *raw as f64 } else { 0.0 };
            println!("  {:<20} {:>8}B raw → {:>8}B compressed ({:.3})", name, raw, comp, ratio);
        }
    }
}
