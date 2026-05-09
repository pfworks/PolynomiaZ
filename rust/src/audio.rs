/// Audio codec: compress PCM audio using PLTZ pattern detection.
///
/// Approach:
/// - Split audio into frames (e.g., 256 or 512 samples)
/// - Each frame = one track on the platter
/// - Lossless: exact pattern detection (periodic, linear, const, delta)
/// - Lossy: allow per-sample error threshold (quality parameter)
///
/// Best for: synthesized tones, calibration signals, telemetry audio,
/// silence detection, simple waveforms.
///
/// Format (PLTA):
///   [4] Magic "PLTA"
///   [2] Sample rate (Hz / 100, e.g., 441 = 44100 Hz)
///   [1] Bits per sample (8 or 16)
///   [1] Channels (1=mono, 2=stereo)
///   [1] Quality (0=lossless, 1-255=max per-sample error)
///   [1] Frame size (samples per frame / 16, e.g., 16 = 256 samples)
///   [2] Number of frames
///   [...] Compressed frames (each independently encoded)

use crate::codec::{encode, decode, RawCompressor};
use crate::image::{compress_image, decompress_image, ImageParams};

const AUDIO_MAGIC: &[u8; 4] = b"PLTA";

#[derive(Debug, Clone, Copy)]
pub struct AudioParams {
    pub sample_rate: u32,
    pub bits_per_sample: u8,  // 8 or 16
    pub channels: u8,         // 1 or 2
    pub quality: u8,          // 0=lossless, 1-255=max error per sample
    pub frame_size: u16,      // samples per frame
}

/// Compress PCM audio data.
/// Input: raw PCM samples (little-endian, interleaved if stereo).
pub fn compress_audio(samples: &[u8], params: AudioParams) -> Vec<u8> {
    let bytes_per_sample = params.bits_per_sample as usize / 8;
    let frame_bytes = params.frame_size as usize * bytes_per_sample * params.channels as usize;
    let num_frames = if samples.is_empty() { 0 } else { (samples.len() + frame_bytes - 1) / frame_bytes };

    let mut buf = Vec::new();
    buf.extend_from_slice(AUDIO_MAGIC);
    buf.extend_from_slice(&((params.sample_rate / 100) as u16).to_le_bytes());
    buf.push(params.bits_per_sample);
    buf.push(params.channels);
    buf.push(params.quality);
    buf.push((params.frame_size / 16) as u8);
    buf.extend_from_slice(&(num_frames as u16).to_le_bytes());

    for i in 0..num_frames {
        let start = i * frame_bytes;
        let end = std::cmp::min(start + frame_bytes, samples.len());
        let frame = &samples[start..end];

        let compressed_frame = if params.quality == 0 {
            // Lossless: use PLTZ directly on the frame bytes
            encode(frame, RawCompressor::Zlib)
        } else {
            // Lossy: treat frame as a 1-row image for PLTI lossy
            // For 16-bit audio, use image codec with 16-bit depth
            if params.bits_per_sample == 16 {
                let width = (frame.len() / 2) as u16;
                compress_image(frame, ImageParams {
                    width,
                    height: 1,
                    bit_depth: 16,
                    quality: params.quality,
                })
            } else {
                let width = frame.len() as u16;
                compress_image(frame, ImageParams {
                    width,
                    height: 1,
                    bit_depth: 8,
                    quality: params.quality,
                })
            }
        };

        buf.extend_from_slice(&(compressed_frame.len() as u16).to_le_bytes());
        buf.extend_from_slice(&compressed_frame);
    }

    buf
}

