/// Binary codec: encode (compress) and decode (decompress).

use crate::analyzer::*;
use crate::platter::*;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{Read, Write};

const MAGIC: &[u8; 4] = b"PLTZ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawCompressor {
    None,
    Zlib,
}

/// Encode (compress) data into PLTZ binary format.
pub fn encode(data: &[u8], raw_compressor: RawCompressor) -> Vec<u8> {
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

    for platter in &platters {
        buf.extend_from_slice(&(platter.len() as u16).to_le_bytes());
        for enc in platter {
            encode_track(&mut buf, enc, geom.sectors_per_track, raw_compressor);
        }
    }

    buf
}

/// Decode (decompress) PLTZ binary format back to original data.
pub fn decode(compressed: &[u8]) -> Result<Vec<u8>, String> {
    if compressed.len() < 12 {
        return Err("Too short".into());
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
        let num_tracks = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        for _ in 0..num_tracks {
            let (track_idx, track_data, new_offset) = decode_track(compressed, offset, sectors)?;
            offset = new_offset;
            if (track_idx as usize) < all_tracks.len() {
                all_tracks[track_idx as usize] = Some(track_data);
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
    let mut best_size = usize::MAX;
    let mut best_geom = compute_geometry(data.len(), 256);

    for &spt in SECTOR_CANDIDATES {
        let geom = compute_geometry(data.len(), spt);
        let tracks = lay_out(data, &geom);
        let encodings = analyze_platter(&tracks);
        let size: usize = encodings.iter().map(|e| estimate_track_size(e, spt)).sum();
        if size < best_size {
            best_size = size;
            best_geom = geom;
        }
    }
    best_geom
}

fn split_platters(tracks: &[Vec<u8>]) -> Vec<Vec<TrackEncoding>> {
    let encodings = analyze_platter(tracks);
    let mut structured = Vec::new();
    let mut raw = Vec::new();

    for enc in encodings {
        if enc.pattern == PatternType::Raw {
            raw.push(enc);
        } else {
            structured.push(enc);
        }
    }

    let mut platters = Vec::new();
    if !structured.is_empty() { platters.push(structured); }
    if !raw.is_empty() { platters.push(raw); }
    if platters.is_empty() { platters.push(Vec::new()); }
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
        TrackParams::Raw { data } => {
            if raw_comp == RawCompressor::Zlib {
                if let Some(compressed) = deflate_compress(data) {
                    if compressed.len() < data.len() {
                        buf.extend_from_slice(&enc.track_idx.to_le_bytes());
                        buf.push(PatternType::RawCompressed as u8);
                        buf.push(1); // method: deflate
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
        PatternType::Raw => {
            let track = buf[pos..pos + sectors].to_vec();
            pos += sectors;
            track
        }
        PatternType::RawCompressed => {
            let _method = buf[pos]; pos += 1;
            let comp_len = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let compressed = &buf[pos..pos + comp_len];
            pos += comp_len;
            deflate_decompress(compressed)?
        }
    };

    Ok((track_idx, track, pos))
}

fn deflate_compress(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

fn deflate_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = DeflateDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| format!("Deflate error: {}", e))?;
    Ok(out)
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
}
