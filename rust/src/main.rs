use clap::{Parser, Subcommand};
use pltz::codec::{decode, encode, RawCompressor};
use std::fs;
use std::io::{self, Read, Write};
use std::process;

#[derive(Parser)]
#[command(name = "pltz", version, about = "Polar coordinate compression")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compress a file
    #[command(alias = "c")]
    Compress {
        /// Input file (- for stdin)
        input: String,
        /// Output file (- for stdout)
        #[arg(short, long)]
        output: Option<String>,
        /// Use zlib for raw (unpatternized) tracks
        #[arg(long)]
        zlib: bool,
    },
    /// Decompress a file
    #[command(alias = "d")]
    Decompress {
        /// Input file (- for stdin)
        input: String,
        /// Output file (- for stdout)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Show info about a compressed file
    Info {
        /// Input .pltz file
        input: String,
    },
}

fn read_input(path: &str) -> io::Result<Vec<u8>> {
    if path == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        fs::read(path)
    }
}

fn write_output(path: &Option<String>, data: &[u8]) -> io::Result<()> {
    match path {
        Some(p) if p != "-" => fs::write(p, data),
        _ => io::stdout().write_all(data),
    }
}

fn default_output(input: &str, compress: bool) -> String {
    if input == "-" {
        return "-".to_string();
    }
    if compress {
        format!("{}.pltz", input)
    } else {
        input.strip_suffix(".pltz").unwrap_or(input).to_string()
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Compress { input, output, zlib } => {
            let data = read_input(&input).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {}", input, e);
                process::exit(1);
            });

            let raw_comp = if zlib { RawCompressor::Zlib } else { RawCompressor::None };
            let compressed = encode(&data, raw_comp);

            let out_path = output.unwrap_or_else(|| default_output(&input, true));
            let ratio = if data.is_empty() { 0.0 } else { compressed.len() as f64 / data.len() as f64 };
            write_output(&Some(out_path.clone()), &compressed).unwrap_or_else(|e| {
                eprintln!("Error writing {}: {}", out_path, e);
                process::exit(1);
            });

            eprintln!(
                "{} → {} ({} → {} bytes, ratio {:.3})",
                input, out_path, data.len(), compressed.len(), ratio
            );
        }
        Commands::Decompress { input, output } => {
            let compressed = read_input(&input).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {}", input, e);
                process::exit(1);
            });

            let data = decode(&compressed).unwrap_or_else(|e| {
                eprintln!("Error decoding: {}", e);
                process::exit(1);
            });

            let out_path = output.unwrap_or_else(|| default_output(&input, false));
            write_output(&Some(out_path.clone()), &data).unwrap_or_else(|e| {
                eprintln!("Error writing {}: {}", out_path, e);
                process::exit(1);
            });

            eprintln!(
                "{} → {} ({} → {} bytes)",
                input, out_path, compressed.len(), data.len()
            );
        }
        Commands::Info { input } => {
            let compressed = read_input(&input).unwrap_or_else(|e| {
                eprintln!("Error reading {}: {}", input, e);
                process::exit(1);
            });

            if compressed.len() < 12 || &compressed[0..4] != b"PLTZ" {
                eprintln!("Not a valid PLTZ file");
                process::exit(1);
            }

            let original_len = u32::from_le_bytes(compressed[4..8].try_into().unwrap());
            let sectors = u16::from_le_bytes(compressed[8..10].try_into().unwrap());
            let num_platters = u16::from_le_bytes(compressed[10..12].try_into().unwrap());

            println!("PLTZ file: {}", input);
            println!("  Compressed size:   {} bytes", compressed.len());
            println!("  Original size:     {} bytes", original_len);
            println!("  Sectors per track: {}", sectors);
            println!("  Platters:          {}", num_platters);
            if original_len > 0 {
                println!("  Ratio:             {:.3}", compressed.len() as f64 / original_len as f64);
            }
        }
    }
}
