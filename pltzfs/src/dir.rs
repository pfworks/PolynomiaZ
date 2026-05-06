/// Directory entries for pltzfs.
/// Stored on Platter 1 (directory platter) — fixed-size records with
/// incrementing inode numbers (LINEAR_WIDE friendly).

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
}

impl DirEntry {
    pub const MAX_NAME: usize = 248;
    pub const SIZE: usize = 256; // 8 (ino) + 248 (name, null-padded)

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE);
        buf.extend_from_slice(&self.ino.to_le_bytes());
        let name_bytes = self.name.as_bytes();
        let len = std::cmp::min(name_bytes.len(), Self::MAX_NAME);
        buf.extend_from_slice(&name_bytes[..len]);
        buf.resize(Self::SIZE, 0);
        buf
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE { return None; }
        let ino = u64::from_le_bytes(data[0..8].try_into().ok()?);
        if ino == 0 { return None; } // empty slot
        let name_end = data[8..].iter().position(|&b| b == 0).unwrap_or(Self::MAX_NAME);
        let name = String::from_utf8_lossy(&data[8..8 + name_end]).to_string();
        Some(DirEntry { ino, name })
    }
}
