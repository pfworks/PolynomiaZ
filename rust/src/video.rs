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

    let mut reference_y: Vec<u8> = Vec::new();
    let mut reference_cb: Vec<u8> = Vec::new();
    let mut reference_cr: Vec<u8> = Vec::new();

    for (i, frame) in frames.iter().enumerate() {
        let is_iframe = i % params.gop_size as usize == 0;

        // Convert RGB to YCbCr
        let (y, cb, cr) = rgb_to_ycbcr(frame, w, h);

        if is_iframe {
            buf.push(0); // I-frame
            let frame_data = encode_iframe(&y, &cb, &cr, w, h, params.quality);
            buf.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&frame_data);
            reference_y = y;
            reference_cb = cb;
            reference_cr = cr;
        } else {
            buf.push(1); // P-frame
            let frame_data = encode_pframe(
                &y, &cb, &cr,
                &reference_y, &reference_cb, &reference_cr,
                w, h, params.quality,
            );
            buf.extend_from_slice(&(frame_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&frame_data);
            // Update reference to current (for next P-frame)
            reference_y = y;
            reference_cb = cb;
            reference_cr = cr;
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
    let mut offset = 12;

    let mut ref_y: Vec<u8> = Vec::new();
    let mut ref_cb: Vec<u8> = Vec::new();
    let mut ref_cr: Vec<u8> = Vec::new();

    for _ in 0..num_frames {
        let frame_type = data[offset]; offset += 1;
        let frame_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let frame_data = &data[offset..offset + frame_len];
        offset += frame_len;

        let (y, cb, cr) = if frame_type == 0 {
            decode_iframe(frame_data, w, h)?
        } else {
            decode_pframe(frame_data, &ref_y, &ref_cb, &ref_cr, w, h)?
        };

        let rgb = ycbcr_to_rgb(&y, &cb, &cr, w, h);
        frames.push(rgb);

        ref_y = y;
        ref_cb = cb;
        ref_cr = cr;
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

fn decode_iframe(data: &[u8], w: usize, h: usize) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
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
    // Compute difference (current - reference + 128 to keep unsigned)
    let dy: Vec<u8> = y.iter().zip(ref_y.iter())
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

    // Compress difference frames (mostly 128 = no change → CONST)
    let dy_comp = compress_image(&dy, ImageParams { width: w as u16, height: h as u16, bit_depth: 8, quality });
    let dcb_comp = compress_image(&dcb, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });
    let dcr_comp = compress_image(&dcr, ImageParams { width: cw as u16, height: ch as u16, bit_depth: 8, quality });

    let mut buf = Vec::new();
    buf.extend_from_slice(&(dy_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dy_comp);
    buf.extend_from_slice(&(dcb_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dcb_comp);
    buf.extend_from_slice(&(dcr_comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&dcr_comp);
    buf
}

fn decode_pframe(
    data: &[u8],
    ref_y: &[u8], ref_cb: &[u8], ref_cr: &[u8],
    w: usize, h: usize,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let mut pos = 0;

    let dy_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dy, _) = decompress_image(&data[pos..pos + dy_len])?; pos += dy_len;

    let dcb_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dcb, _) = decompress_image(&data[pos..pos + dcb_len])?; pos += dcb_len;

    let dcr_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize; pos += 4;
    let (dcr, _) = decompress_image(&data[pos..pos + dcr_len])?;

    // Reconstruct: current = reference + (diff - 128)
    let y: Vec<u8> = dy.iter().zip(ref_y.iter())
        .map(|(&d, &r)| ((r as i16 + d as i16 - 128).clamp(0, 255)) as u8)
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
