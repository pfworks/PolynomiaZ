/// Cross-track pattern detection: finds patterns in the *parameters* across
/// groups of tracks with the same pattern type.
///
/// For example, 100 CONST tracks all with value=0xFF become a single
/// meta-descriptor instead of 100 individual track encodings.
///
/// Meta-descriptor tag = 12, encoded as:
///   [1] meta tag (12)
///   [1] inner pattern type
///   [2] count of tracks
///   [2*count] track indices
///   [...] compressed parameter description

use crate::analyzer::*;

/// Tag for meta-descriptors in the binary format.
pub const META_TAG: u8 = 12;

/// A group of tracks compressed with a single meta-descriptor.
#[derive(Debug, Clone)]
pub struct MetaGroup {
    pub inner_pattern: PatternType,
    pub track_indices: Vec<u16>,
    pub meta_params: MetaParams,
}

/// How the parameters across a group are encoded.
#[derive(Debug, Clone)]
pub enum MetaParams {
    /// All tracks have identical parameters (store once).
    AllSame(TrackParams),
    /// CONST values form a linear sequence.
    ConstLinear { start: u8, step: i8 },
    /// LINEAR starts form a linear sequence (all same step).
    LinearStartsLinear { start_base: i16, start_step: i16, step: i16 },
}

/// Attempt cross-track optimization on a list of encodings.
/// Returns (meta_groups, remaining_individual_tracks).
pub fn cross_track_optimize(encodings: Vec<TrackEncoding>) -> (Vec<MetaGroup>, Vec<TrackEncoding>) {
    let mut groups: Vec<MetaGroup> = Vec::new();
    let mut remaining: Vec<TrackEncoding> = Vec::new();

    // Group by pattern type
    let mut by_type: std::collections::HashMap<u8, Vec<TrackEncoding>> = std::collections::HashMap::new();
    for enc in encodings {
        by_type.entry(enc.pattern as u8).or_default().push(enc);
    }

    for (_tag, mut tracks) in by_type {
        // Sort by track index for consistent ordering
        tracks.sort_by_key(|t| t.track_idx);

        if tracks.len() < 4 {
            // Not worth meta-encoding small groups
            remaining.extend(tracks);
            continue;
        }

        match tracks[0].pattern {
            PatternType::Const => {
                if let Some(group) = try_meta_const(&tracks) {
                    groups.push(group);
                } else {
                    remaining.extend(tracks);
                }
            }
            PatternType::Linear => {
                if let Some(group) = try_meta_linear(&tracks) {
                    groups.push(group);
                } else {
                    remaining.extend(tracks);
                }
            }
            _ => {
                // For other types, check if all parameters are identical
                if let Some(group) = try_meta_all_same(&tracks) {
                    groups.push(group);
                } else {
                    remaining.extend(tracks);
                }
            }
        }
    }

    (groups, remaining)
}

fn try_meta_const(tracks: &[TrackEncoding]) -> Option<MetaGroup> {
    let values: Vec<u8> = tracks.iter().map(|t| {
        match &t.params { TrackParams::Const { value } => *value, _ => 0 }
    }).collect();
    let indices: Vec<u16> = tracks.iter().map(|t| t.track_idx).collect();

    // All same value?
    if values.iter().all(|&v| v == values[0]) {
        let savings = tracks.len() * 4 - (6 + indices.len() * 2 + 1);
        if savings > 0 {
            return Some(MetaGroup {
                inner_pattern: PatternType::Const,
                track_indices: indices,
                meta_params: MetaParams::AllSame(TrackParams::Const { value: values[0] }),
            });
        }
    }

    // Values form a linear sequence?
    if values.len() >= 2 {
        let step = values[1] as i16 - values[0] as i16;
        if step >= -128 && step <= 127 {
            let is_linear = values.windows(2).all(|w| w[1] as i16 - w[0] as i16 == step);
            if is_linear {
                let meta_cost = 6 + indices.len() * 2 + 2; // tag+type+count+indices+start+step
                let individual_cost = tracks.len() * 4; // 3 header + 1 value each
                if meta_cost < individual_cost {
                    return Some(MetaGroup {
                        inner_pattern: PatternType::Const,
                        track_indices: indices,
                        meta_params: MetaParams::ConstLinear { start: values[0], step: step as i8 },
                    });
                }
            }
        }
    }

    None
}

