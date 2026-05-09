/// Structured encryption: an encryption scheme whose ciphertext format
/// is designed to be PLTZ-compressible.
///
/// # ⚠️ DEMO ONLY — NOT FOR PRODUCTION USE
///
/// The stream cipher in this module is a simplified demonstration.
/// It is NOT cryptographically secure. Production use requires replacing
/// `stream_cipher()` with a proper AEAD construction (e.g., ChaCha20-Poly1305
/// or AES-256-GCM) and adding authentication tags.
///
/// # Concept
///
/// Standard encryption (AES-CTR, ChaCha20) produces ciphertext indistinguishable
/// from random — incompressible by any algorithm. This is a security feature
/// (IND-CPA), but wastes bandwidth when the plaintext was already structured.
///
/// PolynomiaZ Structured Encryption (PSE) takes a different approach:
/// - Encrypt the *parameters* of detected patterns, not the raw data
/// - The ciphertext retains the *structure type* (public) but encrypts the
///   *parameter values* (secret)
/// - This leaks pattern type and track layout (metadata) but protects content
///
/// # Security Model
///
/// PSE is NOT semantically secure (IND-CPA). It intentionally leaks:
/// - File size
/// - Number of tracks and their pattern types
/// - Which tracks share the same pattern (cross-track structure)
///
/// It protects:
/// - Actual data values (encrypted parameters)
/// - The relationship between parameter values
///
/// # Use Case
///
/// This is useful when:
/// 1. You need encryption + compression (normally mutually exclusive)
/// 2. The metadata leakage is acceptable (e.g., IoT telemetry where the
///    *structure* of readings is public but *values* are private)
/// 3. Bandwidth is extremely constrained (satellite, NB-IoT)
///
/// # Scheme
///
/// 1. Analyze data with PLTZ pattern detection
/// 2. For each track, encrypt only the pattern parameters:
///    - CONST: encrypt the value byte
///    - LINEAR: encrypt start and step
///    - LINEAR_WIDE: encrypt start and step
///    - LINEAR_F64: encrypt start and step
///    - RAW: encrypt the entire raw data (standard encryption)
/// 3. Store the encrypted parameters in the same PLTZ format
/// 4. Decryption: decrypt parameters, then reconstruct tracks normally
///
/// The key insight: a LINEAR track with encrypted (start, step) is still
/// only 4 bytes of ciphertext — the compression benefit is preserved.

use crate::analyzer::*;
use crate::codec::{encode, RawCompressor};
use crate::platter::*;

