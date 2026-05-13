# PolynomiaZ — Design Session Notes

## Session: 2026-05-05

### Concept

Map data onto virtual disk platters (tracks × sectors),
detect mathematical patterns per track, store compact function descriptions instead of raw bytes.
Multiple platters allow separate functions without overlap.

### Design Decisions

1. **Lossless** — Functions must exactly reproduce every byte. No approximation.
   This means we need exact interpolation or pattern matching, not curve fitting with residuals.

2. **Disk platter metaphor is structural, not just conceptual** — Data is physically laid out
   as if written to a disk (tracks = concentric rings, sectors = angular divisions). This
   geometry choice affects what patterns are detectable.

3. **One function per platter** — Rather than overlapping multiple functions on one platter,
   each function gets its own platter. Structured tracks go on platter 0, raw on platter 1.

4. **Adaptive geometry** — Different sector sizes expose different patterns. We try
   [8, 16, 32, 64, 128, 256, 512] sectors per track and pick the best.

5. **Binary format** — JSON prototype was 4× worse on raw data. Binary format has 12-byte
   header + 3 bytes per track (index + tag) + pattern-specific payload.

6. **Configurable raw fallback** — Raw tracks can optionally be compressed with deflate
   for mixed data scenarios.

7. **Wide-word analysis** — Reinterpret byte tracks as 16/32-bit integers or f64 doubles
   to detect patterns invisible at the byte level (e.g., linear address tables).

8. **Cross-track meta-descriptors** — Groups of tracks with identical or linearly-varying
   parameters are encoded as a single meta-group instead of N individual track entries.
   Uses sentinel track_idx=0xFFFF for unambiguous detection during decode.

### Pattern Detection Priority

Patterns are tried in order of compactness (RLE vs REPEAT picks smaller):
1. CONST (1 byte) — all values identical
2. LINEAR (4 bytes) — arithmetic sequence
3. RLE / REPEAT — pick whichever is smaller
4. LINEAR_WIDE (19 bytes) — linear in 16/32-bit words
5. LINEAR_F64 (18 bytes) — linear in IEEE 754 doubles
6. DELTA_F64 (10 + 4n bytes) — f64 with f32-representable deltas
7. DELTA (2 + n-1 bytes) — small deltas with ≤8 unique values
8. PERIODIC (4 + 18/component) — FFT with few significant components
9. POLY (1 + 8/coeff) — polynomial fit degree ≤ 8
10. XOR_MASK (2 + inner) — data XORed with constant reveals simpler pattern
11. SPARSE (3 + 3×exceptions) — mostly one value with few exceptions
12. PIECEWISE_LINEAR (1 + 6×segments) — 2-4 linear segments
13. MIRROR (n/2) — second half is reverse of first
14. INTERLEAVED_LINEAR (10 bytes) — two interleaved arithmetic sequences
15. EXPONENTIAL (10 bytes) — geometric sequence a×r^i
16. DELTA_RLE (3 + 2×runs) — run-length encoded deltas
17. BIT_PACKED (1 + ⌈n×bits/8⌉) — values fit in <7 bits, stored packed
18. RAW (n bytes) — fallback, optionally deflate-compressed

### Cross-Track Optimization

After per-track analysis, groups of ≥4 tracks with the same pattern type are checked:
- **AllSame** — All parameters identical → store once
- **ConstLinear** — CONST values form arithmetic sequence → store start + step
- **LinearStartsLinear** — LINEAR tracks with same step, starts forming sequence

Results:
- 64KB constant fill: 526B → 278B (1.9× improvement)
- 16KB linear ramps: 462B → 153B (3× improvement)
- 4KB firmware: 90B → 65B (1.4× improvement)

### What Works

- Linear ramps: 5-12× better than zlib
- f64 epoch arrays: 5.4× better than zlib (SPICE kernels)
- Segment address tables: 12.3× better than zlib
- Firmware images: 3-5× better than zlib (structured padding + headers)
- Mixed data: wins when some tracks are structured
- Repeating patterns: competitive with zlib for short periods

### What Doesn't Work

- English text: no mathematical structure at byte level
- Integer-rounded periodic signals: rounding noise requires too many FFT components
- Interleaved multi-channel: patterns broken across tracks
- Chebyshev polynomial coefficients: high-entropy f64 values
- Step sizes that don't align with sector boundaries

### Key Insight

PLTZ wins when data **is** a mathematical function sampled at regular intervals.
Standard compressors (zlib, bz2, lzma) look for byte-level repetition — they can't
recognize that [0, 1, 2, 3, ...] is a function. They just see "no repeated substrings."

### Explored: Text Compression via Transforms