fn try_meta_linear(tracks: &[TrackEncoding]) -> Option<MetaGroup> {
    let params: Vec<(i16, i16)> = tracks.iter().map(|t| {
        match &t.params { TrackParams::Linear { start, step } => (*start, *step), _ => (0, 0) }
    }).collect();
    let indices: Vec<u16> = tracks.iter().map(|t| t.track_idx).collect();

    // All same start and step?
    if params.iter().all(|p| *p == params[0]) {
        let meta_cost = 6 + indices.len() * 2 + 4;
        let individual_cost = tracks.len() * 7; // 3 header + 4 params each
        if meta_cost < individual_cost {
            return Some(MetaGroup {
                inner_pattern: PatternType::Linear,
                track_indices: indices,
                meta_params: MetaParams::AllSame(TrackParams::Linear { start: params[0].0, step: params[0].1 }),
            });
        }
    }

    // All same step, starts form a linear sequence?
    let step = params[0].1;
    if params.iter().all(|p| p.1 == step) && params.len() >= 2 {
        let start_step = params[1].0 as i32 - params[0].0 as i32;
        if start_step >= i16::MIN as i32 && start_step <= i16::MAX as i32 {
            let is_linear = params.windows(2).all(|w| w[1].0 as i32 - w[0].0 as i32 == start_step);
            if is_linear {
                let meta_cost = 6 + indices.len() * 2 + 6; // +start_base+start_step+step
                let individual_cost = tracks.len() * 7;
                if meta_cost < individual_cost {
                    return Some(MetaGroup {
                        inner_pattern: PatternType::Linear,
                        track_indices: indices,
                        meta_params: MetaParams::LinearStartsLinear {
                            start_base: params[0].0,
                            start_step: start_step as i16,
                            step,
                        },
                    });
                }
            }
        }
    }

    None
}

fn try_meta_all_same(tracks: &[TrackEncoding]) -> Option<MetaGroup> {
    // Check if all tracks have byte-identical parameters
    // Simple approach: serialize first track's params and compare
    if tracks.len() < 4 {
        return None;
    }

    let first = &tracks[0].params;
    let all_same = tracks[1..].iter().all(|t| params_equal(&t.params, first));

    if !all_same {
        return None;
    }

    let indices: Vec<u16> = tracks.iter().map(|t| t.track_idx).collect();
    let individual_size: usize = tracks.len() * estimate_single_track_size(&tracks[0]);
    let meta_size = 6 + indices.len() * 2 + estimate_params_size(first);

    if meta_size < individual_size {
        Some(MetaGroup {
            inner_pattern: tracks[0].pattern,
            track_indices: indices,
            meta_params: MetaParams::AllSame(first.clone()),
        })
    } else {
        None
    }
}

fn params_equal(a: &TrackParams, b: &TrackParams) -> bool {
    match (a, b) {
        (TrackParams::Const { value: va }, TrackParams::Const { value: vb }) => va == vb,
        (TrackParams::Linear { start: sa, step: sta }, TrackParams::Linear { start: sb, step: stb }) => sa == sb && sta == stb,
        (TrackParams::Repeat { pattern: pa }, TrackParams::Repeat { pattern: pb }) => pa == pb,
        (TrackParams::LinearWide { width: wa, count: ca, start: sa, step: sta },
         TrackParams::LinearWide { width: wb, count: cb, start: sb, step: stb }) => wa == wb && ca == cb && sa == sb && sta == stb,
        (TrackParams::LinearF64 { count: ca, start: sa, step: sta },
         TrackParams::LinearF64 { count: cb, start: sb, step: stb }) => ca == cb && sa.to_bits() == sb.to_bits() && sta.to_bits() == stb.to_bits(),
        _ => false,
    }
}

