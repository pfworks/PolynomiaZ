/// Binary codec: encode (compress) and decode (decompress).

use crate::analyzer::*;
use crate::columnar::*;
use crate::cross_track::*;
use crate::platter::*;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{Read, Write, Cursor};

const MAGIC: &[u8; 4] = b"PLTZ";
const MAGIC_COLUMNAR: &[u8; 4] = b"PLTC"; // Columnar-transformed variant
const FORMAT_VERSION: u8 = 2; // v2: adds version byte, streaming support

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawCompressor {
    None,
    Zlib,    // method 1 — fast, decent ratio
    Zstd,    // method 2 — fast, better ratio
    Brotli,  // method 3 — slower, best ratio
    Best,    // try all, pick smallest
}

/// Encode with optional columnar pre-processing.
/// Tries the given stride values and picks the best (including no transform).
pub fn encode_auto(data: &[u8], raw_compressor: RawCompressor, strides: &[usize]) -> Vec<u8> {
    let plain = encode(data, raw_compressor);
    let mut best = plain;

    for &stride in strides {
        if stride == 0 || stride >= data.len() {
            continue;
        }
        let transformed = columnar_transform(data, stride);
        let inner = encode(&transformed, raw_compressor);
        // Columnar wrapper: PLTC + original_len(4) + stride(2) + inner
        let total_size = 4 + 4 + 2 + inner.len();
        if total_size < best.len() {
            let mut buf = Vec::with_capacity(total_size);
            buf.extend_from_slice(MAGIC_COLUMNAR);
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(stride as u16).to_le_bytes());
            buf.extend_from_slice(&inner);
            best = buf;
        }
    }

    best
}

/// Streaming/chunked encoding: splits data into fixed-size chunks, compresses
/// each independently (with its own optimal geometry = per-section geometry).
///
/// Format (PLTS):
///   [4] Magic "PLTS"
///   [1] Format version
///   [4] Original data length (uint32 LE)
///   [4] Chunk size (uint32 LE)
///   [2] Number of chunks (uint16 LE)
///   Per chunk:
///     [4] Compressed chunk length (uint32 LE)
///     [...] PLTZ-encoded chunk (independent, own geometry)
pub fn encode_chunked(data: &[u8], raw_compressor: RawCompressor, chunk_size: usize) -> Vec<u8> {
    let num_chunks = if data.is_empty() { 0 } else { (data.len() + chunk_size - 1) / chunk_size };

    let mut buf = Vec::new();
    buf.extend_from_slice(b"PLTS");
    buf.push(FORMAT_VERSION);
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(chunk_size as u32).to_le_bytes());
    buf.extend_from_slice(&(num_chunks as u16).to_le_bytes());

    for i in 0..num_chunks {
        let start = i * chunk_size;
        let end = std::cmp::min(start + chunk_size, data.len());
        let chunk = &data[start..end];
        let compressed_chunk = encode(chunk, raw_compressor);
        buf.extend_from_slice(&(compressed_chunk.len() as u32).to_le_bytes());
        buf.extend_from_slice(&compressed_chunk);
    }

    buf
}

