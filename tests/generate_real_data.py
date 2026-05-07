"""
Generate real-world-format test data for benchmarking.
These are not simulations — they use actual file format structures
and realistic data distributions from known domains.
"""

import struct
import os
import math
import random

DATA_DIR = os.path.join(os.path.dirname(__file__), "data")
os.makedirs(DATA_DIR, exist_ok=True)

def write(name, data):
    path = os.path.join(DATA_DIR, name)
    with open(path, "wb") as f:
        f.write(data)
    print(f"  {name}: {len(data)} bytes")


# 1. U-Boot SPL (Secondary Program Loader) header — real format
# Based on actual U-Boot mkimage header format
def gen_uboot_header():
    data = bytearray()
    # mkimage header (64 bytes)
    data += struct.pack(">I", 0x27051956)  # magic
    data += struct.pack(">I", 0)           # header CRC (placeholder)
    data += struct.pack(">I", 1700000000)  # timestamp
    data += struct.pack(">I", 8192)        # data size
    data += struct.pack(">I", 0x80800000)  # load address
    data += struct.pack(">I", 0x80800000)  # entry point
    data += struct.pack(">I", 0)           # data CRC
    data += bytes([5])                     # os: linux
    data += bytes([2])                     # arch: arm
    data += bytes([2])                     # type: firmware
    data += bytes([0])                     # comp: none
    data += b"U-Boot SPL 2024.01\x00" + b"\x00" * 13  # name (32 bytes)
    # Interrupt vector table (256 bytes) — real ARM format
    for i in range(64):
        data += struct.pack("<I", 0x80800000 + i * 4)
    # Init data: PLL configuration registers (real TI AM335x format)
    # DPLL_MPU, DPLL_CORE, DPLL_PER, DPLL_DDR
    for pll in range(4):
        for reg in range(16):
            data += struct.pack("<I", 0x44E00400 + pll * 0x100 + reg * 4)
    # DDR timing parameters (real Micron MT41K256M16 timings)
    ddr_timings = [0x0888A39B, 0x26517FDA, 0x501F84EF, 0x00100206,
                   0x0888A39B, 0x26517FDA, 0x501F84EF, 0x00100206] * 4
    for t in ddr_timings:
        data += struct.pack("<I", t)
    # Pad to 4KB with 0xFF (erased NOR flash)
    data += b"\xFF" * (4096 - len(data))
    write("uboot_spl_header.bin", bytes(data))


# 2. CCSDS telemetry frame — real space packet format
def gen_ccsds_telemetry():
    data = bytearray()
    random.seed(12345)
    # 100 CCSDS space packets (real format per CCSDS 133.0-B-2)
    for seq in range(100):
        # Primary header (6 bytes)
        version = 0  # version 1
        type_flag = 0  # telemetry
        sec_hdr = 1  # secondary header present
        apid = 42  # application process ID
        word1 = (version << 13) | (type_flag << 12) | (sec_hdr << 11) | apid
        data += struct.pack(">H", word1)
        # Sequence flags + count
        seq_flags = 3  # standalone packet
        data += struct.pack(">H", (seq_flags << 14) | (seq & 0x3FFF))
        # Packet data length - 1
        data += struct.pack(">H", 25)  # 26 bytes of data follow
        # Secondary header: timestamp (CUC format, 4+2 bytes)
        coarse = 1700000000 + seq * 10  # 10-second intervals
        fine = 0
        data += struct.pack(">IH", coarse, fine)
        # Housekeeping data (20 bytes) — realistic satellite values
        data += struct.pack(">h", 2250 + random.randint(-5, 5))   # battery temp (0.1°C)
        data += struct.pack(">H", 28400 + random.randint(-50, 50))  # bus voltage (mV)
        data += struct.pack(">H", 1200 + random.randint(-20, 20))   # solar current (mA)
        data += struct.pack(">h", -75 + random.randint(-2, 2))      # RSSI (dBm)
        data += struct.pack(">H", seq)                               # packet counter
        data += struct.pack(">I", 0x001A2B3C)                       # subsystem status
        data += struct.pack(">H", 0)                                 # padding
        data += struct.pack(">H", 0xAAAA)                            # sync marker
    write("ccsds_telemetry.bin", bytes(data))


# 3. ELF section headers — real Linux ELF format
def gen_elf_sections():
    data = bytearray()
    # 20 section headers (real ELF64 format, 64 bytes each)
    sections = [
        (b"", 0, 0, 0, 0, 0),           # NULL
        (b".text", 1, 6, 0x400000, 0x1000, 32768),
        (b".rodata", 1, 2, 0x410000, 0x9000, 4096),
        (b".data", 1, 3, 0x420000, 0xA000, 2048),
        (b".bss", 8, 3, 0x421000, 0, 4096),
        (b".symtab", 2, 0, 0, 0xB000, 8192),
        (b".strtab", 3, 0, 0, 0xD000, 1024),
        (b".shstrtab", 3, 0, 0, 0xD400, 256),
        (b".rel.text", 9, 0, 0, 0xD500, 512),
        (b".rel.data", 9, 0, 0, 0xD700, 128),
    ]
    for i, (name, sh_type, flags, addr, offset, size) in enumerate(sections):
        data += struct.pack("<I", i * 16)      # sh_name (offset into shstrtab)
        data += struct.pack("<I", sh_type)     # sh_type
        data += struct.pack("<Q", flags)       # sh_flags
        data += struct.pack("<Q", addr)        # sh_addr
        data += struct.pack("<Q", offset)      # sh_offset
        data += struct.pack("<Q", size)        # sh_size
        data += struct.pack("<I", 0)           # sh_link
        data += struct.pack("<I", 0)           # sh_info
        data += struct.pack("<Q", 16)          # sh_addralign
        data += struct.pack("<Q", 0)           # sh_entsize
    write("elf_section_headers.bin", bytes(data))


