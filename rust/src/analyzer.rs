/// Pattern detection for track data.

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
}

#[derive(Debug, Clone)]
pub struct TrackEncoding {
    pub pattern: PatternType,
    pub params: TrackParams,
    pub track_idx: u16,
}

pub fn detect_const(track: &[u8]) -> Option<TrackParams> {
    let first = track[0];
    if track.iter().all(|&b| b == first) {
        Some(TrackParams::Const { value: first })
    } else {
        None
    }
}

pub fn detect_linear(track: &[u8]) -> Option<TrackParams> {
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

pub fn detect_rle(track: &[u8]) -> Option<TrackParams> {
    let mut runs: Vec<(u8, u8)> = Vec::new();
    let mut i = 0;
    while i < track.len() {
        let val = track[i];
        let mut count: u8 = 1;
        while i + count as usize < track.len()
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

pub fn detect_repeat(track: &[u8]) -> Option<TrackParams> {
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

pub fn detect_delta(track: &[u8]) -> Option<TrackParams> {
    let mut deltas: Vec<i8> = Vec::with_capacity(track.len() - 1);
    let mut unique = std::collections::HashSet::new();
    for i in 1..track.len() {
        let d = track[i] as i16 - track[i - 1] as i16;
        if d < -128 || d > 127 {
            return None;
        }
        deltas.push(d as i8);
        unique.insert(d as i8);
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

/// Detect linear patterns in 16-bit or 32-bit word interpretations of the track.
pub fn detect_linear_wide(track: &[u8]) -> Option<TrackParams> {
    let n = track.len();

    // Try 32-bit LE
    if n >= 8 && n % 4 == 0 {
        let count = n / 4;
        let words: Vec<i64> = (0..count)
            .map(|i| {
                u32::from_le_bytes([
                    track[i * 4],
                    track[i * 4 + 1],
                    track[i * 4 + 2],
                    track[i * 4 + 3],
                ]) as i64
            })
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

pub fn analyze_track(track: &[u8], track_idx: u16) -> TrackEncoding {
    if let Some(params) = detect_const(track) {
        return TrackEncoding { pattern: PatternType::Const, params, track_idx };
    }
    if let Some(params) = detect_linear(track) {
        return TrackEncoding { pattern: PatternType::Linear, params, track_idx };
    }
    if let Some(params) = detect_rle(track) {
        return TrackEncoding { pattern: PatternType::Rle, params, track_idx };
    }
    if let Some(params) = detect_repeat(track) {
        return TrackEncoding { pattern: PatternType::Repeat, params, track_idx };
    }
    if let Some(params) = detect_linear_wide(track) {
        return TrackEncoding { pattern: PatternType::LinearWide, params, track_idx };
    }
    if let Some(params) = detect_delta(track) {
        return TrackEncoding { pattern: PatternType::Delta, params, track_idx };
    }
    // Skip periodic and poly for now (require FFT / polyfit — complex in pure Rust)
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