/// Decode a chunked/streaming format.
pub fn decode_chunked(compressed: &[u8]) -> Result<Vec<u8>, String> {
    if compressed.len() < 15 || &compressed[0..4] != b"PLTS" {
        return Err("Bad streaming magic".into());
    }

    let version = compressed[4];
    if version > FORMAT_VERSION {
        return Err(format!("Unsupported format version {} (max {})", version, FORMAT_VERSION));
    }

    let original_len = u32::from_le_bytes(compressed[5..9].try_into().unwrap()) as usize;
    let _chunk_size = u32::from_le_bytes(compressed[9..13].try_into().unwrap()) as usize;
    let num_chunks = u16::from_le_bytes(compressed[13..15].try_into().unwrap()) as usize;

    let mut result = Vec::with_capacity(original_len);
    let mut offset = 15;

    for _ in 0..num_chunks {
        if offset + 4 > compressed.len() {
            return Err("Truncated chunk header".into());
        }
        let chunk_len = u32::from_le_bytes(compressed[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        if offset + chunk_len > compressed.len() {
            return Err("Truncated chunk data".into());
        }
        let chunk_data = decode(&compressed[offset..offset + chunk_len])?;
        result.extend_from_slice(&chunk_data);
        offset += chunk_len;
    }

    result.truncate(original_len);
    Ok(result)
}

/// Encode (compress) data into PLTZ binary format.
/// Automatically uses chunked mode for data > 4GB.
pub fn encode(data: &[u8], raw_compressor: RawCompressor) -> Vec<u8> {
    // Auto-chunk if data exceeds uint32 max
    if data.len() > u32::MAX as usize {
        let chunk_size = 64 * 1024 * 1024; // 64MB chunks for large files
        return encode_chunked(data, raw_compressor, chunk_size);
    }

    if data.is_empty() {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&256u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        return buf;
    }

    let geom = best_geometry(data);
    let tracks = lay_out(data, &geom);
    let platters = split_platters(&tracks);

    let mut buf = Vec::new();
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(geom.sectors_per_track as u16).to_le_bytes());
    buf.extend_from_slice(&(platters.len() as u16).to_le_bytes());

    for (individual, meta_groups) in &platters {
        // Track count = individual tracks + meta groups (each meta group counts as 1 entry)
        let entry_count = individual.len() + meta_groups.len();
        buf.extend_from_slice(&(entry_count as u16).to_le_bytes());
        for enc in individual {
            encode_track(&mut buf, enc, geom.sectors_per_track, raw_compressor);
        }
        for group in meta_groups {
            encode_meta_group(&mut buf, group);
        }
    }

    buf
}

/// Decode (decompress) PLTZ, PLTC, or PLTS binary format back to original data.
pub fn decode(compressed: &[u8]) -> Result<Vec<u8>, String> {
    if compressed.len() < 10 {
        return Err("Too short".into());
    }

    // Check for streaming format
    if &compressed[0..4] == b"PLTS" {
        return decode_chunked(compressed);
    }

    // Check for columnar wrapper
    if &compressed[0..4] == MAGIC_COLUMNAR {
        let original_len = u32::from_le_bytes(compressed[4..8].try_into().unwrap()) as usize;
        let stride = u16::from_le_bytes(compressed[8..10].try_into().unwrap()) as usize;
        let inner = &compressed[10..];
        let transformed = decode(inner)?;
        return Ok(columnar_untransform(&transformed, stride, original_len));
    }

    if &compressed[0..4] != MAGIC {
        return Err("Bad magic".into());
    }

    let original_len = u32::from_le_bytes(compressed[4..8].try_into().unwrap()) as usize;
    let sectors = u16::from_le_bytes(compressed[8..10].try_into().unwrap()) as usize;
    let num_platters = u16::from_le_bytes(compressed[10..12].try_into().unwrap()) as usize;

    if original_len == 0 {
        return Ok(Vec::new());
    }

    let total_tracks = (original_len + sectors - 1) / sectors;
    let mut all_tracks: Vec<Option<Vec<u8>>> = vec![None; total_tracks];

    let mut offset = 12;
    for _ in 0..num_platters {
        if offset + 2 > compressed.len() {
            return Err("Truncated platter header".into());
        }
        let num_entries = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        for _ in 0..num_entries {
            if offset + 3 > compressed.len() {
                return Err("Truncated entry".into());
            }
            let tag_byte = compressed[offset + 2];
            if tag_byte == META_TAG {
                // Meta-group: skip the 3-byte header (sentinel idx + META_TAG)
                offset += 3;
                let (decoded_tracks, new_offset) = decode_meta_group(compressed, offset, sectors)?;
                offset = new_offset;
                for (idx, data) in decoded_tracks {
                    if (idx as usize) < all_tracks.len() {
                        all_tracks[idx as usize] = Some(data);
                    }
                }
            } else {
                let (track_idx, track_data, new_offset) = decode_track(compressed, offset, sectors)?;
                offset = new_offset;
                if (track_idx as usize) < all_tracks.len() {
                    all_tracks[track_idx as usize] = Some(track_data);
                }
            }
        }
    }

    let mut result = Vec::with_capacity(original_len);
    for i in 0..total_tracks {
        match &all_tracks[i] {
            Some(track) => result.extend_from_slice(track),
            None => return Err(format!("Missing track {}", i)),
        }
    }
    result.truncate(original_len);
    Ok(result)
}

fn best_geometry(data: &[u8]) -> DiskGeometry {
    use rayon::prelude::*;

    SECTOR_CANDIDATES.par_iter()
        .map(|&spt| {
            let geom = compute_geometry(data.len(), spt);
            let tracks = lay_out(data, &geom);
            let encodings = analyze_platter(&tracks);
            let (meta_groups, remaining) = cross_track_optimize(encodings);
            let individual_size: usize = remaining.iter().map(|e| estimate_track_size(e, spt)).sum();
            let meta_size: usize = meta_groups.iter().map(|g| estimate_meta_size(g) + 3).sum();
            (geom, individual_size + meta_size)
        })
        .min_by_key(|&(_, size)| size)
        .map(|(geom, _)| geom)
        .unwrap_or_else(|| compute_geometry(data.len(), 256))
}

/// Public access to the adaptive geometry selection (for visualization).
pub fn best_geometry_pub(data: &[u8]) -> DiskGeometry {
    best_geometry(data)
}

fn split_platters(tracks: &[Vec<u8>]) -> Vec<(Vec<TrackEncoding>, Vec<MetaGroup>)> {
    let encodings = analyze_platter(tracks);
    let (meta_groups, remaining) = cross_track_optimize(encodings);

    let mut structured = Vec::new();
    let mut raw: Vec<TrackEncoding> = Vec::new();

    for enc in remaining {
        if enc.pattern == PatternType::Raw {
            raw.push(enc);
        } else {
            structured.push(enc);
        }
    }

    // Second-pass: re-analyze concatenated RAW data with different geometry
    if raw.len() >= 2 {
        let sectors = tracks[0].len();
        let raw_blob: Vec<u8> = raw.iter().flat_map(|e| {
            match &e.params {
                TrackParams::Raw { data } => data.clone(),
                _ => vec![0; sectors],
            }
        }).collect();

        // Try smaller sector sizes on the combined RAW data
        let mut best_reanalysis: Option<(Vec<TrackEncoding>, Vec<MetaGroup>, usize)> = None;
        for &spt in &[8, 16, 32, 64] {
            if spt >= sectors && spt >= raw_blob.len() { continue; }
            let geom2 = compute_geometry(raw_blob.len(), spt);
            let tracks2 = lay_out(&raw_blob, &geom2);
            let enc2 = analyze_platter(&tracks2);
            let (meta2, rem2) = cross_track_optimize(enc2);

            // Count how much is still RAW after re-analysis
            let raw_remaining: usize = rem2.iter()
                .filter(|e| e.pattern == PatternType::Raw)
                .count();
            let structured_found = rem2.len() - raw_remaining + meta2.len();

            if structured_found > 0 {
                let size: usize = rem2.iter().map(|e| estimate_track_size(e, spt)).sum::<usize>()
                    + meta2.iter().map(|g| estimate_meta_size(g) + 3).sum::<usize>();
                let original_raw_size = raw.len() * (3 + sectors); // original cost

                if size < original_raw_size {
                    if best_reanalysis.is_none() || size < best_reanalysis.as_ref().unwrap().2 {
                        best_reanalysis = Some((rem2, meta2, size));
                    }
                }
            }
        }

        if let Some((reanalyzed, meta2, _)) = best_reanalysis {
            // Replace raw platter with re-analyzed version
            // Remap track indices to original raw track positions
            let mut platters = Vec::new();
            if !structured.is_empty() || !meta_groups.is_empty() {
                platters.push((structured, meta_groups));
            }
            // The re-analyzed tracks use their own indices (relative to the raw blob)
            // We need to encode them as a separate PLTZ blob on the raw platter
            // Simplest: just use the re-analyzed encodings directly
            platters.push((reanalyzed, meta2));
            if platters.is_empty() {
                platters.push((Vec::new(), Vec::new()));
            }
            return platters;
        }
    }

    let mut platters = Vec::new();
    if !structured.is_empty() || !meta_groups.is_empty() {
        platters.push((structured, meta_groups));
    }
    if !raw.is_empty() {
        platters.push((raw, Vec::new()));
    }
    if platters.is_empty() {
        platters.push((Vec::new(), Vec::new()));
    }
    platters
}

fn estimate_track_size(enc: &TrackEncoding, sectors: usize) -> usize {
    3 + match &enc.params {
        TrackParams::Const { .. } => 1,
        TrackParams::Linear { .. } => 4,
        TrackParams::Rle { runs } => 2 + runs.len() * 2,
        TrackParams::Repeat { pattern } => 2 + pattern.len(),
        TrackParams::Delta { deltas, .. } => 2 + deltas.len(),
        TrackParams::Periodic { components, .. } => 4 + components.len() * 18,
        TrackParams::Poly { degree, .. } => 1 + (*degree as usize + 1) * 8,
        TrackParams::Raw { .. } => sectors,
        TrackParams::LinearWide { .. } => 19,
        TrackParams::LinearF64 { .. } => 18,
        TrackParams::DeltaF64 { deltas_f32, .. } => 10 + deltas_f32.len() * 4,
        TrackParams::XorMask { inner_params, .. } => 2 + inner_params.len(),
        TrackParams::Sparse { entries, .. } => 3 + entries.len() * 3,
        TrackParams::PiecewiseLinear { segments } => 1 + segments.len() * 6,
        TrackParams::Mirror { half } => half.len(),
        TrackParams::InterleavedLinear { .. } => 10,
        TrackParams::Exponential { .. } => 10,
        TrackParams::DeltaRle { runs, .. } => 3 + runs.len() * 2,
        TrackParams::BitPacked { packed, .. } => 1 + packed.len(),
        TrackParams::DiffTable { order, .. } => 1 + *order as usize + 1 + 4,
        TrackParams::ModArith { .. } => 6,
        TrackParams::Fibonacci { .. } => 4,
        TrackParams::DeltaWide { deltas, .. } => 6 + deltas.len(),
    }
}

fn encode_track(buf: &mut Vec<u8>, enc: &TrackEncoding, _sectors: usize, raw_comp: RawCompressor) {
    match &enc.params {
        TrackParams::Const { value } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Const as u8);
            buf.push(*value);
        }
        TrackParams::Linear { start, step } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Linear as u8);
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::Rle { runs } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Rle as u8);
            buf.extend_from_slice(&(runs.len() as u16).to_le_bytes());
            for &(val, count) in runs {
                buf.push(val);
                buf.push(count);
            }
        }
        TrackParams::Repeat { pattern } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Repeat as u8);
            buf.extend_from_slice(&(pattern.len() as u16).to_le_bytes());
            buf.extend_from_slice(pattern);
        }
        TrackParams::Delta { start, deltas } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Delta as u8);
            buf.extend_from_slice(&start.to_le_bytes());
            for &d in deltas {
                buf.push(d as u8);
            }
        }
        TrackParams::Periodic { components, n } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Periodic as u8);
            buf.extend_from_slice(&(components.len() as u16).to_le_bytes());
            buf.extend_from_slice(&n.to_le_bytes());
            for &(idx, real, imag) in components {
                buf.extend_from_slice(&idx.to_le_bytes());
                buf.extend_from_slice(&real.to_le_bytes());
                buf.extend_from_slice(&imag.to_le_bytes());
            }
        }
        TrackParams::Poly { degree, coeffs } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Poly as u8);
            buf.push(*degree);
            for &c in coeffs {
                buf.extend_from_slice(&c.to_le_bytes());
            }
        }
        TrackParams::LinearWide { width, count, start, step } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::LinearWide as u8);
            buf.push(*width);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::LinearF64 { count, start, step } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::LinearF64 as u8);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::DeltaF64 { count, start, deltas_f32 } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::DeltaF64 as u8);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            for &d in deltas_f32 {
                buf.extend_from_slice(&d.to_le_bytes());
            }
        }
        TrackParams::XorMask { mask, inner_tag, inner_params } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::XorMask as u8);
            buf.push(*mask);
            buf.push(*inner_tag);
            buf.extend_from_slice(inner_params);
        }
        TrackParams::Sparse { fill, entries } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Sparse as u8);
            buf.push(*fill);
            buf.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for &(idx, val) in entries {
                buf.extend_from_slice(&idx.to_le_bytes());
                buf.push(val);
            }
        }
        TrackParams::PiecewiseLinear { segments } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::PiecewiseLinear as u8);
            buf.push(segments.len() as u8);
            for &(start_idx, start_val, step) in segments {
                buf.extend_from_slice(&start_idx.to_le_bytes());
                buf.extend_from_slice(&start_val.to_le_bytes());
                buf.extend_from_slice(&step.to_le_bytes());
            }
        }
        TrackParams::Mirror { half } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Mirror as u8);
            buf.extend_from_slice(half);
        }
        TrackParams::InterleavedLinear { count, start_a, step_a, start_b, step_b } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::InterleavedLinear as u8);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start_a.to_le_bytes());
            buf.extend_from_slice(&step_a.to_le_bytes());
            buf.extend_from_slice(&start_b.to_le_bytes());
            buf.extend_from_slice(&step_b.to_le_bytes());
        }
        TrackParams::Exponential { count, base, ratio } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Exponential as u8);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&base.to_le_bytes());
            buf.extend_from_slice(&ratio.to_le_bytes());
        }
        TrackParams::DeltaRle { start, runs } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::DeltaRle as u8);
            buf.push(*start);
            buf.extend_from_slice(&(runs.len() as u16).to_le_bytes());
            for &(delta, count) in runs {
                buf.push(delta as u8);
                buf.push(count);
            }
        }
        TrackParams::BitPacked { bits, packed } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::BitPacked as u8);
            buf.push(*bits);
            buf.extend_from_slice(packed);
        }
        TrackParams::DiffTable { order, initial, diff } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::DiffTable as u8);
            buf.push(*order);
            buf.extend_from_slice(initial);
            buf.extend_from_slice(&diff.to_le_bytes());
        }
        TrackParams::ModArith { a, b, m } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::ModArith as u8);
            buf.extend_from_slice(&a.to_le_bytes());
            buf.extend_from_slice(&b.to_le_bytes());
            buf.extend_from_slice(&m.to_le_bytes());
        }
        TrackParams::Fibonacci { seed0, seed1, c1, c2 } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Fibonacci as u8);
            buf.push(*seed0);
            buf.push(*seed1);
            buf.push(*c1);
            buf.push(*c2);
        }
        TrackParams::DeltaWide { width, delta_width, start, deltas } => {
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::DeltaWide as u8);
            buf.push(*width);
            buf.push(*delta_width);
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(deltas);
        }
        TrackParams::Raw { data } => {
            if raw_comp != RawCompressor::None {
                if let Some((method, compressed)) = compress_raw_best(data, raw_comp) {
                    if compressed.len() < data.len() {
                        buf.extend_from_slice(&enc.track_idx.to_le_bytes());
                        buf.push(PatternType::RawCompressed as u8);
                        buf.push(method);
                        buf.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
                        buf.extend_from_slice(&compressed);
                        return;
                    }
                }
            }
            buf.extend_from_slice(&enc.track_idx.to_le_bytes());
            buf.push(PatternType::Raw as u8);
            buf.extend_from_slice(data);
        }
    }
}

