/// Video codec: lossy color video compression using PLTZ platter model.
///
/// Approach:
/// - Convert RGB to YCbCr (4:2:0 chroma subsampling)
/// - I-frames: compress each channel with PLTI lossy image codec
/// - P-frames: compute difference from reference, compress difference
///   (static regions become CONST(0) → extreme compression)
///
/// Format (PLTV):
///   [4] Magic "PLTV"
///   [2] Width
///   [2] Height
///   [1] FPS
///   [1] Quality (1-255)
///   [2] Number of frames
///   Per frame:
///     [1] Frame type (0=I, 1=P)
///     [4] Frame data length
///     [...] Compressed frame (3 planes: Y, Cb, Cr)

use crate::image::{compress_image, decompress_image, ImageParams};

const VIDEO_MAGIC: &[u8; 4] = b"PLTV";

#[derive(Debug, Clone, Copy)]
pub struct VideoParams {
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub quality: u8,
    pub gop_size: u16, // frames between I-frames
}

/// A raw frame in RGB24 format (3 bytes per pixel, row-major).
pub type RgbFrame = Vec<u8>;

/// Encode a sequence of RGB frames into PLTV format.
/// GOP structure: I P P P ... with optional B-frames (I B P B P B P ...)
pub fn encode_video(frames: &[RgbFrame], params: VideoParams) -> Vec<u8> {
    let w = params.width as usize;
    let h = params.height as usize;

    let mut buf = Vec::new();
    buf.extend_from_slice(VIDEO_MAGIC);
    buf.extend_from_slice(&params.width.to_le_bytes());
    buf.extend_from_slice(&params.height.to_le_bytes());
    buf.push(params.fps);
    buf.push(params.quality);
    buf.extend_from_slice(&(frames.len() as u16).to_le_bytes());

    // Convert all frames to YCbCr upfront
    let ycbcr_frames: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = frames.iter()
        .map(|f| rgb_to_ycbcr(f, w, h))
        .collect();

    let mut reference_y: Vec<u8> = Vec::new();
    let mut reference_cb: Vec<u8> = Vec::new();
    let mut reference_cr: Vec<u8> = Vec::new();

    for i in 0..ycbcr_frames.len() {
        let (ref y, ref cb, ref cr) = ycbcr_frames[i];
        let is_iframe = i % params.gop_size as usize == 0;

        if is_iframe {
            buf.push(0); // I-frame
            let frame_data = encode_iframe(y, cb, cr, w, h, params.quality);
            buf.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&frame_data);
            reference_y = y.clone();
            reference_cb = cb.clone();
            reference_cr = cr.clone();
        } else {
            // Check if B-frame would be better (has future reference)
            let next_ref_idx = ((i / params.gop_size as usize) + 1) * params.gop_size as usize;
            let use_bframe = next_ref_idx < ycbcr_frames.len() && i % 2 == 1;

            if use_bframe {
                // B-frame: pick better reference (past or future)
                let (ref future_y, ref future_cb, ref future_cr) = ycbcr_frames[std::cmp::min(next_ref_idx, ycbcr_frames.len() - 1)];

                // Compare SAD against past vs future reference
                let sad_past: u64 = y.iter().zip(reference_y.iter())
                    .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs() as u64).sum();
                let sad_future: u64 = y.iter().zip(future_y.iter())
                    .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs() as u64).sum();

                let (best_ref_y, best_ref_cb, best_ref_cr, ref_dir) = if sad_future < sad_past {
                    (future_y.as_slice(), future_cb.as_slice(), future_cr.as_slice(), 1u8)
                } else {
                    (reference_y.as_slice(), reference_cb.as_slice(), reference_cr.as_slice(), 0u8)
                };

                buf.push(2); // B-frame
                let mut frame_data = vec![ref_dir]; // which reference (0=past, 1=future)
                let pframe_data = encode_pframe(y, cb, cr, best_ref_y, best_ref_cb, best_ref_cr, w, h, params.quality);
                frame_data.extend_from_slice(&pframe_data);
                buf.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
                buf.extend_from_slice(&frame_data);
                // B-frames don't update reference
            } else {
                buf.push(1); // P-frame
                let frame_data = encode_pframe(y, cb, cr, &reference_y, &reference_cb, &reference_cr, w, h, params.quality);
                buf.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
                buf.extend_from_slice(&frame_data);
                reference_y = y.clone();
                reference_cb = cb.clone();
                reference_cr = cr.clone();
            }
        }
    }

    buf
}

