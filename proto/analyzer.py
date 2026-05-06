"""
Analyzer: detects patterns in track data and encodes them as compact function descriptions.

Pattern types (in priority order):
- CONST: all values identical
- LINEAR: arithmetic sequence
- RLE: run-length encoded (consecutive repeated values)
- REPEAT: repeating sub-pattern
- DELTA: constant delta between values (stored as base + deltas)
- PERIODIC: FFT-detected periodic signal
- POLY: low-degree polynomial fit
- RAW: no pattern found
"""

from dataclasses import dataclass
from enum import IntEnum
from typing import Dict, List, Optional
import numpy as np


class PatternType(IntEnum):
    """IntEnum so values can be used directly as binary tag bytes."""
    CONST = 0
    LINEAR = 1
    RLE = 2
    REPEAT = 3
    DELTA = 4
    PERIODIC = 5
    POLY = 6
    RAW = 7
    LINEAR_WIDE = 9   # Linear pattern on 16/32-bit words
    LINEAR_F64 = 10   # Linear pattern on f64 values
    DELTA_F64 = 11    # Small deltas between consecutive f64 values


@dataclass
class TrackEncoding:
    pattern: PatternType
    params: Dict
    track_idx: int


def detect_const(track: List[int]) -> Optional[Dict]:
    if len(set(track)) == 1:
        return {"value": track[0]}
    return None


def detect_linear(track: List[int]) -> Optional[Dict]:
    if len(track) < 2:
        return None
    step = track[1] - track[0]
    for i in range(2, len(track)):
        if track[i] - track[i - 1] != step:
            return None
    return {"start": track[0], "step": step}


def detect_rle(track: List[int]) -> Optional[Dict]:
    """Run-length encoding. Worth it if runs compress below raw size."""
    runs = []
    i = 0
    while i < len(track):
        val = track[i]
        count = 1
        while i + count < len(track) and track[i + count] == val and count < 255:
            count += 1
        runs.append((val, count))
        i += count
    # Each run = 2 bytes (value, count). Worth it if < raw length
    if len(runs) * 2 < len(track):
        return {"runs": runs}
    return None