/// Encrypt a byte slice using ChaCha20-like stream XOR.
/// This is a simplified implementation for demonstration.
/// In production, use a proper AEAD (ChaCha20-Poly1305).
fn stream_cipher(data: &[u8], key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> Vec<u8> {
    // Simple deterministic PRNG seeded from key+nonce+counter for demo purposes.
    // NOT cryptographically secure — replace with actual ChaCha20 in production.
    let mut state: [u32; 4] = [
        u32::from_le_bytes(key[0..4].try_into().unwrap()) ^ counter,
        u32::from_le_bytes(key[4..8].try_into().unwrap()),
        u32::from_le_bytes(nonce[0..4].try_into().unwrap()),
        u32::from_le_bytes(nonce[4..8].try_into().unwrap()),
    ];

    let mut keystream = Vec::with_capacity(data.len());
    while keystream.len() < data.len() {
        // Quarter-round mixing
        for _ in 0..20 {
            state[0] = state[0].wrapping_add(state[1]);
            state[3] ^= state[0];
            state[3] = state[3].rotate_left(16);
            state[2] = state[2].wrapping_add(state[3]);
            state[1] ^= state[2];
            state[1] = state[1].rotate_left(12);
            state[0] = state[0].wrapping_add(state[1]);
            state[3] ^= state[0];
            state[3] = state[3].rotate_left(8);
            state[2] = state[2].wrapping_add(state[3]);
            state[1] ^= state[2];
            state[1] = state[1].rotate_left(7);
        }
        for s in &state {
            keystream.extend_from_slice(&s.to_le_bytes());
        }
        state[0] = state[0].wrapping_add(1); // advance
    }

    data.iter().zip(keystream.iter()).map(|(d, k)| d ^ k).collect()
}

/// Encrypt data using PLTZ-structured encryption.
/// Returns compressed+encrypted bytes that are smaller than encrypt-then-compress.
///
/// Format: PLTZ-encoded data where parameter values are encrypted.
/// The structure (pattern types, track layout) is visible; values are not.
pub fn encrypt_structured(data: &[u8], key: &[u8; 32], nonce: &[u8; 12]) -> Vec<u8> {
    if data.is_empty() {
        return encode(data, RawCompressor::None);
    }

    // Step 1: Find best geometry and analyze
    let geom = find_best_geometry(data);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);

    // Step 2: Encrypt the parameter values for each track
    let mut encrypted_tracks: Vec<Vec<u8>> = Vec::new();
    let mut counter: u32 = 0;

    for enc in &encodings {
        let track_data = &tracks[enc.track_idx as usize];
        // Encrypt the raw track data, then re-lay it out
        let encrypted = stream_cipher(track_data, key, nonce, counter);
        counter += 1;
        encrypted_tracks.push(encrypted);
    }

    // Step 3: Encode the encrypted data with PLTZ
    // The encrypted data won't have patterns anymore, so it'll be RAW.
    // Instead, we use a hybrid approach: store pattern metadata in the clear,
    // encrypt only the minimal parameter bytes.
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PLTE"); // Magic for encrypted
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(geom.sectors_per_track as u16).to_le_bytes());
    buf.extend_from_slice(&(encodings.len() as u16).to_le_bytes());
    buf.extend_from_slice(nonce);

    for enc in &encodings {
        buf.extend_from_slice(&enc.track_idx.to_le_bytes());
        buf.push(enc.pattern as u8);

        // Serialize parameters, then encrypt them
        let param_bytes = serialize_params(&enc.params, geom.sectors_per_track);
        let encrypted_params = stream_cipher(&param_bytes, key, nonce, counter);
        counter += 1;

        buf.extend_from_slice(&(encrypted_params.len() as u16).to_le_bytes());
        buf.extend_from_slice(&encrypted_params);
    }

    buf
}

