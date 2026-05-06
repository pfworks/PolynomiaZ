"""
Benchmark: simulate SPICE kernel data structures and test PLTZ compression.

Simulates:
- SPK Type 2 (Chebyshev polynomials for ephemeris)
- CK (quaternion pointing data)
- PCK (orientation polynomial coefficients)
- DAF file structure (headers, summaries, arrays)
"""

import sys
import os
import struct
import math
import random
import zlib
import bz2
import lzma

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "lib"))
from codec import encode, decode, set_raw_compressor


def pack_doubles(values):
    """Pack a list of floats as little-endian f64."""
    return b''.join(struct.pack('<d', v) for v in values)


def generate_spk_type2(num_records=100, degree=13):
    """
    Simulate SPK Type 2: Chebyshev polynomial coefficients.
    Each record has 3 components (X, Y, Z) × (degree+1) coefficients + 2 epoch values.
    Coefficients vary smoothly between adjacent records (planetary motion is continuous).
    """
    random.seed(12345)
    data = bytearray()

    # Base coefficients (simulating Earth's orbit around Sun)
    base_x = [1.496e11 * (0.9 + 0.1 * random.gauss(0, 1)) for _ in range(degree + 1)]
    base_y = [1.496e11 * (0.9 + 0.1 * random.gauss(0, 1)) for _ in range(degree + 1)]
    base_z = [1.496e10 * random.gauss(0, 1) for _ in range(degree + 1)]

    for rec in range(num_records):
        # Epoch boundaries (linearly spaced)
        t_start = 0.0 + rec * 86400.0  # seconds past J2000
        t_end = t_start + 86400.0
        data += pack_doubles([t_start, t_end])

        # Chebyshev coefficients drift slowly between records
        drift = 0.001
        for i in range(degree + 1):
            base_x[i] *= (1.0 + drift * random.gauss(0, 1))
            base_y[i] *= (1.0 + drift * random.gauss(0, 1))
            base_z[i] *= (1.0 + drift * random.gauss(0, 1))

        data += pack_doubles(base_x)
        data += pack_doubles(base_y)
        data += pack_doubles(base_z)

    return bytes(data)


def generate_ck_data(num_records=500):
    """
    Simulate CK (C-kernel): quaternion pointing data.
    During stable pointing, quaternions are near-constant.
    During slews, they change smoothly.
    """
    random.seed(54321)
    data = bytearray()

    # Start with a stable quaternion
    q = [0.0, 0.0, 0.0, 1.0]  # identity

    for rec in range(num_records):
        # Epoch (linearly increasing)
        epoch = 1000000.0 + rec * 10.0
        data += struct.pack('<d', epoch)

        # Small perturbation (stable pointing with jitter)
        if random.random() < 0.05:
            # Slew: larger change
            for i in range(4):
                q[i] += random.gauss(0, 0.01)
        else:
            # Stable: tiny jitter
            for i in range(4):
                q[i] += random.gauss(0, 0.0001)

        # Normalize
        norm = math.sqrt(sum(x*x for x in q))
        q = [x / norm for x in q]

        data += pack_doubles(q)

        # Angular velocity (near zero during stable)
        av = [random.gauss(0, 0.0001) for _ in range(3)]
        data += pack_doubles(av)

    return bytes(data)


def generate_pck_data():
    """
    Simulate PCK (planetary constants): orientation polynomials.
    RA, DEC, W for several bodies, each as polynomial coefficients.
    """
    data = bytearray()

    # Simulate 20 bodies, each with RA(3 coeffs), DEC(3 coeffs), W(3 coeffs)
    bodies = [
        (268.057, -0.009, 0.0, 64.495, 0.003, 0.0, 38.317, 13.1764, 0.0),   # Mars-like
        (281.01, -0.033, 0.0, 61.45, -0.005, 0.0, 329.548, 6.1385, 0.0),     # Jupiter-like
        (40.589, -0.036, 0.0, 83.537, -0.004, 0.0, 38.90, -26.73, 0.0),      # Saturn-like
    ]

    for _ in range(20):
        body = random.choice(bodies)
        # Add small variations
        coeffs = [c + random.gauss(0, 0.001) for c in body]
        data += pack_doubles(coeffs)

    return bytes(data)


