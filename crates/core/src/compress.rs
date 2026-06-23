//! gzip response compression (enabled by the `gzip` feature).
//!
//! Wraps miniz_oxide's raw DEFLATE output in the gzip container so the result
//! is a valid `Content-Encoding: gzip` body.

use alloc::vec::Vec;

/// Compresses `data` into a gzip stream (RFC 1952).
pub fn gzip_encode(data: &[u8]) -> Vec<u8> {
    let deflated = miniz_oxide::deflate::compress_to_vec(data, 6);
    let mut out = Vec::with_capacity(deflated.len() + 18);
    // 10-byte header: magic, DEFLATE method, no flags, zero mtime, unknown OS.
    out.extend_from_slice(&[0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xff]);
    out.extend_from_slice(&deflated);
    // Trailer: CRC-32 of the input, then its size mod 2^32 (both little-endian).
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Standard CRC-32 (IEEE 802.3, reflected, polynomial 0xEDB88320).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_vector() {
        // CRC-32 of "123456789" is 0xCBF43926 (the canonical check value).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn gzip_stream_is_well_formed_and_round_trips() {
        let original = b"ferro ferro ferro ferro ferro ferro ferro ferro".repeat(8);
        let gz = gzip_encode(&original);

        // Header magic and method.
        assert_eq!(&gz[..3], &[0x1f, 0x8b, 0x08]);
        // Trailer ISIZE equals the input length.
        let isize_bytes = [
            gz[gz.len() - 4],
            gz[gz.len() - 3],
            gz[gz.len() - 2],
            gz[gz.len() - 1],
        ];
        assert_eq!(u32::from_le_bytes(isize_bytes), original.len() as u32);

        // The DEFLATE payload (between header and 8-byte trailer) inflates back.
        let deflate = &gz[10..gz.len() - 8];
        let restored = miniz_oxide::inflate::decompress_to_vec(deflate).expect("inflate");
        assert_eq!(restored, original);
    }
}
