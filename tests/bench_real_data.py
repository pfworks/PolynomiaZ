#!/usr/bin/env python3
"""
Benchmark PLTZ against standard compressors on real-world format data.

Run: python3 tests/bench_real_data.py
Requires: tests/data/ populated by tests/generate_real_data.py
"""

import os
import sys
import zlib
import bz2
import lzma
import subprocess

DATA_DIR = os.path.join(os.path.dirname(__file__), "data")
PLTZ = os.path.join(os.path.dirname(__file__), "..", "rust", "target", "release", "pltz")

def compress_pltz(data, extra_args=None):
    """Compress via pltz binary, return size."""
    import tempfile
    with tempfile.NamedTemporaryFile(delete=False, suffix=".bin") as f:
        f.write(data)
        tmp = f.name
    args = [PLTZ, "-kf"]
    if extra_args:
        args.extend(extra_args)
    args.append(tmp)
    subprocess.run(args, capture_output=True)
    out = tmp + ".pltz"
    if os.path.exists(out):
        size = os.path.getsize(out)
        os.unlink(out)
    else:
        size = len(data) + 100  # failed
    os.unlink(tmp)
    return size


def main():
    if not os.path.exists(PLTZ):
        print(f"Build pltz first: cd rust && cargo build --release")
        sys.exit(1)

    if not os.path.exists(DATA_DIR):
        print("Generate test data first: python3 tests/generate_real_data.py")
        sys.exit(1)

    files = sorted(f for f in os.listdir(DATA_DIR) if f.endswith(".bin"))
    if not files:
        print("No .bin files in tests/data/")
        sys.exit(1)

    print("=" * 80)
    print("  Real-World Data Compression Benchmark")
    print("=" * 80)
    print()
    print(f"  {'File':<30} {'Raw':>7} {'PLTZ':>7} {'PLTZ-b':>7} {'zlib':>7} {'lzma':>7} {'Winner':<8} {'vs zlib'}")
    print(f"  {'-'*30} {'-'*7} {'-'*7} {'-'*7} {'-'*7} {'-'*7} {'-'*8} {'-'*7}")

    total_raw = 0
    total_pltz_b = 0
    total_zlib = 0
    total_lzma = 0

    for fname in files:
        path = os.path.join(DATA_DIR, fname)
        data = open(path, "rb").read()
        raw = len(data)

        pltz_size = compress_pltz(data)
        pltz_best = compress_pltz(data, ["-b"])
        zlib_size = len(zlib.compress(data, 9))
        lzma_size = len(lzma.compress(data))

        results = {"PLTZ": pltz_size, "PLTZ-b": pltz_best, "zlib": zlib_size, "lzma": lzma_size}
        winner = min(results, key=results.get)
        best_size = results[winner]

        vs_zlib = f"{zlib_size/pltz_best:.1f}x" if pltz_best < zlib_size else f"{pltz_best/zlib_size:.1f}x worse"

        print(f"  {fname:<30} {raw:>6}B {pltz_size:>6}B {pltz_best:>6}B {zlib_size:>6}B {lzma_size:>6}B {winner:<8} {vs_zlib}")

        total_raw += raw
        total_pltz_b += pltz_best
        total_zlib += zlib_size
        total_lzma += lzma_size

    print(f"  {'-'*30} {'-'*7} {'-'*7} {'-'*7} {'-'*7} {'-'*7}")
    print(f"  {'TOTAL':<30} {total_raw:>6}B {'':>7} {total_pltz_b:>6}B {total_zlib:>6}B {total_lzma:>6}B")
    print()
    print(f"  PLTZ-b total: {total_pltz_b}B ({total_raw/total_pltz_b:.1f}:1)")
    print(f"  zlib total:   {total_zlib}B ({total_raw/total_zlib:.1f}:1)")
    print(f"  lzma total:   {total_lzma}B ({total_raw/total_lzma:.1f}:1)")


if __name__ == "__main__":
    main()
