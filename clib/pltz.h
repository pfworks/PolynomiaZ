#ifndef PLTZ_H
#define PLTZ_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Compress data using PLTZ.
 * 
 * @param input      Input data buffer
 * @param input_len  Length of input data in bytes
 * @param output     Output buffer (caller-allocated, must be at least input_len + 64)
 * @param output_len On input: size of output buffer. On output: actual compressed size.
 * @return 0 on success, -1 on error
 */
int pltz_compress(const uint8_t *input, size_t input_len,
                  uint8_t *output, size_t *output_len);

/**
 * Decompress PLTZ data.
 *
 * @param input      Compressed data buffer
 * @param input_len  Length of compressed data
 * @param output     Output buffer (caller-allocated)
 * @param output_len On input: size of output buffer. On output: actual decompressed size.
 * @return 0 on success, -1 on error
 */
int pltz_decompress(const uint8_t *input, size_t input_len,
                    uint8_t *output, size_t *output_len);

/**
 * Get the original (decompressed) size from a PLTZ header.
 * Useful for allocating the output buffer before decompression.
 *
 * @param input      Compressed data buffer (at least 12 bytes)
 * @param input_len  Length of compressed data
 * @return Original data size, or 0 on error
 */
size_t pltz_decompressed_size(const uint8_t *input, size_t input_len);

/**
 * Get the maximum compressed size for a given input length.
 * Use this to allocate the output buffer for compression.
 */
size_t pltz_max_compressed_size(size_t input_len);

#ifdef __cplusplus
}
#endif

#endif /* PLTZ_H */
