/// Generate SVG visualization of disk platter layout.

use pltz::platter::*;
use pltz::analyzer::*;
use pltz::columnar;

pub fn visualize_data(path: &str, data: &[u8]) -> Result<(), String> {
    // Use the same logic as the encoder (optimize for compressed size)
    let best_data: Vec<u8>;
    let best_geom: pltz::platter::DiskGeometry;
    let used_stride: Option<usize>;

    // Try plain with encoder's geometry selection
    let plain_geom = pltz::codec::best_geometry_pub(data);
    let plain_tracks = lay_out(data, &plain_geom);
    let plain_enc = analyze_platter(&plain_tracks);
    let plain_raw: usize = plain_enc.iter().filter(|e| e.pattern == PatternType::Raw).count();

    // Try columnar strides
    let strides = super::detect_strides(data);
    let mut col_best: Option<(usize, pltz::platter::DiskGeometry, Vec<u8>, usize)> = None;
    for stride in &strides {
        let stride = *stride;
        if stride < 2 || stride >= data.len() { continue; }
        let transformed = pltz::columnar::columnar_transform(data, stride);
        let geom = pltz::codec::best_geometry_pub(&transformed);
        let tracks = lay_out(&transformed, &geom);
        let enc = analyze_platter(&tracks);
        let raw_count = enc.iter().filter(|e| e.pattern == PatternType::Raw).count();
        if col_best.is_none() || raw_count < col_best.as_ref().unwrap().3 {
            col_best = Some((stride, geom, transformed, raw_count));
        }
    }

    if let Some((stride, geom, transformed, raw_count)) = col_best {
        if raw_count < plain_raw {
            best_data = transformed;
            best_geom = geom;
            used_stride = Some(stride);
        } else {
            best_data = data.to_vec();
            best_geom = plain_geom;
            used_stride = None;
        }
    } else {
        best_data = data.to_vec();
        best_geom = plain_geom;
        used_stride = None;
    }

    let tracks = lay_out(&best_data, &best_geom);
    let encodings = analyze_platter(&tracks);

    let num_tracks = encodings.len();
    let spt = best_geom.sectors_per_track;
    let subtitle = match used_stride {
        Some(s) => format!("columnar stride={}, spt={}", s, spt),
        None => format!("spt={}", spt),
    };

    // Determine which RAW tracks benefit from deflate
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;
    use std::collections::HashSet;

    let mut deflatable: HashSet<u16> = HashSet::new();
    for enc in &encodings {
        if enc.pattern == PatternType::Raw {
            if let TrackParams::Raw { data } = &enc.params {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
                let _ = encoder.write_all(data);
                if let Ok(compressed) = encoder.finish() {
                    if compressed.len() < data.len() {
                        deflatable.insert(enc.track_idx);
                    }
                }
            }
        }
    }

    // Split into platters
    let structured: Vec<&TrackEncoding> = encodings.iter().filter(|e| e.pattern != PatternType::Raw && e.pattern != PatternType::RawCompressed).collect();
    let raw: Vec<&TrackEncoding> = encodings.iter().filter(|e| e.pattern == PatternType::Raw || e.pattern == PatternType::RawCompressed).collect();
    let has_two_platters = !structured.is_empty() && !raw.is_empty();

    let img_width = if has_two_platters { 1300 } else { 750 };
    let mut svg = String::new();
    svg.push_str(&format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"620\" viewBox=\"0 0 {} 620\">\n", img_width, img_width));
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#1a1a2e\"/>\n");

    // Helper to draw a platter
    let draw_platter = |svg: &mut String, cx: f64, cy: f64, tracks_to_draw: &[&TrackEncoding], label: &str, deflatable: &HashSet<u16>| {
        let r_min = 50.0f64;
        let r_max = 200.0f64;
        let n = tracks_to_draw.len();
        let tw = if n > 0 { (r_max - r_min) / n as f64 } else { 10.0 };

        for (i, enc) in tracks_to_draw.iter().enumerate() {
            let r_outer = r_max - i as f64 * tw;
            let r_inner = r_outer - tw + 1.0;
            let color = if enc.pattern == PatternType::Raw {
                if deflatable.contains(&enc.track_idx) {
                    "#616161" // darker gray = deflated
                } else {
                    "#BDBDBD" // lighter gray = stored raw
                }
            } else {
                pattern_color(enc.pattern)
            };
            let r_mid = (r_outer + r_inner) / 2.0;
            let sw = (tw - 1.0).max(1.0).min(20.0);
            svg.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{:.1}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{:.1}\" opacity=\"0.85\"/>\n",
                cx, cy, r_mid, color, sw
            ));
        }

        // Center hub
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"55\" fill=\"#333\" stroke=\"#555\" stroke-width=\"2\"/>\n", cx, cy
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">{}</text>\n",
            cx, cy as i32 - 8, label
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"white\" font-size=\"9\" font-family=\"monospace\">{} tracks</text>\n",
            cx, cy as i32 + 8, n
        ));
    };

    if has_two_platters {
        // Platter 0 (structured) on left
        svg.push_str("<text x=\"240\" y=\"580\" text-anchor=\"middle\" fill=\"#aaa\" font-size=\"12\" font-family=\"monospace\">Platter 0 (Structured)</text>\n");
        draw_platter(&mut svg, 240.0, 310.0, &structured, "Platter 0", &deflatable);

        // Platter 1 (raw) on right
        svg.push_str("<text x=\"700\" y=\"580\" text-anchor=\"middle\" fill=\"#aaa\" font-size=\"12\" font-family=\"monospace\">Platter 1 (Raw)</text>\n");
        draw_platter(&mut svg, 700.0, 310.0, &raw, "Platter 1", &deflatable);
    } else {
        let all: Vec<&TrackEncoding> = encodings.iter().collect();
        draw_platter(&mut svg, 300.0, 310.0, &all, &subtitle, &deflatable);
    }

    // Legend
    let mut seen: Vec<PatternType> = Vec::new();
    for enc in &encodings {
        if !seen.contains(&enc.pattern) { seen.push(enc.pattern); }
    }

    let legend_x = if has_two_platters { 1050 } else { 620 };
    let mut legend_y = 50;
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"13\" font-family=\"monospace\" font-weight=\"bold\">Legend</text>\n",
        legend_x, legend_y - 15
    ));

    for &pat in &seen {
        if pat == PatternType::Raw || pat == PatternType::RawCompressed {
            // Show two entries for RAW
            let deflated_count = encodings.iter()
                .filter(|e| e.pattern == PatternType::Raw && deflatable.contains(&e.track_idx))
                .count();
            let raw_count = encodings.iter()
                .filter(|e| e.pattern == PatternType::Raw && !deflatable.contains(&e.track_idx))
                .count();
            if deflated_count > 0 {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"14\" fill=\"#616161\" rx=\"2\"/>\n",
                    legend_x, legend_y - 11
                ));
                svg.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">RAW deflated ({})</text>\n",
                    legend_x + 20, legend_y, deflated_count
                ));
                legend_y += 20;
            }
            if raw_count > 0 {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"14\" fill=\"#BDBDBD\" rx=\"2\"/>\n",
                    legend_x, legend_y - 11
                ));
                svg.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">RAW stored ({})</text>\n",
                    legend_x + 20, legend_y, raw_count
                ));
                legend_y += 20;
            }
        } else {
            let color = pattern_color(pat);
            let name = pattern_name(pat);
            let count = encodings.iter().filter(|e| e.pattern == pat).count();
            svg.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"14\" fill=\"{}\" rx=\"2\"/>\n",
                legend_x, legend_y - 11, color
            ));
            svg.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">{} ({})</text>\n",
                legend_x + 20, legend_y, name, count
            ));
            legend_y += 20;
        }
    }

    // Title
    let title_x = if has_two_platters { 650 } else { 300 };
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"20\" text-anchor=\"middle\" fill=\"white\" font-size=\"14\" font-family=\"monospace\">{} \u{2014} {} bytes ({})</text>\n",
        title_x, path, data.len(), subtitle
    ));

    svg.push_str("</svg>\n");

    let out_path = format!("{}.svg", path);
    std::fs::write(&out_path, &svg).map_err(|e| e.to_string())?;
    eprintln!("  Visualization: {}", out_path);
    Ok(())
}

