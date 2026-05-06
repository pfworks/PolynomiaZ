# pltzfs — PLTZ Compressed Filesystem

A compressed filesystem prototype that stores metadata and data on separate PLTZ-compressed platters.

## Concept

pltzfs uses the disk platter model literally: different types of data live on different platters, each independently compressed with PLTZ pattern detection.

| Platter | Contents | Why it compresses well |
|---------|----------|----------------------|
| 0 | Inode table | Block addresses are LINEAR_WIDE, permissions are CONST |
| 1 | Directory entries | Fixed-size records with incrementing inodes |
| 2 | Structured file data | Files with detected patterns (ramps, constants, etc.) |
| 3 | Raw file data | Incompressible files (deflate fallback) |
| 4 | Free space bitmap | Mostly zeros or ones (CONST) |

## Usage (Library API)

```rust
use pltzfs::fs::PltzFs;

// Create a filesystem with 4096 blocks
let mut fs = PltzFs::new(4096);

// Create files
let ino = fs.create_file(1, "sensor_data.bin", &data)?;

// Read files
let contents = fs.read_file(ino)?;

// Create directories
let dir_ino = fs.mkdir(1, "logs")?;
fs.create_file(dir_ino, "log001.bin", &log_data)?;

// List directory
for entry in fs.readdir(dir_ino)? {
    println!("{} -> inode {}", entry.name, entry.ino);
}

// Sync to compressed image (serializes all platters)
let image_size = fs.sync();

// Get per-platter compression stats
for (name, raw, compressed) in fs.stats() {
    println!("{}: {}B -> {}B", name, raw, compressed);
}

// Serialize to bytes (for writing to disk/flash)
let blob = fs.storage.serialize();

// Deserialize from bytes
let storage = Storage::deserialize(&blob)?;
```

## Building & Testing

```bash
cd pltzfs
cargo test
```

## Results

With 100 files of structured data (linear ramps):
- Inodes: 12.9KB → 6.2KB (2:1)
- Directories: 26KB → 3.2KB (8:1)
- Structured data: 25.6KB → 8.3KB (3:1)
- Bitmap: 512B → 27B (19:1)

## Status

This is a prototype demonstrating the concept. Not yet suitable for production use:
- In-memory only (no disk persistence layer)
- No file deletion/truncation
- No permissions enforcement
- No journaling or crash recovery
- Single-threaded

## Best Fit

- Firmware storage (read-heavy, structured data)
- Satellite onboard storage (telemetry accumulates linearly)
- IoT edge storage (sensor data with known structure)
- Archival/cold storage (write once, read many)
