"""
Simulate a realistic U-Boot firmware image and benchmark PLTZ vs standard compressors.

Typical U-Boot image structure (512KB):
- Vector table (256B): sequential addresses, stride=4
- Reset/init code (4KB): pseudo-random machine code
- PLL/clock config table (512B): arithmetic sequences
- DDR timing table (1KB): structured calibration values
- Relocation table (2KB): base + incrementing offsets
- CRC32 lookup table (1KB): polynomial-generated
- Main code (64KB): machine code (pseudo-random)
- String table (8KB): help text, error messages
- Environment (8KB): key=value + 0x00 padding
- Device tree (16KB): structured headers + padding
- Padding to 512KB: 0xFF fill
"""

import sys
import os
import random
import struct
import zlib
import bz2
import lzma
import math

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "lib"))
from codec import encode, decode, set_raw_compressor


def generate_uboot_image(size=512 * 1024):
    """Generate a synthetic U-Boot image with realistic structure."""
    random.seed(0xDEADBEEF)
    image = bytearray()

    # 1. Interrupt vector table (256B) - sequential addresses at stride 4
    base_addr = 0x80000000
    for i in range(64):
        image += struct.pack("<I", base_addr + i * 0x20)

    # 2. Reset/init code (4KB) - looks like machine code
    for _ in range(4096):
        image.append(random.randint(0, 255))

    # 3. PLL configuration table (512B) - divider ramps
    for i in range(128):
        image += struct.pack("<I", 0x01000000 + i * 0x100)

    # 4. DDR timing table (1KB) - calibration staircase
    for phase in range(16):
        for step in range(64):
            image.append(phase * 16 + step // 4)

    # 5. Relocation table (2KB) - base + incrementing offsets
    reloc_base = 0x80100000
    for i in range(512):
        image += struct.pack("<I", reloc_base + i * 8)

    # 6. CRC32 lookup table (1KB) - polynomial generated
    for i in range(256):
        crc = i
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xEDB88320
            else:
                crc >>= 1
        image += struct.pack("<I", crc)

    # 7. Main bootloader code (64KB) - pseudo-random (machine code)
    for _ in range(65536):
        image.append(random.randint(0, 255))

    # 8. String table (8KB) - ASCII text with null terminators
    strings = [
        b"Usage: bootm [addr [arg ...]]\n",
        b"    - boot application image stored in memory\n",
        b"Error: bad magic number\n",
        b"Loading kernel from 0x%08x to 0x%08x\n",
        b"Starting kernel ...\n\n",
        b"DRAM:  %d MiB\n",
        b"Flash: %d MiB\n",
        b"Net:   %s\n",
        b"Hit any key to stop autoboot: %d\n",
        b"Unknown command '%s' - try 'help'\n",
    ]
    str_section = bytearray()
    while len(str_section) < 8192:
        s = random.choice(strings)
        str_section += s + b'\x00'
    image += str_section[:8192]

    # 9. Environment block (8KB) - key=value pairs + 0x00 fill
    env = bytearray()
    env_vars = [
        b"bootcmd=run distro_bootcmd",
        b"bootdelay=2",
        b"baudrate=115200",
        b"ipaddr=192.168.1.100",
        b"serverip=192.168.1.1",
        b"netmask=255.255.255.0",
        b"ethaddr=00:11:22:33:44:55",
        b"bootargs=console=ttyS0,115200 root=/dev/mmcblk0p2 rootwait",
        b"loadaddr=0x82000000",
        b"kernel_addr_r=0x82000000",
        b"fdt_addr_r=0x88000000",
        b"ramdisk_addr_r=0x88080000",
    ]
    for var in env_vars:
        env += var + b'\x00'
    env += b'\x00' * (8192 - len(env))
    image += env

    # 10. Device tree blob (16KB) - structured with padding
    dtb = bytearray()
    # FDT header
    dtb += struct.pack(">IIIIIIIII", 
        0xD00DFEED,  # magic
        16384,       # totalsize
        0x38,        # off_dt_struct
        0x3000,      # off_dt_strings
        0x28,        # off_mem_rsvmap
        17,          # version
        16,          # last_comp_version
        0,           # boot_cpuid_phys
        0x1000,      # size_dt_strings
    )
    # Padding + structured nodes (property headers repeat)
    while len(dtb) < 12288:
        # FDT_PROP token + len + nameoff pattern
        dtb += struct.pack(">III", 0x00000003, 4, len(dtb) % 256)
        dtb += struct.pack(">I", random.randint(0, 0xFFFFFFFF))
    dtb += b'\x00' * (16384 - len(dtb))
    image += dtb

    # 11. Fill remainder with 0xFF (erased flash)
    remaining = size - len(image)
    if remaining > 0:
        image += b'\xFF' * remaining

    return bytes(image[:size])


def main():
    print("=" * 70)
    print("  U-Boot Firmware Image Compression Benchmark")
    print("=" * 70)

    image = generate_uboot_image()
    print(f"\n  Synthetic U-Boot image: {len(image)} bytes ({len(image)//1024} KB)")

    # Analyze structure
    sections = [
        ("Vector table", 0, 256),
        ("Init code", 256, 4096),
        ("PLL config", 256+4096, 512),
        ("DDR timing", 256+4096+512, 1024),
        ("Relocation table", 256+4096+512+1024, 2048),
        ("CRC32 table", 256+4096+512+1024+2048, 1024),
        ("Main code", 256+4096+512+1024+2048+1024, 65536),
        ("Strings", 256+4096+512+1024+2048+1024+65536, 8192),
        ("Environment", 256+4096+512+1024+2048+1024+65536+8192, 8192),
        ("Device tree", 256+4096+512+1024+2048+1024+65536+8192+8192, 16384),
        ("0xFF padding", 256+4096+512+1024+2048+1024+65536+8192+8192+16384, 
         512*1024 - (256+4096+512+1024+2048+1024+65536+8192+8192+16384)),
    ]

    print(f"\n  Section breakdown:")
    for name, offset, size in sections:
        pct = size / len(image) * 100
        print(f"    {name:<20} {size:>7}B  ({pct:>5.1f}%)")

    # Compress full image
    print(f"\n  {'Method':<20} {'Size':>10} {'Ratio':>8} {'Savings':>10}")
    print(f"  {'─'*50}")

    # zlib
    z = zlib.compress(image, 9)
    print(f"  {'zlib':<20} {len(z):>10}B {len(z)/len(image):>8.4f} {len(image)-len(z):>9}B saved")

    # bz2
    b = bz2.compress(image, 9)
    print(f"  {'bz2':<20} {len(b):>10}B {len(b)/len(image):>8.4f} {len(image)-len(b):>9}B saved")

    # lzma
    l = lzma.compress(image)
    print(f"  {'lzma':<20} {len(l):>10}B {len(l)/len(image):>8.4f} {len(image)-len(l):>9}B saved")

    # PLTZ (no fallback)
    set_raw_compressor(None)
    p = encode(image)
    assert decode(p) == image, "PLTZ lossless check failed!"
    print(f"  {'PLTZ':<20} {len(p):>10}B {len(p)/len(image):>8.4f} {len(image)-len(p):>9}B saved")

    # PLTZ + zlib fallback
    set_raw_compressor("zlib")
    pz = encode(image)
    assert decode(pz) == image, "PLTZ+zlib lossless check failed!"
    print(f"  {'PLTZ+zlib':<20} {len(pz):>10}B {len(pz)/len(image):>8.4f} {len(image)-len(pz):>9}B saved")

    # PLTZ + bz2 fallback
    set_raw_compressor("bz2")
    pb = encode(image)
    assert decode(pb) == image, "PLTZ+bz2 lossless check failed!"
    print(f"  {'PLTZ+bz2':<20} {len(pb):>10}B {len(pb)/len(image):>8.4f} {len(image)-len(pb):>9}B saved")

    # Per-section analysis
    print(f"\n  Per-section compression (PLTZ vs zlib):")
    print(f"  {'Section':<20} {'Raw':>7} {'PLTZ':>7} {'zlib':>7} {'Winner':>8}")
    print(f"  {'─'*55}")

    set_raw_compressor(None)
    for name, offset, size in sections:
        section = image[offset:offset+size]
        pc = encode(section)
        zc = zlib.compress(section, 9)
        winner = "PLTZ" if len(pc) <= len(zc) else "zlib"
        flag = " ★" if winner == "PLTZ" else ""
        print(f"  {name:<20} {size:>6}B {len(pc):>6}B {len(zc):>6}B {winner:>6}{flag}")

    print(f"\n{'='*70}")
    print(f"  Summary:")
    results = {"zlib": len(z), "bz2": len(b), "lzma": len(l), 
               "PLTZ": len(p), "PLTZ+zlib": len(pz), "PLTZ+bz2": len(pb)}
    winner = min(results, key=results.get)
    print(f"  Best overall: {winner} ({results[winner]}B, {results[winner]/len(image):.1%} of original)")
    print(f"  PLTZ+zlib vs zlib alone: {len(pz)}B vs {len(z)}B ", end="")
    if len(pz) < len(z):
        print(f"(PLTZ+zlib wins by {len(z)-len(pz)}B)")
    else:
        print(f"(zlib wins by {len(pz)-len(z)}B)")


if __name__ == "__main__":
    main()