def generate_daf_structure(payload_size=65536):
    """
    Simulate DAF (Double precision Array File) structure.
    Header + summary records + data arrays.
    """
    data = bytearray()

    # DAF file record (1024 bytes)
    # Architecture string
    data += b'DAF/SPK\x00' + b'\x00' * 56  # 64 bytes ID
    # ND, NI (number of double/int components in summary)
    data += struct.pack('<II', 2, 6)
    # Internal file name
    data += b'test_kernel.bsp\x00' + b'\x00' * 48  # 64 bytes
    # First/last summary record, first free address
    data += struct.pack('<III', 2, 2, payload_size // 8 + 100)
    # Pad to 1024
    data += b'\x00' * (1024 - len(data))

    # Summary record (1024 bytes)
    # Contains segment descriptors: start_epoch, end_epoch, target, center, frame, type, start_addr, end_addr
    num_segments = 10
    summary = bytearray()
    summary += struct.pack('<d', float(num_segments))  # next/prev pointers
    summary += struct.pack('<d', 0.0)
    summary += struct.pack('<d', float(num_segments))

    for seg in range(num_segments):
        # Double components: start_epoch, end_epoch
        summary += struct.pack('<dd', seg * 86400.0, (seg + 1) * 86400.0)
        # Integer components: target, center, frame, type, start_i, end_i
        start_i = 100 + seg * (payload_size // (num_segments * 8))
        end_i = start_i + payload_size // (num_segments * 8) - 1
        summary += struct.pack('<iiiiii', 399, 10, 1, 2, start_i, end_i)

    summary += b'\x00' * (1024 - len(summary))
    data += summary

    # Data arrays (the actual ephemeris data - use SPK-like content)
    data += generate_spk_type2(num_records=payload_size // 3000)

    return bytes(data)


def bench(data, label):
    """Compress with all methods and compare."""
    set_raw_compressor(None)
    p = encode(data)
    assert decode(p) == data, f"PLTZ lossless failed on {label}"

    set_raw_compressor("zlib")
    pz = encode(data)
    assert decode(pz) == data, f"PLTZ+zlib lossless failed on {label}"

    z = zlib.compress(data, 9)
    b = bz2.compress(data, 9)
    l = lzma.compress(data)

    print(f"\n{'─'*60}")
    print(f"  {label} ({len(data)} bytes)")
    print(f"  {'Method':<15} {'Size':>8} {'Ratio':>8} {'vs zlib':>10}")
    print(f"  {'─'*45}")
    print(f"  {'zlib':<15} {len(z):>7}B {len(z)/len(data):>8.3f} {'baseline':>10}")
    print(f"  {'bz2':<15} {len(b):>7}B {len(b)/len(data):>8.3f} {len(b)-len(z):>+9}B")
    print(f"  {'lzma':<15} {len(l):>7}B {len(l)/len(data):>8.3f} {len(l)-len(z):>+9}B")
    print(f"  {'PLTZ':<15} {len(p):>7}B {len(p)/len(data):>8.3f} {len(p)-len(z):>+9}B")
    print(f"  {'PLTZ+zlib':<15} {len(pz):>7}B {len(pz)/len(data):>8.3f} {len(pz)-len(z):>+9}B")

    winner = min([("zlib", len(z)), ("bz2", len(b)), ("lzma", len(l)),
                  ("PLTZ", len(p)), ("PLTZ+zlib", len(pz))], key=lambda x: x[1])
    print(f"  → Winner: {winner[0]} ({winner[1]}B)")
    return winner[0]


def main():
    print("=" * 60)
    print("  SPICE Kernel Compression Benchmark")
    print("=" * 60)

    # SPK Type 2 - Chebyshev coefficients
    spk = generate_spk_type2(num_records=200, degree=13)
    bench(spk, "SPK Type 2 (Chebyshev ephemeris)")

    # CK - Quaternion pointing
    ck = generate_ck_data(num_records=1000)
    bench(ck, "CK (quaternion pointing)")

    # PCK - Orientation polynomials
    pck = generate_pck_data()
    bench(pck, "PCK (orientation constants)")

    # Full DAF file structure
    daf = generate_daf_structure(payload_size=65536)
    bench(daf, "DAF file (headers + SPK payload)")

    # Linear epoch arrays (common in SPICE - evenly spaced time tags)
    epochs = pack_doubles([1000000.0 + i * 60.0 for i in range(1024)])
    bench(epochs, "Linear epoch array (1024 times)")

    # Constant quaternion (stable pointing for long duration)
    stable = pack_doubles([0.0, 0.0, 0.0, 1.0] * 256)
    bench(stable, "Constant quaternion (stable pointing)")

    print(f"\n{'='*60}")
    print("  Done.")


if __name__ == "__main__":
    main()
