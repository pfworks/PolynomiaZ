# PLTZ Wire Format Specification

Version: 2  
Date: 2026-05-07  
Status: Stable (backward-compatible additions only)

## Overview

PLTZ uses four magic signatures for different container formats:

| Magic | Format | Description |
|-------|--------|-------------|
| `PLTZ` | Standard | Single-geometry compressed data |
| `PLTC` | Columnar | Columnar-transformed wrapper around PLTZ |
| `PLTS` | Streaming | Chunked/streaming with per-chunk geometry |
| `PLTI` | Image | Lossy/lossless image codec |

All multi-byte integers are **little-endian** unless noted otherwise.

---

## PLTZ Format (Standard)

```
Offset  Size  Field
------  ----  -----
0       4     Magic: "PLTZ" (0x50 0x4C 0x54 0x5A)
4       4     Original data length (uint32 LE)
8       2     Sectors per track (uint16 LE)
10      2     Number of platters (uint16 LE)

Per platter:
  0     2     Number of entries (uint16 LE)
  Per entry: either Individual Track or Meta-Group
```

### Individual Track Entry

```
Offset  Size  Field
------  ----  -----
0       2     Track index (uint16 LE)
2       1     Pattern tag (uint8)
3       var   Pattern-specific payload
```

### Meta-Group Entry

```
Offset  Size  Field
------  ----  -----
0       2     Sentinel: 0xFFFF
2       1     META_TAG: 12
3       1     Inner pattern type (uint8)
4       2     Track count N (uint16 LE)
6       2*N   Track indices (uint16 LE each)
6+2*N   1     Sub-tag (0=AllSame, 1=ConstLinear, 2=LinearStartsLinear)
7+2*N   var   Meta parameters (depends on sub-tag)
```

**Sub-tag 0 (AllSame):** Parameters for the inner pattern type (same as individual track payload).

**Sub-tag 1 (ConstLinear):** `start(u8) + step(i8)` — CONST values form arithmetic sequence.

**Sub-tag 2 (LinearStartsLinear):** `start_base(i16) + start_step(i16) + step(i16)` — LINEAR tracks with linearly-incrementing starts.

---

## Pattern Tags and Payloads

| Tag | Name | Payload |
|-----|------|---------|
| 0 | CONST | `value(u8)` |
| 1 | LINEAR | `start(i16) + step(i16)` |
| 2 | RLE | `num_runs(u16) + [value(u8) + count(u8)] × N` |
| 3 | REPEAT | `pattern_len(u16) + pattern_bytes` |
| 4 | DELTA | `start(i16) + deltas(i8 × (sectors-1))` |
| 5 | PERIODIC | `num_components(u16) + n(u16) + [index(u16) + real(f64) + imag(f64)] × K` |
| 6 | POLY | `degree(u8) + coefficients(f64 × (degree+1))` |
| 7 | RAW | `raw_bytes(sectors bytes)` |
| 8 | RAW_COMPRESSED | `method(u8) + compressed_len(u16) + compressed_data` |
| 9 | LINEAR_WIDE | `width(u8) + count(u16) + start(i64) + step(i64)` |
| 10 | LINEAR_F64 | `count(u16) + start(f64) + step(f64)` |
| 11 | DELTA_F64 | `count(u16) + start(f64) + deltas(f32 × (count-1))` |
| 13 | XOR_MASK | `mask(u8) + inner_tag(u8) + inner_payload` |
| 14 | SPARSE | `fill(u8) + num_entries(u16) + [index(u16) + value(u8)] × N` |
| 15 | PIECEWISE_LINEAR | `num_segments(u8) + [start_idx(u16) + start_val(i16) + step(i16)] × N` |
| 16 | MIRROR | `half_data(sectors/2 bytes)` |
| 17 | INTERLEAVED_LINEAR | `count(u16) + start_a(i16) + step_a(i16) + start_b(i16) + step_b(i16)` |
| 18 | EXPONENTIAL | `count(u16) + base(f32) + ratio(f32)` |
| 19 | DELTA_RLE | `start(u8) + num_runs(u16) + [delta(i8) + count(u8)] × N` |
| 20 | BIT_PACKED | `bits(u8) + packed_data(⌈sectors×bits/8⌉ bytes)` |
| 21 | DIFF_TABLE | `order(u8) + initial(order+1 bytes) + diff(i32 LE)` |
| 22 | MOD_ARITH | `a(u16) + b(u16) + m(u16)` |
| 23 | FIBONACCI | `seed0(u8) + seed1(u8) + c1(u8) + c2(u8)` |
| 24 | DELTA_WIDE | `width(u8) + delta_width(u8) + start(u32 LE) + deltas((count-1)×dw bytes)` |

### RAW_COMPRESSED Methods

| Method | Algorithm |
|--------|-----------|
| 1 | DEFLATE (RFC 1951) |
| 2 | Zstandard (RFC 8878) |
| 3 | Brotli (RFC 7932) |

Default (`--raw best`): tries all three, stores whichever is smallest. Decoder auto-detects from method byte.

---

## PLTC Format (Columnar Wrapper)

```
Offset  Size  Field
------  ----  -----
0       4     Magic: "PLTC" (0x50 0x4C 0x54 0x43)
4       4     Original data length (uint32 LE)
8       2     Stride / record size (uint16 LE)
10      var   Inner PLTZ-encoded data (columnar-transformed)
```

Decoding: decode inner PLTZ data, then apply inverse columnar transform with the given stride and original length.

---

## PLTS Format (Streaming/Chunked)

