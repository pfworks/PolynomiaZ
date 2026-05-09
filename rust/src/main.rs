use clap::Parser;
use pltz::codec::{decode, encode, encode_auto, encode_chunked, RawCompressor};
use std::fs;
use std::io::{self, Read, Write};
use std::process;

mod visualize;

/// Polar coordinate compression. Works like bzip2:
///   pltz file          — compress file → file.pltz (removes original)
///   pltz -d file.pltz  — decompress file.pltz → file (removes compressed)
///   pltz -k file       — compress, keep original
///   pltz -c file       — compress to stdout
#[derive(Parser)]
#[command(name = "pltz", version, about)]
struct Cli {
    /// Decompress
    #[arg(short, long)]
    decompress: bool,

    /// Write to stdout
    #[arg(short = 'c', long = "stdout")]
    to_stdout: bool,

    /// Keep input files
    #[arg(short, long)]
    keep: bool,

    /// Force overwrite
    #[arg(short, long)]
    force: bool,

    /// Skip compression on raw tracks (fastest; decompress auto-detects)
    #[arg(long = "no-zlib")]
    no_zlib: bool,

    /// Raw track compressor: fast (zlib), good (zstd), best (try all). Default: best
    #[arg(long, default_value = "best")]
    raw: String,

    /// Record size for columnar pre-processing (e.g. 21, 1k).
    /// Transposes data so each byte-position across records is contiguous.
    #[arg(short = 'r', long = "record-size")]
    record_size: Option<String>,

    /// Chunk size for streaming mode (e.g. 64k, 1M, auto)
    #[arg(short = 'C', long = "chunk-size")]
    chunk_size: Option<String>,

    /// Test integrity
    #[arg(short, long)]
    test: bool,

    /// Analyze data and recommend optimal settings
    #[arg(long)]
    analyze: bool,

    /// Output SVG visualization of disk layout
    #[arg(long)]
    visualize: bool,

    /// Auto-select best compression settings
    #[arg(short = 'b', long)]
    best: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Input files (use - for stdin)
    files: Vec<String>,
}