Tested pre-processing transforms to make English text PLTZ-friendly:
- **Columnar reordering** — marginal improvement, doesn't beat zlib
- **Delta encoding** — helps slightly, still loses to zlib
- **BWT (Burrows-Wheeler)** — creates runs that RLE can catch, but still loses to zlib

Conclusion: English text is fundamentally not mathematical at the byte level.
PLTZ should not try to be a general-purpose compressor.

### Explored: SPICE Kernel Compression

- **Epoch directories** — LINEAR_F64 detects perfectly: 5.4× better than zlib
- **Segment address tables** — LINEAR_WIDE detects perfectly: 12.3× better than zlib
- **Chebyshev coefficients** — High-entropy, PLTZ falls through to RAW. lzma wins.
- **Columnar transform** helps (groups same coefficient across records) but zlib on
  columnar data still beats PLTZ on the coefficient arrays.

Best strategy: use PLTZ for structured metadata (epochs, addresses) and zlib/lzma
for the coefficient payload.

### Explored: U-Boot Firmware

- **Vector tables** — LINEAR_WIDE detects u32 address sequences: 36B vs zlib 88B
- **PLL config** — LINEAR_WIDE: 36B vs zlib 201B
- **Relocation tables** — LINEAR_WIDE: 102B vs zlib 576B
- **Machine code** — Falls through to RAW (expected)
- **0xFF padding** — CONST with cross-track meta

### Target Use Cases (Real-World)

1. Embedded firmware images (0xFF padding, address tables, init ramps)
2. Satellite telemetry (epoch arrays, attitude data, CCSDS framing)
3. SPICE kernels (epoch directories, segment address tables)
4. ADC/DAC calibration data (linear sweeps, staircase patterns)
5. Sensor calibration tables (polynomial curves, linearization LUTs)
6. Protocol framing (sync patterns, incrementing sequence numbers)
7. Motor control profiles (trapezoidal velocity ramps, PWM tables)

### Not Suitable For

- LLM weights/embeddings (high-entropy learned representations)
- Natural language text (statistical patterns, not mathematical)
- Already-compressed data (images, video, archives)
- Encrypted data (indistinguishable from random)
- Complex audio/video (use H.264/AAC for general content; PLTV/PLTA target niche scenarios)

### Architecture

```
rust/src/
├── lib.rs           — Module declarations
├── main.rs          — CLI binary (bzip2-style interface)
├── platter.rs       — Disk geometry, data layout
├── analyzer.rs      — Pattern detection (23 types, rustfft for periodic)
├── codec.rs         — Binary encode/decode, adaptive geometry, multi-platter
├── columnar.rs      — Columnar pre-processing transform
├── cross_track.rs   — Cross-track meta-descriptor optimization
├── encrypt.rs       — Structured encryption (⚠️ demo)
├── image.rs         — Image codec (lossless + lossy, 8/16-bit)
├── video.rs         — Video codec (PLTV — YCbCr 4:2:0, motion compensation, B-frames)
├── audio.rs         — Audio codec (PLTA — PCM frame-based, lossless + lossy)
├── visualize.rs     — SVG platter diagram generation (--visualize)
└── ffi.rs           — C FFI bindings (libpltz.so / libpltz.dylib)

clib/
├── pltz.h           — C header for FFI bindings
├── test_pltz.c      — C integration test
└── README.md        — C library usage guide

pltzfs/
├── src/             — Compressed filesystem prototype (multi-platter storage)
├── Cargo.toml       — Rust crate config
└── README.md        — Filesystem usage guide

tests/
├── test_codec.py    — Python round-trip tests
├── benchmark.py     — Comparison vs zlib/bz2/lzma
├── bench_uboot.py   — U-Boot firmware simulation
├── bench_spice.py   — SPICE kernel simulation
└── lib/             — Python reference codec
```

### Binary Format

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

### Roadmap

