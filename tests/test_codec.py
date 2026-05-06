"""
Test the polar compression codec for lossless round-trip.
"""

import os
import sys
import random

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "lib"))

from codec import encode, decode


def test_roundtrip(data: bytes, label: str) -> bool:
    compressed = encode(data)
    decompressed = decode(compressed)
    ratio = len(compressed) / len(data) if len(data) > 0 else 0
    status = "✓" if decompressed == data else "✗ MISMATCH"
    print(f"  {status} {label}: {len(data)}B → {len(compressed)}B (ratio: {ratio:.2f})")
    if decompressed != data:
        for i, (a, b) in enumerate(zip(data, decompressed)):
            if a != b:
                print(f"    First diff at byte {i}: expected {a}, got {b}")
                break
        if len(decompressed) != len(data):
            print(f"    Length mismatch: expected {len(data)}, got {len(decompressed)}")
        return False
    return True


def main():
    all_pass = True

    print("=== Basic patterns ===")
    all_pass &= test_roundtrip(b"\x00" * 1024, "all zeros (1KB)")
    all_pass &= test_roundtrip(b"\xFF" * 4096, "all 0xFF (4KB)")
    all_pass &= test_roundtrip(b"\xAB\xCD" * 512, "repeating 2-byte")
    all_pass &= test_roundtrip(b"\x01\x02\x03\x04" * 256, "repeating 4-byte")
    all_pass &= test_roundtrip(bytes(range(256)) * 4, "linear ramp 0-255")
    all_pass &= test_roundtrip(bytes(range(0, 200, 2)) * 10, "linear ramp step=2")

    print("\n=== RLE-friendly data ===")
    rle_data = b"\x00" * 50 + b"\xFF" * 50 + b"\x42" * 50 + b"\x13" * 106
    all_pass &= test_roundtrip(rle_data, "block runs")

    print("\n=== Delta-friendly data ===")
    # Slowly varying data
    delta_data = bytes([128 + (i % 5) - 2 for i in range(512)])
    all_pass &= test_roundtrip(delta_data, "small deltas")

    print("\n=== Periodic data ===")
    import math
    periodic = bytes([int(128 + 100 * math.sin(2 * math.pi * i / 32)) for i in range(256)])
    all_pass &= test_roundtrip(periodic, "sine wave period=32")

    print("\n=== Random / incompressible ===")
    random.seed(42)
    random_data = bytes(random.randint(0, 255) for _ in range(1000))
    all_pass &= test_roundtrip(random_data, "random 1KB")
    random_data_4k = bytes(random.randint(0, 255) for _ in range(4096))
    all_pass &= test_roundtrip(random_data_4k, "random 4KB")

    print("\n=== Edge cases ===")
    all_pass &= test_roundtrip(b"", "empty")
    all_pass &= test_roundtrip(b"\x42", "single byte")
    all_pass &= test_roundtrip(b"Hello, Platter!", "short string")
    all_pass &= test_roundtrip(bytes(range(256)), "exactly 256 bytes")

    print()
    if all_pass:
        print("ALL PASSED ✓")
    else:
        print("SOME TESTS FAILED ✗")
    return 0 if all_pass else 1


if __name__ == "__main__":
    sys.exit(main())