/// Decode PLTV back to RGB frames.
pub fn decode_video(data: &[u8]) -> Result<(Vec<RgbFrame>, VideoParams), String> {
    if data.len() < 12 || &data[0..4] != VIDEO_MAGIC {
        return Err("Bad video magic".into());
    }

    let width = u16::from_le_bytes(data[4..6].try_into().unwrap());
    let height = u16::from_le_bytes(data[6..8].try_into().unwrap());
    let fps = data[8];
    let quality = data[9];
    let num_frames = u16::from_le_bytes(data[10..12].try_into().unwrap()) as usize;

    let params = VideoParams { width, height, fps, quality, gop_size: 0 };
    let w = width as usize;
    let h = height as usize;

    let mut frames = Vec::with_capacity(num_frames);

    // First pass: collect all frame metadata
    struct FrameInfo { frame_type: u8, data_offset: usize, data_len: usize }
    let mut frame_infos: Vec<FrameInfo> = Vec::new();
    let mut scan_offset = 12;
    for _ in 0..num_frames {
        let frame_type = data[scan_offset]; scan_offset += 1;
        let frame_len = u32::from_le_bytes(data[scan_offset..scan_offset + 4].try_into().unwrap()) as usize;
        scan_offset += 4;
        frame_infos.push(FrameInfo { frame_type, data_offset: scan_offset, data_len: frame_len });
        scan_offset += frame_len;
    }

    // Decode I and P frames first to build reference list
    let mut decoded_ycbcr: Vec<Option<(Vec<u8>, Vec<u8>, Vec<u8>)>> = vec![None; num_frames];
    let mut ref_y: Vec<u8> = Vec::new();
    let mut ref_cb: Vec<u8> = Vec::new();
    let mut ref_cr: Vec<u8> = Vec::new();

    // Find next I/P frame after each position (for B-frame future reference)
    let mut next_ref: Vec<usize> = vec![0; num_frames];
    let mut last_ref = num_frames.saturating_sub(1);
    for i in (0..num_frames).rev() {
        if frame_infos[i].frame_type != 2 { last_ref = i; }
        next_ref[i] = last_ref;
    }

    // Decode I/P frames
    for i in 0..num_frames {
        let fi = &frame_infos[i];
        let frame_data = &data[fi.data_offset..fi.data_offset + fi.data_len];

        if fi.frame_type == 0 {
            let (y, cb, cr) = decode_iframe(frame_data, w, h)?;
            ref_y = y.clone(); ref_cb = cb.clone(); ref_cr = cr.clone();
            decoded_ycbcr[i] = Some((y, cb, cr));
        } else if fi.frame_type == 1 {
            let (y, cb, cr) = decode_pframe(frame_data, &ref_y, &ref_cb, &ref_cr, w, h)?;
            ref_y = y.clone(); ref_cb = cb.clone(); ref_cr = cr.clone();
            decoded_ycbcr[i] = Some((y, cb, cr));
        }
        // Skip B-frames for now
    }

    // Decode B-frames (they reference already-decoded I/P frames)
    for i in 0..num_frames {
        let fi = &frame_infos[i];
        if fi.frame_type != 2 { continue; }

        let frame_data = &data[fi.data_offset..fi.data_offset + fi.data_len];
        let ref_dir = frame_data[0];
        let pframe_data = &frame_data[1..];

        // Find past and future references
        let past_idx = (0..i).rev().find(|&j| frame_infos[j].frame_type != 2).unwrap_or(0);
        let future_idx = next_ref[i];

        let (ry, rcb, rcr) = if ref_dir == 1 && decoded_ycbcr[future_idx].is_some() {
            let f = decoded_ycbcr[future_idx].as_ref().unwrap();
            (&f.0, &f.1, &f.2)
        } else {
            let p = decoded_ycbcr[past_idx].as_ref().unwrap();
            (&p.0, &p.1, &p.2)
        };

        let (y, cb, cr) = decode_pframe(pframe_data, ry, rcb, rcr, w, h)?;
        decoded_ycbcr[i] = Some((y, cb, cr));
    }

    // Convert all to RGB
    for i in 0..num_frames {
        let (ref y, ref cb, ref cr) = decoded_ycbcr[i].as_ref().ok_or("Missing frame")?;
        frames.push(ycbcr_to_rgb(y, cb, cr, w, h));
    }

    Ok((frames, params))
}

