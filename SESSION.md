# PolynomiaZ — Design Session Notes

## Session: 2026-05-05

### Concept

Polar coordinate compression. Map data onto virtual disk platters (tracks × sectors),
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
   [32, 64, 128, 256, 512] sectors per track and pick the best.

5. **Binary format** — JSON prototype was 4× worse on raw data. Binary format has 12-byte
   header + 3 bytes per track (index + tag) + pattern-specific payload.

6. **Configurable raw fallback** — Raw tracks can optionally be compressed with zlib/bz2
   for mixed data scenarios.

### Pattern Detection Priority

Patterns are tried in order of compactness:
1. CONST (1 byte) — all values identical
2. LINEAR (4 bytes) — arithmetic sequence
3. RLE (2 bytes/run) — consecutive repeated values
4. REPEAT (pattern_len bytes) — repeating sub-pattern
5. DELTA (2 + n-1 bytes) — small deltas with ≤8 unique values
6. PERIODIC (4 + 18/component) — FFT with few significant components
7. POLY (1 + 8/coeff) — polynomial fit degree ≤ 8
8. RAW (n bytes) — fallback

### What Works

- Linear ramps: 10-15× better than zlib (256B → 21B vs 267B)
- Firmware images: 3-4× better than zlib (structured padding + headers)
- Mixed data: wins when some tracks are structured
- Repeating patterns: competitive with zlib for short periods

### What Doesn't Work

- English text: no mathematical structure at byte level
- Integer-rounded periodic signals: rounding noise requires too many FFT components
- Interleaved multi-channel: patterns broken across tracks
- Large constant fills: zlib's run-length is more byte-efficient at scale
- Step sizes that don't align with sector boundaries

### Key Insight

PLTZ wins when data **is** a mathematical function sampled at regular intervals.
Standard compressors (zlib, bz2, lzma) look for byte-level repetition — they can't
recognize that [0, 1, 2, 3, ...] is a function. They just see "no repeated substrings."

### Explored: Text Compression via Transforms

Tested pre-processing transforms to make English text PLTZ-friendly:
- **Columnar reordering** — marginal improvement, doesn't beat zlib
- **Delta encoding** — helps slightly, still loses to zlib
- **BWT (Burrows-Wheeler)** — creates runs that RLE can catch, but still loses to zlib on non-repeating text

Conclusion: English text is fundamentally not mathematical at the byte level.
PLTZ should not try to be a general-purpose compressor.

### Target Use Cases (Real-World)

1. Embedded firmware images (0xFF padding, address tables, init ramps)
2. ADC/DAC calibration data (linear sweeps, staircase patterns)
3. Sensor calibration tables (polynomial curves, linearization LUTs)
4. Test equipment waveforms (sawtooth, triangle, ramps)
5. Protocol framing (sync patterns, incrementing sequence numbers)
6. Scientific instrument sweep data (frequency/temperature/voltage ramps)
7. Motor control profiles (trapezoidal velocity ramps, PWM tables)

### Not Suitable For

- LLM weights/embeddings (high-entropy learned representations)
- Natural language text (statistical patterns, not mathematical)
- Already-compressed data (images, video, archives)
- Encrypted data (indistinguishable from random)
- Audio/video (use domain-specific codecs)

### Next Steps

- [ ] Fix sector alignment issue for non-power-of-2 step sizes
- [ ] Add cross-track detection (same pattern repeated across tracks → single descriptor)
- [ ] De-interleaving pre-processor for multi-channel data
- [ ] Streaming mode for large files
- [ ] Rust/Go implementation for production use
- [ ] Benchmark against real firmware images and sensor logs

### Architecture

```
proto/
├── platter.py          — Disk geometry, data layout, polar coordinates
├── analyzer.py         — Pattern detection (8 types)
├── codec.py            — Binary encode/decode, adaptive geometry, multi-platter
├── test_codec.py       — Lossless round-trip tests
├── benchmark.py        — Comparison vs zlib/bz2/lzma
└── text_transforms.py  — Experimental text pre-processing (BWT, columnar, delta)
```

### Binary Format

```
Header (12 bytes):
  [4] Magic "PLTZ"
  [4] Original data length (uint32 LE)
  [2] Sectors per track (uint16 LE)
  [2] Number of platters (uint16 LE)

Per platter:
  [2] Number of tracks (uint16 LE)
  Per track:
    [2] Track index (uint16 LE)
    [1] Pattern tag (uint8)
    [...] Pattern-specific payload

RAW_COMPRESSED (tag 8):
    [1] Method (1=zlib, 2=bz2)
    [2] Compressed length (uint16 LE)
    [...] Compressed data
```
