# PolynomiaZ (PLTZ)

Polar coordinate compression — lossless codec that models data as mathematical functions on virtual disk platters. 5-12× better than zlib on structured numerical data.

## What It Does

PolynomiaZ maps input data onto a virtual disk platter geometry (concentric tracks divided into sectors). Each track is analyzed for mathematical patterns. Tracks that match a known pattern are stored as compact function descriptors instead of raw bytes. Groups of tracks with identical or linearly-varying parameters are further compressed via cross-track meta-descriptors.

**Core idea:** If 256 bytes form a linear sequence `[0, 1, 2, ..., 255]`, store `LINEAR(start=0, step=1)` in 4 bytes instead of 256.

### How It Works

1. **Layout** — Input bytes are arranged into tracks × sectors (configurable geometry)
2. **Analysis** — Each track is tested against 18 pattern types
3. **Cross-track** — Groups of tracks with related parameters are merged into meta-descriptors
4. **Encoding** — Matched patterns become compact function descriptors; unmatched tracks stored raw
5. **Adaptive geometry** — 7 sector sizes are tried; the best-compressing geometry wins
6. **Multi-platter** — Structured and raw tracks are separated onto different platters

### Pattern Types

| Pattern | What it detects | Storage cost |
|---------|----------------|-------------|
| CONST | All identical values | 1 byte |
| LINEAR | Arithmetic sequences (any start, any step) | 4 bytes |
| RLE | Runs of repeated values | 2 bytes/run |
| REPEAT | Repeating sub-patterns | pattern length |
| DELTA | Values with few unique deltas | 2 + (n-1) bytes |
| PERIODIC | FFT-decomposable signals | 18 bytes/component |
| POLY | Polynomial curves (degree ≤ 8) | 8 bytes/coefficient |
| LINEAR_WIDE | Linear sequence in 16/32-bit words | 19 bytes |
| LINEAR_F64 | Linear sequence in IEEE 754 doubles | 18 bytes |
| DELTA_F64 | f64 values with f32-representable deltas | 10 + 4(n-1) bytes |
| XOR_MASK | Data XORed with constant reveals simpler pattern | 2 + inner bytes |
| SPARSE | Mostly one value with few exceptions | 3 + 3×exceptions |
| PIECEWISE_LINEAR | 2-4 linear segments with different slopes | 1 + 6×segments |
| MIRROR | Second half is reverse of first half | n/2 bytes |
| INTERLEAVED_LINEAR | Two interleaved arithmetic sequences | 10 bytes |
| EXPONENTIAL | Geometric sequence a×r^i | 10 bytes |
| DELTA_RLE | Run-length encoded deltas (staircases) | 3 + 2×runs |
| RAW | No pattern (optional deflate fallback) | n bytes |

### Cross-Track Meta-Descriptors

When multiple tracks share the same pattern type with identical or linearly-varying parameters, they are encoded as a single meta-group:

- **AllSame** — N tracks with identical parameters stored once
- **ConstLinear** — N CONST tracks whose values form an arithmetic sequence
- **LinearStartsLinear** — N LINEAR tracks with same step, starts forming a sequence

---

## Compression Results

| Data | PLTZ | zlib | bz2 | lzma |
|------|------|------|-----|------|
| Linear ramp 4KB | **126B** | 315B | 657B | 324B |
| Linear f64 epochs 8KB | **350B** | 1,881B | 1,917B | 1,548B |
| Segment addresses (u32) 4KB | **182B** | 2,244B | — | 1,636B |
| Firmware (padding + headers) 4KB | **65B** | 307B | 580B | 324B |
| Constant fill 64KB | **278B** | 84B | 43B | 92B |
| Linear ramps 16KB | **153B** | 408B | — | — |
| Mixed structured + random 4KB | **2.1KB** | 2.3KB | 3.0KB | 2.3KB |
| English text 1.5KB | 1,656B | **680B** | 696B | — |
| Random data 4KB | 4,134B | **4,109B** | 4,608B | 4,156B |

---

## Installation

### Rust (primary implementation)

```bash
cd rust
cargo build --release
# Binary at rust/target/release/pltz
```

### Usage (bzip2-style)

