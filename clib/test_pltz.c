/* Test the PLTZ C library */
#include <stdio.h>
#include <string.h>
#include "pltz.h"

int main() {
    /* Create test data: linear ramp */
    uint8_t input[256];
    for (int i = 0; i < 256; i++) input[i] = (uint8_t)i;

    /* Compress */
    uint8_t compressed[512];
    size_t comp_len = sizeof(compressed);
    int ret = pltz_compress(input, sizeof(input), compressed, &comp_len);
    if (ret != 0) { printf("FAIL: compress returned %d\n", ret); return 1; }
    printf("Compressed: %zu -> %zu bytes (%.1f:1)\n", sizeof(input), comp_len,
           (double)sizeof(input) / comp_len);

    /* Check decompressed size */
    size_t orig_size = pltz_decompressed_size(compressed, comp_len);
    printf("Reported original size: %zu\n", orig_size);

    /* Decompress */
    uint8_t output[256];
    size_t out_len = sizeof(output);
    ret = pltz_decompress(compressed, comp_len, output, &out_len);
    if (ret != 0) { printf("FAIL: decompress returned %d\n", ret); return 1; }

    /* Verify */
    if (out_len != sizeof(input) || memcmp(input, output, sizeof(input)) != 0) {
        printf("FAIL: data mismatch\n");
        return 1;
    }

    printf("Roundtrip OK: %zu bytes match\n", out_len);
    return 0;
}
