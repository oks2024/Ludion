//! Minimal standard (RFC 4648) base64 encoding, for the `render.screenshot` PNG payload.
//!
//! The protocol layer carries a screenshot as a plain base64 `String` (no base64
//! dependency in `ludion-protocol`; the codec lives in consumers). This is that codec's encode half:
//! a tiny, deterministic, dependency-free implementation. Decoding lives in the clients that need it.

/// The standard base64 alphabet (RFC 4648 §4), index 0–63.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The `=` padding byte.
const PAD: u8 = b'=';

/// Encodes `input` as standard base64 with `=` padding. Behavior is pinned by the module tests
/// against the RFC 4648 `foobar` vectors.
pub(crate) fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            PAD as char
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            PAD as char
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn matches_rfc4648_vectors() {
        // The canonical "foobar" progression exercises all three padding cases.
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encodes_all_byte_values() {
        // 0..=255 round-trips through the alphabet without panicking and yields a multiple-of-4 length.
        let bytes: Vec<u8> = (0..=255u8).collect();
        let encoded = encode(&bytes);
        assert_eq!(encoded.len() % 4, 0);
        assert!(
            encoded
                .bytes()
                .all(|b| super::ALPHABET.contains(&b) || b == super::PAD)
        );
    }
}