```bash
pltz file.bin           # compress → file.bin.pltz (removes original)
pltz -d file.bin.pltz   # decompress → file.bin (removes compressed)
pltz -k file.bin        # compress, keep original
pltz -c file.bin        # compress to stdout
pltz --no-zlib file.bin # skip deflate on raw tracks (no zlib dep)
pltz -t file.bin.pltz   # test integrity
pltz -v file.bin        # verbose output
pltz -r 21 file.bin     # columnar pre-processing (stride=21 bytes)
pltz -C 64k file.bin    # chunked mode (64KB chunks, per-section geometry)
pltz -C auto file.bin   # auto-detect chunk size
pltz -b file.bin        # auto-select best compression settings
pltz --analyze file.bin # analyze data and recommend settings
cat data | pltz -c | pltz -dc   # pipe round-trip
```

### Python (benchmarking only)

```bash
pip install numpy
python3 tests/test_codec.py      # verify lossless round-trip
python3 tests/benchmark.py       # compare against zlib/bz2/lzma
python3 tests/bench_uboot.py     # U-Boot firmware simulation
python3 tests/bench_spice.py     # SPICE kernel simulation
```

---

## Real-World Use Cases

**Embedded systems / IoT firmware** — Flash images with 0xFF padding, address tables, calibration ramps, bootloader images.

**Satellite telemetry** — Housekeeping data, attitude quaternions, epoch arrays, CCSDS framing, incrementing sequence counters.

**SPICE kernels (JPL)** — Epoch directories (5.4× better than zlib), segment address tables (12.3× better), orientation polynomials.

**Sensor data** — ADC/DAC sweeps, calibration staircases, linearization LUTs, temperature compensation tables.

**Protocol framing** — Sync patterns, packet headers with incrementing sequence numbers, fixed-structure telemetry frames.

**Lookup tables** — Gamma curves, PWM tables, motor control profiles, CRC tables.

---

## Architecture

```
rust/src/
├── lib.rs           — Module declarations
├── main.rs          — CLI binary (bzip2-style interface)
├── platter.rs       — Disk geometry, data layout
├── analyzer.rs      — Pattern detection (18 types, rustfft for periodic)
├── codec.rs         — Binary encode/decode, adaptive geometry, multi-platter
├── columnar.rs      — Columnar pre-processing transform
├── cross_track.rs   — Cross-track meta-descriptor optimization
├── encrypt.rs       — Structured encryption (⚠️ demo)
└── image.rs         — Image codec (lossless + lossy, 8/16-bit)

tests/
├── test_codec.py    — Python round-trip tests
├── benchmark.py     — Comparison vs zlib/bz2/lzma
├── bench_uboot.py   — U-Boot firmware simulation
├── bench_spice.py   — SPICE kernel simulation
└── lib/             — Python reference codec
```

---

## Binary Format

```
Header (12 bytes):
  [4] Magic "PLTZ"
  [4] Original data length (uint32 LE)
  [2] Sectors per track (uint16 LE)
  [2] Number of platters (uint16 LE)

Per platter:
  [2] Number of entries (uint16 LE)
  Per entry (individual track):
    [2] Track index (uint16 LE)
    [1] Pattern tag (uint8)
    [...] Pattern-specific payload
  Per entry (meta-group):
    [2] Sentinel 0xFFFF
    [1] META_TAG (12)
    [1] Inner pattern type
    [2] Track count
    [2×N] Track indices
    [1] Sub-tag (0=AllSame, 1=ConstLinear, 2=LinearStartsLinear)
    [...] Meta parameters
```

---

## Roadmap

- [x] Rust implementation with 18 pattern types
- [x] Binary format (12-byte header + compact track encoding)
- [x] Adaptive sector sizing (7 geometries)
- [x] Multi-platter splitting (structured vs raw)
- [x] Configurable raw fallback compressor (deflate on by default, `--no-zlib` to disable)
- [x] Wide-word analysis (16/32-bit integers, f64 doubles)
- [x] Cross-track meta-descriptors
- [x] bzip2-style CLI
- [x] Columnar pre-processing (`pltz -r <stride>`)
- [x] Structured encryption (demo — see below)
- [x] Per-section geometry (chunked mode, each chunk gets optimal geometry)
- [x] Streaming/chunked encoding (`pltz -C 64k` or `pltz -C auto`)
- [x] File format versioning (PLTS v2 header)