fn estimate_single_track_size(enc: &TrackEncoding) -> usize {
    3 + estimate_params_size(&enc.params)
}

fn estimate_params_size(params: &TrackParams) -> usize {
    match params {
        TrackParams::Const { .. } => 1,
        TrackParams::Linear { .. } => 4,
        TrackParams::Rle { runs } => 2 + runs.len() * 2,
        TrackParams::Repeat { pattern } => 2 + pattern.len(),
        TrackParams::Delta { deltas, .. } => 2 + deltas.len(),
        TrackParams::Periodic { components, .. } => 4 + components.len() * 18,
        TrackParams::Poly { degree, .. } => 1 + (*degree as usize + 1) * 8,
        TrackParams::Raw { data } => data.len(),
        TrackParams::LinearWide { .. } => 19,
        TrackParams::LinearF64 { .. } => 18,
        TrackParams::DeltaF64 { deltas_f32, .. } => 10 + deltas_f32.len() * 4,
        TrackParams::XorMask { inner_params, .. } => 2 + inner_params.len(),
        TrackParams::Sparse { entries, .. } => 3 + entries.len() * 3,
        TrackParams::PiecewiseLinear { segments } => 1 + segments.len() * 6,
        TrackParams::Mirror { half } => half.len(),
    }
}

/// Estimate the binary size of a meta-group.
pub fn estimate_meta_size(group: &MetaGroup) -> usize {
    let header = 1 + 1 + 2 + group.track_indices.len() * 2; // meta_tag + inner_type + count + indices
    let params_size = match &group.meta_params {
        MetaParams::AllSame(p) => 1 + estimate_params_size(p), // sub-tag + params
        MetaParams::ConstLinear { .. } => 1 + 2, // sub-tag + start + step
        MetaParams::LinearStartsLinear { .. } => 1 + 6, // sub-tag + start_base + start_step + step
    };
    header + params_size
}

