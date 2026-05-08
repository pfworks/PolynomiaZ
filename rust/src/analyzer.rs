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
    XorMask = 13,
    Sparse = 14,
    PiecewiseLinear = 15,
    Mirror = 16,
    InterleavedLinear = 17,
    Exponential = 18,
    DeltaRle = 19,
    BitPacked = 20,
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
            13 => Some(Self::XorMask),
            14 => Some(Self::Sparse),
            15 => Some(Self::PiecewiseLinear),
            16 => Some(Self::Mirror),
            17 => Some(Self::InterleavedLinear),
            18 => Some(Self::Exponential),
            19 => Some(Self::DeltaRle),
            20 => Some(Self::BitPacked),
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
    /// XOR mask: data XORed with `mask` yields an inner pattern (tag + params)
    XorMask { mask: u8, inner_tag: u8, inner_params: Vec<u8> },
    /// Sparse: mostly one fill value with a few exceptions at given indices
    Sparse { fill: u8, entries: Vec<(u16, u8)> },
    /// Piecewise linear: 2-4 segments each with start_idx, start_val, step
    PiecewiseLinear { segments: Vec<(u16, i16, i16)> },
    /// Mirror: first half stored, second half is reverse of first
    Mirror { half: Vec<u8> },
    /// Interleaved linear: 2 interleaved linear sequences
    InterleavedLinear { count: u16, start_a: i16, step_a: i16, start_b: i16, step_b: i16 },
    /// Exponential/geometric: a * r^i (stored as f32 base + f32 ratio)
    Exponential { count: u16, base: f32, ratio: f32 },
    /// Delta-RLE: deltas between values are run-length encoded
    DeltaRle { start: u8, runs: Vec<(i8, u8)> },
    /// BitPacked: all values fit in fewer than 8 bits, stored packed
    BitPacked { bits: u8, packed: Vec<u8> },
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

/// Compute real FFT using rustfft. Returns (real, imag) arrays of length n/2+1.
fn rfft(data: &[u8]) -> (Vec<f64>, Vec<f64>) {
    use rustfft::{FftPlanner, num_complex::Complex};

    let n = data.len();
    let fft_len = n / 2 + 1;

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);

    let mut buffer: Vec<Complex<f64>> = data.iter()
        .map(|&b| Complex { re: b as f64, im: 0.0 })
        .collect();

    fft.process(&mut buffer);

    let real: Vec<f64> = buffer[..fft_len].iter().map(|c| c.re).collect();
    let imag: Vec<f64> = buffer[..fft_len].iter().map(|c| c.im).collect();
    (real, imag)
}

/// Inverse real FFT — reconstruct n samples from rfft components.
fn irfft_reconstruct(real: &[f64], imag: &[f64], n: usize) -> Vec<u8> {
    use rustfft::{FftPlanner, num_complex::Complex};

    let mut planner = FftPlanner::<f64>::new();
    let ifft = planner.plan_fft_inverse(n);

    // Build full complex spectrum from half-spectrum
    let mut buffer: Vec<Complex<f64>> = Vec::with_capacity(n);
    let fft_len = real.len(); // n/2 + 1
    for k in 0..n {
        if k < fft_len {
            buffer.push(Complex { re: real[k], im: imag[k] });
        } else {
            // Mirror: X[n-k] = conj(X[k])
            let mirror = n - k;
            buffer.push(Complex { re: real[mirror], im: -imag[mirror] });
        }
    }

    ifft.process(&mut buffer);

    // IFFT result needs to be divided by n
    buffer.iter().map(|c| (c.re / n as f64).round() as u8).collect()
}