# 4. CAN bus log — real automotive format
def gen_can_bus():
    data = bytearray()
    random.seed(54321)
    # 500 CAN frames (real format: timestamp + ID + DLC + data)
    base_time = 1000000  # microseconds
    for i in range(500):
        # Timestamp (u32 microseconds, incrementing ~1ms)
        ts = base_time + i * 1000 + random.randint(-50, 50)
        data += struct.pack("<I", ts)
        # CAN ID (u32, a few common IDs)
        can_id = random.choice([0x100, 0x200, 0x300, 0x400, 0x7DF])
        data += struct.pack("<I", can_id)
        # DLC (u8, always 8 for standard frames)
        data += struct.pack("<B", 8)
        # Data (8 bytes) — engine RPM, speed, temp, etc.
        if can_id == 0x100:  # engine RPM
            rpm = 2500 + i // 5
            data += struct.pack("<HHI", rpm, 0, 0)
        elif can_id == 0x200:  # vehicle speed
            speed = min(120, i // 4)
            data += struct.pack("<HHHH", speed * 100, 0, 0, 0)
        else:
            data += bytes(random.randint(0, 255) for _ in range(8))
    write("can_bus_log.bin", bytes(data))


# 5. ADC calibration sweep — real 12-bit ADC (e.g., TI ADS1115)
def gen_adc_calibration():
    data = bytearray()
    # 4096 samples of a linear voltage ramp (0V to 3.3V)
    # 16-bit signed values (real ADS1115 output format)
    for i in range(4096):
        # Linear ramp with small noise (±1 LSB)
        value = int(32767 * i / 4095) + random.randint(-1, 1)
        value = max(-32768, min(32767, value))
        data += struct.pack(">h", value)  # big-endian (I2C format)
    write("adc_calibration_sweep.bin", bytes(data))


# 6. GPS NMEA-derived binary — real position log
def gen_gps_track():
    data = bytearray()
    random.seed(99)
    # 200 GPS fixes (binary packed from NMEA)
    lat = 37.7749 * 1e7  # San Francisco, scaled to int
    lon = -122.4194 * 1e7
    for i in range(200):
        # Move slightly (walking speed ~1.4 m/s)
        lat += random.randint(10, 30)  # ~1-3 meters north
        lon += random.randint(-5, 15)  # slight east drift
        data += struct.pack("<I", 1700000000 + i * 5)  # timestamp (5s intervals)
        data += struct.pack("<i", int(lat))             # latitude (1e-7 degrees)
        data += struct.pack("<i", int(lon))             # longitude (1e-7 degrees)
        data += struct.pack("<H", 50 + random.randint(-5, 5))  # altitude (dm)
        data += struct.pack("<B", random.randint(6, 12))       # num satellites
        data += struct.pack("<B", 1)                           # fix quality
    write("gps_track.bin", bytes(data))


# 7. Thermal image frame — real 16-bit FLIR-style
def gen_thermal_frame():
    data = bytearray()
    # 80x60 frame (FLIR Lepton resolution), 16-bit
    # Room temperature scene: ~295K (2950 in 0.1K units) with gradients
    for row in range(60):
        for col in range(80):
            # Smooth gradient + small noise (real thermal camera behavior)
            base = 2950 + row * 2 + col // 4
            noise = random.randint(-3, 3)
            data += struct.pack("<H", base + noise)
    write("thermal_frame_80x60.bin", bytes(data))


# 8. FAT32 directory entries — real filesystem metadata
def gen_fat32_dir():
    data = bytearray()
    # 64 FAT32 directory entries (32 bytes each)
    for i in range(64):
        if i == 0:
            # Volume label
            data += b"FIRMWARE   "  # 11 bytes name
            data += bytes([0x08])    # attribute: volume label
            data += b"\x00" * 20    # rest of entry
        elif i < 20:
            # Regular files
            name = f"FILE{i:04d} BIN".encode()[:11]
            data += name
            data += bytes([0x20])    # attribute: archive
            data += bytes([0x00])    # reserved
            data += struct.pack("<B", 0)  # creation time tenths
            data += struct.pack("<H", 0x8000 + i * 0x100)  # creation time
            data += struct.pack("<H", 0x5A00 + i)  # creation date
            data += struct.pack("<H", 0x5A00 + i)  # access date
            data += struct.pack("<H", 0)           # cluster high
            data += struct.pack("<H", 0x8000 + i * 0x100)  # mod time
            data += struct.pack("<H", 0x5A00 + i)  # mod date
            data += struct.pack("<H", 100 + i * 2) # cluster low
            data += struct.pack("<I", 4096 * (i + 1))  # file size
        else:
            # Empty entries
            data += b"\x00" * 32
    write("fat32_directory.bin", bytes(data))


if __name__ == "__main__":
    print("Generating real-world format test data:")
    gen_uboot_header()
    gen_ccsds_telemetry()
    gen_elf_sections()
    gen_can_bus()
    gen_adc_calibration()
    gen_gps_track()
    gen_thermal_frame()
    gen_fat32_dir()
    print("\nDone. Files in tests/data/")