/// Encode a meta-group into the binary buffer.
/// Uses track_idx=0xFFFF as sentinel, then META_TAG as the pattern byte.
pub fn encode_meta_group(buf: &mut Vec<u8>, group: &MetaGroup) {
    // Sentinel track index + META_TAG (same 3-byte header as individual tracks)
    buf.extend_from_slice(&0xFFFFu16.to_le_bytes());
    buf.push(META_TAG);
    // Then the meta-group payload
    buf.push(group.inner_pattern as u8);
    buf.extend_from_slice(&(group.track_indices.len() as u16).to_le_bytes());
    for &idx in &group.track_indices {
        buf.extend_from_slice(&idx.to_le_bytes());
    }

    match &group.meta_params {
        MetaParams::AllSame(params) => {
            buf.push(0); // sub-tag: all same
            encode_params_only(buf, params);
        }
        MetaParams::ConstLinear { start, step } => {
            buf.push(1); // sub-tag: const linear
            buf.push(*start);
            buf.push(*step as u8);
        }
        MetaParams::LinearStartsLinear { start_base, start_step, step } => {
            buf.push(2); // sub-tag: linear starts linear
            buf.extend_from_slice(&start_base.to_le_bytes());
            buf.extend_from_slice(&start_step.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
    }
}

/// Encode just the parameters (no track index or pattern tag).
fn encode_params_only(buf: &mut Vec<u8>, params: &TrackParams) {
    match params {
        TrackParams::Const { value } => buf.push(*value),
        TrackParams::Linear { start, step } => {
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::Repeat { pattern } => {
            buf.extend_from_slice(&(pattern.len() as u16).to_le_bytes());
            buf.extend_from_slice(pattern);
        }
        TrackParams::LinearWide { width, count, start, step } => {
            buf.push(*width);
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::LinearF64 { count, start, step } => {
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        _ => {} // Other types handled by AllSame only for simple params
    }
}

/// Decode a meta-group from the binary buffer. Returns (tracks, new_offset).
pub fn decode_meta_group(buf: &[u8], offset: usize, sectors: usize) -> Result<(Vec<(u16, Vec<u8>)>, usize), String> {
    let mut pos = offset;
    // META_TAG already consumed by caller
    let inner_type = PatternType::from_u8(buf[pos]).ok_or("Bad inner type")?;
    pos += 1;
    let count = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let mut indices = Vec::with_capacity(count);
    for _ in 0..count {
        indices.push(u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()));
        pos += 2;
    }

    let sub_tag = buf[pos];
    pos += 1;

    let mut result = Vec::with_capacity(count);

    match (inner_type, sub_tag) {
        (PatternType::Const, 0) => {
            // AllSame: single value
            let value = buf[pos]; pos += 1;
            for &idx in &indices {
                result.push((idx, vec![value; sectors]));
            }
        }
        (PatternType::Const, 1) => {
            // ConstLinear: start + step
            let start = buf[pos]; pos += 1;
            let step = buf[pos] as i8; pos += 1;
            for (i, &idx) in indices.iter().enumerate() {
                let val = (start as i16 + (i as i16) * (step as i16)) as u8;
                result.push((idx, vec![val; sectors]));
            }
        }
        (PatternType::Linear, 0) => {
            // AllSame: same start/step for all
            let start = i16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
            let step = i16::from_le_bytes(buf[pos + 2..pos + 4].try_into().unwrap());
            pos += 4;
            let track: Vec<u8> = (0..sectors).map(|i| (start.wrapping_add((i as i16).wrapping_mul(step))) as u8).collect();
            for &idx in &indices {
                result.push((idx, track.clone()));
            }
        }
        (PatternType::Linear, 2) => {
            // LinearStartsLinear: start_base + start_step + step
            let start_base = i16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap());
            let start_step = i16::from_le_bytes(buf[pos + 2..pos + 4].try_into().unwrap());
            let step = i16::from_le_bytes(buf[pos + 4..pos + 6].try_into().unwrap());
            pos += 6;
            for (i, &idx) in indices.iter().enumerate() {
                let start = start_base.wrapping_add((i as i16).wrapping_mul(start_step));
                let track: Vec<u8> = (0..sectors).map(|s| (start.wrapping_add((s as i16).wrapping_mul(step))) as u8).collect();
                result.push((idx, track));
            }
        }
        (PatternType::Repeat, 0) => {
            let pat_len = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            let pattern = &buf[pos..pos + pat_len];
            pos += pat_len;
            let track: Vec<u8> = (0..sectors).map(|i| pattern[i % pat_len]).collect();
            for &idx in &indices {
                result.push((idx, track.clone()));
            }
        }
        (PatternType::LinearWide, 0) => {
            let width = buf[pos] as usize; pos += 1;
            let wcount = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize; pos += 2;
            let start = i64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()); pos += 8;
            let step = i64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()); pos += 8;
            let mut track = Vec::with_capacity(wcount * width);
            for i in 0..wcount {
                let val = start.wrapping_add((i as i64).wrapping_mul(step));
                if width == 4 { track.extend_from_slice(&(val as u32).to_le_bytes()); }
                else { track.extend_from_slice(&(val as u16).to_le_bytes()); }
            }
            track.truncate(sectors);
            for &idx in &indices {
                result.push((idx, track.clone()));
            }
        }
        (PatternType::LinearF64, 0) => {
            let fcount = u16::from_le_bytes(buf[pos..pos + 2].try_into().unwrap()) as usize; pos += 2;
            let start = f64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()); pos += 8;
            let step = f64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()); pos += 8;
            let mut track = Vec::with_capacity(fcount * 8);
            for i in 0..fcount {
                track.extend_from_slice(&(start + (i as f64) * step).to_le_bytes());
            }
            track.truncate(sectors);
            for &idx in &indices {
                result.push((idx, track.clone()));
            }
        }
        _ => return Err(format!("Unknown meta sub-tag {} for {:?}", sub_tag, inner_type)),
    }

    Ok((result, pos))
}
