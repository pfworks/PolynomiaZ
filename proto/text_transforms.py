"""
Text transforms: reorder/transform text data to expose patterns for PLTZ compression.
"""

import sys
import os
sys.path.insert(0, os.path.dirname(__file__))

from codec import encode, decode, set_raw_compressor


def transform_columnar(data: bytes, stride: int) -> bytes:
    """Transpose data: read sequentially, write in columns.
    Groups every Nth byte together on the same track."""
    n = len(data)
    rows = (n + stride - 1) // stride
    # Pad to fill the matrix
    padded = data + b'\x00' * (rows * stride - n)
    # Transpose: read column by column
    out = bytearray(rows * stride)
    for col in range(stride):
        for row in range(rows):
            out[col * rows + row] = padded[row * stride + col]
    return bytes(out)


def untransform_columnar(transformed: bytes, stride: int, original_len: int) -> bytes:
    """Inverse of columnar transform."""
    rows = (original_len + stride - 1) // stride
    out = bytearray(rows * stride)
    for col in range(stride):
        for row in range(rows):
            out[row * stride + col] = transformed[col * rows + row]
    return bytes(out[:original_len])


def transform_delta(data: bytes) -> bytes:
    """Store each byte as delta from previous. First byte stored as-is."""
    if len(data) == 0:
        return b''
    out = bytearray(len(data))
    out[0] = data[0]
    for i in range(1, len(data)):
        out[i] = (data[i] - data[i-1]) & 0xFF
    return bytes(out)


def untransform_delta(transformed: bytes) -> bytes:
    """Inverse of delta transform."""
    if len(transformed) == 0:
        return b''
    out = bytearray(len(transformed))
    out[0] = transformed[0]
    for i in range(1, len(transformed)):
        out[i] = (out[i-1] + transformed[i]) & 0xFF
    return bytes(out)


def transform_bwt(data: bytes) -> tuple:
    """Burrows-Wheeler Transform. Returns (transformed, index)."""
    n = len(data)
    # Add sentinel
    text = data + b'\x00'
    n1 = n + 1
    # Build suffix array (simple O(n log n) for prototype)
    indices = sorted(range(n1), key=lambda i: text[i:] + text[:i])
    transformed = bytes(text[(i - 1) % n1] for i in indices)
    original_idx = indices.index(0)
    return transformed, original_idx


def untransform_bwt(transformed: bytes, idx: int) -> bytes:
    """Inverse BWT."""
    n = len(transformed)
    # Build the T-transform
    table = sorted(range(n), key=lambda i: transformed[i])
    result = bytearray(n)
    j = idx
    for i in range(n):
        j = table[j]
        result[i] = transformed[j]
    return bytes(result[:-1])  # remove sentinel


def test_transforms():
    """Test all transforms + PLTZ compression on English text."""
    import zlib

    text = (b"The quick brown fox jumps over the lazy dog. " * 200)[:4096]
    print(f"Original: {len(text)}B")
    print(f"zlib:     {len(zlib.compress(text, 9))}B")
    print()

    # Raw PLTZ
    set_raw_compressor(None)
    c = encode(text)
    assert decode(c) == text
    print(f"PLTZ raw:           {len(c)}B  (ratio {len(c)/len(text):.3f})")

    set_raw_compressor("zlib")
    c = encode(text)
    assert decode(c) == text
    print(f"PLTZ+zlib:          {len(c)}B  (ratio {len(c)/len(text):.3f})")

    # Columnar + PLTZ
    for stride in [5, 8, 16, 45]:
        set_raw_compressor(None)
        t = transform_columnar(text, stride)
        c = encode(t)
        d = decode(c)
        r = untransform_columnar(d, stride, len(text))
        assert r == text, f"Columnar stride={stride} failed!"
        print(f"Columnar(s={stride:>2})+PLTZ: {len(c)}B  (ratio {len(c)/len(text):.3f})")

    # Columnar + PLTZ+zlib
    for stride in [5, 8, 45]:
        set_raw_compressor("zlib")
        t = transform_columnar(text, stride)
        c = encode(t)
        d = decode(c)
        r = untransform_columnar(d, stride, len(text))
        assert r == text, f"Columnar+zlib stride={stride} failed!"
        print(f"Columnar(s={stride:>2})+PLTZ+z: {len(c)}B  (ratio {len(c)/len(text):.3f})")

    # Delta + PLTZ
    set_raw_compressor(None)
    t = transform_delta(text)
    c = encode(t)
    d = decode(c)
    r = untransform_delta(d)
    assert r == text, "Delta failed!"
    print(f"Delta+PLTZ:         {len(c)}B  (ratio {len(c)/len(text):.3f})")

    set_raw_compressor("zlib")
    t = transform_delta(text)
    c = encode(t)
    d = decode(c)
    r = untransform_delta(d)
    assert r == text, "Delta+zlib failed!"
    print(f"Delta+PLTZ+zlib:    {len(c)}B  (ratio {len(c)/len(text):.3f})")

    # BWT + PLTZ
    set_raw_compressor(None)
    t, idx = transform_bwt(text)
    c = encode(t)
    d = decode(c)
    r = untransform_bwt(d, idx)
    assert r == text, "BWT failed!"
    print(f"BWT+PLTZ:           {len(c)}B  (ratio {len(c)/len(text):.3f})")

    set_raw_compressor("zlib")
    t, idx = transform_bwt(text)
    c = encode(t)
    d = decode(c)
    r = untransform_bwt(d, idx)
    assert r == text, "BWT+zlib failed!"
    print(f"BWT+PLTZ+zlib:      {len(c)}B  (ratio {len(c)/len(text):.3f})")


if __name__ == "__main__":
    test_transforms()