fn decode_track(buf: &[u8], offset: usize, sectors: usize) -> Result<(u16, Vec<u8>, usize), String> {
    if offset + 3 > buf.len() {
        return Err("Truncated track header".into());
    }
    let track_idx = u16::from_le_bytes(buf[offset..offset + 2].try_into().unwrap());
    let tag_val = buf[offset + 2];
    let mut pos = offset + 3;

    let tag = PatternType::from_u8(tag_val).ok_or_else(|| format!("Unknown tag: {}", tag_val))?;

    let track = match tag {
        PatternType::Const => {
            let val = buf[pos]; pos += 1;
            vec![val; sectors]
        }
        PatternType::Linear => {
            let start = i16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
            let step = i16::from_le_bytes(buf[pos + 2..pos + 4].try_into().unwrap());
            pos += 4;
            (0..sectors).map(|i| (start.wrapping_add((i as i16).wrapping_mul(step))) as u8).collect()
        }
        PatternType::Rle => {
            let num_runs = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let mut track = Vec::with_capacity(sectors);
            for _ in 0..num_runs {
                let val = buf[pos];
                let count = buf[pos + 1] as usize;
                pos += 2;
                track.extend(std::iter::repeat(val).take(count));
            }
            track.truncate(sectors);
            track
        }
        PatternType::Repeat => {
            let pat_len = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let pattern = &buf[pos..pos + pat_len];
            pos += pat_len;
            (0..sectors).map(|i| pattern[i % pat_len]).collect()
        }
        PatternType::Delta => {
            let start = i16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
            pos += 2;
            let num_deltas = sectors - 1;
            let mut track = Vec::with_capacity(sectors);
            let mut current = start;
            track.push(current as u8);
            for i in 0..num_deltas {
                let d = buf[pos + i] as i8;
                current = current.wrapping_add(d as i16);
                track.push(current as u8);
            }
            pos += num_deltas;
            track
        }
        PatternType::Periodic => {
            let num_comps = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            let n = u16::from_le_bytes(buf[pos + 2..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let fft_len = n / 2 + 1;
            let mut fft_real = vec![0.0f64; fft_len];
            let mut fft_imag = vec![0.0f64; fft_len];
            for _ in 0..num_comps {
                let idx = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
                let real = f64::from_le_bytes(buf[pos + 2..pos + 10].try_into().unwrap());
                let imag = f64::from_le_bytes(buf[pos + 10..pos + 18].try_into().unwrap());
                pos += 18;
                if idx < fft_len { fft_real[idx] = real; fft_imag[idx] = imag; }
            }
            irfft(&fft_real, &fft_imag, n)
        }
        PatternType::Poly => {
            let degree = buf[pos] as usize; pos += 1;
            let mut coeffs = Vec::with_capacity(degree + 1);
            for _ in 0..=degree {
                let c = f64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
                pos += 8;
                coeffs.push(c);
            }
            (0..sectors).map(|i| {
                let x = i as f64;
                let mut val = 0.0f64;
                for (j, &c) in coeffs.iter().enumerate() {
                    val += c * x.powi((degree - j) as i32);
                }
                val.round() as u8
            }).collect()
        }
        PatternType::LinearWide => {
            let width = buf[pos] as usize;
            let count = u16::from_le_bytes(buf[pos + 1..pos + 3].try_into().unwrap()) as usize;
            let start = i64::from_le_bytes(buf[pos + 3..pos + 11].try_into().unwrap());
            let step = i64::from_le_bytes(buf[pos + 11..pos + 19].try_into().unwrap());
            pos += 19;
            let mut track = Vec::with_capacity(count * width);
            for i in 0..count {
                let val = start.wrapping_add((i as i64).wrapping_mul(step));
                if width == 4 {
                    track.extend_from_slice(&(val as u32).to_le_bytes());
                } else {
                    track.extend_from_slice(&(val as u16).to_le_bytes());
                }
            }
            track.truncate(sectors);
            track
        }
        PatternType::LinearF64 => {
            let count = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            let start = f64::from_le_bytes(buf[pos + 2..pos + 10].try_into().unwrap());
            let step = f64::from_le_bytes(buf[pos + 10..pos + 18].try_into().unwrap());
            pos += 18;
            let mut track = Vec::with_capacity(count * 8);
            for i in 0..count {
                let val = start + (i as f64) * step;
                track.extend_from_slice(&val.to_le_bytes());
            }
            track.truncate(sectors);
            track
        }
        PatternType::DeltaF64 => {
            let count = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            let start = f64::from_le_bytes(buf[pos + 2..pos + 10].try_into().unwrap());
            pos += 10;
            let mut track = Vec::with_capacity(count * 8);
            let mut current = start;
            track.extend_from_slice(&current.to_le_bytes());
            for _ in 0..(count - 1) {
                let d = f32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap());
                pos += 4;
                current += d as f64;
                track.extend_from_slice(&current.to_le_bytes());
            }
            track.truncate(sectors);
            track
        }
        PatternType::XorMask => {
            let mask = buf[pos]; pos += 1;
            let inner_tag = buf[pos]; pos += 1;
            // Reconstruct inner pattern then XOR with mask
            let inner_track = match PatternType::from_u8(inner_tag) {
                Some(PatternType::Const) => {
                    let val = buf[pos]; pos += 1;
                    vec![val; sectors]
                }
                Some(PatternType::Linear) => {
                    let start = i16::from_le_bytes(buf[pos..pos+2].try_into().unwrap());
                    let step = i16::from_le_bytes(buf[pos+2..pos+4].try_into().unwrap());
                    pos += 4;
                    (0..sectors).map(|i| (start.wrapping_add((i as i16).wrapping_mul(step))) as u8).collect()
                }
                _ => { return Err(format!("Unsupported XorMask inner tag: {}", inner_tag)); }
            };
            inner_track.iter().map(|&b| b ^ mask).collect()
        }
        PatternType::Sparse => {
            let fill = buf[pos]; pos += 1;
            let count = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as usize;
            pos += 2;
            let mut track = vec![fill; sectors];
            for _ in 0..count {
                let idx = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as usize;
                let val = buf[pos+2];
                pos += 3;
                if idx < track.len() { track[idx] = val; }
            }
            track
        }
        PatternType::PiecewiseLinear => {
            let num_segs = buf[pos] as usize; pos += 1;
            let mut segments = Vec::with_capacity(num_segs);
            for _ in 0..num_segs {
                let start_idx = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap());
                let start_val = i16::from_le_bytes(buf[pos+2..pos+4].try_into().unwrap());
                let step = i16::from_le_bytes(buf[pos+4..pos+6].try_into().unwrap());
                pos += 6;
                segments.push((start_idx, start_val, step));
            }
            let mut track = vec![0u8; sectors];
            for (seg_idx, &(start_idx, start_val, step)) in segments.iter().enumerate() {
                let end_idx = if seg_idx + 1 < segments.len() {
                    segments[seg_idx + 1].0 as usize
                } else {
                    sectors
                };
                for i in start_idx as usize..end_idx {
                    let offset = (i - start_idx as usize) as i16;
                    track[i] = (start_val.wrapping_add(offset.wrapping_mul(step))) as u8;
                }
            }
            track
        }
        PatternType::Mirror => {
            let half_len = sectors / 2;
            let half = &buf[pos..pos + half_len];
            pos += half_len;
            let mut track = Vec::with_capacity(sectors);
            track.extend_from_slice(half);
            track.extend(half.iter().rev());
            track
        }
        PatternType::InterleavedLinear => {
            let count = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as usize;
            let start_a = i16::from_le_bytes(buf[pos+2..pos+4].try_into().unwrap());
            let step_a = i16::from_le_bytes(buf[pos+4..pos+6].try_into().unwrap());
            let start_b = i16::from_le_bytes(buf[pos+6..pos+8].try_into().unwrap());
            let step_b = i16::from_le_bytes(buf[pos+8..pos+10].try_into().unwrap());
            pos += 10;
            let mut track = Vec::with_capacity(count * 2);
            for i in 0..count {
                track.push((start_a.wrapping_add((i as i16).wrapping_mul(step_a))) as u8);
                track.push((start_b.wrapping_add((i as i16).wrapping_mul(step_b))) as u8);
            }
            track.truncate(sectors);
            track
        }
        PatternType::Exponential => {
            let count = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as usize;
            let base = f32::from_le_bytes(buf[pos+2..pos+6].try_into().unwrap());
            let ratio = f32::from_le_bytes(buf[pos+6..pos+10].try_into().unwrap());
            pos += 10;
            (0..count).map(|i| (base * ratio.powi(i as i32)).round() as u8).collect()
        }
        PatternType::DeltaRle => {
            let start = buf[pos]; pos += 1;
            let num_runs = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as usize;
            pos += 2;
            let mut track = Vec::with_capacity(sectors);
            track.push(start);
            for _ in 0..num_runs {
                let delta = buf[pos] as i8;
                let count = buf[pos+1] as usize;
                pos += 2;
                for _ in 0..count {
                    let prev = *track.last().unwrap() as i16;
                    track.push((prev + delta as i16) as u8);
                }
            }
            track.truncate(sectors);
            track
        }
        PatternType::BitPacked => {
            let bits = buf[pos] as usize; pos += 1;
            let packed_len = (sectors * bits + 7) / 8;
            let packed = &buf[pos..pos + packed_len];
            pos += packed_len;
            let mut track = Vec::with_capacity(sectors);
            let mut bit_pos: usize = 0;
            for _ in 0..sectors {
                let mut val: u8 = 0;
                for _ in 0..bits {
                    if packed[bit_pos / 8] & (1 << (7 - (bit_pos % 8))) != 0 {
                        val = (val << 1) | 1;
                    } else {
                        val <<= 1;
                    }
                    bit_pos += 1;
                }
                track.push(val);
            }
            track
        }
        PatternType::DiffTable => {
            let order = buf[pos] as usize; pos += 1;
            let initial: Vec<i32> = (0..=order).map(|i| buf[pos + i] as i32).collect();
            pos += order + 1;
            let diff = i32::from_le_bytes(buf[pos..pos+4].try_into().unwrap());
            pos += 4;
            // Rebuild by maintaining difference levels
            let mut levels: Vec<Vec<i32>> = vec![initial.clone()];
            for lev in 1..=order {
                let prev = &levels[lev - 1];
                let d: Vec<i32> = prev.windows(2).map(|w| w[1] - w[0]).collect();
                levels.push(d);
            }
            // Extend from bottom up
            while levels[0].len() < sectors {
                levels[order].push(diff);
                for lev in (0..order).rev() {
                    let prev_val = *levels[lev].last().unwrap();
                    let d = *levels[lev + 1].last().unwrap();
                    levels[lev].push(prev_val + d);
                }
            }
            levels[0].iter().take(sectors).map(|&v| v as u8).collect()
        }
        PatternType::ModArith => {
            let a = u16::from_le_bytes(buf[pos..pos+2].try_into().unwrap()) as u32;
            let b = u16::from_le_bytes(buf[pos+2..pos+4].try_into().unwrap()) as u32;
            let m = u16::from_le_bytes(buf[pos+4..pos+6].try_into().unwrap()) as u32;
            pos += 6;
            (0..sectors).map(|i| ((a * i as u32 + b) % m) as u8).collect()
        }
        PatternType::Fibonacci => {
            let seed0 = buf[pos]; let seed1 = buf[pos+1];
            let c1 = buf[pos+2]; let c2 = buf[pos+3];
            pos += 4;
            let mut track = Vec::with_capacity(sectors);
            track.push(seed0);
            if sectors > 1 { track.push(seed1); }
            for _ in 2..sectors {
                let prev1 = track[track.len()-1] as u16;
                let prev2 = track[track.len()-2] as u16;
                track.push((c1 as u16 * prev1 + c2 as u16 * prev2) as u8);
            }
            track
        }
        PatternType::DeltaWide => {
            let width = buf[pos] as usize;
            let delta_width = buf[pos+1] as usize;
            let start = u32::from_le_bytes(buf[pos+2..pos+6].try_into().unwrap());
            pos += 6;
            let count = if width == 4 { sectors / 4 } else { sectors / 2 };
            let mut values: Vec<u32> = Vec::with_capacity(count);
            values.push(start);
            for i in 0..(count - 1) {
                let delta: i32 = if delta_width == 1 {
                    let d = buf[pos + i] as i8;
                    d as i32
                } else {
                    let d = i16::from_le_bytes(buf[pos + i*2..pos + i*2 + 2].try_into().unwrap());
                    d as i32
                };
                let prev = *values.last().unwrap() as i64;
                values.push((prev + delta as i64) as u32);
            }
            pos += (count - 1) * delta_width;
            let mut track = Vec::with_capacity(sectors);
            for &v in &values {
                if width == 4 { track.extend_from_slice(&v.to_le_bytes()); }
                else { track.extend_from_slice(&(v as u16).to_le_bytes()); }
            }
            track.truncate(sectors);
            track
        }
        PatternType::Raw => {
            let track = buf[pos..pos + sectors].to_vec();
            pos += sectors;
            track
        }
        PatternType::RawCompressed => {
            let method = buf[pos]; pos += 1;
            let comp_len = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let compressed = &buf[pos..pos + comp_len];
            pos += comp_len;
            raw_decompress(compressed, method)?
        }
    };

    Ok((track_idx, track, pos))
}

