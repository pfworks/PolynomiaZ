"""
Codec: encode (compress) and decode (decompress) using the platter/analyzer system.

Binary format:
  Header (12 bytes):
    - Magic: 4 bytes "PLTZ"
    - Original length: 4 bytes (uint32 LE)
    - Sectors per track: 2 bytes (uint16 LE)
    - Num platters: 2 bytes (uint16 LE)

  Per platter:
    - Num tracks: 2 bytes (uint16 LE)
    - Per track:
      - Track index: 2 bytes (uint16 LE)
      - Pattern tag: 1 byte
      - Pattern-specific payload (variable)

  RAW_COMPRESSED (tag 8): raw data compressed with a secondary compressor
    - Compressed length: 2 bytes (uint16 LE)
    - Compressed data
"""

import struct
import zlib
import bz2
import numpy as np
from typing import List, Optional, Tuple

from platter import DiskGeometry, compute_geometry, lay_out, SECTOR_CANDIDATES
from analyzer import PatternType, TrackEncoding, analyze_platter, analyze_track

# --- Configurable raw fallback compressor ---

_raw_compressor = None  # None, "zlib", or "bz2"
RAW_COMPRESSED_TAG = 8  # Extra tag beyond PatternType values


def set_raw_compressor(method: Optional[str]):
    """Set the compressor for raw (unpatternized) tracks. None, 'zlib', or 'bz2'."""
    global _raw_compressor
    _raw_compressor = method


def _compress_raw(data: bytes) -> Optional[bytes]:
    """Compress raw data with the configured compressor. Returns None if not configured."""
    if _raw_compressor == "zlib":
        return zlib.compress(data, 9)
    elif _raw_compressor == "bz2":
        return bz2.compress(data, 9)
    return None


def _decompress_raw(data: bytes, method_tag: int) -> bytes:
    """Decompress raw data. method_tag: 1=zlib, 2=bz2."""
    if method_tag == 1:
        return zlib.decompress(data)
    elif method_tag == 2:
        return bz2.decompress(data)
    raise ValueError(f"Unknown raw compression method: {method_tag}")


def _raw_method_tag() -> int:
    if _raw_compressor == "zlib":
        return 1
    elif _raw_compressor == "bz2":
        return 2
    return 0


# --- Binary serialization ---

def _encode_track_binary(enc: TrackEncoding, sectors: int) -> bytes:
    """Serialize a single track encoding to binary. Prefixes with track index."""
    # Track index (uint16) + pattern tag (uint8)
    tag = struct.pack("<HB", enc.track_idx, int(enc.pattern))
    p = enc.params

    if enc.pattern == PatternType.CONST:
        # 1 byte value
        return tag + struct.pack("<B", p["value"])

    elif enc.pattern == PatternType.LINEAR:
        # start (int16), step (int16)
        return tag + struct.pack("<hh", p["start"], p["step"])

    elif enc.pattern == PatternType.RLE:
        # num_runs (uint16), then (value, count) pairs
        runs = p["runs"]
        buf = tag + struct.pack("<H", len(runs))
        for val, count in runs:
            buf += struct.pack("<BB", val, count)
        return buf

    elif enc.pattern == PatternType.REPEAT:
        # pattern_len (uint16), pattern bytes
        pat = p["pattern"]
        return tag + struct.pack("<H", len(pat)) + bytes(pat)

    elif enc.pattern == PatternType.DELTA:
        # start (int16), then signed byte deltas
        deltas = p["deltas"]
        buf = tag + struct.pack("<h", p["start"])
        buf += struct.pack("<%db" % len(deltas), *deltas)
        return buf

    elif enc.pattern == PatternType.PERIODIC:
        # num_components (uint16), n (uint16), then (index uint16, real float64, imag float64)
        comps = p["components"]
        buf = tag + struct.pack("<HH", len(comps), p["n"])
        for idx, real, imag in comps:
            buf += struct.pack("<Hdd", idx, real, imag)
        return buf

    elif enc.pattern == PatternType.POLY:
        # degree (uint8), coefficients (float64 each)
        deg = p["degree"]
        coeffs = p["coeffs"]
        buf = tag + struct.pack("<B", deg)
        for c in coeffs:
            buf += struct.pack("<d", c)
        return buf

    elif enc.pattern == PatternType.LINEAR_WIDE:
        # width (uint8), count (uint16), start (int64), step (int64)
        return tag + struct.pack("<BHqq", p["width"], p["count"], p["start"], p["step"])

    elif enc.pattern == PatternType.LINEAR_F64:
        # count (uint16), start (float64), step (float64)
        return tag + struct.pack("<Hdd", p["count"], p["start"], p["step"])

    elif enc.pattern == PatternType.DELTA_F64:
        # count (uint16), start (float64), deltas as float32
        deltas = p["deltas_f32"]
        buf2 = tag + struct.pack("<Hd", p["count"], p["start"])
        for d in deltas:
            buf2 += struct.pack("<f", d)
        return buf2

    elif enc.pattern == PatternType.RAW:
        raw_bytes = bytes(p["data"])
        compressed = _compress_raw(raw_bytes)
        if compressed and len(compressed) < len(raw_bytes):
            # Use compressed raw: tag becomes RAW_COMPRESSED_TAG
            header = struct.pack("<HB", enc.track_idx, RAW_COMPRESSED_TAG)
            return header + struct.pack("<BH", _raw_method_tag(), len(compressed)) + compressed
        return tag + raw_bytes

    raise ValueError(f"Unknown pattern: {enc.pattern}")


