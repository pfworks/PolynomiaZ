/// Generate SVG visualization of disk platter layout.

use pltz::platter::*;
use pltz::analyzer::*;

pub fn visualize_data(path: &str, data: &[u8]) -> Result<(), String> {
    let geom = compute_geometry(data.len(), 256);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);

    let num_tracks = encodings.len();
    let spt = geom.sectors_per_track;

    let cx = 300.0f64;
    let cy = 300.0f64;
    let r_min = 60.0;
    let r_max = 270.0;
    let track_width = if num_tracks > 0 { (r_max - r_min) / num_tracks as f64 } else { 10.0 };

    let mut svg = String::new();
    svg.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"750\" height=\"620\" viewBox=\"0 0 750 620\">\n");
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#1a1a2e\"/>\n");

    // Draw tracks as concentric rings
    for enc in &encodings {
        let t = enc.track_idx as f64;
        let r_outer = r_max - t * track_width;
        let r_inner = r_outer - track_width + 1.0;
        let color = pattern_color(enc.pattern);
        let r_mid = (r_outer + r_inner) / 2.0;
        let sw = (track_width - 1.0).max(1.0);

        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{:.1}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{:.1}\" opacity=\"0.85\"/>\n",
            cx, cy, r_mid, color, sw
        ));
    }

    // Center hub
    svg.push_str(&format!(
        "<circle cx=\"{}\" cy=\"{}\" r=\"55\" fill=\"#333\" stroke=\"#555\" stroke-width=\"2\"/>\n",
        cx, cy
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">{} tracks</text>\n",
        cx, cy as i32 - 8, num_tracks
    ));
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">spt={}</text>\n",
        cx, cy as i32 + 8, spt
    ));

    // Legend
    let mut seen: Vec<PatternType> = Vec::new();
    for enc in &encodings {
        if !seen.contains(&enc.pattern) { seen.push(enc.pattern); }
    }

    let legend_x = 620;
    let mut legend_y = 50;
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"13\" font-family=\"monospace\" font-weight=\"bold\">Legend</text>\n",
        legend_x - 10, legend_y - 15
    ));

    for &pat in &seen {
        let color = pattern_color(pat);
        let name = pattern_name(pat);
        let count = encodings.iter().filter(|e| e.pattern == pat).count();
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"14\" fill=\"{}\" rx=\"2\"/>\n",
            legend_x - 10, legend_y - 11, color
        ));
        svg.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"white\" font-size=\"11\" font-family=\"monospace\">{} ({})</text>\n",
            legend_x + 10, legend_y, name, count
        ));
        legend_y += 20;
    }

    // Title
    svg.push_str(&format!(
        "<text x=\"{}\" y=\"20\" text-anchor=\"middle\" fill=\"white\" font-size=\"14\" font-family=\"monospace\">{} \u{2014} {} bytes</text>\n",
        cx, path, data.len()
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
        PatternType::Raw | PatternType::RawCompressed => "RAW",
    }
}
