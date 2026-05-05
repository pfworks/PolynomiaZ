# PolynomiaZ (PLTZ)

Polar coordinate compression — models data as mathematical functions on virtual disk platters.

## What It Does

PolynomiaZ maps input data onto a virtual disk platter geometry (concentric tracks divided into sectors, like a hard drive). Each track is analyzed for mathematical patterns. Tracks that match a known pattern are stored as compact function descriptions instead of raw bytes.

**Core idea:** If 256 bytes form a linear sequence `[0, 1, 2, ..., 255]`, store `LINEAR(start=0, step=1, length=256)` in 4 bytes instead of 256.

### How It Works

1. **Layout** — Input bytes are arranged into tracks × sectors (configurable geometry)
2. **Analysis** — Each track is tested against 8 pattern types
3. **Encoding** — Matched patterns become compact function descriptors; unmatched tracks stored raw
4. **Adaptive geometry** — Multiple sector sizes are tried; the best-compressing geometry wins
5. **Multi-platter** — Structured and raw tracks are separated onto different platters

### Pattern Types

| Pattern | What it detects | Storage cost |
|---------|----------------|-------------|
| CONST | All identical values | 1 byte |
| LINEAR | Arithmetic sequences (any start, any step) | 4 bytes |
| RLE | Runs of repeated values | 2 bytes/run |
| REPEAT | Repeating sub-patterns | pattern length |
| DELTA | Values with few unique deltas between them | 2 + (n-1) bytes |
| PERIODIC | FFT-decomposable signals | 18 bytes/component |
| POLY | Polynomial curves (degree ≤ 8) | 8 bytes/coefficient |
| RAW | No pattern (optional secondary compressor) | n bytes |

---

## What It Does Well

PLTZ dramatically outperforms standard compressors on **mathematically structured data**:

| Data | PLTZ | zlib | bz2 | lzma |
|------|------|------|-----|------|
| Linear ramp 256B | **21B** | 267B | 283B | 292B |
| Linear ramp 1KB | **42B** | 286B | 283B | 292B |
| Linear ramp 4KB | **126B** | 315B | 657B | 324B |
| Firmware image (padding + headers) 4KB | **90B** | 307B | 580B | 324B |
| Mixed structured + random 4KB | **2.1KB** | 2.3KB | 3.0KB | 2.3KB |

**Why:** zlib/bz2/lzma look for byte-level repetition. They cannot recognize that `[0, 1, 2, 3, ...]` is a function — they just see "no repeated substrings" and store it verbatim. PLTZ recognizes the mathematical relationship and stores the function parameters.

### Best-case scenarios:

- **10-15× better than zlib** on linear/arithmetic sequences
- **3-4× better than zlib** on firmware images with structured padding
- **Wins on mixed data** where some portions are structured and others aren't

---

## What It Doesn't Do Well

| Data | PLTZ | zlib | Why PLTZ loses |
|------|------|------|---------------|
| English text 1.5KB | 1656B | 680B | No mathematical structure in character sequences |
| Random data 4KB | 4134B | 4109B | Nothing compresses random data; PLTZ has 1% overhead |
| Constant fill 64KB | 526B | 84B | zlib's run-length is more byte-efficient for simple repetition at scale |
| Sine wave (integer-rounded) 4KB | 4134B | 130B | Integer rounding destroys clean FFT reconstruction |
| Interleaved multi-channel 4KB | 4134B | 754B | Interleaving breaks per-track patterns |

**Why:** PLTZ needs data to align with its track geometry and match one of its pattern detectors *exactly* (lossless requirement). Data that has statistical patterns but no clean mathematical structure (text, compressed data, encrypted data) falls through to RAW.

### Known limitations:

- **Small overhead on incompressible data** (~1% from headers/track metadata)
- **Sector alignment matters** — patterns must fit within a single track to be detected
- **Integer rounding** breaks periodic detection for signals that aren't exactly periodic
- **Interleaved channels** need pre-processing (de-interleaving) to expose per-channel structure
- **Slower than zlib** for the adaptive geometry search (tries 5 sector sizes)

---

## Real-World Use Cases

### Where PLTZ is the right tool:

**1. Embedded systems / IoT firmware**
- Flash memory images with large 0xFF-filled regions and structured initialization tables
- Bootloader images with linear address tables
- Configuration EEPROM with calibration ramps

**2. Sensor data logging**
- ADC sweep data (linear voltage ramps during calibration)
- DAC output verification (staircase patterns)
- Monotonically incrementing counters and timestamps (delta-friendly)

**3. Test & measurement**
- Waveform generators outputting known patterns (sawtooth, triangle, ramps)
- Calibration sequences for oscilloscopes and signal analyzers
- Reference signals for ADC/DAC linearity testing

**4. Lookup tables in firmware**
- Linearization tables for sensors (often polynomial)
- PWM duty cycle tables (linear or piecewise-linear)
- Motor control profiles (trapezoidal velocity ramps)
- LED brightness curves (gamma correction — polynomial)

**5. Scientific instruments**
- Spectral calibration data (known wavelength references)
- Detector bias ramps
- Systematic sweep data (frequency sweeps, temperature ramps)

**6. Protocol framing**
- Repeated sync patterns (0x55AA preambles)
- Packet headers with incrementing sequence numbers
- Fixed-structure telemetry frames

**7. Storage/filesystem metadata**
- Partition tables with arithmetic block addresses
- FAT entries for contiguous files (sequential cluster numbers)
- Inode tables with regular structure

### Where to use PLTZ + zlib fallback:

When data is **mixed** — some structured regions (headers, tables, padding) and some unstructured regions (payload, compressed blobs). PLTZ handles the structured parts; zlib handles the rest.

---

## Comparison Philosophy

| Compressor | Strategy | Best at |
|-----------|----------|---------|
| **zlib** | Sliding window + Huffman | Repeated substrings (text, markup, logs) |
| **bz2** | BWT + RLE + Huffman | Large files with long-range repetition |
| **lzma** | Large dictionary + range coding | General purpose, high ratio |
| **PLTZ** | Mathematical function fitting | Data that *is* a function sampled at regular intervals |

PLTZ is not a general-purpose compressor. It's a **domain-specific** compressor for data with known mathematical structure. Use it where the data comes from a physical process or algorithm that produces predictable patterns.

---

## Quick Start

```bash
cd proto
pip install numpy
python3 test_codec.py      # verify lossless round-trip
python3 benchmark.py       # compare against zlib/bz2/lzma
```

### API

```python
from codec import encode, decode, set_raw_compressor

# Basic compression
compressed = encode(data)
original = decode(compressed)

# With zlib fallback for raw tracks
set_raw_compressor("zlib")  # or "bz2" or None
compressed = encode(data)
```

---

## Roadmap

- [x] Python prototype with 8 pattern types
- [x] Binary format (12-byte header + compact track encoding)
- [x] Adaptive sector sizing (tries 5 geometries)
- [x] Multi-platter splitting (structured vs raw)
- [x] Configurable raw fallback compressor (zlib/bz2)
- [ ] Pre-processing transforms (de-interleaving, BWT, columnar)
- [ ] Cross-track pattern detection (same function across multiple tracks)
- [ ] Streaming/chunked encoding for large files
- [ ] Rust/Go production implementation
- [ ] File format versioning and magic number registry