/// Decompress PLTA audio back to PCM samples.
pub fn decompress_audio(data: &[u8]) -> Result<(Vec<u8>, AudioParams), String> {
    if data.len() < 12 || &data[0..4] != AUDIO_MAGIC {
        return Err("Bad audio magic".into());
    }

    let sample_rate = u16::from_le_bytes(data[4..6].try_into().unwrap()) as u32 * 100;
    let bits_per_sample = data[6];
    let channels = data[7];
    let quality = data[8];
    let frame_size = data[9] as u16 * 16;
    let num_frames = u16::from_le_bytes(data[10..12].try_into().unwrap()) as usize;

    let params = AudioParams { sample_rate, bits_per_sample, channels, quality, frame_size };

    let mut samples = Vec::new();
    let mut offset = 12;

    for _ in 0..num_frames {
        if offset + 2 > data.len() { return Err("Truncated frame".into()); }
        let frame_len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let frame_data = &data[offset..offset + frame_len];
        offset += frame_len;

        let pcm = if quality == 0 {
            decode(frame_data)?
        } else {
            let (pixels, _) = decompress_image(frame_data)?;
            pixels
        };

        samples.extend_from_slice(&pcm);
    }

    Ok((samples, params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_8bit() {
        // 1 second of 8-bit silence at 8kHz
        let samples = vec![128u8; 8000];
        let params = AudioParams {
            sample_rate: 8000, bits_per_sample: 8, channels: 1,
            quality: 0, frame_size: 256,
        };
        let compressed = compress_audio(&samples, params);
        let (decoded, _) = decompress_audio(&compressed).unwrap();
        assert_eq!(samples, decoded);

        let ratio = samples.len() as f64 / compressed.len() as f64;
        println!("Silence 8kHz 8-bit 1s: {}B → {}B ({:.1}:1)", samples.len(), compressed.len(), ratio);
        assert!(compressed.len() < samples.len() / 10);
    }

    #[test]
    fn test_tone_16bit() {
        // 1 second of 440Hz sine at 16kHz sample rate, 16-bit
        let sample_rate = 16000;
        let freq = 440.0f64;
        let mut samples = Vec::new();
        for i in 0..sample_rate {
            let val = (16000.0 * (2.0 * std::f64::consts::PI * freq * i as f64 / sample_rate as f64).sin()) as i16;
            samples.extend_from_slice(&val.to_le_bytes());
        }

        let params = AudioParams {
            sample_rate: 16000, bits_per_sample: 16, channels: 1,
            quality: 0, frame_size: 256,
        };
        let compressed = compress_audio(&samples, params);
        let (decoded, _) = decompress_audio(&compressed).unwrap();
        assert_eq!(samples, decoded);

        println!("440Hz tone 16kHz 16-bit 1s: {}B → {}B ({:.1}:1)",
            samples.len(), compressed.len(), samples.len() as f64 / compressed.len() as f64);
    }

    #[test]
    fn test_lossy_tone() {
        // Lossy compression of a tone
        let sample_rate = 8000;
        let freq = 1000.0f64;
        let samples: Vec<u8> = (0..sample_rate)
            .map(|i| (128.0 + 100.0 * (2.0 * std::f64::consts::PI * freq * i as f64 / sample_rate as f64).sin()) as u8)
            .collect();

        let params = AudioParams {
            sample_rate: 8000, bits_per_sample: 8, channels: 1,
            quality: 5, frame_size: 256,
        };
        let compressed = compress_audio(&samples, params);
        let (decoded, _) = decompress_audio(&compressed).unwrap();

        // Check max error
        let max_err = samples.iter().zip(decoded.iter())
            .map(|(&a, &b)| (a as i16 - b as i16).unsigned_abs())
            .max().unwrap();
        assert!(max_err <= 5, "Max error {} > quality 5", max_err);

        println!("1kHz lossy q=5: {}B → {}B ({:.1}:1)",
            samples.len(), compressed.len(), samples.len() as f64 / compressed.len() as f64);
    }

    #[test]
    fn test_dtmf() {
        // DTMF tone (two frequencies: 697Hz + 1209Hz) — common in telephony
        let sample_rate = 8000;
        let samples: Vec<u8> = (0..4000) // 0.5 seconds
            .map(|i| {
                let t = i as f64 / sample_rate as f64;
                let val = 64.0 * (2.0 * std::f64::consts::PI * 697.0 * t).sin()
                        + 64.0 * (2.0 * std::f64::consts::PI * 1209.0 * t).sin();
                (128.0 + val).clamp(0.0, 255.0) as u8
            })
            .collect();

        let params = AudioParams {
            sample_rate: 8000, bits_per_sample: 8, channels: 1,
            quality: 0, frame_size: 256,
        };
        let compressed = compress_audio(&samples, params);
        let (decoded, _) = decompress_audio(&compressed).unwrap();
        assert_eq!(samples, decoded);

        println!("DTMF 0.5s lossless: {}B → {}B ({:.1}:1)",
            samples.len(), compressed.len(), samples.len() as f64 / compressed.len() as f64);
    }
}
