/// Pattern detection for track data.
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PatternType {
    Const = 0,
    Linear = 1,
    Rle = 2,
    Repeat = 3,
    Delta = 4,
    Periodic = 5,
    Poly = 6,
    Raw = 7,
    RawCompressed = 8,
    LinearWide = 9,
    LinearF64 = 10,
    DeltaF64 = 11,
}

impl PatternType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Const),
            1 => Some(Self::Linear),
            2 => Some(Self::Rle),
            3 => Some(Self::Repeat),
            4 => Some(Self::Delta),
            5 => Some(Self::Periodic),
            6 => Some(Self::Poly),
            7 => Some(Self::Raw),
            8 => Some(Self::RawCompressed),
            9 => Some(Self::LinearWide),
            10 => Some(Self::LinearF64),
            11 => Some(Self::DeltaF64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TrackParams {
    Const { value: u8 },
    Linear { start: i16, step: i16 },
    Rle { runs: Vec<(u8, u8)> },
    Repeat { pattern: Vec<u8> },
    Delta { start: i16, deltas: Vec<i8> },
    Periodic { components: Vec<(u16, f64, f64)>, n: u16 },
    Poly { degree: u8, coeffs: Vec<f64> },
    Raw { data: Vec<u8> },
    LinearWide { width: u8, count: u16, start: i64, step: i64 },
    LinearF64 { count: u16, start: f64, step: f64 },
    DeltaF64 { count: u16, start: f64, deltas_f32: Vec<f32> },
}

#[derive(Debug, Clone)]
pub struct TrackEncoding {
    pub pattern: PatternType,
    pub params: TrackParams,
    pub track_idx: u16,
}

fn detect_const(track: &[u8]) -> Option<TrackParams> {
    let first = track[0];
    if track.iter().all(|&b| b == first) {
        Some(TrackParams::Const { value: first })
    } else {
        None
    }
}

fn detect_linear(track: &[u8]) -> Option<TrackParams> {
    if track.len() < 2 {
        return None;
    }
    let start = track[0] as i16;
    let step = track[1] as i16 - start;
    for i in 2..track.len() {
        if track[i] as i16 - track[i - 1] as i16 != step {
            return None;
        }
    }
    Some(TrackParams::Linear { start, step })
}

fn detect_rle(track: &[u8]) -> Option<TrackParams> {
    let mut runs: Vec<(u8, u8)> = Vec::new();
    let mut i = 0;
    while i < track.len() {
        let val = track[i];
        let mut count: u8 = 1;
        while i + (count as usize) < track.len()
            && track[i + count as usize] == val
            && count < 255
        {
            count += 1;
        }
        runs.push((val, count));
        i += count as usize;
    }
    if runs.len() * 2 < track.len() {
        Some(TrackParams::Rle { runs })
    } else {
        None
    }
}

fn detect_repeat(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    for period in 1..=(n / 2) {
        let pattern = &track[..period];
        if track.iter().enumerate().all(|(i, &b)| b == pattern[i % period]) {
            if period < n / 3 {
                return Some(TrackParams::Repeat { pattern: pattern.to_vec() });
            }
        }
    }
    None
}

fn detect_delta(track: &[u8]) -> Option<TrackParams> {
    let mut deltas: Vec<i8> = Vec::with_capacity(track.len() - 1);
    let mut unique: HashSet<i8> = HashSet::new();
    for i in 1..track.len() {
        let d = track[i] as i16 - track[i - 1] as i16;
        if d < -128 || d > 127 {
            return None;
        }
        let d8 = d as i8;
        deltas.push(d8);
        unique.insert(d8);
    }
    if unique.len() <= 8 {
        Some(TrackParams::Delta {
            start: track[0] as i16,
            deltas,
        })
    } else {
        None
    }
}

fn detect_linear_wide(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();

    // Try 32-bit LE
    if n >= 8 && n % 4 == 0 {
        let count = n / 4;
        let words: Vec<i64> = (0..count)
            .map(|i| u32::from_le_bytes([
                track[i * 4], track[i * 4 + 1], track[i * 4 + 2], track[i * 4 + 3],
            ]) as i64)
            .collect();
        if let Some((start, step)) = check_linear_i64(&words) {
            return Some(TrackParams::LinearWide {
                width: 4,
                count: count as u16,
                start,
                step,
            });
        }
    }

    // Try 16-bit LE
    if n >= 4 && n % 2 == 0 {
        let count = n / 2;
        let words: Vec<i64> = (0..count)
            .map(|i| u16::from_le_bytes([track[i * 2], track[i * 2 + 1]]) as i64)
            .collect();
        if let Some((start, step)) = check_linear_i64(&words) {
            return Some(TrackParams::LinearWide {
                width: 2,
                count: count as u16,
                start,
                step,
            });
        }
    }

    None
}

fn check_linear_i64(words: &[i64]) -> Option<(i64, i64)> {
    if words.len() < 2 {
        return None;
    }
    let start = words[0];
    let step = words[1] - start;
    for i in 2..words.len() {
        if words[i] - words[i - 1] != step {
            return None;
        }
    }
    Some((start, step))
}

fn detect_linear_f64(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 16 || n % 8 != 0 {
        return None;
    }
    let count = n / 8;
    let doubles: Vec<f64> = (0..count)
        .map(|i| f64::from_le_bytes(track[i * 8..(i + 1) * 8].try_into().unwrap()))
        .collect();

    // Check for NaN/Inf
    if doubles.iter().any(|&d| d.is_nan() || d.is_infinite()) {
        return None;
    }

    if count < 2 {
        return None;
    }

    let start = doubles[0];
    let step = doubles[1] - start;

    for i in 2..count {
        let expected_step = doubles[i] - doubles[i - 1];
        if (expected_step - step).abs() > step.abs() * 1e-12 + 1e-15 {
            return None;
        }
    }

    // Verify byte-exact reconstruction
    for i in 0..count {
        let reconstructed = start + (i as f64) * step;
        let orig_bytes = &track[i * 8..(i + 1) * 8];
        let recon_bytes = reconstructed.to_le_bytes();
        if orig_bytes != recon_bytes {
            return None;
        }
    }

    if 18 < n {
        Some(TrackParams::LinearF64 { count: count as u16, start, step })
    } else {
        None
    }
}

fn detect_delta_f64(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 16 || n % 8 != 0 {
        return None;
    }
    let count = n / 8;
    let doubles: Vec<f64> = (0..count)
        .map(|i| f64::from_le_bytes(track[i * 8..(i + 1) * 8].try_into().unwrap()))
        .collect();

    if doubles.iter().any(|&d| d.is_nan() || d.is_infinite()) {
        return None;
    }

    let mut deltas_f32: Vec<f32> = Vec::with_capacity(count - 1);
    for i in 1..count {
        let d = doubles[i] - doubles[i - 1];
        if d.abs() > 3.4e38 {
            return None;
        }
        let f32_val = d as f32;
        if ((f32_val as f64) - d).abs() > d.abs() * 1e-6 + 1e-30 {
            return None;
        }
        deltas_f32.push(f32_val);
    }

    // Cost check
    let cost = 10 + (count - 1) * 4; // 2 (count) + 8 (start) + deltas*4
    if cost >= n {
        return None;
    }

    // Verify byte-exact reconstruction
    let mut current = doubles[0];
    for i in 0..deltas_f32.len() {
        current += deltas_f32[i] as f64;
        let orig_bytes = &track[(i + 1) * 8..(i + 2) * 8];
        let recon_bytes = current.to_le_bytes();
        if orig_bytes != recon_bytes {
            return None;
        }
    }

    Some(TrackParams::DeltaF64 {
        count: count as u16,
        start: doubles[0],
        deltas_f32,
    })
}

/// Compute real DFT of integer data (rfft equivalent).
/// Returns (real, imag) arrays of length n/2+1.
fn rfft(data: &[u8]) -> (Vec<f64>, Vec<f64>) {
    let n = data.len();
    let fft_len = n / 2 + 1;
    let mut real = vec![0.0f64; fft_len];
    let mut imag = vec![0.0f64; fft_len];

    for k in 0..fft_len {
        for t in 0..n {
            let angle = 2.0 * std::f64::consts::PI * (k as f64) * (t as f64) / (n as f64);
            real[k] += data[t] as f64 * angle.cos();
            imag[k] -= data[t] as f64 * angle.sin();
        }
    }
    (real, imag)
}

/// Inverse real DFT — reconstruct n samples from rfft components.
fn irfft_reconstruct(real: &[f64], imag: &[f64], n: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(n);
    for t in 0..n {
        let mut sum = real[0]; // DC (not doubled)
        for k in 1..real.len() - 1 {
            let angle = 2.0 * std::f64::consts::PI * (k as f64) * (t as f64) / (n as f64);
            sum += 2.0 * (real[k] * angle.cos() - imag[k] * angle.sin());
        }
        // Nyquist (not doubled if n is even)
        if n % 2 == 0 && real.len() > 1 {
            let k = real.len() - 1;
            let angle = 2.0 * std::f64::consts::PI * (k as f64) * (t as f64) / (n as f64);
            sum += real[k] * angle.cos() - imag[k] * angle.sin();
        }
        result.push((sum / n as f64).round() as u8);
    }
    result
}

fn detect_periodic(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 16 || n > 128 {
        return None; // DFT is O(n²); skip large tracks
    }

    let (real, imag) = rfft(track);
    let fft_len = real.len();

    // Compute magnitudes and sort by descending magnitude
    let mut mag_indices: Vec<(usize, f64)> = (0..fft_len)
        .map(|k| (k, (real[k] * real[k] + imag[k] * imag[k]).sqrt()))
        .collect();
    mag_indices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Try keeping progressively more components until exact reconstruction
    for num_keep in 1..std::cmp::min(fft_len, n / 4) {
        let kept: Vec<usize> = mag_indices[..num_keep].iter().map(|&(k, _)| k).collect();

        // Build sparse rfft
        let mut sparse_real = vec![0.0f64; fft_len];
        let mut sparse_imag = vec![0.0f64; fft_len];
        for &k in &kept {
            sparse_real[k] = real[k];
            sparse_imag[k] = imag[k];
        }

        let reconstructed = irfft_reconstruct(&sparse_real, &sparse_imag, n);
        if reconstructed == track {
            // Cost: 4 bytes header + 18 bytes per component
            let cost = 4 + num_keep * 18;
            if cost < n {
                let components: Vec<(u16, f64, f64)> = kept.iter()
                    .map(|&k| (k as u16, real[k], imag[k]))
                    .collect();
                return Some(TrackParams::Periodic { components, n: n as u16 });
            }
            return None; // exact but not worth it
        }
    }
    None
}

fn detect_poly(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 4 {
        return None;
    }

    // Try polynomial degrees 2 through 8
    for degree in 2..=std::cmp::min(8, n - 1) {
        if let Some(coeffs) = polyfit_exact(track, degree) {
            // Cost: 1 byte (degree) + (degree+1)*8 bytes (f64 coefficients)
            let cost = 1 + (degree + 1) * 8;
            if cost < n {
                return Some(TrackParams::Poly {
                    degree: degree as u8,
                    coeffs,
                });
            }
        }
    }
    None
}

/// Fit a polynomial of given degree to the track data.
/// Returns coefficients [c_d, c_{d-1}, ..., c_0] (highest degree first) if exact.
fn polyfit_exact(track: &[u8], degree: usize) -> Option<Vec<f64>> {
    let n = track.len();
    if degree >= n {
        return None;
    }

    // Build Vandermonde system and solve via Gaussian elimination
    let m = degree + 1;
    // Normal equations: (X^T X) c = X^T y
    // X[i][j] = i^(degree-j) for row i, col j
    let mut ata = vec![0.0f64; m * m];
    let mut aty = vec![0.0f64; m];

    for i in 0..n {
        let x = i as f64;
        let y = track[i] as f64;
        // Compute powers of x
        let mut powers = vec![1.0f64; m];
        for j in 1..m {
            powers[j] = powers[j - 1] * x;
        }
        // powers[j] = x^j, but we want coefficient order [x^d, x^(d-1), ..., x^0]
        // So basis[col] = x^(degree - col)
        for col in 0..m {
            let basis_col = powers[degree - col];
            aty[col] += basis_col * y;
            for row in 0..m {
                let basis_row = powers[degree - row];
                ata[row * m + col] += basis_row * basis_col;
            }
        }
    }

    // Solve via Gaussian elimination with partial pivoting
    let mut aug = vec![0.0f64; m * (m + 1)];
    for i in 0..m {
        for j in 0..m {
            aug[i * (m + 1) + j] = ata[i * m + j];
        }
        aug[i * (m + 1) + m] = aty[i];
    }

    for col in 0..m {
        // Partial pivot
        let mut max_row = col;
        let mut max_val = aug[col * (m + 1) + col].abs();
        for row in (col + 1)..m {
            let val = aug[row * (m + 1) + col].abs();
            if val > max_val {
                max_val = val;
                max_row = row;
            }
        }
        if max_val < 1e-12 {
            return None; // singular
        }
        if max_row != col {
            for j in 0..=(m) {
                aug.swap(col * (m + 1) + j, max_row * (m + 1) + j);
            }
        }
        let pivot = aug[col * (m + 1) + col];
        for j in col..=(m) {
            aug[col * (m + 1) + j] /= pivot;
        }
        for row in 0..m {
            if row == col { continue; }
            let factor = aug[row * (m + 1) + col];
            for j in col..=(m) {
                aug[row * (m + 1) + j] -= factor * aug[col * (m + 1) + j];
            }
        }
    }

    let coeffs: Vec<f64> = (0..m).map(|i| aug[i * (m + 1) + m]).collect();

    // Verify exact reconstruction
    for i in 0..n {
        let x = i as f64;
        let mut val = 0.0f64;
        for (j, &c) in coeffs.iter().enumerate() {
            val += c * x.powi((degree - j) as i32);
        }
        if val.round() as i64 != track[i] as i64 {
            return None;
        }
    }

    Some(coeffs)
}

pub fn analyze_track(track: &[u8], track_idx: u16) -> TrackEncoding {
    if let Some(params) = detect_const(track) {
        return TrackEncoding { pattern: PatternType::Const, params, track_idx };
    }
    if let Some(params) = detect_linear(track) {
        return TrackEncoding { pattern: PatternType::Linear, params, track_idx };
    }

    // Try both RLE and REPEAT, pick smaller
    let rle = detect_rle(track);
    let repeat = detect_repeat(track);
    match (&rle, &repeat) {
        (Some(TrackParams::Rle { runs }), Some(TrackParams::Repeat { pattern })) => {
            let rle_cost = 2 + runs.len() * 2;
            let repeat_cost = 2 + pattern.len();
            if repeat_cost <= rle_cost {
                return TrackEncoding { pattern: PatternType::Repeat, params: repeat.unwrap(), track_idx };
            } else {
                return TrackEncoding { pattern: PatternType::Rle, params: rle.unwrap(), track_idx };
            }
        }
        (Some(_), None) => {
            return TrackEncoding { pattern: PatternType::Rle, params: rle.unwrap(), track_idx };
        }
        (None, Some(_)) => {
            return TrackEncoding { pattern: PatternType::Repeat, params: repeat.unwrap(), track_idx };
        }
        _ => {}
    }

    if let Some(params) = detect_linear_wide(track) {
        return TrackEncoding { pattern: PatternType::LinearWide, params, track_idx };
    }
    if let Some(params) = detect_linear_f64(track) {
        return TrackEncoding { pattern: PatternType::LinearF64, params, track_idx };
    }
    if let Some(params) = detect_delta_f64(track) {
        return TrackEncoding { pattern: PatternType::DeltaF64, params, track_idx };
    }
    if let Some(params) = detect_delta(track) {
        return TrackEncoding { pattern: PatternType::Delta, params, track_idx };
    }
    if let Some(params) = detect_periodic(track) {
        return TrackEncoding { pattern: PatternType::Periodic, params, track_idx };
    }
    if let Some(params) = detect_poly(track) {
        return TrackEncoding { pattern: PatternType::Poly, params, track_idx };
    }

    TrackEncoding {
        pattern: PatternType::Raw,
        params: TrackParams::Raw { data: track.to_vec() },
        track_idx,
    }
}

pub fn analyze_platter(tracks: &[Vec<u8>]) -> Vec<TrackEncoding> {
    tracks.iter().enumerate()
        .map(|(i, track)| analyze_track(track, i as u16))
        .collect()
}