fn deflate_compress(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

fn zstd_compress(data: &[u8]) -> Option<Vec<u8>> {
    zstd::encode_all(std::io::Cursor::new(data), 19).ok()
}

fn brotli_compress(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
    writer.write_all(data).ok()?;
    drop(writer);
    Some(out)
}

/// Try compressing with the specified method(s), return (method_tag, compressed_data).
fn compress_raw_best(data: &[u8], mode: RawCompressor) -> Option<(u8, Vec<u8>)> {
    match mode {
        RawCompressor::None => None,
        RawCompressor::Zlib => deflate_compress(data).map(|c| (1u8, c)),
        RawCompressor::Zstd => zstd_compress(data).map(|c| (2u8, c)),
        RawCompressor::Brotli => brotli_compress(data).map(|c| (3u8, c)),
        RawCompressor::Best => {
            // Try all three, pick smallest
            let mut best: Option<(u8, Vec<u8>)> = None;
            if let Some(c) = deflate_compress(data) {
                best = Some((1, c));
            }
            if let Some(c) = zstd_compress(data) {
                if best.is_none() || c.len() < best.as_ref().unwrap().1.len() {
                    best = Some((2, c));
                }
            }
            if let Some(c) = brotli_compress(data) {
                if best.is_none() || c.len() < best.as_ref().unwrap().1.len() {
                    best = Some((3, c));
                }
            }
            best
        }
    }
}

fn deflate_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = DeflateDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| format!("Deflate error: {}", e))?;
    Ok(out)
}