def _decode_track_binary(buf: bytes, offset: int, sectors: int) -> Tuple[int, List[int], int]:
    """Deserialize a track from binary. Returns (track_idx, track_data, new_offset)."""
    track_idx, tag_val = struct.unpack_from("<HB", buf, offset)
    offset += 3

    # Handle compressed raw tracks
    if tag_val == RAW_COMPRESSED_TAG:
        method_tag, comp_len = struct.unpack_from("<BH", buf, offset)
        offset += 3
        comp_data = buf[offset:offset + comp_len]
        offset += comp_len
        raw = _decompress_raw(comp_data, method_tag)
        return track_idx, list(raw), offset

    tag = PatternType(tag_val)

    if tag == PatternType.CONST:
        val = buf[offset]
        return track_idx, [val] * sectors, offset + 1

    elif tag == PatternType.LINEAR:
        start, step = struct.unpack_from("<hh", buf, offset)
        offset += 4
        return track_idx, [start + i * step for i in range(sectors)], offset

    elif tag == PatternType.RLE:
        num_runs, = struct.unpack_from("<H", buf, offset)
        offset += 2
        track = []
        for _ in range(num_runs):
            val, count = struct.unpack_from("<BB", buf, offset)
            offset += 2
            track.extend([val] * count)
        return track_idx, track[:sectors], offset

    elif tag == PatternType.REPEAT:
        pat_len, = struct.unpack_from("<H", buf, offset)
        offset += 2
        pattern = list(buf[offset:offset + pat_len])
        offset += pat_len
        return track_idx, [pattern[i % pat_len] for i in range(sectors)], offset

    elif tag == PatternType.DELTA:
        start, = struct.unpack_from("<h", buf, offset)
        offset += 2
        num_deltas = sectors - 1
        deltas = struct.unpack_from("<%db" % num_deltas, buf, offset)
        offset += num_deltas
        track = [start]
        for d in deltas:
            track.append(track[-1] + d)
        return track_idx, track, offset

    elif tag == PatternType.PERIODIC:
        num_comps, n = struct.unpack_from("<HH", buf, offset)
        offset += 4
        fft = np.zeros(n // 2 + 1, dtype=np.complex128)
        for _ in range(num_comps):
            idx, real, imag = struct.unpack_from("<Hdd", buf, offset)
            offset += 18
            fft[idx] = complex(real, imag)
        track = np.round(np.fft.irfft(fft, n=n)).astype(int).tolist()
        return track_idx, track, offset

    elif tag == PatternType.POLY:
        deg = buf[offset]
        offset += 1
        coeffs = []
        for _ in range(deg + 1):
            c, = struct.unpack_from("<d", buf, offset)
            offset += 8
            coeffs.append(c)
        x = np.arange(sectors, dtype=np.float64)
        track = np.round(np.polyval(coeffs, x)).astype(int).tolist()
        return track_idx, track, offset

    elif tag == PatternType.LINEAR_WIDE:
        width, count, start, step = struct.unpack_from("<BHqq", buf, offset)
        offset += 19
        track = []
        for i in range(count):
            val = (start + i * step) & (0xFFFF if width == 2 else 0xFFFFFFFF)
            if width == 4:
                track.extend(struct.pack("<I", val))
            else:
                track.extend(struct.pack("<H", val))
        return track_idx, track[:sectors], offset

    elif tag == PatternType.LINEAR_F64:
        count, start, step = struct.unpack_from("<Hdd", buf, offset)
        offset += 18
        track = []
        for i in range(count):
            val = start + i * step
            track.extend(struct.pack("<d", val))
        return track_idx, list(track[:sectors]), offset

    elif tag == PatternType.DELTA_F64:
        count, start = struct.unpack_from("<Hd", buf, offset)
        offset += 10
        track = []
        current = start
        track.extend(struct.pack("<d", current))
        for i in range(count - 1):
            delta, = struct.unpack_from("<f", buf, offset)
            offset += 4
            current += delta
            track.extend(struct.pack("<d", current))
        return track_idx, list(track[:sectors]), offset

    elif tag == PatternType.RAW:
        track = list(buf[offset:offset + sectors])
        return track_idx, track, offset + sectors

    raise ValueError(f"Unknown tag: {tag}")


# --- Multi-platter splitting ---

def _split_platters(tracks: List[List[int]], geom: DiskGeometry) -> List[List[TrackEncoding]]:
    """Split tracks into platters: structured tracks on platter 0, raw on platter 1."""
    encodings = analyze_platter(tracks)

    structured = [e for e in encodings if e.pattern != PatternType.RAW]
    raw = [e for e in encodings if e.pattern == PatternType.RAW]

    platters = []
    if structured:
        platters.append(structured)
    if raw:
        platters.append(raw)
    if not platters:
        platters.append([])
    return platters


# --- Adaptive sector sizing ---

def _estimate_size(encodings: List[TrackEncoding], sectors: int) -> int:
    """Estimate binary size of encoded tracks."""
    total = 0
    for enc in encodings:
        total += len(_encode_track_binary(enc, sectors))
    return total


def _best_geometry(data: bytes) -> DiskGeometry:
    """Try multiple sector sizes and pick the one that compresses best."""
    if len(data) == 0:
        return compute_geometry(0)

    best_size = float("inf")
    best_geom = None

    for spt in SECTOR_CANDIDATES:
        geom = compute_geometry(len(data), spt)
        tracks = lay_out(data, geom)
        encodings = analyze_platter(tracks)
        size = _estimate_size(encodings, spt)
        if size < best_size:
            best_size = size
            best_geom = geom

    return best_geom


# --- Public API ---

MAGIC = b"PLTZ"


def encode(data: bytes) -> bytes:
    """Compress data into binary platter-encoded format."""
    if len(data) == 0:
        return MAGIC + struct.pack("<IHH", 0, 256, 0)

    geom = _best_geometry(data)
    tracks = lay_out(data, geom)
    platters = _split_platters(tracks, geom)

    # Header
    buf = MAGIC
    buf += struct.pack("<IHH", len(data), geom.sectors_per_track, len(platters))

    # Platters
    for platter_encodings in platters:
        buf += struct.pack("<H", len(platter_encodings))
        for enc in platter_encodings:
            buf += _encode_track_binary(enc, geom.sectors_per_track)

    return buf


def decode(compressed: bytes) -> bytes:
    """Decompress binary platter-encoded data."""
    if compressed[:4] != MAGIC:
        raise ValueError("Invalid format: bad magic")

    original_len, sectors, num_platters = struct.unpack_from("<IHH", compressed, 4)
    if original_len == 0:
        return b""

    offset = 12
    all_tracks = {}

    for _ in range(num_platters):
        num_tracks, = struct.unpack_from("<H", compressed, offset)
        offset += 2
        for _ in range(num_tracks):
            track_idx, track_data, offset = _decode_track_binary(compressed, offset, sectors)
            all_tracks[track_idx] = track_data

    # Reassemble in track order
    total_tracks = (original_len + sectors - 1) // sectors
    result = []
    for i in range(total_tracks):
        result.extend(all_tracks[i])

    return bytes(result[:original_len])
