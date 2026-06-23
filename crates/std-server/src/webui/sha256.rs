//! Minimal SHA-256 (FIPS 180-4) and HTTP Basic auth helpers for the admin
//! panel. Hand-rolled to keep the `webui` feature dependency-free.

/// SHA-256 initial hash values.
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256 round constants.
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Computes the SHA-256 digest of `input`, returned as a 64-char lowercase hex
/// string.
pub fn sha256_hex(input: &[u8]) -> String {
    let mut h = H0;

    // Pad: append 0x80, then zeros until 56 mod 64, then the 64-bit length.
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity(input.len() + 72);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for block in msg.chunks_exact(64) {
        for (i, word) in w.iter_mut().enumerate().take(16) {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut hex = String::with_capacity(64);
    for word in h {
        hex.push_str(&format!("{word:08x}"));
    }
    hex
}

/// Verifies an HTTP Basic `Authorization` header value against the configured
/// admin username and password hash. Returns true only when the decoded
/// `user:password` matches the username exactly and `SHA-256(password)` equals
/// `password_sha256`. An empty configured username or hash always fails.
pub fn basic_auth_ok(header: &str, username: &str, password_sha256: &str) -> bool {
    if username.is_empty() || password_sha256.is_empty() {
        return false;
    }
    let encoded = match header.strip_prefix("Basic ") {
        Some(rest) => rest.trim(),
        None => return false,
    };
    let decoded = match base64_decode(encoded) {
        Some(bytes) => bytes,
        None => return false,
    };
    let text = match core::str::from_utf8(&decoded) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let (user, pass) = match text.split_once(':') {
        Some(pair) => pair,
        None => return false,
    };
    let pass_hash = sha256_hex(pass.as_bytes());
    // Non-short-circuiting `&` so both comparisons always run.
    constant_time_eq(user.as_bytes(), username.as_bytes())
        & constant_time_eq(pass_hash.as_bytes(), password_sha256.as_bytes())
}

/// Verifies a plaintext password against a stored lowercase-hex SHA-256 digest,
/// in constant time. Used to confirm the current password before changing it.
pub fn password_matches(password: &str, password_sha256: &str) -> bool {
    constant_time_eq(
        sha256_hex(password.as_bytes()).as_bytes(),
        password_sha256.as_bytes(),
    )
}

/// Length-checked, constant-time byte comparison (no early exit on first diff).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Decodes standard base64 (alphabet `A-Za-z0-9+/`, optional `=` padding).
/// Returns None on any invalid character or a truncated group.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() == 1 {
            return None; // a lone trailing character is malformed
        }
        let mut buf = [0u8; 4];
        for (i, &c) in chunk.iter().enumerate() {
            buf[i] = val(c)?;
        }
        out.push((buf[0] << 2) | (buf[1] >> 4));
        if chunk.len() >= 3 {
            out.push((buf[1] << 4) | (buf[2] >> 2));
        }
        if chunk.len() == 4 {
            out.push((buf[2] << 6) | buf[3]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        // FIPS 180-4 / NIST test vectors.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn sha256_handles_block_boundary_lengths() {
        // 55, 56, 64 bytes exercise the padding edge cases around a block.
        assert_eq!(sha256_hex(&[0x61u8; 55]).len(), 64);
        assert_eq!(sha256_hex(&[0x61u8; 56]).len(), 64);
        assert_eq!(
            sha256_hex(&[0x61u8; 64]),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
    }

    #[test]
    fn base64_decode_round_trips_known_value() {
        // "admin:secret" -> YWRtaW46c2VjcmV0
        assert_eq!(base64_decode("YWRtaW46c2VjcmV0").unwrap(), b"admin:secret");
        // padded value: "admin:wrong" -> YWRtaW46d3Jvbmc=
        assert_eq!(base64_decode("YWRtaW46d3Jvbmc=").unwrap(), b"admin:wrong");
        assert!(base64_decode("not base64 !!").is_none());
    }

    #[test]
    fn basic_auth_accepts_only_correct_credentials() {
        let hash = sha256_hex(b"secret");
        // Why: the panel must authenticate the exact username and password, and
        // never accept a wrong password, wrong user, missing header, or empty config.
        assert!(basic_auth_ok("Basic YWRtaW46c2VjcmV0", "admin", &hash));
        assert!(!basic_auth_ok("Basic YWRtaW46d3Jvbmc=", "admin", &hash)); // wrong pass
        assert!(!basic_auth_ok("Basic d3Jvbmc6c2VjcmV0", "admin", &hash)); // wrong user
        assert!(!basic_auth_ok("Bearer YWRtaW46c2VjcmV0", "admin", &hash)); // wrong scheme
        assert!(!basic_auth_ok("", "admin", &hash)); // no header
        assert!(!basic_auth_ok("Basic YWRtaW46c2VjcmV0", "", "")); // empty config
    }

    #[test]
    fn password_matches_only_the_right_password() {
        let hash = sha256_hex(b"secret");
        assert!(password_matches("secret", &hash));
        assert!(!password_matches("wrong", &hash));
        assert!(!password_matches("secret", "")); // empty stored hash never matches
    }
}