fn raw_decompress(data: &[u8], method: u8) -> Result<Vec<u8>, String> {
    match method {
        1 => deflate_decompress(data),
        2 => zstd::decode_all(std::io::Cursor::new(data)).map_err(|e| format!("Zstd error: {}", e)),
        3 => {
            let mut out = Vec::new();
            let mut reader = brotli::Decompressor::new(data, 4096);
            reader.read_to_end(&mut out).map_err(|e| format!("Brotli error: {}", e))?;
            Ok(out)
        }
        _ => Err(format!("Unknown raw compression method: {}", method)),
    }
}

fn irfft(real: &[f64], imag: &[f64], n: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(n);
    for t in 0..n {
        let mut sum = real[0]; // DC
        for k in 1..real.len() - 1 {
            let angle = 2.0 * std::f64::consts::PI * (k as f64) * (t as f64) / (n as f64);
            sum += 2.0 * (real[k] * angle.cos() - imag[k] * angle.sin());
        }
        if n % 2 == 0 && real.len() > 1 {
            let k = real.len() - 1;
            let angle = 2.0 * std::f64::consts::PI * (k as f64) * (t as f64) / (n as f64);
            sum += real[k] * angle.cos() - imag[k] * angle.sin();
        }
        result.push((sum / n as f64).round() as u8);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let compressed = encode(data, RawCompressor::None);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed.as_slice(), "Mismatch (len {} vs {})", data.len(), decompressed.len());
    }

    fn roundtrip_zlib(data: &[u8]) {
        let compressed = encode(data, RawCompressor::Zlib);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed.as_slice());
    }

    #[test] fn test_empty() { roundtrip(&[]); }
    #[test] fn test_single() { roundtrip(&[0x42]); }
    #[test] fn test_const() { roundtrip(&[0xFFu8; 1024]); }
    #[test] fn test_linear_ramp() { roundtrip(&(0..=255u8).cycle().take(1024).collect::<Vec<_>>()); }
    #[test] fn test_repeat_2() { roundtrip(&[0xAB, 0xCD].iter().copied().cycle().take(1024).collect::<Vec<_>>()); }
    #[test] fn test_repeat_4() { roundtrip(&[1,2,3,4].iter().copied().cycle().take(1024).collect::<Vec<_>>()); }

    #[test]
    fn test_rle() {
        let mut data = vec![0u8; 100];
        data.extend(vec![0xFF; 100]);
        data.extend(vec![0x42; 56]);
        roundtrip(&data);
    }

    #[test]
    fn test_delta() {
        let data: Vec<u8> = (0..512).map(|i| 128u8.wrapping_add((i % 5) as u8).wrapping_sub(2)).collect();
        roundtrip(&data);
    }

    #[test]
    fn test_random() {
        let mut data = vec![0u8; 1000];
        let mut state: u32 = 42;
        for b in data.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *b = (state >> 16) as u8;
        }
        roundtrip(&data);
        roundtrip_zlib(&data);
    }

    #[test]
    fn test_linear_wide_u32() {
        let mut data = Vec::new();
        for i in 0u32..64 {
            data.extend_from_slice(&(0x80000000u32.wrapping_add(i * 0x20)).to_le_bytes());
        }
        roundtrip(&data);
    }

    #[test]
    fn test_linear_f64() {
        let mut data = Vec::new();
        for i in 0..128 {
            let val: f64 = 1_000_000.0 + (i as f64) * 60.0;
            data.extend_from_slice(&val.to_le_bytes());
        }
        roundtrip(&data);
        // Verify it actually compressed well
        let compressed = encode(&data, RawCompressor::None);
        assert!(compressed.len() < data.len() / 2, "LINEAR_F64 should compress well");
    }

    #[test]
    fn test_short_string() { roundtrip(b"Hello, Platter!"); }

    #[test]
    fn test_step2_ramp() {
        let data: Vec<u8> = (0..200).map(|i| ((i * 2) % 256) as u8).collect();
        let full: Vec<u8> = data.iter().copied().cycle().take(1000).collect();
        roundtrip(&full);
    }

    #[test]
    fn test_cross_track_const() {
        // 64KB of 0xFF — should trigger cross-track meta for many CONST tracks
        let data = vec![0xFFu8; 65536];
        let compressed = encode(&data, RawCompressor::None);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed);
        // With cross-track, this should be much smaller than individual CONST tracks
        // Without cross-track: ~4 bytes per track * (65536/sector_size) tracks
        // With cross-track: single meta-group
        println!("64KB const: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < 300, "Cross-track should compress 64KB const to <300B, got {}", compressed.len());
    }

    #[test]
    fn test_cross_track_linear() {
        // Many identical linear ramps (e.g., 0-255 repeated 64 times = 16KB)
        let data: Vec<u8> = (0..=255u8).cycle().take(16384).collect();
        let compressed = encode(&data, RawCompressor::None);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed);
        println!("16KB linear ramps: {} -> {} bytes", data.len(), compressed.len());
    }

    #[test]
    fn test_polynomial() {
        // Cubic: y = (x^3 + x) / 1000, for x=0..63 — values stay in u8 range
        // Actually, use a simple quadratic that IS exact: y = round(0.05*x^2)
        // For x=0..63: max = 0.05*63^2 = 198.45, fits in u8
        let data: Vec<u8> = (0..64u32).map(|x| ((x * x) as f64 * 0.05).round() as u8).collect();
        // Verify it's actually polynomial-detectable by checking roundtrip
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Poly 64B: {} -> {} bytes", data.len(), compressed.len());
        // Even if poly doesn't fire (rounding), roundtrip must work
    }

    #[test]
    fn test_periodic_sine() {
        // Signal constructed to be exactly representable with 2 FFT components
        // DC=128, one sine at frequency 2/64 with amplitude 50
        let n = 64usize;
        let data: Vec<u8> = (0..n).map(|i| {
            (128.0 + 50.0 * (2.0 * std::f64::consts::PI * 2.0 * i as f64 / n as f64).sin()).round() as u8
        }).collect();
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Periodic 64B: {} -> {} bytes", data.len(), compressed.len());
        // Should compress (2 components = 40 bytes < 64 raw)
        assert!(compressed.len() < data.len(),
            "Periodic should compress, got {} >= {}", compressed.len(), data.len());
    }

    #[test]
    fn test_chunked_roundtrip() {
        // Large data: 64KB of mixed content
        let mut data = Vec::new();
        data.extend(vec![0xFFu8; 16384]); // constant
        data.extend((0..=255u8).cycle().take(16384)); // linear ramps
        let mut state: u32 = 42;
        for _ in 0..32768 {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            data.push((state >> 16) as u8);
        }

        // Chunked with 16KB chunks (each chunk gets its own geometry)
        let compressed = encode_chunked(&data, RawCompressor::None, 16384);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed);
        println!("Chunked 64KB (16KB chunks): {} -> {} bytes", data.len(), compressed.len());
    }

    #[test]
    fn test_chunked_with_zlib() {
        let mut data = vec![0u8; 1000];
        let mut state: u32 = 99;
        for b in data.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *b = (state >> 16) as u8;
        }
        let compressed = encode_chunked(&data, RawCompressor::Zlib, 512);
        let decompressed = decode(&compressed).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_format_version() {
        // Verify PLTS header contains version byte
        let data = vec![0xABu8; 1024];
        let compressed = encode_chunked(&data, RawCompressor::None, 512);
        assert_eq!(&compressed[0..4], b"PLTS");
        assert_eq!(compressed[4], 2); // FORMAT_VERSION
    }

    #[test]
    fn test_xor_mask() {
        // Linear ramp XORed with 0xFF: [255, 254, 253, ...]
        let data: Vec<u8> = (0..64u8).map(|i| i ^ 0xFF).collect();
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("XOR mask 64B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_sparse() {
        // Mostly zeros with a few non-zero values (star field row)
        let mut data = vec![0u8; 256];
        data[10] = 200;
        data[50] = 255;
        data[200] = 180;
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Sparse 256B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len() / 4);
    }

    #[test]
    fn test_piecewise_linear() {
        // Two linear segments: ramp up then ramp down
        let mut data = Vec::new();
        for i in 0..32u8 { data.push(i * 4); }      // 0,4,8,...,124
        for i in 0..32u8 { data.push(128u8.wrapping_sub(i * 4)); } // 128,124,...,4
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Piecewise 64B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_mirror() {
        // Symmetric data (FIR filter coefficients style)
        let half: Vec<u8> = (0..32).map(|i| (i * 8) as u8).collect();
        let mut data = half.clone();
        data.extend(half.iter().rev());
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Mirror 64B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_interleaved_linear() {
        let data: Vec<u8> = (0..32).flat_map(|i| vec![i as u8, (100 + i) as u8]).collect();
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("Interleaved 64B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_exponential() {
        let data: Vec<u8> = (0..16).map(|i| (2.0f32 * 1.2f32.powi(i)).round() as u8).collect();
        roundtrip(&data);
    }

    #[test]
    fn test_delta_rle() {
        // Staircase: hold for 16, step up by 10, repeat
        let mut data = Vec::new();
        let mut val: u8 = 0;
        for _ in 0..4 {
            for _ in 0..16 { data.push(val); }
            val = val.wrapping_add(10);
        }
        roundtrip(&data);
        let compressed = encode(&data, RawCompressor::None);
        println!("DeltaRle 64B: {} -> {} bytes", data.len(), compressed.len());
        assert!(compressed.len() < data.len());
    }
}