fn pattern_color(p: PatternType) -> &'static str {
    match p {
        PatternType::Const => "#4CAF50",
        PatternType::Linear => "#2196F3",
        PatternType::Rle => "#FF9800",
        PatternType::Repeat => "#9C27B0",
        PatternType::Delta => "#00BCD4",
        PatternType::Periodic => "#E91E63",
        PatternType::Poly => "#673AB7",
        PatternType::LinearWide => "#3F51B5",
        PatternType::LinearF64 => "#1565C0",
        PatternType::DeltaF64 => "#00838F",
        PatternType::XorMask => "#F44336",
        PatternType::Sparse => "#FFC107",
        PatternType::PiecewiseLinear => "#8BC34A",
        PatternType::Mirror => "#FF5722",
        PatternType::InterleavedLinear => "#607D8B",
        PatternType::Exponential => "#795548",
        PatternType::DeltaRle => "#CDDC39",
        PatternType::BitPacked => "#009688",
        PatternType::DiffTable => "#880E4F",
        PatternType::ModArith => "#1B5E20",
        PatternType::Fibonacci => "#E65100",
        PatternType::DeltaWide => "#AD1457",
        PatternType::Raw | PatternType::RawCompressed => "#9E9E9E",
    }
}

fn pattern_name(p: PatternType) -> &'static str {
    match p {
        PatternType::Const => "CONST",
        PatternType::Linear => "LINEAR",
        PatternType::Rle => "RLE",
        PatternType::Repeat => "REPEAT",
        PatternType::Delta => "DELTA",
        PatternType::Periodic => "PERIODIC",
        PatternType::Poly => "POLY",
        PatternType::LinearWide => "LINEAR_WIDE",
        PatternType::LinearF64 => "LINEAR_F64",
        PatternType::DeltaF64 => "DELTA_F64",
        PatternType::XorMask => "XOR_MASK",
        PatternType::Sparse => "SPARSE",
        PatternType::PiecewiseLinear => "PIECEWISE_LIN",
        PatternType::Mirror => "MIRROR",
        PatternType::InterleavedLinear => "INTERLEAVED",
        PatternType::Exponential => "EXPONENTIAL",
        PatternType::DeltaRle => "DELTA_RLE",
        PatternType::BitPacked => "BIT_PACKED",
        PatternType::DiffTable => "DIFF_TABLE",
        PatternType::ModArith => "MOD_ARITH",
        PatternType::Fibonacci => "FIBONACCI",
        PatternType::DeltaWide => "DELTA_WIDE",
        PatternType::Raw | PatternType::RawCompressed => "RAW",
    }
}