// --- Color conversion ---

fn rgb_to_ycbcr(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = Vec::with_capacity(w * h);
    let mut cb_full = Vec::with_capacity(w * h);
    let mut cr_full = Vec::with_capacity(w * h);

    for i in 0..(w * h) {
        let r = rgb[i * 3] as f32;
        let g = rgb[i * 3 + 1] as f32;
        let b = rgb[i * 3 + 2] as f32;

        y.push((0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8);
        cb_full.push((128.0 - 0.169 * r - 0.331 * g + 0.500 * b).round().clamp(0.0, 255.0) as u8);
        cr_full.push((128.0 + 0.500 * r - 0.419 * g - 0.081 * b).round().clamp(0.0, 255.0) as u8);
    }

    // 4:2:0 subsampling for Cb and Cr
    let cw = w / 2;
    let ch = h / 2;
    let mut cb = Vec::with_capacity(cw * ch);
    let mut cr = Vec::with_capacity(cw * ch);
    for row in 0..ch {
        for col in 0..cw {
            let idx = (row * 2) * w + col * 2;
            cb.push(cb_full[idx]);
            cr.push(cr_full[idx]);
        }
    }

    (y, cb, cr)
}

fn ycbcr_to_rgb(y: &[u8], cb: &[u8], cr: &[u8], w: usize, h: usize) -> Vec<u8> {
    let cw = w / 2;
    let mut rgb = Vec::with_capacity(w * h * 3);

    for row in 0..h {
        for col in 0..w {
            let yi = y[row * w + col] as f32;
            let ci = (row / 2) * cw + col / 2;
            let cbi = cb[ci] as f32 - 128.0;
            let cri = cr[ci] as f32 - 128.0;

            let r = (yi + 1.402 * cri).round().clamp(0.0, 255.0) as u8;
            let g = (yi - 0.344 * cbi - 0.714 * cri).round().clamp(0.0, 255.0) as u8;
            let b = (yi + 1.772 * cbi).round().clamp(0.0, 255.0) as u8;
            rgb.push(r);
            rgb.push(g);
            rgb.push(b);
        }
    }

    rgb
}

// --- Frame encoding ---

fn encode_iframe(y: &[u8], cb: &[u8], cr: &[u8], w: usize, h: usize, quality: u8) -> Vec<u8> {
    let cw = w / 2;
    let ch = h / 2;

    let y_comp = compress_image(y, ImageParams { width: w as u16, height: h as u16, bit_depth: 8, quality });
    let cb_comp = compress_image(cb, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });
    let cr_comp = compress_image(cr, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });

    let mut buf = Vec::new();
    buf.extend_from_slice(&(y_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&y_comp);
    buf.extend_from_slice(&(cb_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&cb_comp);
    buf.extend_from_slice(&(cr_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&cr_comp);
    buf
}

fn decode_iframe(data: &[u8], _w: usize, _h: usize) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let mut pos = 0;

    let y_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (y, _) = decompress_image(&data[pos..pos + y_len])?; pos += y_len;

    let cb_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (cb, _) = decompress_image(&data[pos..pos + cb_len])?; pos += cb_len;

    let cr_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (cr, _) = decompress_image(&data[pos..pos + cr_len])?;

    Ok((y, cb, cr))
}

fn encode_pframe(
    y: &[u8], cb: &[u8], cr: &[u8],
    ref_y: &[u8], ref_cb: &[u8], ref_cr: &[u8],
    w: usize, h: usize, quality: u8,
) -> Vec<u8> {
    // Block-based motion compensation on Y channel
    let block_size = 16;
    let search_range: i32 = 16;
    let bw = (w + block_size - 1) / block_size;
    let bh = (h + block_size - 1) / block_size;

    // Find motion vectors for each block
    let mut mvs: Vec<(i8, i8)> = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            let mv = find_motion_vector(y, ref_y, w, h, bx, by, block_size, search_range);
            mvs.push(mv);
        }
    }

    // Build motion-compensated prediction
    let predicted_y = apply_motion_compensation(ref_y, &mvs, w, h, block_size, bw, bh);

    // Compute residual (current - predicted + 128)
    let dy: Vec<u8> = y.iter().zip(predicted_y.iter())
        .map(|(&a, &b)| ((a as i16 - b as i16 + 128).clamp(0, 255)) as u8)
        .collect();
    let dcb: Vec<u8> = cb.iter().zip(ref_cb.iter())
        .map(|(&a, &b)| ((a as i16 - b as i16 + 128).clamp(0, 255)) as u8)
        .collect();
    let dcr: Vec<u8> = cr.iter().zip(ref_cr.iter())
        .map(|(&a, &b)| ((a as i16 - b as i16 + 128).clamp(0, 255)) as u8)
        .collect();

    let cw = w / 2;
    let ch = h / 2;

    let dy_comp = compress_image(&dy, ImageParams { width: w as u16, height: h as u16, bit_depth: 8, quality });
    let dcb_comp = compress_image(&dcb, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });
    let dcr_comp = compress_image(&dcr, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });

    let mut buf = Vec::new();
    // Motion vectors: num_blocks(u16) + [(dx i8, dy i8)] per block
    buf.extend_from_slice(&(mvs.len() as u16).to_le_bytes());
    for &(dx, dy_mv) in &mvs {
        buf.push(dx as u8);
        buf.push(dy_mv as u8);
    }
    // Compressed residuals
    buf.extend_from_slice(&(dy_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dy_comp);
    buf.extend_from_slice(&(dcb_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dcb_comp);
    buf.extend_from_slice(&(dcr_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dcr_comp);
    buf
}

fn find_motion_vector(
    current: &[u8], reference: &[u8],
    w: usize, h: usize,
    bx: usize, by: usize,
    block_size: usize, search_range: i32,
) -> (i8, i8) {
    let x0 = bx * block_size;
    let y0 = by * block_size;
    let mut best_dx: i8 = 0;
    let mut best_dy: i8 = 0;
    let mut best_sad = u64::MAX;

    for dy in -search_range..=search_range {
        for dx in -search_range..=search_range {
            let rx = x0 as i32 + dx;
            let ry = y0 as i32 + dy;
            if rx < 0 || ry < 0 { continue; }

            let mut sad: u64 = 0;
            let mut valid = true;
            for py in 0..block_size {
                for px in 0..block_size {
                    let cx = x0 + px;
                    let cy = y0 + py;
                    let refx = (rx + px as i32) as usize;
                    let refy = (ry + py as i32) as usize;
                    if cx >= w || cy >= h || refx >= w || refy >= h {
                        valid = false;
                        break;
                    }
                    let c = current[cy * w + cx] as i32;
                    let r = reference[refy * w + refx] as i32;
                    sad += (c - r).unsigned_abs() as u64;
                }
                if !valid { break; }
            }
            if valid && sad < best_sad {
                best_sad = sad;
                best_dx = dx as i8;
                best_dy = dy as i8;
            }
        }
    }
    (best_dx, best_dy)
}

fn apply_motion_compensation(
    reference: &[u8], mvs: &[(i8, i8)],
    w: usize, h: usize, block_size: usize,
    bw: usize, _bh: usize,
) -> Vec<u8> {
    let mut predicted = vec![128u8; w * h];
    for (idx, &(dx, dy)) in mvs.iter().enumerate() {
        let bx = idx % bw;
        let by = idx / bw;
        let x0 = bx * block_size;
        let y0 = by * block_size;
        for py in 0..block_size {
            for px in 0..block_size {
                let cx = x0 + px;
                let cy = y0 + py;
                let rx = (x0 as i32 + dx as i32 + px as i32) as usize;
                let ry = (y0 as i32 + dy as i32 + py as i32) as usize;
                if cx < w && cy < h && rx < w && ry < h {
                    predicted[cy * w + cx] = reference[ry * w + rx];
                }
            }
        }
    }
    predicted
}

fn decode_pframe(
    data: &[u8],
    ref_y: &[u8], ref_cb: &[u8], ref_cr: &[u8],
    w: usize, h: usize,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let mut pos = 0;
    let block_size = 16;
    let bw = (w + block_size - 1) / block_size;
    let bh = (h + block_size - 1) / block_size;

    // Read motion vectors
    let num_mvs = u16::from_le_bytes(data[pos..pos + 2].try_into().unwrap()) as usize;
    pos += 2;
    let mut mvs: Vec<(i8, i8)> = Vec::with_capacity(num_mvs);
    for _ in 0..num_mvs {
        let dx = data[pos] as i8;
        let dy_mv = data[pos + 1] as i8;
        pos += 2;
        mvs.push((dx, dy_mv));
    }

    // Apply motion compensation to get predicted Y
    let predicted_y = apply_motion_compensation(ref_y, &mvs, w, h, block_size, bw, bh);

    // Read compressed residuals
    let dy_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dy, _) = decompress_image(&data[pos..pos + dy_len])?; pos += dy_len;

    let dcb_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dcb, _) = decompress_image(&data[pos..pos + dcb_len])?; pos += dcb_len;

    let dcr_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dcr, _) = decompress_image(&data[pos..pos + dcr_len])?;

    // Reconstruct: current = predicted + (residual - 128)
    let y: Vec<u8> = dy.iter().zip(predicted_y.iter())
        .map(|(&d, &p)| ((p as i16 + d as i16 - 128).clamp(0, 255)) as u8)
        .collect();
    let cb: Vec<u8> = dcb.iter().zip(ref_cb.iter())
        .map(|(&d, &r)| ((r as i16 + d as i16 - 128).clamp(0, 255)) as u8)
        .collect();
    let cr: Vec<u8> = dcr.iter().zip(ref_cr.iter())
        .map(|(&d, &r)| ((r as i16 + d as i16 - 128).clamp(0, 255)) as u8)
        .collect();

    Ok((y, cb, cr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_solid_frame(w: usize, h: usize, r: u8, g: u8, b: u8) -> RgbFrame {
        vec![r, g, b].repeat(w * h)
    }

    #[test]
    fn test_static_scene() {
        // 10 identical frames (surveillance camera, nothing moving)
        let w = 64;
        let h = 48;
        let frame = make_solid_frame(w, h, 100, 150, 200);
        let frames: Vec<RgbFrame> = vec![frame; 10];

        let params = VideoParams { width: w as u16, height: h as u16, fps: 15, quality: 5, gop_size: 10 };
        let compressed = encode_video(&frames, params);
        let (decoded, _) = decode_video(&compressed).unwrap();

        assert_eq!(decoded.len(), 10);
        // Check first frame roughly matches (lossy)
        let max_err = frames[0].iter().zip(decoded[0].iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs())
            .max().unwrap();
        assert!(max_err <= 10, "Max error {} too high", max_err);

        let raw_size = frames.len() * w * h * 3;
        println!("Static 64x48 10 frames: {}B raw → {}B compressed ({:.1}:1)",
            raw_size, compressed.len(), raw_size as f64 / compressed.len() as f64);
        assert!(compressed.len() < raw_size / 10, "Should compress >10:1 on static");
    }

    #[test]
    fn test_motion() {
        // Frame with a moving bright spot
        let w = 64;
        let h = 48;
        let mut frames: Vec<RgbFrame> = Vec::new();
        for f in 0..10 {
            let mut frame = vec![50u8; w * h * 3]; // dark background
            // Bright spot moves across
            let spot_x = 10 + f * 4;
            let spot_y = 24;
            for dy in 0..4 {
                for dx in 0..4 {
                    let px = (spot_y + dy) * w + spot_x + dx;
                    if px < w * h {
                        frame[px * 3] = 255;
                        frame[px * 3 + 1] = 255;
                        frame[px * 3 + 2] = 255;
                    }
                }
            }
            frames.push(frame);
        }

        let params = VideoParams { width: w as u16, height: h as u16, fps: 15, quality: 5, gop_size: 10 };
        let compressed = encode_video(&frames, params);
        let (decoded, _) = decode_video(&compressed).unwrap();

        assert_eq!(decoded.len(), 10);
        let raw_size = frames.len() * w * h * 3;
        println!("Motion 64x48 10 frames: {}B raw → {}B compressed ({:.1}:1)",
            raw_size, compressed.len(), raw_size as f64 / compressed.len() as f64);
    }
}
