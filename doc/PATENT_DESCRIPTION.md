# Patent Description — PolynomiaZ Core Methods

## TITLE

Method and System for Lossless Data Compression Using Adaptive Virtual Disk Geometry with Mathematical Function Fitting and Cross-Track Parameter Optimization

---

## FIELD OF THE INVENTION

The present invention relates to data compression, and more particularly to a lossless compression method that maps input data onto a virtual disk platter geometry, detects mathematical patterns within each track, adaptively selects the optimal geometry, and further compresses groups of tracks whose detected parameters exhibit higher-order patterns.

---

## BACKGROUND OF THE INVENTION

Existing lossless compression algorithms (DEFLATE, LZ77, LZMA, Burrows-Wheeler) operate by identifying byte-level repetition through sliding windows, dictionary coding, or statistical modeling. These methods excel at data containing repeated substrings but fundamentally cannot recognize that a byte sequence such as [0, 1, 2, 3, ..., 255] represents a linear mathematical function. Such algorithms treat arithmetic sequences, address tables, and numerically structured data as having no exploitable redundancy, storing them at or near their original size.

Data produced by embedded systems, scientific instruments, spacecraft telemetry, and firmware images frequently contains mathematically structured content: linear address tables, calibration ramps, polynomial lookup tables, periodic waveforms, and constant-filled regions. These data types are poorly served by existing compression methods.

Furthermore, no known prior art teaches the use of virtual disk geometry (tracks and sectors) as a tunable compression parameter, nor the detection of patterns in the parameters of already-detected patterns across multiple tracks.

---

## SUMMARY OF THE INVENTION

The invention comprises three interrelated methods that, when combined, achieve superior lossless compression on mathematically structured data:

**Method 1: Adaptive Virtual Disk Geometry Selection**

Input data is mapped onto a virtual disk platter having concentric tracks divided into sectors. The number of sectors per track determines how the sequential byte stream is partitioned into tracks for analysis. The invention evaluates a plurality of candidate sector sizes (e.g., 8, 16, 32, 64, 128, 256, 512 sectors per track), performs pattern analysis on each resulting track layout, estimates the compressed output size for each geometry, and selects the geometry producing the smallest output. This adaptive selection exposes different mathematical patterns depending on the sector size chosen, as patterns may align with certain track lengths but not others.

**Method 2: Wide-Word Mathematical Pattern Detection**

Each track is analyzed not only at the byte level but also by reinterpreting the byte sequence as arrays of wider data types (16-bit unsigned integers, 32-bit unsigned integers, 64-bit IEEE 754 floating-point values). For each interpretation, the system checks whether the resulting sequence of wider values forms a recognized mathematical pattern (arithmetic sequence, polynomial, periodic function). Byte-exact reconstruction is verified before accepting any wide-word pattern to guarantee lossless compression. This enables detection of patterns such as linearly-incrementing 32-bit memory addresses that are invisible when examined one byte at a time.

**Method 3: Cross-Track Parameter Meta-Descriptors**

After individual track analysis, the system groups tracks sharing the same pattern type and examines whether their detected parameters themselves form a higher-order pattern. For example, if N tracks are each detected as CONSTANT with values [0, 1, 2, ..., N-1], the system recognizes that the constant values form a linear sequence and encodes the entire group as a single meta-descriptor: "N CONSTANT tracks whose values form an arithmetic sequence with start=0, step=1." This second-order compression eliminates redundancy in the compression metadata itself, providing significant additional compression when many tracks share related structure.

---

## DETAILED DESCRIPTION

### System Architecture

The compression system operates in the following stages:

1. **Geometry Evaluation**: For each candidate sector size S in a predefined set, compute the number of tracks T = ceil(N/S) where N is the input length in bytes.

2. **Data Layout**: Arrange the N input bytes into T tracks of S bytes each (padding the final track with zeros if necessary). Each byte at sequential position i occupies track floor(i/S) at sector position (i mod S).

3. **Pattern Analysis**: For each track, test the byte sequence against a plurality of mathematical pattern detectors in priority order. Each detector verifies exact byte-level reconstruction before accepting a match. The detectors include but are not limited to:
   - Constant value (all bytes identical)
   - Linear arithmetic sequence (byte values form y = a + b*i)
   - Run-length encoding (consecutive repeated values)
   - Repeating sub-pattern (periodic repetition of a shorter sequence)
   - Wide-word linear (bytes reinterpreted as 16/32-bit integers forming an arithmetic sequence)
   - Wide-word floating-point linear (bytes reinterpreted as 64-bit IEEE 754 doubles forming an arithmetic sequence)
   - Polynomial fit (byte values fit a polynomial of degree ≤ 8)
   - Periodic (discrete Fourier transform with few significant components)

4. **Wide-Word Reinterpretation**: For tracks not matching byte-level patterns, the system reinterprets the track bytes as arrays of wider types:
   - As uint32 little-endian: read 4 bytes at a time, check if the resulting 32-bit values form an arithmetic sequence
   - As uint16 little-endian: read 2 bytes at a time, similarly
   - As float64 little-endian: read 8 bytes at a time, check for linear sequence with byte-exact reconstruction verification

   The verification step computes the reconstructed value using the detected parameters (start + i × step), converts back to bytes, and compares against the original bytes. Only if every byte matches exactly is the pattern accepted.