fn detect_periodic(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 16 || n > 4096 {
        return None; // skip very large tracks (diminishing returns)
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

/// XOR mask: try XORing with common masks and see if the result is a simpler pattern.
fn detect_xor_mask(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 { return None; }

    for &mask in &[0xFF, 0xAA, 0x55, 0x0F, 0xF0] {
        let unmasked: Vec<u8> = track.iter().map(|&b| b ^ mask).collect();

        // Check if unmasked is CONST
        if unmasked.iter().all(|&b| b == unmasked[0]) {
            // Cost: 1 (mask) + 1 (inner_tag) + 1 (value) = 3 bytes
            if 3 < n {
                return Some(TrackParams::XorMask {
                    mask,
                    inner_tag: PatternType::Const as u8,
                    inner_params: vec![unmasked[0]],
                });
            }
        }

        // Check if unmasked is LINEAR
        if unmasked.len() >= 2 {
            let start = unmasked[0] as i16;
            let step = unmasked[1] as i16 - start;
            let is_linear = unmasked.windows(2).all(|w| w[1] as i16 - w[0] as i16 == step);
            if is_linear {
                // Cost: 1 (mask) + 1 (inner_tag) + 4 (start+step) = 6 bytes
                if 6 < n {
                    let mut inner = Vec::new();
                    inner.extend_from_slice(&start.to_le_bytes());
                    inner.extend_from_slice(&step.to_le_bytes());
                    return Some(TrackParams::XorMask {
                        mask,
                        inner_tag: PatternType::Linear as u8,
                        inner_params: inner,
                    });
                }
            }
        }
    }
    None
}

/// Sparse: mostly one fill value with few exceptions.
fn detect_sparse(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 { return None; }

    // Find most common value
    let mut counts = [0u32; 256];
    for &b in track { counts[b as usize] += 1; }
    let fill = counts.iter().enumerate().max_by_key(|(_, &c)| c).unwrap().0 as u8;
    let fill_count = counts[fill as usize] as usize;

    // Only worth it if >75% is fill
    if fill_count * 4 < n * 3 { return None; }

    let entries: Vec<(u16, u8)> = track.iter().enumerate()
        .filter(|(_, &b)| b != fill)
        .map(|(i, &b)| (i as u16, b))
        .collect();

    // Cost: 1 (fill) + 2 (count) + entries * 3 (idx u16 + val u8)
    let cost = 3 + entries.len() * 3;
    if cost < n {
        Some(TrackParams::Sparse { fill, entries })
    } else {
        None
    }
}

/// Piecewise linear: 2-4 linear segments.
fn detect_piecewise_linear(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 16 { return None; }

    // Try splitting into 2, 3, or 4 equal segments
    for num_segs in 2..=4usize {
        let seg_len = n / num_segs;
        if seg_len < 4 { continue; }

        let mut segments: Vec<(u16, i16, i16)> = Vec::new();
        let mut all_linear = true;

        for s in 0..num_segs {
            let start_idx = s * seg_len;
            let end_idx = if s == num_segs - 1 { n } else { (s + 1) * seg_len };
            let seg = &track[start_idx..end_idx];

            let start_val = seg[0] as i16;
            let step = seg[1] as i16 - start_val;
            if !seg.windows(2).all(|w| w[1] as i16 - w[0] as i16 == step) {
                all_linear = false;
                break;
            }
            segments.push((start_idx as u16, start_val, step));
        }

        if all_linear {
            // Cost: 1 (num_segs) + num_segs * 6 (idx u16 + start i16 + step i16)
            let cost = 1 + segments.len() * 6;
            if cost < n {
                return Some(TrackParams::PiecewiseLinear { segments });
            }
        }
    }
    None
}

/// Mirror: second half is reverse of first half.
fn detect_mirror(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 || n % 2 != 0 { return None; }

    let half = n / 2;
    let first = &track[..half];
    let second = &track[half..];

    if first.iter().zip(second.iter().rev()).all(|(a, b)| a == b) {
        // Cost: half bytes (store first half only)
        if half < n {
            return Some(TrackParams::Mirror { half: first.to_vec() });
        }
    }
    None
}

/// Interleaved linear: two interleaved arithmetic sequences.
/// e.g., [a0, b0, a1, b1, a2, b2, ...] where a_i = start_a + i*step_a
fn detect_interleaved_linear(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 || n % 2 != 0 { return None; }

    let count = (n / 2) as u16;
    let start_a = track[0] as i16;
    let start_b = track[1] as i16;

    if n < 4 { return None; }
    let step_a = track[2] as i16 - start_a;
    let step_b = track[3] as i16 - start_b;

    for i in 0..count as usize {
        let expected_a = (start_a + (i as i16) * step_a) as u8;
        let expected_b = (start_b + (i as i16) * step_b) as u8;
        if track[i * 2] != expected_a || track[i * 2 + 1] != expected_b {
            return None;
        }
    }

    // Cost: 2 (count) + 4*2 (start_a, step_a, start_b, step_b) = 10 bytes
    if 10 < n {
        Some(TrackParams::InterleavedLinear { count, start_a, step_a, start_b, step_b })
    } else {
        None
    }
}

/// Exponential/geometric: values follow a * r^i, rounded to u8.
fn detect_exponential(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 { return None; }

    // Need first value > 0 and second value > 0 to compute ratio
    if track[0] == 0 || track[1] == 0 { return None; }

    let base = track[0] as f32;
    let ratio = track[1] as f32 / base;

    // Ratio must be meaningful (not 1.0 — that's CONST, not 0)
    if ratio <= 0.0 || ratio == 1.0 { return None; }

    // Verify all values match
    for i in 0..n {
        let expected = (base * ratio.powi(i as i32)).round() as i32;
        if expected < 0 || expected > 255 || expected as u8 != track[i] {
            return None;
        }
    }

    // Cost: 2 (count) + 4 (base f32) + 4 (ratio f32) = 10 bytes
    if 10 < n {
        Some(TrackParams::Exponential { count: n as u16, base, ratio })
    } else {
        None
    }
}

/// Delta-RLE: compute deltas, then run-length encode the deltas.
fn detect_delta_rle(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 { return None; }

    // Compute deltas
    let mut runs: Vec<(i8, u8)> = Vec::new();
    let mut i = 1;
    while i < n {
        let d = track[i] as i16 - track[i - 1] as i16;
        if d < -128 || d > 127 { return None; }
        let d8 = d as i8;
        let mut count: u8 = 1;
        while i + (count as usize) < n
            && (track[i + count as usize] as i16 - track[i + count as usize - 1] as i16) == d as i16
            && count < 255
        {
            count += 1;
        }
        runs.push((d8, count));
        i += count as usize;
    }

    // Cost: 1 (start) + 2 (num_runs) + runs * 2 (delta i8 + count u8)
    let cost = 3 + runs.len() * 2;
    if cost < n && runs.len() * 2 < n - 1 {
        Some(TrackParams::DeltaRle { start: track[0], runs })
    } else {
        None
    }
}

/// BitPacked: if all values fit in fewer than 8 bits, pack them tightly.
fn detect_bit_packed(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();
    if n < 8 { return None; }

    let max_val = *track.iter().max().unwrap();

    // Determine minimum bits needed
    let bits = if max_val == 0 { 1 }
        else { 8 - max_val.leading_zeros() as u8 };

    // Only worth it if we save at least 25% (bits <= 6)
    if bits >= 7 { return None; }

    // Pack values: bits per value, MSB first
    let packed_len = (n * bits as usize + 7) / 8;
    let cost = 1 + packed_len; // 1 byte for bits + packed data

    if cost >= n { return None; }

    let mut packed = vec![0u8; packed_len];
    let mut bit_pos: usize = 0;
    for &val in track {
        for b in (0..bits).rev() {
            if val & (1 << b) != 0 {
                packed[bit_pos / 8] |= 1 << (7 - (bit_pos % 8));
            }
            bit_pos += 1;
        }
    }

    Some(TrackParams::BitPacked { bits, packed })
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
    if let Some(params) = detect_bit_packed(track) {
        return TrackEncoding { pattern: PatternType::BitPacked, params, track_idx };
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
    if let Some(params) = detect_mirror(track) {
        return TrackEncoding { pattern: PatternType::Mirror, params, track_idx };
    }
    if let Some(params) = detect_sparse(track) {
        return TrackEncoding { pattern: PatternType::Sparse, params, track_idx };
    }
    if let Some(params) = detect_piecewise_linear(track) {
        return TrackEncoding { pattern: PatternType::PiecewiseLinear, params, track_idx };
    }
    if let Some(params) = detect_interleaved_linear(track) {
        return TrackEncoding { pattern: PatternType::InterleavedLinear, params, track_idx };
    }
    if let Some(params) = detect_exponential(track) {
        return TrackEncoding { pattern: PatternType::Exponential, params, track_idx };
    }
    if let Some(params) = detect_delta_rle(track) {
        return TrackEncoding { pattern: PatternType::DeltaRle, params, track_idx };
    }
    if let Some(params) = detect_xor_mask(track) {
        return TrackEncoding { pattern: PatternType::XorMask, params, track_idx };
    }

    TrackEncoding {
        pattern: PatternType::Raw,
        params: TrackParams::Raw { data: track.to_vec() },
        track_idx,
    }
}

pub fn analyze_platter(tracks: &[Vec<u8>]) -> Vec<TrackEncoding> {
    use rayon::prelude::*;
    tracks.par_iter().enumerate()
        .map(|(i, track)| analyze_track(track, i as u16))
        .collect()
}