```
Offset  Size  Field
------  ----  -----
0       4     Magic: "PLTS" (0x50 0x4C 0x54 0x53)
4       1     Format version (uint8, currently 2)
5       4     Original data length (uint32 LE)
9       4     Chunk size (uint32 LE)
13      2     Number of chunks (uint16 LE)

Per chunk:
  0     4     Compressed chunk length (uint32 LE)
  4     var   PLTZ-encoded chunk (independent, own geometry)
```

Each chunk is a self-contained PLTZ blob. Chunks can be decoded independently (random access).

---

## PLTI Format (Image)

```
Offset  Size  Field
------  ----  -----
0       4     Magic: "PLTI" (0x50 0x4C 0x54 0x49)
4       4     Original pixel data length (uint32 LE)
8       2     Width in pixels (uint16 LE)
10      2     Height in pixels (uint16 LE)
12      1     Bit depth (8 or 16)
13      1     Quality (0=lossless, 1-255=max per-pixel error)
14      2     Reserved (0x0000)
16      var   Encoded row data (PLTZ for lossless, custom for lossy)
```

### Lossless mode (quality=0)

Bytes 16+ are a standard PLTZ blob encoding the raw pixel data.

### Lossy mode (quality>0)

```
Offset  Size  Field
------  ----  -----
16      2     Number of entries (uint16 LE)

Per entry: either Meta-Row-Group or Individual Row

Individual Row:
  0     2     Row index (uint16 LE)
  2     1     Row pattern tag (0=CONST, 1=LINEAR, 2=POLY2, 3=POLY3, 7=RAW)
  3     2     Param length (uint16 LE)
  5     var   Parameters

Meta-Row-Group:
  0     1     Marker: 0xFF
  1     1     Pattern tag
  2     2     Row count N (uint16 LE)
  4     2*N   Row indices (uint16 LE each)
  4+2*N 2     Param length (uint16 LE)
  6+2*N var   Parameters (shared by all rows in group)
```

---

## Reconstruction Rules

### Track Reconstruction

Given pattern tag and payload, reconstruct `sectors` bytes:

- **CONST:** Fill with `value` repeated `sectors` times.
- **LINEAR:** `output[i] = (start + i * step) as u8` for i in 0..sectors.
- **RLE:** Expand runs: each (value, count) pair produces `count` copies of `value`. Truncate to `sectors`.
- **REPEAT:** `output[i] = pattern[i % pattern_len]` for i in 0..sectors.
- **DELTA:** `output[0] = start as u8`, then `output[i] = output[i-1] + delta[i-1]` (wrapping).
- **PERIODIC:** Build complex spectrum from components, inverse FFT, round to u8.
- **POLY:** `output[i] = round(sum(coeffs[j] * i^(degree-j) for j))` as u8.
- **LINEAR_WIDE:** Generate `count` values as `(start + i*step)`, pack as LE u16 or u32. Truncate to `sectors` bytes.
- **LINEAR_F64:** Generate `count` values as `start + i*step`, pack as LE f64. Truncate to `sectors` bytes.
- **DELTA_F64:** Accumulate f32 deltas onto f64 start, pack each as LE f64.
- **XOR_MASK:** Reconstruct inner pattern, then XOR each byte with `mask`.
- **SPARSE:** Fill with `fill`, then set `entries[i].index` to `entries[i].value`.
- **PIECEWISE_LINEAR:** For each segment, fill from `start_idx` to next segment's start with `start_val + offset*step`.
- **MIRROR:** Output = `half ++ reverse(half)`.
- **INTERLEAVED_LINEAR:** `output[2i] = (start_a + i*step_a) as u8`, `output[2i+1] = (start_b + i*step_b) as u8`.
- **EXPONENTIAL:** `output[i] = round(base * ratio^i) as u8`.
- **DELTA_RLE:** Start with `start`, expand each (delta, count) run: add `delta` to current value `count` times.
- **BIT_PACKED:** Read `bits` per value from packed data (MSB first within each byte). Each value is `bits` wide.
- **DIFF_TABLE:** Reconstruct using forward differences: maintain `order` levels of differences, extend from bottom (constant diff) up.
- **MOD_ARITH:** `output[i] = (a*i + b) mod m` as u8.
- **FIBONACCI:** `output[0]=seed0, output[1]=seed1, output[i]=(c1*output[i-1]+c2*output[i-2]) mod 256`.
- **DELTA_WIDE:** Read start as u32, then accumulate deltas (i8 if dw=1, i16 if dw=2). Pack each value as u32 LE (or u16 if width=2).
- **RAW:** Copy bytes directly.
- **RAW_COMPRESSED:** Decompress with indicated method, then copy.

---

## Versioning

- The PLTS format includes an explicit version byte. Decoders MUST reject versions higher than they support.
- PLTZ/PLTC/PLTI formats are implicitly version 1. Future incompatible changes will use new magic bytes.
- New pattern tags may be added without changing the version (decoders that encounter unknown tags should fail gracefully).

---

## Byte Order

All integers are little-endian (LE) except:
- f64/f32 values are IEEE 754 LE
- Pattern tag 5 (PERIODIC): component index is u16 LE, real/imag are f64 LE

---

## Limits

- Maximum original data length per PLTZ blob: 4 GB (uint32). Files >4GB automatically use PLTS chunked mode.
- Maximum sectors per track: 65535 (uint16)
- Maximum tracks per platter: 65535 (uint16, due to track index field)
- Maximum platters: 65535 (uint16)
- Maximum chunk count (PLTS): 65535 (uint16)
- Maximum compressed chunk size: 4 GB (uint32)
- **Effective maximum file size: unlimited** (via PLTS chunked mode, 64MB chunks)
