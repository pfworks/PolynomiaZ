/// Columnar pre-processing: transpose record-oriented data so that
/// each field is contiguous, exposing per-field patterns for PLTZ.
///
/// Input: N records of R bytes each (row-major)
/// Output: R columns of N bytes each (column-major)
///
/// The transform is its own inverse when applied with the same parameters.

/// Transpose data from row-major to column-major.
/// `stride` is the record size in bytes.
/// Pads to a full matrix if needed.
pub fn columnar_transform(data: &[u8], stride: usize) -> Vec<u8> {
    if stride == 0 || data.is_empty() {
        return data.to_vec();
    }
    let rows = (data.len() + stride - 1) / stride;
    let padded_len = rows * stride;

    // Pad input
    let mut padded = data.to_vec();
    padded.resize(padded_len, 0);

    // Transpose: read column by column
    let mut out = Vec::with_capacity(padded_len);
    for col in 0..stride {
        for row in 0..rows {
            out.push(padded[row * stride + col]);
        }
    }
    out
}

/// Inverse columnar transform (column-major back to row-major).
pub fn columnar_untransform(data: &[u8], stride: usize, original_len: usize) -> Vec<u8> {
    if stride == 0 || data.is_empty() {
        return data.to_vec();
    }
    let rows = (original_len + stride - 1) / stride;
    let padded_len = rows * stride;

    let mut padded = data.to_vec();
    padded.resize(padded_len, 0);

    let mut out = Vec::with_capacity(padded_len);
    for row in 0..rows {
        for col in 0..stride {
            out.push(padded[col * rows + row]);
        }
    }
    out.truncate(original_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip_exact() {
        let data = b"AABBCCDD11223344";
        let transformed = columnar_transform(data, 4);
        let restored = columnar_untransform(&transformed, 4, data.len());
        assert_eq!(data.as_slice(), restored.as_slice());
    }

    #[test]
    fn test_roundtrip_uneven() {
        let data = b"Hello, World!"; // 13 bytes, stride 5
        let transformed = columnar_transform(data, 5);
        let restored = columnar_untransform(&transformed, 5, data.len());
        assert_eq!(data.as_slice(), restored.as_slice());
    }

    #[test]
    fn test_groups_fields() {
        // 4 records of 3 bytes: [A,1,x], [A,2,x], [A,3,x], [A,4,x]
        let data = b"A1xA2xA3xA4x";
        let transformed = columnar_transform(data, 3);
        // Column 0 should be "AAAA", column 1 should be "1234", column 2 should be "xxxx"
        assert_eq!(&transformed[0..4], b"AAAA");  // field 0: constant
        assert_eq!(&transformed[4..8], b"1234");  // field 1: linear
        assert_eq!(&transformed[8..12], b"xxxx"); // field 2: constant
    }
}
