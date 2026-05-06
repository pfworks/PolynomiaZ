/// Image codec: compress 2D grayscale images (space probe camera data)
/// using the platter model where each image row = one track.
///
/// Supports:
/// - Lossless mode (exact reconstruction)
/// - Lossy mode with configurable quality (MSE threshold)
/// - 8-bit and 16-bit pixel depths
/// - Row-by-row pattern detection optimized for space imagery
///
/// Image format (PLTI):
///   [4] Magic "PLTI"
///   [4] Original data length (bytes)
///   [2] Width (pixels)
///   [2] Height (pixels)
///   [1] Bit depth (8 or 16)
///   [1] Quality (0=lossless, 1-255=lossy threshold)
///   [2] Reserved
///   [...] PLTZ-encoded row data

use crate::codec::{encode, decode, RawCompressor};

const IMAGE_MAGIC: &[u8; 4] = b"PLTI";

#[derive(Debug, Clone, Copy)]
pub struct ImageParams {
    pub width: u16,
    pub height: u16,
    pub bit_depth: u8,    // 8 or 16
    pub quality: u8,      // 0 = lossless, 1-255 = max allowed error per pixel
}

/// Compress a raw image (row-major pixel data).
pub fn compress_image(pixels: &[u8], params: ImageParams) -> Vec<u8> {
    let row_bytes = params.width as usize * (params.bit_depth as usize / 8);

    if params.quality == 0 {
        // Lossless: use standard PLTZ with sector size = row width
        let compressed_inner = encode(pixels, RawCompressor::Zlib);
        let mut buf = Vec::new();
        buf.extend_from_slice(IMAGE_MAGIC);
        buf.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        buf.extend_from_slice(&params.width.to_le_bytes());
        buf.extend_from_slice(&params.height.to_le_bytes());
        buf.push(params.bit_depth);
        buf.push(0); // quality = lossless
        buf.extend_from_slice(&0u16.to_le_bytes()); // reserved
        buf.extend_from_slice(&compressed_inner);
        return buf;
    }

    // Lossy mode: fit functions per row, accept if error <= quality threshold
    let mut buf = Vec::new();
    buf.extend_from_slice(IMAGE_MAGIC);
    buf.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    buf.extend_from_slice(&params.width.to_le_bytes());
    buf.extend_from_slice(&params.height.to_le_bytes());
    buf.push(params.bit_depth);
    buf.push(params.quality);
    buf.extend_from_slice(&0u16.to_le_bytes());

    // Encode each row
    let rows: Vec<&[u8]> = (0..params.height as usize)
        .map(|r| &pixels[r * row_bytes..(r + 1) * row_bytes])
        .collect();

    // Try lossy pattern fitting per row
    let mut encodings: Vec<RowEncoding> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let enc = fit_row_lossy(row, params.quality, params.bit_depth);
        encodings.push(RowEncoding { row_idx: i as u16, enc });
    }

    // Cross-row optimization: group identical encodings
    let (groups, remaining) = cross_row_optimize(&encodings);

    // Write entry count
    let entry_count = groups.len() + remaining.len();
    buf.extend_from_slice(&(entry_count as u16).to_le_bytes());

    // Write meta-groups
    for group in &groups {
        buf.push(0xFF); // meta marker
        buf.push(group.pattern_tag);
        buf.extend_from_slice(&(group.row_indices.len() as u16).to_le_bytes());
        for &idx in &group.row_indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        buf.extend_from_slice(&(group.params.len() as u16).to_le_bytes());
        buf.extend_from_slice(&group.params);
    }

    // Write individual rows
    for row_enc in &remaining {
        buf.extend_from_slice(&row_enc.row_idx.to_le_bytes());
        buf.push(row_enc.enc.tag);
        buf.extend_from_slice(&(row_enc.enc.params.len() as u16).to_le_bytes());
        buf.extend_from_slice(&row_enc.enc.params);
    }

    buf
}

