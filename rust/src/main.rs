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

    /// Write to stdout (don't create files)
    #[arg(short = 'c', long = "stdout")]
    to_stdout: bool,

    /// Keep input files (don't delete)
    #[arg(short, long)]
    keep: bool,

    /// Force overwrite of output files
    #[arg(short, long)]
    force: bool,

    /// Use zlib for raw (unpatternized) tracks
    #[arg(short = 'z', long)]
    zlib: bool,

    /// Columnar pre-processing with given record stride (bytes)
    #[arg(short = 'r', long = "record-size")]
    record_size: Option<usize>,

    /// Chunked/streaming mode with given chunk size (bytes)
    #[arg(short = 'C', long = "chunk-size")]
    chunk_size: Option<usize>,

    /// Test integrity (decompress and discard)
    #[arg(short, long)]
    test: bool,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Input files (use - for stdin)
    files: Vec<String>,
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
        let raw_comp = if cli.zlib { RawCompressor::Zlib } else { RawCompressor::None };
        let compressed = if let Some(chunk_size) = cli.chunk_size {
            encode_chunked(&input_data, raw_comp, chunk_size)
        } else if let Some(stride) = cli.record_size {
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