- [x] Python prototype (reference implementation, now in tests/lib/)
- [x] Rust implementation with bzip2-style CLI
- [x] 23 pattern types including wide-word and f64 analysis
- [x] Cross-track meta-descriptors
- [x] Adaptive sector sizing (7 geometries)
- [x] Multi-platter splitting
- [x] Configurable raw fallback (zlib/zstd/brotli; `--raw best` default, `--no-zlib` to disable)
- [x] Technical whitepaper
- [x] Columnar pre-processing (`pltz -r <stride>`)
- [x] Structured encryption (demo — NOT production-ready)
- [x] Per-section geometry (chunked mode, each chunk gets optimal geometry)
- [x] Streaming/chunked encoding (`pltz -C 64k` or `pltz -C auto`)
- [x] File format versioning (PLTS v2 header)
- [x] Human-readable size arguments (64k, 1M, auto)
- [x] Image codec (PLTI) for space/surveillance cameras
- [x] `--analyze` flag (recommends optimal settings)
- [x] `-b/--best` flag (auto-selects best compression)
- [x] Parallel processing (rayon — 9× speedup on test suite)
- [x] Wire format specification (`doc/WIRE_FORMAT.md`)
- [x] Auto-chunking for files >4GB (transparent, no user action needed)
- [x] Smart stride auto-detection (byte-equality autocorrelation for `-b`/`--analyze`)
- [x] Visualization (`pltz --visualize` → SVG platter diagram with legend)
- [x] Video codec (PLTV — YCbCr 4:2:0, motion compensation, B-frames)
- [x] Audio codec (PLTA — PCM frame-based, lossless + lossy)
- [x] C FFI bindings (libpltz.so / libpltz.dylib) + clib/ integration test
- [x] Compressed filesystem prototype (pltzfs — multi-platter storage)
- [x] Multiple raw fallback compressors (zlib/zstd/brotli, `--raw best` default)
- [x] Verbose pattern breakdown (`-v` shows detected patterns)

### Columnar Pre-Processing

Added `columnar.rs` — the `-r` (record size) flag tells PLTZ that input consists of
fixed-size records. PLTZ transposes the data so that byte 0 of every record is
contiguous, then byte 1, etc. This turns interleaved multi-field records into
per-field columns that pattern detectors can recognize.

Example: `pltz -r 21 telemetry.bin` means "each record is 21 bytes." PLTZ groups
all byte-0s together, all byte-1s together, etc. If byte 0-3 is a timestamp across
100 records, those 400 bytes now form a contiguous column that LINEAR_WIDE detects.

Exposed via `pltz -r <stride>` CLI flag and `encode_auto()` API.
Uses PLTC magic to signal columnar wrapper in the format.

Results on IoT telemetry (100 records × 21 bytes):
- Plain PLTZ: 2189B (no patterns detected in interleaved data)
- Columnar PLTZ (stride=21): 846B (fields become LINEAR_WIDE, CONST, DELTA)
- Columnar PLTZ + zlib: 787B (competitive with zlib's 784B)

### Structured Encryption (⚠️ DEMO ONLY)

Added `encrypt.rs` — encrypts pattern parameters while preserving PLTZ structure.
Uses PLTE magic. The stream cipher is a DEMO and is NOT cryptographically secure.

**Concept:** Standard encryption destroys compressibility. PLTZ structured encryption
encrypts only the minimal parameter bytes (start, step, etc.) while leaving pattern
types visible. This preserves the compression ratio.

**Results:**
- 4KB linear ramp: 168B encrypted (vs 4096B with standard AES-CTR)
- Compress-then-encrypt (zlib+AES): ~315B
- PLTZ structured encrypt: 168B (2× better than compress-then-encrypt)

**Security tradeoff:** Leaks pattern types and track layout. Acceptable for IoT/telemetry
where schema is public. NOT acceptable for IND-CPA requirements.

**Full encryption mode (PLTF):** Added second stream that encrypts metadata
(pattern types, track layout, geometry) in addition to parameters. No information
leakage except total compressed size. Only 4 bytes larger than structured mode.

**Dual-key mode (PLTD):** Outer key encrypts metadata, inner key encrypts parameters.
- Outer key only → see structure ("track 0 is CONST, track 2 is LINEAR")
- Both keys → full decryption
- Use case: tiered access control (operators see schema, analysts see values)

| Mode | 4KB linear | Leakage |
|------|-----------|---------|
| Structured (PLTE) | 168B | Pattern types visible |
| Full (PLTF) | 172B | Nothing (except total size) |
| Dual-key (PLTD) | 172B | Outer key: structure only |
| Compress-then-encrypt | ~315B | Compressed size |
| Encrypt-then-compress | 4,096B | Nothing |

**Production path:** Replace `stream_cipher()` with ChaCha20-Poly1305 or AES-256-GCM.
Add authentication tags. Add key derivation (HKDF). Add nonce management.

### Image Codec (PLTI)

Added `image.rs` — lossless and lossy compression for 8/16-bit grayscale images.
Each row = one track. Lossy mode fits rows with CONST/LINEAR/POLY2/POLY3 within
a configurable per-pixel error threshold. Cross-row grouping merges identical rows.

**Results (lossy):**
- Star field 256×64 (q=5): 16KB → 930B (17.6:1)
- Illumination gradient 512×16 (q=2): 8KB → 72B (113:1)
- Dark calibration frame 128×32 16-bit (q=4): 8KB → 95B (86:1)

**Target applications:**
- Space probe cameras: star fields (95% black), calibration frames, gradients
- Security/surveillance: static scenes where difference frames are mostly zero
- Scientific imaging: thermal cameras, spectrograms, oscilloscope captures

**Security camera use case:** For a static hallway camera, difference frames between
consecutive frames are almost entirely zero (nothing moved). Each difference frame
compresses to ~100 bytes (all rows CONST(0) in one meta-group). Only frames with
actual motion have significant data. This could reduce storage 10-50× vs H.264 on
the static portions of 24/7 surveillance footage.

### Video Codec (PLTV) — Motion Compensation + B-frames

Added `video.rs` — color lossy video with:
- YCbCr 4:2:0 chroma subsampling
- 16×16 block-based motion compensation (±16 pixel search)
- B-frames (bidirectional prediction, picks better reference)
- GOP structure: I B P B P ...

**Benchmark (2 min, 160×120, 5fps, 600 frames):**

Noisy surveillance (sensor noise ~10 DN):
- PLTV q=30: 1.5MB (22.8:1) — beats H.264 crf=23 (2.9MB, 11.7:1) by 2×
- PLTV q=50: 544KB (63.5:1) — beats H.264 by 5.4×

Clean synthetic (no noise):
- PLTV: 440KB (78:1) — H.264 wins at 1874:1 (ideal for block matching)

**Key insight:** Sensor noise defeats H.264's motion estimation (it sees "motion"
that's actually noise). PLTV's quality threshold absorbs the noise, making static
regions collapse to CONST regardless of noise level.

The alternative geometries (spiral, zoned, fractal) were tested against
the standard adaptive approach on all 8 real-world test files.

Result: No improvement. The adaptive geometry search (7 sector sizes)
already achieves 0 RAW tracks at spt=8, but the per-track overhead
(3 bytes × many tracks) makes larger sector sizes optimal for total
compressed size. The current approach finds this sweet spot automatically.

Alternative geometries remain on the roadmap as 'Future Investigation'
but are deprioritized based on these results.

## Session: 2026-05-12

### Thread Pool Control

Added `-j`/`--threads` CLI argument for controlling parallelism:
- `0`, `unlimited`, `all` → use all available cores (rayon default)
- Specific number (e.g., `4`) → limit to exactly N threads
- Percentage (e.g., `50%`) → use that percentage of available cores, rounded down (min 1)

Configures rayon's global thread pool at startup. All parallel work (geometry search,
track analysis, chunk encoding, best-mode search) respects the setting.