/// Parse a human-readable size string: "64k", "1M", "4096", etc.
fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty size".into());
    }
    let (num_str, multiplier) = match s.as_bytes().last() {
        Some(b'k' | b'K') => (&s[..s.len()-1], 1024),
        Some(b'm' | b'M') => (&s[..s.len()-1], 1024 * 1024),
        Some(b'g' | b'G') => (&s[..s.len()-1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    let num: usize = num_str.parse().map_err(|_| format!("invalid size: {}", s))?;
    Ok(num * multiplier)
}

/// Auto-detect optimal chunk size based on file size.
/// Heuristic: aim for 16-256 chunks, with chunk sizes that are powers of 2.
fn auto_chunk_size(data_len: usize) -> usize {
    if data_len <= 32 * 1024 {
        return data_len; // small file: single chunk (no overhead)
    }
    // Target ~64 chunks, rounded to power of 2
    let target = data_len / 64;
    // Round up to next power of 2
    let mut chunk = 4096; // minimum 4KB
    while chunk < target && chunk < 1024 * 1024 {
        chunk *= 2;
    }
    chunk
}

fn main() {
    let cli = Cli::parse();

    if cli.files.is_empty() && atty::is(atty::Stream::Stdin) {
        eprintln!("pltz: no files specified. Use - for stdin or --help for usage.");
        process::exit(1);
    }

    let files = if cli.files.is_empty() { vec!["-".to_string()] } else { cli.files.clone() };
    let mut exit_code = 0;

    for file in &files {
        if let Err(e) = process_file(&cli, file) {
            eprintln!("pltz: {}: {}", file, e);
            exit_code = 1;
        }
    }

    process::exit(exit_code);
}

fn process_file(cli: &Cli, path: &str) -> Result<(), String> {
    let decompress = cli.decompress || cli.test || path.ends_with(".pltz");

    let input_data = read_input(path)?;

    if cli.analyze {
        return analyze_data(path, &input_data);
    }

    if cli.visualize {
        return visualize::visualize_data(path, &input_data);
    }

    if decompress || cli.test {
        // Decompress
        let decompressed = decode(&input_data)?;

        if cli.test {
            if cli.verbose {
                eprintln!("  {}: ok ({} → {} bytes)", path, input_data.len(), decompressed.len());
            }
            return Ok(());
        }

        if cli.to_stdout || path == "-" {
            io::stdout().write_all(&decompressed).map_err(|e| e.to_string())?;
        } else {
            let out_path = output_path_decompress(path)?;
            if !cli.force && fs::metadata(&out_path).is_ok() {
                return Err(format!("output file {} already exists (use -f to force)", out_path));
            }
            fs::write(&out_path, &decompressed).map_err(|e| e.to_string())?;
            if cli.verbose {
                let ratio = input_data.len() as f64 / decompressed.len() as f64;
                eprintln!("  {}: {:.1}% ({} → {} bytes)",
                    path, ratio * 100.0, input_data.len(), decompressed.len());
            }
            if !cli.keep {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    } else {
        // Compress
        let raw_comp = if cli.no_zlib {
            RawCompressor::None
        } else {
            match cli.raw.as_str() {
                "none" => RawCompressor::None,
                "fast" | "zlib" => RawCompressor::Zlib,
                "good" | "zstd" => RawCompressor::Zstd,
                "brotli" => RawCompressor::Brotli,
                "best" => RawCompressor::Best,
                _ => RawCompressor::Best,
            }
        };
        let compressed = if cli.best {
            find_best_compression(&input_data, cli.verbose)
        } else if let Some(ref cs) = cli.chunk_size {
            let chunk_size = if cs == "auto" {
                auto_chunk_size(input_data.len())
            } else {
                parse_size(cs)?
            };
            if chunk_size >= input_data.len() {
                encode(&input_data, raw_comp)
            } else {
                encode_chunked(&input_data, raw_comp, chunk_size)
            }
        } else if let Some(ref stride_str) = cli.record_size {
            let stride = parse_size(stride_str)?;
            encode_auto(&input_data, raw_comp, &[stride])
        } else {
            encode(&input_data, raw_comp)
        };

        if cli.to_stdout || path == "-" {
            io::stdout().write_all(&compressed).map_err(|e| e.to_string())?;
        } else {
            let out_path = format!("{}.pltz", path);
            if !cli.force && fs::metadata(&out_path).is_ok() {
                return Err(format!("output file {} already exists (use -f to force)", out_path));
            }
            fs::write(&out_path, &compressed).map_err(|e| e.to_string())?;
            if cli.verbose {
                let ratio = compressed.len() as f64 / input_data.len() as f64;
                eprintln!("  {}: {:.3}:1 ({} → {} bytes)",
                    path, 1.0 / ratio, input_data.len(), compressed.len());
                print_geometry_info(&compressed);
                print_pattern_breakdown(&input_data);
            }
            if !cli.keep {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(())
}

fn read_input(path: &str) -> Result<Vec<u8>, String> {
    if path == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
    } else {
        fs::read(path).map_err(|e| e.to_string())
    }
}

fn output_path_decompress(path: &str) -> Result<String, String> {
    if let Some(stripped) = path.strip_suffix(".pltz") {
        Ok(stripped.to_string())
    } else {
        Err(format!("can't guess original name for {} (no .pltz suffix)", path))
    }
}

/// Detect likely record strides by finding periodicities in the byte stream.
/// Looks for repeating byte values at regular intervals (e.g., null terminators,
/// sync bytes, constant fields).
fn detect_strides(data: &[u8]) -> Vec<usize> {
    let mut candidates: Vec<usize> = Vec::new();
    let max_stride = std::cmp::min(1024, data.len() / 4);
    if max_stride < 2 { return candidates; }

    // Method 1: Autocorrelation of byte equality
    // For each candidate stride, count how many positions have data[i] == data[i+stride]
    let sample_len = std::cmp::min(data.len(), 4096);
    let mut best_scores: Vec<(usize, f64)> = Vec::new();

    for stride in 2..=max_stride {
        let mut matches = 0usize;
        let comparisons = sample_len - stride;
        if comparisons == 0 { continue; }
        for i in 0..comparisons {
            if data[i] == data[i + stride] {
                matches += 1;
            }
        }
        let score = matches as f64 / comparisons as f64;
        // A good stride has high autocorrelation (>0.5 means >50% of bytes repeat)
        if score > 0.4 {
            best_scores.push((stride, score));
        }
    }

    // Sort by score descending, take top candidates
    best_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Take the top 5 unique strides (and their multiples/divisors)
    for &(stride, _) in best_scores.iter().take(8) {
        if !candidates.contains(&stride) {
            candidates.push(stride);
        }
    }

    // Method 2: Look for common struct sizes (powers of 2, common record sizes)
    for &s in &[4, 8, 12, 16, 20, 24, 32, 48, 64, 128, 256, 512] {
        if s < max_stride && !candidates.contains(&s) {
            candidates.push(s);
        }
    }

    candidates
}

fn find_best_compression(data: &[u8], verbose: bool) -> Vec<u8> {
    use pltz::codec::RawCompressor;

    // Use fast compressor for search, then re-encode winner with best
    let search_comp = RawCompressor::Zlib;
    let final_comp = RawCompressor::Best;

    if verbose { eprint!("  [best] plain..."); }
    let mut best = pltz::codec::encode(data, search_comp);
    let mut best_mode: BestMode = BestMode::Plain;
    if verbose { eprintln!(" {}B", best.len()); }

    // Auto-detect strides + try them
    let strides = detect_strides(data);
    for stride in strides {
        if stride >= data.len() || stride < 2 { continue; }
        if verbose { eprint!("  [best] stride={}...", stride); }
        let transformed = pltz::columnar::columnar_transform(data, stride);
        let inner = pltz::codec::encode(&transformed, search_comp);
        let total_size = 10 + inner.len();
        if total_size < best.len() {
            best_mode = BestMode::Columnar(stride);
            let mut buf = Vec::with_capacity(total_size);
            buf.extend_from_slice(b"PLTC");
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(stride as u16).to_le_bytes());
            buf.extend_from_slice(&inner);
            best = buf;
            if verbose { eprintln!(" {}B ★", total_size); }
        } else {
            if verbose { eprintln!(" {}B", total_size); }
        }
    }

    // Try chunked
    for &chunk in &[4096, 8192, 16384, 32768, 65536] {
        if chunk >= data.len() { continue; }
        if verbose { eprint!("  [best] chunk={}k...", chunk/1024); }
        let compressed = pltz::codec::encode_chunked(data, search_comp, chunk);
        if compressed.len() < best.len() {
            best_mode = BestMode::Chunked(chunk);
            best = compressed;
            if verbose { eprintln!(" {}B ★", best.len()); }
        } else {
            if verbose { eprintln!(" {}B", compressed.len()); }
        }
    }

    // Re-encode the winner with best raw compressor
    if verbose { eprint!("  [best] finalizing..."); }
    let final_result = match best_mode {
        BestMode::Plain => pltz::codec::encode(data, final_comp),
        BestMode::Columnar(stride) => {
            let transformed = pltz::columnar::columnar_transform(data, stride);
            let inner = pltz::codec::encode(&transformed, final_comp);
            let mut buf = Vec::with_capacity(10 + inner.len());
            buf.extend_from_slice(b"PLTC");
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(stride as u16).to_le_bytes());
            buf.extend_from_slice(&inner);
            buf
        }
        BestMode::Chunked(chunk) => pltz::codec::encode_chunked(data, final_comp, chunk),
    };
    if verbose { eprintln!(" {}B", final_result.len()); }
    final_result
}

enum BestMode { Plain, Columnar(usize), Chunked(usize) }

fn analyze_data(path: &str, data: &[u8]) -> Result<(), String> {
    use pltz::codec::RawCompressor;
    use pltz::platter::*;
    use pltz::analyzer::*;
    use pltz::cross_track::cross_track_optimize;
    use std::collections::HashMap;

    eprintln!("Analyzing {} ({} bytes)...", path, data.len());
    eprintln!();

    // 1. Try plain encode
    let plain = pltz::codec::encode(data, RawCompressor::Zlib);
    eprintln!("  Default:          {} bytes (ratio {:.3})", plain.len(), plain.len() as f64 / data.len() as f64);

    // 2. Try auto-detected strides for columnar
    let strides = detect_strides(data);
    let mut best_stride: Option<(usize, usize)> = None;
    for stride in &strides {
        let stride = *stride;
        if stride >= data.len() || stride < 2 { continue; }
        let transformed = pltz::columnar::columnar_transform(data, stride);
        let compressed = pltz::codec::encode(&transformed, RawCompressor::Zlib);
        let total = 10 + compressed.len(); // PLTC header overhead
        if best_stride.is_none() || total < best_stride.unwrap().1 {
            best_stride = Some((stride, total));
        }
    }

    if let Some((stride, size)) = best_stride {
        if size < plain.len() {
            eprintln!("  Columnar -r {}:   {} bytes (ratio {:.3})", stride, size, size as f64 / data.len() as f64);
        }
    }

    // 3. Try chunked
    let mut best_chunk: Option<(usize, usize)> = None;
    for &chunk in &[4096, 8192, 16384, 32768, 65536] {
        if chunk >= data.len() { continue; }
        let compressed = pltz::codec::encode_chunked(data, RawCompressor::Zlib, chunk);
        if best_chunk.is_none() || compressed.len() < best_chunk.unwrap().1 {
            best_chunk = Some((chunk, compressed.len()));
        }
    }

    if let Some((chunk, size)) = best_chunk {
        if size < plain.len() {
            let label = if chunk >= 1024 { format!("{}k", chunk / 1024) } else { format!("{}", chunk) };
            eprintln!("  Chunked -C {}:  {} bytes (ratio {:.3})", label, size, size as f64 / data.len() as f64);
        }
    }

    // 4. Pattern breakdown
    eprintln!();
    let geom = compute_geometry(data.len(), 256);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for enc in &encodings {
        let name = match enc.pattern {
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
            PatternType::PiecewiseLinear => "PIECEWISE_LINEAR",
            PatternType::Mirror => "MIRROR",
            PatternType::InterleavedLinear => "INTERLEAVED_LINEAR",
            PatternType::Exponential => "EXPONENTIAL",
            PatternType::DeltaRle => "DELTA_RLE",
            PatternType::BitPacked => "BIT_PACKED",
            PatternType::Raw | PatternType::RawCompressed => "RAW",
        };
        *counts.entry(name).or_insert(0) += 1;
    }

    eprintln!("  Pattern breakdown ({} tracks at spt=256):", encodings.len());
    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (name, count) in sorted {
        eprintln!("    {:<20} {:>4} tracks", name, count);
    }

    // 5. Recommendation
    eprintln!();
    let mut best_size = plain.len();
    let mut recommendation = String::from("pltz");

    if let Some((stride, size)) = best_stride {
        if size < best_size {
            best_size = size;
            recommendation = format!("pltz -r {}", stride);
        }
    }
    if let Some((chunk, size)) = best_chunk {
        if size < best_size {
            best_size = size;
            let label = if chunk >= 1024 { format!("{}k", chunk / 1024) } else { format!("{}", chunk) };
            recommendation = format!("pltz -C {}", label);
        }
    }

    eprintln!("  Recommended: {} {}", recommendation, path);
    eprintln!("  Expected:    {} → {} bytes ({:.1}:1)", data.len(), best_size,
        data.len() as f64 / best_size as f64);

    Ok(())
}

fn print_pattern_breakdown(data: &[u8]) {
    use pltz::platter::*;
    use pltz::analyzer::*;
    use std::collections::HashMap;

    let geom = pltz::codec::best_geometry_pub(data);
    let tracks = lay_out(data, &geom);
    let encodings = analyze_platter(&tracks);

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for enc in &encodings {
        let name = match enc.pattern {
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
        };
        *counts.entry(name).or_insert(0) += 1;
    }

    let mut sorted: Vec<_> = counts.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));

    eprintln!("    patterns: {}", sorted.iter()
        .map(|(name, count)| format!("{}×{}", count, name))
        .collect::<Vec<_>>()
        .join(", "));
}

fn print_geometry_info(compressed: &[u8]) {
    if compressed.len() < 12 { return; }
    let magic = &compressed[0..4];
    match magic {
        b"PLTZ" => {
            let orig = u32::from_le_bytes(compressed[4..8].try_into().unwrap());
            let spt = u16::from_le_bytes(compressed[8..10].try_into().unwrap());
            let platters = u16::from_le_bytes(compressed[10..12].try_into().unwrap());
            let tracks = if spt > 0 { (orig as usize + spt as usize - 1) / spt as usize } else { 0 };
            eprintln!("    geometry: {} tracks × {} sectors, {} platters",
                tracks, spt, platters);
        }
        b"PLTC" => {
            let orig = u32::from_le_bytes(compressed[4..8].try_into().unwrap());
            let stride = u16::from_le_bytes(compressed[8..10].try_into().unwrap());
            eprintln!("    columnar: stride={}, inner:", stride);
            if compressed.len() > 10 {
                print_geometry_info(&compressed[10..]);
            }
            let _ = orig;
        }
        b"PLTS" => {
            let orig = u32::from_le_bytes(compressed[5..9].try_into().unwrap());
            let chunk_size = u32::from_le_bytes(compressed[9..13].try_into().unwrap());
            let num_chunks = u16::from_le_bytes(compressed[13..15].try_into().unwrap());
            eprintln!("    chunked: {}B original, {} chunks of {}B",
                orig, num_chunks, chunk_size);
        }
        _ => {}
    }
}

// Minimal atty replacement for detecting if stdin is a terminal
mod atty {
    pub enum Stream { Stdin }
    pub fn is(_: Stream) -> bool {
        unsafe { libc::isatty(0) != 0 }
    }
    mod libc {
        extern "C" { pub fn isatty(fd: i32) -> i32; }
    }
}
