/// C FFI bindings for the PLTZ codec.
/// Build with: cargo build --release
/// Produces: libpltz.so (Linux) / libpltz.dylib (macOS) / pltz.dll (Windows)

use std::slice;

/// Compress data. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn pltz_compress(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: *mut usize,
) -> i32 {
    if input.is_null() || output.is_null() || output_len.is_null() {
        return -1;
    }

    let input_slice = slice::from_raw_parts(input, input_len);
    let compressed = crate::codec::encode(input_slice, crate::codec::RawCompressor::Best);

    let out_cap = *output_len;
    if compressed.len() > out_cap {
        return -1;
    }

    let output_slice = slice::from_raw_parts_mut(output, out_cap);
    output_slice[..compressed.len()].copy_from_slice(&compressed);
    *output_len = compressed.len();
    0
}

/// Decompress data. Returns 0 on success, -1 on error.
#[no_mangle]
pub unsafe extern "C" fn pltz_decompress(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: *mut usize,
) -> i32 {
    if input.is_null() || output.is_null() || output_len.is_null() {
        return -1;
    }

    let input_slice = slice::from_raw_parts(input, input_len);
    let decompressed = match crate::codec::decode(input_slice) {
        Ok(d) => d,
        Err(_) => return -1,
    };

    let out_cap = *output_len;
    if decompressed.len() > out_cap {
        *output_len = decompressed.len(); // tell caller how much space needed
        return -1;
    }

    let output_slice = slice::from_raw_parts_mut(output, out_cap);
    output_slice[..decompressed.len()].copy_from_slice(&decompressed);
    *output_len = decompressed.len();
    0
}

/// Get decompressed size from header. Returns 0 on error.
#[no_mangle]
pub unsafe extern "C" fn pltz_decompressed_size(
    input: *const u8,
    input_len: usize,
) -> usize {
    if input.is_null() || input_len < 12 {
        return 0;
    }

    let input_slice = slice::from_raw_parts(input, input_len);
    let magic = &input_slice[0..4];

    match magic {
        b"PLTZ" => u32::from_le_bytes(input_slice[4..8].try_into().unwrap()) as usize,
        b"PLTC" => u32::from_le_bytes(input_slice[4..8].try_into().unwrap()) as usize,
        b"PLTS" => {
            if input_len < 15 { return 0; }
            u32::from_le_bytes(input_slice[5..9].try_into().unwrap()) as usize
        }
        _ => 0,
    }
}

/// Maximum compressed size (input_len + overhead).
#[no_mangle]
pub extern "C" fn pltz_max_compressed_size(input_len: usize) -> usize {
    input_len + 64 + input_len / 100 // ~1% overhead + header
}