def detect_repeat(track: List[int]) -> Optional[Dict]:
    n = len(track)
    for period in range(1, n // 2 + 1):
        pattern = track[:period]
        if all(track[i] == pattern[i % period] for i in range(n)):
            if period < n // 3:  # worth it if pattern is compact
                return {"pattern": pattern}
    return None


def detect_delta(track: List[int]) -> Optional[Dict]:
    """Delta encoding: store first value + differences. Good when deltas are small."""
    deltas = [track[i] - track[i - 1] for i in range(1, len(track))]
    # Worth it if all deltas fit in a signed byte (-128..127)
    if all(-128 <= d <= 127 for d in deltas):
        # Cost: 2 bytes (start) + len(deltas) bytes. Only worth if fewer unique deltas
        # indicate structure (otherwise it's just raw with extra steps)
        unique_deltas = len(set(deltas))
        # Must actually save space vs raw: delta cost = 2 + n-1, raw cost = n
        # So only worth it if we can later compress the deltas, or if unique count is low
        if unique_deltas <= 8:
            return {"start": track[0], "deltas": deltas}
    return None


def detect_periodic(track: List[int]) -> Optional[Dict]:
    """FFT-based periodic detection. Keep only components needed for exact reconstruction."""
    n = len(track)
    if n < 8:
        return None

    y = np.array(track, dtype=np.float64)
    fft = np.fft.rfft(y)
    magnitudes = np.abs(fft)

    # Sort components by magnitude, try keeping progressively more until exact
    sorted_indices = np.argsort(magnitudes)[::-1]

    for num_keep in range(1, min(len(fft), n // 4)):
        kept_indices = sorted_indices[:num_keep]
        kept_fft = np.zeros_like(fft)
        kept_fft[kept_indices] = fft[kept_indices]
        reconstructed = np.fft.irfft(kept_fft, n=n)

        if np.all(np.round(reconstructed).astype(int) == track):
            # Cost: 18 bytes per component + 4 bytes header
            cost = 4 + num_keep * 18
            if cost < n:  # only worth it if smaller than raw
                components = [(int(idx), float(fft[idx].real), float(fft[idx].imag))
                              for idx in kept_indices]
                return {"components": components, "n": n}
            return None  # found exact but not worth it size-wise

    return None


def detect_poly(track: List[int], max_degree: int = 8) -> Optional[Dict]:
    """Polynomial fit that exactly reproduces the track."""
    n = len(track)
    x = np.arange(n, dtype=np.float64)
    y = np.array(track, dtype=np.float64)

    for deg in range(2, min(max_degree + 1, n)):
        coeffs = np.polyfit(x, y, deg)
        reconstructed = np.polyval(coeffs, x)
        if np.all(np.round(reconstructed).astype(int) == track):
            return {"coeffs": coeffs.tolist(), "degree": deg}
    return None


def _detect_wide(track: List[int]) -> Optional[tuple]:
    """Try interpreting track as 16-bit or 32-bit words and detect linear patterns."""
    import struct as st
    n = len(track)
    raw = bytes(track)

    # Try 32-bit little-endian
    if n >= 8 and n % 4 == 0:
        words = [st.unpack_from("<I", raw, i)[0] for i in range(0, n, 4)]
        result = detect_linear(words)
        if result:
            # Cost: 1 byte width + 8 bytes (start u32 + step i32) = 9 bytes
            return (PatternType.LINEAR_WIDE, {"start": result["start"], "step": result["step"], "width": 4, "count": len(words)})

    # Try 16-bit little-endian
    if n >= 4 and n % 2 == 0:
        words = [st.unpack_from("<H", raw, i)[0] for i in range(0, n, 2)]
        result = detect_linear(words)
        if result:
            return (PatternType.LINEAR_WIDE, {"start": result["start"], "step": result["step"], "width": 2, "count": len(words)})

    return None


def _detect_f64(track: List[int]) -> Optional[tuple]:
    """Detect patterns in f64 (double) values: linear sequences or small deltas."""
    import struct as st
    n = len(track)
    raw = bytes(track)

    if n < 16 or n % 8 != 0:
        return None

    count = n // 8
    doubles = [st.unpack_from("<d", raw, i * 8)[0] for i in range(count)]

    # Check for NaN/Inf
    if any(d != d or abs(d) == float('inf') for d in doubles):
        return None

    # Try linear f64: check if step is constant
    if count >= 2:
        step = doubles[1] - doubles[0]
        is_linear = True
        for i in range(2, count):
            if abs((doubles[i] - doubles[i-1]) - step) > abs(step) * 1e-12 + 1e-15:
                is_linear = False
                break
        if is_linear and 16 < n:
            # Verify byte-exact reconstruction
            recon_bytes = b''.join(st.pack('<d', doubles[0] + i * step) for i in range(count))
            if recon_bytes == raw:
                return (PatternType.LINEAR_F64, {"start": doubles[0], "step": step, "count": count})

    # Try delta f64: store first value + deltas as f32 (if precision allows)
    deltas = [doubles[i] - doubles[i-1] for i in range(1, count)]
    f32_ok = True
    for d in deltas:
        if abs(d) > 3.4e38:  # f32 max
            f32_ok = False
            break
        f32_val = st.unpack('<f', st.pack('<f', d))[0]
        if abs(f32_val - d) > abs(d) * 1e-6 + 1e-30:
            f32_ok = False
            break

    if f32_ok:
        # Cost: 8 (start f64) + (count-1)*4 (f32 deltas) vs raw n bytes
        cost = 8 + (count - 1) * 4
        if cost < n:
            # Verify exact reconstruction
            reconstructed = [doubles[0]]
            current = doubles[0]
            for d in deltas:
                f32_d = st.unpack('<f', st.pack('<f', d))[0]
                current += f32_d
                reconstructed.append(current)
            # Check byte-exact reconstruction
            orig_bytes = raw
            recon_bytes = b''.join(st.pack('<d', v) for v in reconstructed)
            if orig_bytes == recon_bytes:
                return (PatternType.DELTA_F64, {"start": doubles[0], "deltas_f32": deltas, "count": count})

    return None


def analyze_track(track: List[int], track_idx: int) -> TrackEncoding:
    """Analyze a single track and return its most compact encoding."""
    result = detect_const(track)
    if result:
        return TrackEncoding(PatternType.CONST, result, track_idx)

    result = detect_linear(track)
    if result:
        return TrackEncoding(PatternType.LINEAR, result, track_idx)

    result = detect_rle(track)
    result_repeat = detect_repeat(track)

    # Pick the smaller of RLE vs REPEAT if both match
    if result and result_repeat:
        rle_cost = 2 + len(result["runs"]) * 2
        repeat_cost = 2 + len(result_repeat["pattern"])
        if repeat_cost <= rle_cost:
            return TrackEncoding(PatternType.REPEAT, result_repeat, track_idx)
        else:
            return TrackEncoding(PatternType.RLE, result, track_idx)
    elif result:
        return TrackEncoding(PatternType.RLE, result, track_idx)
    elif result_repeat:
        return TrackEncoding(PatternType.REPEAT, result_repeat, track_idx)

    # Try wide-word analysis (interpret bytes as 16/32/64-bit values)
    result = _detect_wide(track)
    if result:
        return TrackEncoding(result[0], result[1], track_idx)

    result = _detect_f64(track)
    if result:
        return TrackEncoding(result[0], result[1], track_idx)

    result = detect_delta(track)
    if result:
        return TrackEncoding(PatternType.DELTA, result, track_idx)

    result = detect_periodic(track)
    if result:
        return TrackEncoding(PatternType.PERIODIC, result, track_idx)

    result = detect_poly(track)
    if result:
        return TrackEncoding(PatternType.POLY, result, track_idx)

    return TrackEncoding(PatternType.RAW, {"data": track}, track_idx)


def analyze_platter(tracks: List[List[int]]) -> List[TrackEncoding]:
    """Analyze all tracks on a platter."""
    return [analyze_track(track, i) for i, track in enumerate(tracks)]