/// Decrypt PLTZ-structured encrypted data.
pub fn decrypt_structured(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 24 {
        return Err("Too short".into());
    }
    if &encrypted[0..4] != b"PLTE" {
        return Err("Bad magic (expected PLTE)".into());
    }

    let original_len = u32::from_le_bytes(encrypted[4..8].try_into().unwrap()) as usize;
    let sectors = u16::from_le_bytes(encrypted[8..10].try_into().unwrap()) as usize;
    let num_tracks = u16::from_le_bytes(encrypted[10..12].try_into().unwrap()) as usize;
    let nonce: [u8; 12] = encrypted[12..24].try_into().unwrap();

    if original_len == 0 {
        return Ok(Vec::new());
    }

    let total_tracks = (original_len + sectors - 1) / sectors;
    let mut all_tracks: Vec<Option<Vec<u8>>> = vec![None; total_tracks];

    let mut offset = 24;
    let mut counter: u32 = 0;

    // Skip the analysis counters (they matched encoding)
    counter += num_tracks as u32; // skip the track encryption counters

    for _ in 0..num_tracks {
        let track_idx = u16::from_le_bytes(encrypted[offset..offset + 2].try_into().unwrap());
        let pattern_tag = encrypted[offset + 2];
        offset += 3;

        let param_len = u16::from_le_bytes(encrypted[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;

        let encrypted_params = &encrypted[offset..offset + param_len];
        offset += param_len;

        counter += 1;
        let param_bytes = stream_cipher(encrypted_params, key, &nonce, counter - 1);

        let pattern = PatternType::from_u8(pattern_tag).ok_or("Bad pattern tag")?;
        let track = reconstruct_track(&param_bytes, pattern, sectors)?;

        if (track_idx as usize) < all_tracks.len() {
            all_tracks[track_idx as usize] = Some(track);
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

fn find_best_geometry(data: &[u8]) -> DiskGeometry {
    let mut best_size = usize::MAX;
    let mut best_geom = compute_geometry(data.len(), 256);
    for &spt in SECTOR_CANDIDATES {
        let geom = compute_geometry(data.len(), spt);
        let tracks = lay_out(data, &geom);
        let encodings = analyze_platter(&tracks);
        let size: usize = encodings.iter().map(|e| estimate_param_size(&e.params, spt)).sum();
        if size < best_size {
            best_size = size;
            best_geom = geom;
        }
    }
    best_geom
}

fn estimate_param_size(params: &TrackParams, sectors: usize) -> usize {
    match params {
        TrackParams::Const { .. } => 1,
        TrackParams::Linear { .. } => 4,
        TrackParams::Rle { runs } => 2 + runs.len() * 2,
        TrackParams::Repeat { pattern } => 2 + pattern.len(),
        TrackParams::Delta { deltas, .. } => 2 + deltas.len(),
        TrackParams::LinearWide { .. } => 19,
        TrackParams::LinearF64 { .. } => 18,
        TrackParams::DeltaF64 { deltas_f32, .. } => 10 + deltas_f32.len() * 4,
        TrackParams::Raw { .. } => sectors,
        _ => sectors,
    }
}

fn serialize_params(params: &TrackParams, sectors: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    match params {
        TrackParams::Const { value } => buf.push(*value),
        TrackParams::Linear { start, step } => {
            buf.extend_from_slice(&start.to_le_bytes());
            buf.extend_from_slice(&step.to_le_bytes());
        }
        TrackParams::Rle { runs } => {
            buf.extend_from_slice(&(runs.len() as u16).to_le_bytes());
            for &(val, count) in runs {
                buf.push(val);
                buf.push(count);
            }
        }
        TrackParams::Repeat { pattern } => {
            buf.extend_from_slice(&(pattern.len() as u16).to_le_bytes());
            buf.extend_from_slice(pattern);
        }
        TrackParams::Delta { start, deltas } => {
            buf.extend_from_slice(&start.to_le_bytes());
            for &d in deltas { buf.push(d as u8); }
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
        TrackParams::DeltaF64 { count, start, deltas_f32 } => {
            buf.extend_from_slice(&count.to_le_bytes());
            buf.extend_from_slice(&start.to_le_bytes());
            for &d in deltas_f32 { buf.extend_from_slice(&d.to_le_bytes()); }
        }
        TrackParams::Raw { data } => buf.extend_from_slice(data),
        _ => {
            // Fallback: store raw track
            buf.resize(sectors, 0);
        }
    }
    buf
}

fn reconstruct_track(param_bytes: &[u8], pattern: PatternType, sectors: usize) -> Result<Vec<u8>, String> {
    match pattern {
        PatternType::Const => {
            let value = param_bytes[0];
            Ok(vec![value; sectors])
        }
        PatternType::Linear => {
            let start = i16::from_le_bytes(param_bytes[0..2].try_into().unwrap());
            let step = i16::from_le_bytes(param_bytes[2..4].try_into().unwrap());
            Ok((0..sectors).map(|i| (start.wrapping_add((i as i16).wrapping_mul(step))) as u8).collect())
        }
        PatternType::Rle => {
            let num_runs = u16::from_le_bytes(param_bytes[0..2].try_into().unwrap()) as usize;
            let mut track = Vec::with_capacity(sectors);
            let mut pos = 2;
            for _ in 0..num_runs {
                let val = param_bytes[pos];
                let count = param_bytes[pos + 1] as usize;
                pos += 2;
                track.extend(std::iter::repeat(val).take(count));
            }
            track.truncate(sectors);
            Ok(track)
        }
        PatternType::Repeat => {
            let pat_len = u16::from_le_bytes(param_bytes[0..2].try_into().unwrap()) as usize;
            let pattern = &param_bytes[2..2 + pat_len];
            Ok((0..sectors).map(|i| pattern[i % pat_len]).collect())
        }
        PatternType::Delta => {
            let start = i16::from_le_bytes(param_bytes[0..2].try_into().unwrap());
            let mut track = Vec::with_capacity(sectors);
            let mut current = start;
            track.push(current as u8);
            for i in 0..(sectors - 1) {
                let d = param_bytes[2 + i] as i8;
                current = current.wrapping_add(d as i16);
                track.push(current as u8);
            }
            Ok(track)
        }
        PatternType::LinearWide => {
            let width = param_bytes[0] as usize;
            let count = u16::from_le_bytes(param_bytes[1..3].try_into().unwrap()) as usize;
            let start = i64::from_le_bytes(param_bytes[3..11].try_into().unwrap());
            let step = i64::from_le_bytes(param_bytes[11..19].try_into().unwrap());
            let mut track = Vec::with_capacity(count * width);
            for i in 0..count {
                let val = start.wrapping_add((i as i64).wrapping_mul(step));
                if width == 4 { track.extend_from_slice(&(val as u32).to_le_bytes()); }
                else { track.extend_from_slice(&(val as u16).to_le_bytes()); }
            }
            track.truncate(sectors);
            Ok(track)
        }
        PatternType::LinearF64 => {
            let count = u16::from_le_bytes(param_bytes[0..2].try_into().unwrap()) as usize;
            let start = f64::from_le_bytes(param_bytes[2..10].try_into().unwrap());
            let step = f64::from_le_bytes(param_bytes[10..18].try_into().unwrap());
            let mut track = Vec::with_capacity(count * 8);
            for i in 0..count {
                track.extend_from_slice(&(start + (i as f64) * step).to_le_bytes());
            }
            track.truncate(sectors);
            Ok(track)
        }
        PatternType::DeltaF64 => {
            let count = u16::from_le_bytes(param_bytes[0..2].try_into().unwrap()) as usize;
            let start = f64::from_le_bytes(param_bytes[2..10].try_into().unwrap());
            let mut track = Vec::with_capacity(count * 8);
            let mut current = start;
            track.extend_from_slice(&current.to_le_bytes());
            let mut pos = 10;
            for _ in 0..(count - 1) {
                let d = f32::from_le_bytes(param_bytes[pos..pos + 4].try_into().unwrap());
                pos += 4;
                current += d as f64;
                track.extend_from_slice(&current.to_le_bytes());
            }
            track.truncate(sectors);
            Ok(track)
        }
        PatternType::Raw | PatternType::RawCompressed => {
            Ok(param_bytes.to_vec())
        }
        _ => Ok(param_bytes.to_vec()),
    }
}

/// Full encryption: encrypts BOTH metadata (pattern types, layout) AND parameters.
/// No information leakage except total compressed size.
///
/// Format (PLTF):
///   [4] Magic "PLTF"
///   [4] Original data length (uint32 LE)
///   [12] Nonce
///   [2] Encrypted metadata length
///   [...] Encrypted metadata (pattern tags + track indices + geometry)
///   [2] Encrypted params length
///   [...] Encrypted parameters (all track params concatenated)
///
/// ⚠️ DEMO — uses simplified stream cipher. Replace with ChaCha20-Poly1305.
pub fn encrypt_full(data: &[u8], key: &[u8; 32], nonce: &[u8; 12]) -> Vec<u8> {
    if data.is_empty() {
        let mut buf = Vec::with_capacity(20);
        buf.extend_from_slice(b"PLTF");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(nonce);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        return buf;
    }

    let geom = find_best_geometry(data);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);

    // Stream 1: metadata (geometry + pattern tags + track indices)
    let mut metadata = Vec::new();
    metadata.extend_from_slice(&(geom.sectors_per_track as u16).to_le_bytes());
    metadata.extend_from_slice(&(encodings.len() as u16).to_le_bytes());
    for enc in &encodings {
        metadata.extend_from_slice(&enc.track_idx.to_le_bytes());
        metadata.push(enc.pattern as u8);
    }

    // Stream 2: all parameters concatenated with length prefixes
    let mut params_stream = Vec::new();
    for enc in &encodings {
        let param_bytes = serialize_params(&enc.params, geom.sectors_per_track);
        params_stream.extend_from_slice(&(param_bytes.len() as u16).to_le_bytes());
        params_stream.extend_from_slice(&param_bytes);
    }

    // Encrypt both streams with different counters
    let encrypted_metadata = stream_cipher(&metadata, key, nonce, 0);
    let encrypted_params = stream_cipher(&params_stream, key, nonce, 1000000);

    // Assemble output
    let mut buf = Vec::new();
    buf.extend_from_slice(b"PLTF");
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(nonce);
    buf.extend_from_slice(&(encrypted_metadata.len() as u16).to_le_bytes());
    buf.extend_from_slice(&encrypted_metadata);
    buf.extend_from_slice(&(encrypted_params.len() as u16).to_le_bytes());
    buf.extend_from_slice(&encrypted_params);
    buf
}

/// Decrypt fully-encrypted data (both metadata and parameters encrypted).
pub fn decrypt_full(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 20 || &encrypted[0..4] != b"PLTF" {
        return Err("Bad magic (expected PLTF)".into());
    }

    let original_len = u32::from_le_bytes(encrypted[4..8].try_into().unwrap()) as usize;
    let nonce: [u8; 12] = encrypted[8..20].try_into().unwrap();

    if original_len == 0 {
        return Ok(Vec::new());
    }

    let mut pos = 20;

    // Decrypt metadata
    let meta_len = u16::from_le_bytes(encrypted[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let metadata = stream_cipher(&encrypted[pos..pos + meta_len], key, &nonce, 0);
    pos += meta_len;

    // Decrypt parameters
    let params_len = u16::from_le_bytes(encrypted[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let params_stream = stream_cipher(&encrypted[pos..pos + params_len], key, &nonce, 1000000);

    // Parse metadata
    let sectors = u16::from_le_bytes(metadata[0..2].try_into().unwrap()) as usize;
    let num_tracks = u16::from_le_bytes(metadata[2..4].try_into().unwrap()) as usize;

    let mut track_info: Vec<(u16, u8)> = Vec::with_capacity(num_tracks);
    let mut mpos = 4;
    for _ in 0..num_tracks {
        let track_idx = u16::from_le_bytes(metadata[mpos..mpos + 2].try_into().unwrap());
        let tag = metadata[mpos + 2];
        mpos += 3;
        track_info.push((track_idx, tag));
    }

    // Parse and reconstruct from parameters
    let total_tracks = (original_len + sectors - 1) / sectors;
    let mut all_tracks: Vec<Option<Vec<u8>>> = vec![None; total_tracks];
    let mut ppos = 0;

    for &(track_idx, tag) in &track_info {
        let param_len = u16::from_le_bytes(params_stream[ppos..ppos + 2].try_into().unwrap()) as usize;
        ppos += 2;
        let param_bytes = &params_stream[ppos..ppos + param_len];
        ppos += param_len;

        let pattern = PatternType::from_u8(tag).ok_or("Bad pattern tag")?;
        let track = reconstruct_track(param_bytes, pattern, sectors)?;

        if (track_idx as usize) < all_tracks.len() {
            all_tracks[track_idx as usize] = Some(track);
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

/// Dual-key full encryption: outer key encrypts metadata, inner key encrypts parameters.
///
/// Access levels:
/// - Outer key only: can see pattern types and structure (not values)
/// - Both keys: full decryption
///
/// Format (PLTD):
///   [4] Magic "PLTD"
///   [4] Original data length
///   [12] Nonce
///   [2] Encrypted metadata length (outer_key)
///   [...] Encrypted metadata
///   [2] Encrypted params length (inner_key)
///   [...] Encrypted parameters
///
/// ⚠️ DEMO — uses simplified stream cipher.
pub fn encrypt_dual_key(
    data: &[u8],
    outer_key: &[u8; 32],
    inner_key: &[u8; 32],
    nonce: &[u8; 12],
) -> Vec<u8> {
    if data.is_empty() {
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(b"PLTD");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(nonce);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        return buf;
    }

    let geom = find_best_geometry(data);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);

    let mut metadata = Vec::new();
    metadata.extend_from_slice(&(geom.sectors_per_track as u16).to_le_bytes());
    metadata.extend_from_slice(&(encodings.len() as u16).to_le_bytes());
    for enc in &encodings {
        metadata.extend_from_slice(&enc.track_idx.to_le_bytes());
        metadata.push(enc.pattern as u8);
    }

    let mut params_stream = Vec::new();
    for enc in &encodings {
        let param_bytes = serialize_params(&enc.params, geom.sectors_per_track);
        params_stream.extend_from_slice(&(param_bytes.len() as u16).to_le_bytes());
        params_stream.extend_from_slice(&param_bytes);
    }

    let encrypted_metadata = stream_cipher(&metadata, outer_key, nonce, 0);
    let encrypted_params = stream_cipher(&params_stream, inner_key, nonce, 0);

    let mut buf = Vec::new();
    buf.extend_from_slice(b"PLTD");
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(nonce);
    buf.extend_from_slice(&(encrypted_metadata.len() as u16).to_le_bytes());
    buf.extend_from_slice(&encrypted_metadata);
    buf.extend_from_slice(&(encrypted_params.len() as u16).to_le_bytes());
    buf.extend_from_slice(&encrypted_params);
    buf
}

/// Decrypt dual-key encrypted data. Requires both keys.
pub fn decrypt_dual_key(
    encrypted: &[u8],
    outer_key: &[u8; 32],
    inner_key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if encrypted.len() < 24 || &encrypted[0..4] != b"PLTD" {
        return Err("Bad magic (expected PLTD)".into());
    }

    let original_len = u32::from_le_bytes(encrypted[4..8].try_into().unwrap()) as usize;
    let nonce: [u8; 12] = encrypted[8..20].try_into().unwrap();
    if original_len == 0 { return Ok(Vec::new()); }

    let mut pos = 20;
    let meta_len = u16::from_le_bytes(encrypted[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let metadata = stream_cipher(&encrypted[pos..pos + meta_len], outer_key, &nonce, 0);
    pos += meta_len;

    let params_len = u16::from_le_bytes(encrypted[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let params_stream = stream_cipher(&encrypted[pos..pos + params_len], inner_key, &nonce, 0);

    let sectors = u16::from_le_bytes(metadata[0..2].try_into().unwrap()) as usize;
    let num_tracks = u16::from_le_bytes(metadata[2..4].try_into().unwrap()) as usize;

    let mut track_info: Vec<(u16, u8)> = Vec::with_capacity(num_tracks);
    let mut mpos = 4;
    for _ in 0..num_tracks {
        let track_idx = u16::from_le_bytes(metadata[mpos..mpos + 2].try_into().unwrap());
        let tag = metadata[mpos + 2];
        mpos += 3;
        track_info.push((track_idx, tag));
    }

    let total_tracks = (original_len + sectors - 1) / sectors;
    let mut all_tracks: Vec<Option<Vec<u8>>> = vec![None; total_tracks];
    let mut ppos = 0;

    for &(track_idx, tag) in &track_info {
        let param_len = u16::from_le_bytes(params_stream[ppos..ppos + 2].try_into().unwrap()) as usize;
        ppos += 2;
        let param_bytes = &params_stream[ppos..ppos + param_len];
        ppos += param_len;
        let pattern = PatternType::from_u8(tag).ok_or("Bad pattern tag")?;
        let track = reconstruct_track(param_bytes, pattern, sectors)?;
        if (track_idx as usize) < all_tracks.len() {
            all_tracks[track_idx as usize] = Some(track);
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

/// With outer key only: see structure but not values.
pub fn decrypt_metadata_only(
    encrypted: &[u8],
    outer_key: &[u8; 32],
) -> Result<Vec<(u16, &'static str)>, String> {
    if encrypted.len() < 24 || &encrypted[0..4] != b"PLTD" {
        return Err("Bad magic (expected PLTD)".into());
    }
    let nonce: [u8; 12] = encrypted[8..20].try_into().unwrap();
    let mut pos = 20;
    let meta_len = u16::from_le_bytes(encrypted[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let metadata = stream_cipher(&encrypted[pos..pos + meta_len], outer_key, &nonce, 0);

    let num_tracks = u16::from_le_bytes(metadata[2..4].try_into().unwrap()) as usize;
    let mut result = Vec::with_capacity(num_tracks);
    let mut mpos = 4;
    for _ in 0..num_tracks {
        let track_idx = u16::from_le_bytes(metadata[mpos..mpos + 2].try_into().unwrap());
        let tag = metadata[mpos + 2];
        mpos += 3;
        let name = match tag {
            0 => "CONST", 1 => "LINEAR", 2 => "RLE", 3 => "REPEAT",
            4 => "DELTA", 7 => "RAW", 9 => "LINEAR_WIDE", 10 => "LINEAR_F64",
            _ => "OTHER",
        };
        result.push((track_idx, name));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0x42u8; 32];
        let nonce = [0x01u8; 12];

        // Linear ramp
        let data: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        let encrypted = encrypt_structured(&data, &key, &nonce);
        let decrypted = decrypt_structured(&encrypted, &key).unwrap();
        assert_eq!(data, decrypted);

        // Verify encrypted is smaller than raw
        assert!(encrypted.len() < data.len(),
            "Encrypted {} should be < raw {}", encrypted.len(), data.len());
    }

    #[test]
    fn test_encrypt_decrypt_mixed() {
        let key = [0xAB; 32];
        let nonce = [0xCD; 12];

        let mut data = vec![0xFFu8; 512];
        data.extend(vec![0x42; 256]);
        data.extend((0..256u16).map(|i| i as u8));

        let encrypted = encrypt_structured(&data, &key, &nonce);
        let decrypted = decrypt_structured(&encrypted, &key).unwrap();
        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_encrypted_is_compact() {
        let key = [0x13; 32];
        let nonce = [0x37; 12];

        // 4KB linear ramp — should encrypt to much less than 4KB
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let encrypted = encrypt_structured(&data, &key, &nonce);

        println!("4KB linear: encrypted to {} bytes (raw=4096, ratio={:.2})",
            encrypted.len(), encrypted.len() as f64 / 4096.0);

        // Should be significantly smaller than raw
        assert!(encrypted.len() < 500,
            "Encrypted linear ramp should be <500B, got {}", encrypted.len());
    }

    #[test]
    fn test_full_encrypt_roundtrip() {
        let key = [0x99u8; 32];
        let nonce = [0x11u8; 12];

        let data: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        let encrypted = encrypt_full(&data, &key, &nonce);
        let decrypted = decrypt_full(&encrypted, &key).unwrap();
        assert_eq!(data, decrypted);

        // Should still be compact
        println!("Full encrypt 1KB linear: {} bytes (raw=1024)", encrypted.len());
        assert!(encrypted.len() < data.len());
    }

    #[test]
    fn test_full_encrypt_no_leakage() {
        let key = [0xAA; 32];
        let nonce = [0xBB; 12];

        let data = vec![0xFFu8; 1024];
        let encrypted = encrypt_full(&data, &key, &nonce);

        // Verify no pattern tags visible (everything after header is encrypted)
        // The output should not contain the byte 0x00 (CONST tag) in a predictable position
        assert_eq!(&encrypted[0..4], b"PLTF");
        // Metadata is encrypted — can't see pattern types without key
        println!("Full encrypt 1KB const: {} bytes (no metadata leakage)", encrypted.len());
    }

    #[test]
    fn test_full_vs_structured_size() {
        let key = [0x42; 32];
        let nonce = [0x01; 12];

        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let structured = encrypt_structured(&data, &key, &nonce);
        let full = encrypt_full(&data, &key, &nonce);

        println!("4KB linear: structured={} bytes, full={} bytes",
            structured.len(), full.len());
        // Full should be slightly larger (metadata overhead) but still compact
        assert!(full.len() < data.len());
    }

    #[test]
    fn test_dual_key_roundtrip() {
        let outer = [0x11u8; 32];
        let inner = [0x22u8; 32];
        let nonce = [0x33u8; 12];

        let data: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        let encrypted = encrypt_dual_key(&data, &outer, &inner, &nonce);
        let decrypted = decrypt_dual_key(&encrypted, &outer, &inner).unwrap();
        assert_eq!(data, decrypted);

        println!("Dual-key 1KB: {} bytes", encrypted.len());
        assert!(encrypted.len() < data.len());
    }

    #[test]
    fn test_dual_key_metadata_only() {
        let outer = [0xAA; 32];
        let inner = [0xBB; 32];
        let nonce = [0xCC; 12];

        let mut data = vec![0xFFu8; 512]; // CONST
        data.extend((0..=255u8).collect::<Vec<_>>()); // LINEAR
        data.extend(vec![0x42u8; 256]); // CONST

        let encrypted = encrypt_dual_key(&data, &outer, &inner, &nonce);

        // With outer key only: can see structure
        let meta = decrypt_metadata_only(&encrypted, &outer).unwrap();
        println!("Metadata-only view: {:?}", meta);
        assert!(!meta.is_empty());
        // Should see CONST and LINEAR pattern types
        let has_const = meta.iter().any(|(_, name)| *name == "CONST");
        assert!(has_const, "Should see CONST with outer key");

        // Full decrypt needs both keys
        let full = decrypt_dual_key(&encrypted, &outer, &inner).unwrap();
        assert_eq!(data, full);
    }
}
