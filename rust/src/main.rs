use clap::Parser;
use pltz::codec::{decode, encode, encode_auto, encode_chunked, RawCompressor};
use std::fs;
use std::io::{self, Read, Write};
use std::process;

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

    /// Skip deflate on raw tracks (no zlib dep; decompress auto-detects)
    #[arg(long = "no-zlib")]
    no_zlib: bool,

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
        let raw_comp = if cli.no_zlib { RawCompressor::None } else { RawCompressor::Zlib };
        let compressed = if let Some(ref cs) = cli.chunk_size {
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
