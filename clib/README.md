# PLTZ C Library

C bindings for the PLTZ compression codec.

## Building

```bash
cd rust
cargo build --release
# Produces: rust/target/release/libpltz.so (Linux)
#           rust/target/release/libpltz.dylib (macOS)
```

## API

```c
#include "pltz.h"

// Compress
uint8_t compressed[pltz_max_compressed_size(input_len)];
size_t comp_len = sizeof(compressed);
pltz_compress(input, input_len, compressed, &comp_len);

// Get original size (for allocation)
size_t orig_size = pltz_decompressed_size(compressed, comp_len);

// Decompress
uint8_t *output = malloc(orig_size);
size_t out_len = orig_size;
pltz_decompress(compressed, comp_len, output, &out_len);
```

## Linking

```bash
gcc -o myapp myapp.c -I/path/to/clib -L/path/to/rust/target/release -lpltz
```

## Test

```bash
cd clib
gcc -o test_pltz test_pltz.c -I. -L../rust/target/release -lpltz
LD_LIBRARY_PATH=../rust/target/release ./test_pltz
```
