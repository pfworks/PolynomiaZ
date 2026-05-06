/// Inode structure for pltzfs.
/// Stored on Platter 0 (metadata platter) — highly structured,
/// compresses well with LINEAR_WIDE (block addresses) and CONST (permissions).

use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Inode {
    pub ino: u64,
    pub mode: u16,       // file type + permissions
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,     // number of 512-byte blocks
    pub atime: u64,      // seconds since epoch
    pub mtime: u64,
    pub ctime: u64,
    pub nlink: u16,
    pub block_addrs: Vec<u64>,  // direct block addresses (LINEAR_WIDE friendly)
}

impl Inode {
    pub const SIZE: usize = 128; // fixed serialized size

    pub fn new_file(ino: u64, uid: u32, gid: u32) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Inode {
            ino,
            mode: 0o100644, // regular file
            uid,
            gid,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            nlink: 1,
            block_addrs: Vec::new(),
        }
    }

    pub fn new_dir(ino: u64, uid: u32, gid: u32) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Inode {
            ino,
            mode: 0o040755, // directory
            uid,
            gid,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            nlink: 2,
            block_addrs: Vec::new(),
        }
    }

    pub fn is_dir(&self) -> bool {
        (self.mode & 0o170000) == 0o040000
    }

    /// Serialize to fixed-size bytes (for platter storage).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE);
        buf.extend_from_slice(&self.ino.to_le_bytes());
        buf.extend_from_slice(&self.mode.to_le_bytes());
        buf.extend_from_slice(&self.uid.to_le_bytes());
        buf.extend_from_slice(&self.gid.to_le_bytes());
        buf.extend_from_slice(&self.size.to_le_bytes());
        buf.extend_from_slice(&self.blocks.to_le_bytes());
        buf.extend_from_slice(&self.atime.to_le_bytes());
        buf.extend_from_slice(&self.mtime.to_le_bytes());
        buf.extend_from_slice(&self.ctime.to_le_bytes());
        buf.extend_from_slice(&self.nlink.to_le_bytes());
        // Store up to 6 direct block addresses
        let addr_count = std::cmp::min(self.block_addrs.len(), 6);
        buf.extend_from_slice(&(addr_count as u16).to_le_bytes());
        for i in 0..6 {
            let addr = self.block_addrs.get(i).copied().unwrap_or(0);
            buf.extend_from_slice(&addr.to_le_bytes());
        }
        buf.resize(Self::SIZE, 0);
        buf
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE { return None; }
        let ino = u64::from_le_bytes(data[0..8].try_into().ok()?);
        let mode = u16::from_le_bytes(data[8..10].try_into().ok()?);
        let uid = u32::from_le_bytes(data[10..14].try_into().ok()?);
        let gid = u32::from_le_bytes(data[14..18].try_into().ok()?);
        let size = u64::from_le_bytes(data[18..26].try_into().ok()?);
        let blocks = u64::from_le_bytes(data[26..34].try_into().ok()?);
        let atime = u64::from_le_bytes(data[34..42].try_into().ok()?);
        let mtime = u64::from_le_bytes(data[42..50].try_into().ok()?);
        let ctime = u64::from_le_bytes(data[50..58].try_into().ok()?);
        let nlink = u16::from_le_bytes(data[58..60].try_into().ok()?);
        let addr_count = u16::from_le_bytes(data[60..62].try_into().ok()?) as usize;
        let mut block_addrs = Vec::with_capacity(addr_count);
        for i in 0..addr_count {
            let offset = 62 + i * 8;
            let addr = u64::from_le_bytes(data[offset..offset + 8].try_into().ok()?);
            block_addrs.push(addr);
        }
        Some(Inode { ino, mode, uid, gid, size, blocks, atime, mtime, ctime, nlink, block_addrs })
    }
}