### Additional Parallelism

- `encode_chunked()` now compresses all chunks in parallel via `par_iter`
- `find_best_compression()` (`-b` mode) evaluates stride candidates and chunk sizes in parallel

### Performance: Entropy Fast-Path

Two optimizations that dramatically improve speed on high-entropy (random/compressed) data:

**1. Analyzer fast-path** (`analyzer.rs`):
When a track has >200 unique byte values, skip expensive detectors (FFT/periodic,
polynomial fitting, diff_table, mod_arith, fibonacci, mirror, sparse, piecewise_linear,
interleaved_linear, exponential, delta_rle, xor_mask). These can't find patterns in
random data and were consuming most of the analysis time.

**2. Raw compressor fast-path** (`codec.rs`):
When a raw track has ≥150 unique byte values, skip zstd and brotli — just use deflate.
zstd has significant per-call initialization overhead, and on high-entropy data it
can't beat deflate anyway. This eliminated a 680ms → 25ms improvement on 4KB random data.

**Results:**
- Random 4KB: 680ms → 25ms (27× faster)
- Mixed structured+random 4KB: 680ms → 29ms (23× faster)
- Structured data: unchanged (19-25ms)
- All 48 tests still pass — no compression ratio regression

### Benchmark Results (post-optimization)

| Data | PLTZ | gzip | zstd | PLTZ advantage |
|------|------|------|------|----------------|
| Linear ramp 4KB | **57B** | 342B | 280B | 6× better than gzip |
| Linear ramps 16KB | **153B** | 436B | 280B | 2.8× better |
| f64 epoch array 8KB | **350B** | 2664B | 2325B | 7.6× better |
| u32 address table 4KB | **190B** | 1018B | 2223B | 5.4× better |
| Firmware 4KB | **66B** | 254B | 286B | 3.8× better |
| Mixed structured+random | **2115B** | 2404B | 2328B | 1.1× better |
| Constant fill 64KB | 278B | 106B | **24B** | zstd wins |
| Random data 4KB | 4134B | 4129B | **4110B** | ~1% overhead |
| English text 4KB | 414B | 101B | **68B** | text not our domain |