### Future Investigation: Alternative Geometries

- [ ] Radial geometry (pie slices — columnar transform baked into geometry)
- [ ] Zoned geometry (variable sectors per track, like modern HDDs)
- [ ] Fractal/hierarchical (recursive subdivision, multi-scale pattern detection)
- [ ] Spiral geometry (no track boundaries, patterns can span any length)
- [ ] Cylindrical (cross-platter patterns at same radius)
- [ ] Helical scan (diagonal traversal for 2D-structured data)

---

## Columnar Pre-Processing

For record-oriented data (IoT telemetry, database rows, fixed-size structs), the columnar transform groups each field across all records together, exposing per-field patterns:

```bash
pltz -r 21 telemetry.bin    # stride = record size in bytes
```

The `-r` (record size) flag tells PLTZ that the input consists of fixed-size records of the given byte length. PLTZ transposes the data so that byte 0 of every record is contiguous, then byte 1 of every record, and so on. This turns interleaved multi-field records into per-field columns that PLTZ's pattern detectors can recognize.

**Example:** 100 IoT readings of 21 bytes each (timestamp + device\_id + seq + battery + temp + ...). Without `-r`, PLTZ sees interleaved fields — no patterns. With `-r 21`, all timestamps are grouped together (LINEAR\_WIDE), all device IDs together (CONST), all sequence numbers together (LINEAR), etc.

Without columnar, a batch of 100 IoT readings (2.1KB) compresses poorly. With columnar (stride = record size), compression improves dramatically because each field column has strong mathematical structure.

---

## Structured Encryption (⚠️ DEMO)

> **WARNING:** The encryption implementation uses a simplified stream cipher for demonstration purposes only. It is NOT cryptographically secure. Do not use for real data protection without replacing the cipher with a proper AEAD (ChaCha20-Poly1305 or AES-256-GCM).

PLTZ Structured Encryption (PSE) encrypts pattern *parameters* while preserving the compressed structure:

| Approach | 4KB linear ramp | Security |
|----------|----------------|----------|
| Encrypt-then-compress | 4,096B | IND-CPA (full) |
| Compress-then-encrypt | ~315B | Leaks compressed size |
| **PLTZ structured encrypt** | **168B** | Leaks pattern types |

**How it works:** Analyze data → detect patterns → encrypt only the parameter values (start, step, etc.) → store pattern types in the clear with encrypted parameters.

**What's leaked:** Pattern types and track layout (attacker sees "track 5 is LINEAR" but not the values).

**Acceptable for:** IoT telemetry where schema is public but values are private, satellite housekeeping with public frame structure, sensor networks with known measurement cadence.

**Not acceptable for:** Any scenario requiring semantic security (IND-CPA), or where pattern type itself is sensitive.

---

## Image Codec (PLTI)

Lossless and lossy compression for grayscale images (8-bit and 16-bit). Each image row is treated as a track and fit with mathematical functions.

### Lossy Mode

Quality parameter (0=lossless, 1-255=max per-pixel error):

```bash
# In code:
let params = ImageParams { width: 1024, height: 1024, bit_depth: 16, quality: 5 };
let compressed = compress_image(&pixels, params);
```

### Results

| Image type | Original | Compressed | Ratio |
|-----------|----------|-----------|-------|
| Star field 256×64 (8-bit, q=5) | 16KB | 930B | 17.6:1 |
| Illumination gradient 512×16 | 8KB | 72B | 113:1 |
| Dark calibration frame 128×32 (16-bit, q=4) | 8KB | 95B | 86:1 |

### Target Applications

**Space probe cameras** — Star fields (95% black), calibration frames (near-constant), illumination gradients (polynomial per row).

**Security/surveillance cameras** — Static scenes where 90-99% of pixels don't change between frames. Difference frames become CONST(0) rows → extreme compression on "nothing happening" footage.

**Scientific imaging** — Thermal cameras, spectrograms, oscilloscope captures. Mathematically structured by nature.