/// Decompress an image.
pub fn decompress_image(compressed: &[u8]) -> Result<(Vec<u8>, ImageParams), String> {
    if compressed.len() < 16 || &compressed[0..4] != IMAGE_MAGIC {
        return Err("Bad image magic".into());
    }

    let original_len = u32::from_le_bytes(compressed[4..8].try_into().unwrap()) as usize;
    let width = u16::from_le_bytes(compressed[8..10].try_into().unwrap());
    let height = u16::from_le_bytes(compressed[10..12].try_into().unwrap());
    let bit_depth = compressed[12];
    let quality = compressed[13];

    let params = ImageParams { width, height, bit_depth, quality };
    let row_bytes = width as usize * (bit_depth as usize / 8);

    if quality == 0 {
        // Lossless: inner is standard PLTZ
        let inner = &compressed[16..];
        let pixels = decode(inner)?;
        return Ok((pixels, params));
    }

    // Lossy decode
    let mut pixels = vec![0u8; original_len];
    let mut offset = 16;

    let entry_count = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
    offset += 2;

    for _ in 0..entry_count {
        if compressed[offset] == 0xFF {
            // Meta-group
            offset += 1;
            let tag = compressed[offset]; offset += 1;
            let count = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            let mut indices = Vec::with_capacity(count);
            for _ in 0..count {
                indices.push(u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()));
                offset += 2;
            }
            let param_len = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            let param_data = &compressed[offset..offset + param_len];
            offset += param_len;

            let row = reconstruct_row(tag, param_data, row_bytes, bit_depth);
            for &idx in &indices {
                let start = idx as usize * row_bytes;
                pixels[start..start + row_bytes].copy_from_slice(&row);
            }
        } else {
            // Individual row
            let row_idx = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            let tag = compressed[offset]; offset += 1;
            let param_len = u16::from_le_bytes(compressed[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            let param_data = &compressed[offset..offset + param_len];
            offset += param_len;

            let row = reconstruct_row(tag, param_data, row_bytes, bit_depth);
            let start = row_idx * row_bytes;
            pixels[start..start + row_bytes].copy_from_slice(&row);
        }
    }

    Ok((pixels, params))
}

// --- Internal types ---

#[derive(Clone)]
struct LossyEncoding {
    tag: u8,
    params: Vec<u8>,
}

struct RowEncoding {
    row_idx: u16,
    enc: LossyEncoding,
}

struct RowMetaGroup {
    pattern_tag: u8,
    row_indices: Vec<u16>,
    params: Vec<u8>,
}

// --- Lossy pattern tags ---
const TAG_CONST: u8 = 0;
const TAG_LINEAR: u8 = 1;
const TAG_POLY2: u8 = 2;   // quadratic
const TAG_POLY3: u8 = 3;   // cubic
const TAG_RAW: u8 = 7;

// --- Lossy fitting ---

fn fit_row_lossy(row: &[u8], quality: u8, bit_depth: u8) -> LossyEncoding {
    let threshold = quality as f64;

    // Try CONST (mean value)
    if let Some(enc) = try_const_lossy(row, threshold, bit_depth) {
        return enc;
    }

    // Try LINEAR (least-squares line)
    if let Some(enc) = try_linear_lossy(row, threshold, bit_depth) {
        return enc;
    }

    // Try POLY2 (quadratic)
    if let Some(enc) = try_poly_lossy(row, threshold, bit_depth, 2) {
        return enc;
    }

    // Try POLY3 (cubic)
    if let Some(enc) = try_poly_lossy(row, threshold, bit_depth, 3) {
        return enc;
    }

    // Fallback: store raw
    LossyEncoding { tag: TAG_RAW, params: row.to_vec() }
}

fn try_const_lossy(row: &[u8], threshold: f64, bit_depth: u8) -> Option<LossyEncoding> {
    if bit_depth == 16 {
        let n = row.len() / 2;
        let values: Vec<u16> = (0..n)
            .map(|i| u16::from_le_bytes([row[i * 2], row[i * 2 + 1]]))
            .collect();
        let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
        let max_err = values.iter().map(|&v| (v as f64 - mean).abs()).fold(0.0f64, f64::max);
        if max_err <= threshold {
            let val = mean.round() as u16;
            return Some(LossyEncoding { tag: TAG_CONST, params: val.to_le_bytes().to_vec() });
        }
    } else {
        let mean = row.iter().map(|&v| v as f64).sum::<f64>() / row.len() as f64;
        let max_err = row.iter().map(|&v| (v as f64 - mean).abs()).fold(0.0f64, f64::max);
        if max_err <= threshold {
            return Some(LossyEncoding { tag: TAG_CONST, params: vec![mean.round() as u8] });
        }
    }
    None
}

fn try_linear_lossy(row: &[u8], threshold: f64, bit_depth: u8) -> Option<LossyEncoding> {
    let (values, n) = extract_values(row, bit_depth);

    // Least-squares linear fit: y = a + b*x
    let n_f = n as f64;
    let sx: f64 = (0..n).map(|i| i as f64).sum();
    let sy: f64 = values.iter().sum();
    let sxx: f64 = (0..n).map(|i| (i as f64) * (i as f64)).sum();
    let sxy: f64 = (0..n).map(|i| (i as f64) * values[i]).sum();

    let denom = n_f * sxx - sx * sx;
    if denom.abs() < 1e-10 { return None; }

    let b = (n_f * sxy - sx * sy) / denom;
    let a = (sy - b * sx) / n_f;

    let max_err = (0..n).map(|i| (values[i] - (a + b * i as f64)).abs()).fold(0.0f64, f64::max);
    if max_err <= threshold {
        let mut params = Vec::new();
        params.extend_from_slice(&a.to_le_bytes());
        params.extend_from_slice(&b.to_le_bytes());
        return Some(LossyEncoding { tag: TAG_LINEAR, params });
    }
    None
}

fn try_poly_lossy(row: &[u8], threshold: f64, bit_depth: u8, degree: usize) -> Option<LossyEncoding> {
    let (values, n) = extract_values(row, bit_depth);
    if n <= degree { return None; }

    // Least-squares polynomial fit via normal equations
    let m = degree + 1;
    let mut ata = vec![0.0f64; m * m];
    let mut aty = vec![0.0f64; m];

    for i in 0..n {
        let x = i as f64 / n as f64; // normalize to [0,1] for numerical stability
        let y = values[i];
        let mut xpow = vec![1.0f64; m];
        for j in 1..m { xpow[j] = xpow[j - 1] * x; }
        for r in 0..m {
            aty[r] += xpow[r] * y;
            for c in 0..m {
                ata[r * m + c] += xpow[r] * xpow[c];
            }
        }
    }

    // Solve via Gaussian elimination
    let coeffs = solve_linear_system(&ata, &aty, m)?;

    // Check max error
    let max_err = (0..n).map(|i| {
        let x = i as f64 / n as f64;
        let mut val = 0.0f64;
        let mut xpow = 1.0f64;
        for &c in &coeffs {
            val += c * xpow;
            xpow *= x;
        }
        (values[i] - val).abs()
    }).fold(0.0f64, f64::max);

    if max_err <= threshold {
        let tag = if degree == 2 { TAG_POLY2 } else { TAG_POLY3 };
        let mut params = Vec::new();
        for &c in &coeffs {
            params.extend_from_slice(&c.to_le_bytes());
        }
        return Some(LossyEncoding { tag, params });
    }
    None
}

fn extract_values(row: &[u8], bit_depth: u8) -> (Vec<f64>, usize) {
    if bit_depth == 16 {
        let n = row.len() / 2;
        let values: Vec<f64> = (0..n)
            .map(|i| u16::from_le_bytes([row[i * 2], row[i * 2 + 1]]) as f64)
            .collect();
        (values, n)
    } else {
        let values: Vec<f64> = row.iter().map(|&v| v as f64).collect();
        let n = values.len();
        (values, n)
    }
}

fn solve_linear_system(ata: &[f64], aty: &[f64], m: usize) -> Option<Vec<f64>> {
    let mut aug = vec![0.0f64; m * (m + 1)];
    for i in 0..m {
        for j in 0..m { aug[i * (m + 1) + j] = ata[i * m + j]; }
        aug[i * (m + 1) + m] = aty[i];
    }

    for col in 0..m {
        let mut max_row = col;
        let mut max_val = aug[col * (m + 1) + col].abs();
        for row in (col + 1)..m {
            let val = aug[row * (m + 1) + col].abs();
            if val > max_val { max_val = val; max_row = row; }
        }
        if max_val < 1e-12 { return None; }
        if max_row != col {
            for j in 0..=m { aug.swap(col * (m + 1) + j, max_row * (m + 1) + j); }
        }
        let pivot = aug[col * (m + 1) + col];
        for j in col..=m { aug[col * (m + 1) + j] /= pivot; }
        for row in 0..m {
            if row == col { continue; }
            let factor = aug[row * (m + 1) + col];
            for j in col..=m { aug[row * (m + 1) + j] -= factor * aug[col * (m + 1) + j]; }
        }
    }

    Some((0..m).map(|i| aug[i * (m + 1) + m]).collect())
}

fn reconstruct_row(tag: u8, params: &[u8], row_bytes: usize, bit_depth: u8) -> Vec<u8> {
    let n = if bit_depth == 16 { row_bytes / 2 } else { row_bytes };

    match tag {
        TAG_CONST => {
            if bit_depth == 16 {
                let val = u16::from_le_bytes([params[0], params[1]]);
                let mut row = Vec::with_capacity(row_bytes);
                for _ in 0..n { row.extend_from_slice(&val.to_le_bytes()); }
                row
            } else {
                vec![params[0]; n]
            }
        }
        TAG_LINEAR => {
            let a = f64::from_le_bytes(params[0..8].try_into().unwrap());
            let b = f64::from_le_bytes(params[8..16].try_into().unwrap());
            let mut row = Vec::with_capacity(row_bytes);
            for i in 0..n {
                let val = (a + b * i as f64).round();
                if bit_depth == 16 {
                    row.extend_from_slice(&(val.clamp(0.0, 65535.0) as u16).to_le_bytes());
                } else {
                    row.push(val.clamp(0.0, 255.0) as u8);
                }
            }
            row
        }
        TAG_POLY2 | TAG_POLY3 => {
            let degree = if tag == TAG_POLY2 { 2 } else { 3 };
            let m = degree + 1;
            let coeffs: Vec<f64> = (0..m)
                .map(|i| f64::from_le_bytes(params[i * 8..(i + 1) * 8].try_into().unwrap()))
                .collect();
            let mut row = Vec::with_capacity(row_bytes);
            for i in 0..n {
                let x = i as f64 / n as f64;
                let mut val = 0.0f64;
                let mut xpow = 1.0f64;
                for &c in &coeffs {
                    val += c * xpow;
                    xpow *= x;
                }
                if bit_depth == 16 {
                    row.extend_from_slice(&(val.round().clamp(0.0, 65535.0) as u16).to_le_bytes());
                } else {
                    row.push(val.round().clamp(0.0, 255.0) as u8);
                }
            }
            row
        }
        TAG_RAW => params.to_vec(),
        _ => params.to_vec(),
    }
}

fn cross_row_optimize(encodings: &[RowEncoding]) -> (Vec<RowMetaGroup>, Vec<&RowEncoding>) {
    // Group rows with identical tag + params
    let mut groups: std::collections::HashMap<(u8, &[u8]), Vec<u16>> = std::collections::HashMap::new();
    for enc in encodings {
        groups.entry((enc.enc.tag, enc.enc.params.as_slice()))
            .or_default()
            .push(enc.row_idx);
    }

    let mut meta_groups = Vec::new();
    let mut remaining_indices: std::collections::HashSet<u16> = std::collections::HashSet::new();

    for ((tag, params), indices) in &groups {
        if indices.len() >= 4 {
            meta_groups.push(RowMetaGroup {
                pattern_tag: *tag,
                row_indices: indices.clone(),
                params: params.to_vec(),
            });
        } else {
            for &idx in indices {
                remaining_indices.insert(idx);
            }
        }
    }

    let remaining: Vec<&RowEncoding> = encodings.iter()
        .filter(|e| remaining_indices.contains(&e.row_idx))
        .collect();

    (meta_groups, remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lossless_roundtrip() {
        // 8x8 gradient image
        let mut pixels = Vec::new();
        for row in 0..8u8 {
            for col in 0..8u8 {
                pixels.push(row * 32 + col * 4);
            }
        }
        let params = ImageParams { width: 8, height: 8, bit_depth: 8, quality: 0 };
        let compressed = compress_image(&pixels, params);
        let (decompressed, _) = decompress_image(&compressed).unwrap();
        assert_eq!(pixels, decompressed);
    }

    #[test]
    fn test_lossy_star_field() {
        // 256x64 star field: mostly black with a few bright pixels
        let width = 256u16;
        let height = 64u16;
        let mut pixels = vec![0u8; width as usize * height as usize];
        // Add a few "stars"
        pixels[128] = 200;
        pixels[256 * 10 + 50] = 255;
        pixels[256 * 30 + 200] = 180;

        let params = ImageParams { width, height, bit_depth: 8, quality: 5 };
        let compressed = compress_image(&pixels, params);
        let (decompressed, _) = decompress_image(&compressed).unwrap();

        // Check max error <= quality
        let max_err = pixels.iter().zip(decompressed.iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs())
            .max().unwrap();
        assert!(max_err <= 5, "Max error {} > quality 5", max_err);

        let ratio = compressed.len() as f64 / pixels.len() as f64;
        println!("Star field 256x64: {} -> {} bytes (ratio {:.3})",
            pixels.len(), compressed.len(), ratio);
        assert!(compressed.len() < pixels.len() / 4,
            "Star field should compress >4:1, got {}", compressed.len());
    }

    #[test]
    fn test_lossy_gradient() {
        // 512x16 smooth gradient (simulates illumination gradient)
        let width = 512u16;
        let height = 16u16;
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for _row in 0..height {
            for col in 0..width {
                pixels.push((col / 2) as u8);
            }
        }

        let params = ImageParams { width, height, bit_depth: 8, quality: 2 };
        let compressed = compress_image(&pixels, params);
        let (decompressed, _) = decompress_image(&compressed).unwrap();

        let max_err = pixels.iter().zip(decompressed.iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs())
            .max().unwrap();
        assert!(max_err <= 2);

        println!("Gradient 512x16: {} -> {} bytes (ratio {:.3})",
            pixels.len(), compressed.len(), compressed.len() as f64 / pixels.len() as f64);
    }

    #[test]
    fn test_16bit_calibration() {
        // 128x32 dark frame (16-bit, values near 100 with ±3 noise)
        let width = 128u16;
        let height = 32u16;
        let mut pixels = Vec::new();
        let mut state: u32 = 99;
        for _ in 0..(width as usize * height as usize) {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let noise = ((state >> 16) % 7) as i16 - 3; // -3 to +3
            let val = (100i16 + noise) as u16;
            pixels.extend_from_slice(&val.to_le_bytes());
        }

        let params = ImageParams { width, height, bit_depth: 16, quality: 4 };
        let compressed = compress_image(&pixels, params);
        let (decompressed, _) = decompress_image(&compressed).unwrap();

        // Check error in 16-bit values
        let n = width as usize * height as usize;
        let max_err = (0..n).map(|i| {
            let orig = u16::from_le_bytes([pixels[i * 2], pixels[i * 2 + 1]]);
            let recon = u16::from_le_bytes([decompressed[i * 2], decompressed[i * 2 + 1]]);
            (orig as i32 - recon as i32).unsigned_abs()
        }).max().unwrap();
        assert!(max_err <= 4, "Max error {} > quality 4", max_err);

        println!("Dark frame 128x32 16-bit: {} -> {} bytes (ratio {:.3})",
            pixels.len(), compressed.len(), compressed.len() as f64 / pixels.len() as f64);
    }
}
