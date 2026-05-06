"""
Benchmark: compare PolynomiaZ against standard compression algorithms.
Also tests configurable raw fallback compressors.
"""

import os
import sys
import zlib
import bz2
import lzma
import random
import math
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "lib"))

from codec import encode, decode, set_raw_compressor


def size_str(n):
    if n < 1024:
        return f"{n}B"
    return f"{n/1024:.1f}KB"


def ratio_str(compressed, original):
    if original == 0:
        return "—"
    return f"{compressed/original:.3f}"


def bench_all(data: bytes, label: str):
    """Compress with all methods and print comparison."""
    results = {}

    # PolynomiaZ (no raw compressor)
    set_raw_compressor(None)
    t0 = time.time()
    pltz = encode(data)
    t_pltz = time.time() - t0
    assert decode(pltz) == data, "PLTZ lossless check failed!"
    results["PLTZ"] = (len(pltz), t_pltz)

    # PolynomiaZ + zlib raw fallback
    set_raw_compressor("zlib")
    t0 = time.time()
    pltz_z = encode(data)
    t_pltz_z = time.time() - t0
    assert decode(pltz_z) == data, "PLTZ+zlib lossless check failed!"
    results["PLTZ+zlib"] = (len(pltz_z), t_pltz_z)

    # PolynomiaZ + bz2 raw fallback
    set_raw_compressor("bz2")
    t0 = time.time()
    pltz_bz2 = encode(data)
    t_pltz_bz2 = time.time() - t0
    assert decode(pltz_bz2) == data, "PLTZ+bz2 lossless check failed!"
    results["PLTZ+bz2"] = (len(pltz_bz2), t_pltz_bz2)

    # Reset
    set_raw_compressor(None)

    # zlib
    t0 = time.time()
    z = zlib.compress(data, 9)
    t_z = time.time() - t0
    results["zlib"] = (len(z), t_z)

    # bz2
    t0 = time.time()
    b = bz2.compress(data, 9)
    t_b = time.time() - t0
    results["bz2"] = (len(b), t_b)

    # lzma
    t0 = time.time()
    l = lzma.compress(data)
    t_l = time.time() - t0
    results["lzma"] = (len(l), t_l)

    # Print
    orig = len(data)
    print(f"\n{'─'*70}")
    print(f"  {label} ({size_str(orig)})")
    print(f"  {'Method':<12} {'Size':>8} {'Ratio':>8} {'Time':>10}")
    print(f"  {'─'*40}")

    for name in ["PLTZ", "PLTZ+zlib", "PLTZ+bz2", "zlib", "bz2", "lzma"]:
        sz, t = results[name]
        print(f"  {name:<12} {size_str(sz):>8} {ratio_str(sz, orig):>8} {t*1000:>8.1f}ms")

    # Find winner
    winner = min(results, key=lambda k: results[k][0])
    print(f"  → Winner: {winner} ({size_str(results[winner][0])})")


def main():
    print("=" * 70)
    print("  PolynomiaZ Compression Benchmark")
    print("=" * 70)

    # Structured data (PLTZ should shine)
    bench_all(b"\x00" * 4096, "All zeros 4KB")
    bench_all(b"\xAB\xCD\xEF" * 1000, "Repeating 3-byte pattern 3KB")
    bench_all(bytes(range(256)) * 16, "Linear ramp 4KB")

    # RLE-friendly
    rle_data = b""
    for v in range(20):
        rle_data += bytes([v * 12]) * 200
    bench_all(rle_data, "Block runs 4KB")

    # Periodic
    periodic = bytes([int(128 + 100 * math.sin(2 * math.pi * i / 64)) for i in range(4096)])
    bench_all(periodic, "Sine wave 4KB")

    # Mixed: some structure, some random
    mixed = bytes(range(256)) * 8 + bytes(random.randint(0, 255) for _ in range(2048))
    bench_all(mixed, "Mixed structured+random 4KB")

    # Pure random (incompressible)
    random.seed(42)
    bench_all(bytes(random.randint(0, 255) for _ in range(4096)), "Random 4KB")

    # Text-like (English text has patterns)
    text = (b"The quick brown fox jumps over the lazy dog. " * 100)[:4096]
    bench_all(text, "English text 4KB")

    # Binary with some structure (like an executable)
    binary = bytes([(i * 7 + 13) & 0xFF for i in range(2048)]) + \
             bytes(random.randint(0, 255) for _ in range(2048))
    bench_all(binary, "Pseudo-binary 4KB")

    print(f"\n{'='*70}")
    print("  Done.")


if __name__ == "__main__":
    main()