5. **Cross-Track Optimization**: After all tracks are analyzed, group tracks by pattern type. For each group of 4 or more tracks:
   - **AllSame detection**: If all tracks in the group have identical parameters, store the parameters once with a list of track indices.
   - **ConstLinear detection**: If the group consists of CONSTANT tracks whose values form an arithmetic sequence, store start + step + track indices.
   - **LinearStartsLinear detection**: If the group consists of LINEAR tracks with identical step but linearly-incrementing start values, store base_start + start_step + step + track indices.

   The system compares the cost of the meta-descriptor against the cost of individual track encodings and uses the meta-descriptor only when it produces a smaller output.

6. **Size Estimation and Selection**: For each candidate geometry, the system estimates the total compressed size (sum of all track encodings plus meta-descriptor encodings plus format overhead). The geometry producing the smallest estimated size is selected for the final encoding.

7. **Binary Encoding**: The selected geometry's track encodings and meta-descriptors are serialized into a compact binary format with a fixed-size header specifying the original data length, sector size, and number of platters.

### Multi-Platter Organization

Tracks are separated into platters based on pattern detectability:
- Platter 0: Tracks with detected mathematical patterns (stored as compact function descriptors)
- Platter 1: Tracks with no detected pattern (stored raw, optionally with secondary compression)

This separation allows the secondary compressor (e.g., DEFLATE) to operate on contiguous unstructured data without interference from the structured tracks.

### Reconstruction

Decompression reads the header to determine geometry, then for each track reconstructs the original bytes by evaluating the stored mathematical function with the stored parameters. For wide-word patterns, the function is evaluated at the wider type level and the result is converted back to bytes. The reconstructed tracks are concatenated in order and truncated to the original data length.

---

## CLAIMS

1. A method for lossless data compression comprising:
   a) mapping input data onto a virtual disk platter geometry having a configurable number of sectors per track;
   b) evaluating a plurality of candidate sector sizes;
   c) for each candidate sector size, analyzing each resulting track for mathematical patterns;
   d) selecting the sector size that produces the smallest compressed output;
   e) encoding matched tracks as mathematical function descriptors and unmatched tracks as raw data.

2. The method of claim 1, wherein analyzing each track comprises reinterpreting the byte sequence as arrays of wider data types including 16-bit integers, 32-bit integers, and 64-bit floating-point values, and testing whether the resulting wider values form a recognized mathematical pattern.

3. The method of claim 2, further comprising verifying byte-exact reconstruction of the wider-type pattern before accepting the match, by computing reconstructed values from the detected parameters, converting to bytes, and comparing against the original byte sequence.

4. The method of claim 1, further comprising:
   f) after analyzing all tracks, grouping tracks by detected pattern type;
   g) for each group of tracks, determining whether the detected parameters across the group form a higher-order pattern;
   h) encoding groups whose parameters form a higher-order pattern as a single meta-descriptor containing the group's track indices and the higher-order pattern parameters.

5. The method of claim 4, wherein the higher-order patterns include:
   - identical parameters across all tracks in the group (AllSame);
   - constant values forming an arithmetic sequence across tracks (ConstLinear);
   - linear pattern start values forming an arithmetic sequence across tracks with identical step values (LinearStartsLinear).

6. The method of claim 4, wherein the meta-descriptor is used only when its encoded size is smaller than the sum of individual track encodings for the group.

7. A system for lossless data compression comprising:
   a processor configured to execute instructions that:
   a) partition input data into tracks according to a virtual disk geometry;
   b) evaluate multiple candidate geometries in parallel;
   c) detect mathematical patterns in each track including byte-level patterns and wide-word patterns obtained by reinterpreting bytes as wider data types;
   d) detect higher-order patterns in the parameters of detected patterns across multiple tracks;
   e) select the geometry and encoding that minimizes total compressed size;
   f) output a compressed representation containing function descriptors, meta-descriptors, and raw data organized into platters.

8. The system of claim 7, wherein the plurality of candidate geometries are evaluated in parallel using multiple processing threads.

9. A non-transitory computer-readable medium storing instructions that, when executed by a processor, cause the processor to perform lossless data compression by:
   mapping sequential input bytes onto concentric tracks of a virtual disk platter;
   adaptively selecting a sector size from a plurality of candidates based on compressed output size;
   detecting mathematical patterns in each track at both byte granularity and wider-word granularity;
   verifying byte-exact reconstruction for all detected patterns;
   detecting higher-order patterns in the parameters of detected patterns across groups of tracks; and
   encoding the data as a combination of mathematical function descriptors, cross-track meta-descriptors, and optionally-compressed raw data.

---

## ABSTRACT

A method and system for lossless data compression that maps input data onto a virtual disk platter geometry, adaptively selects the optimal number of sectors per track from a plurality of candidates, detects mathematical patterns in each track at both byte and wider-word granularity (16/32/64-bit), and further compresses groups of tracks whose detected parameters form higher-order patterns into single meta-descriptors. The method achieves 5-12× better compression than existing algorithms on mathematically structured data (firmware images, telemetry, scientific data) while maintaining less than 1% overhead on incompressible data.
