//! [`WireHash`] — a stable 64-bit content digest rendered as 16 hex digits.
//!
//! This is the same textual form as `ludion-sim`'s `WorldHash`: XXH3-64 over some canonical byte
//! stream, displayed and serialized as exactly 16 lowercase hex digits so it compares as a string
//! everywhere and never loses precision in a JSON consumer that reads numbers as `f64`.
//!
//! Two protocol values use it:
//!
//! - the [`schema_hash`](crate::schema::schema_hash) announced in the handshake — XXH3-64 of the
//!   canonical protocol-schema document; and
//! - the world hash carried in `sim.step` / `sim.hash` responses (the dispatcher renders the sim's
//!   `WorldHash` into this form; the protocol layer never depends on `ludion-sim`).

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

/// A stable 64-bit digest, displayed and serialized as exactly 16 lowercase hex digits.
///
/// # Example
/// ```
/// use ludion_protocol::WireHash;
///
/// let a = WireHash::of_bytes(b"ludion");
/// let b = WireHash::of_bytes(b"ludion");
/// assert_eq!(a, b); // same bytes, same hash — always
/// assert_eq!(a.to_string().len(), 16);
///
/// // Round-trips through its 16-hex string form.
/// assert_eq!(WireHash::parse(&a.to_string()), Some(a));
/// assert!(WireHash::parse("0xdeadbeef").is_none());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireHash(u64);

impl WireHash {
    /// Computes the XXH3-64 digest of `bytes` (seed 0).
    pub fn of_bytes(bytes: &[u8]) -> WireHash {
        WireHash(xxh3_64(bytes))
    }

    /// Wraps a raw 64-bit digest (e.g. one already computed elsewhere, such as a `WorldHash`).
    pub const fn from_u64(digest: u64) -> WireHash {
        WireHash(digest)
    }

    /// The raw 64-bit digest.
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Parses the canonical 16 lowercase hex digits (as produced by [`Display`](fmt::Display), and
    /// as the published schema's pattern requires), returning `None` for anything else.
    pub fn parse(text: &str) -> Option<WireHash> {
        if text.len() != 16 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        u64::from_str_radix(text, 16).ok().map(WireHash)
    }
}

impl fmt::Display for WireHash {
    /// Exactly 16 lowercase hex digits, zero-padded.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for WireHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WireHash({self})")
    }
}

impl Serialize for WireHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WireHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        WireHash::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid wire hash '{s}', expected 16 lowercase hex digits"
            ))
        })
    }
}

// `WireHash` has a custom string serde form, so its schema is hand-written to match (a 16-hex-digit
// string) rather than derived.
impl schemars::JsonSchema for WireHash {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("WireHash")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{16}$",
            "description": "A 64-bit XXH3 digest as exactly 16 lowercase hex digits."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_16_lowercase_hex() {
        assert_eq!(
            WireHash::from_u64(0x0123_4567_89ab_cdef).to_string(),
            "0123456789abcdef"
        );
        assert_eq!(WireHash::from_u64(42).to_string(), "000000000000002a");
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(WireHash::parse("").is_none());
        assert!(WireHash::parse("0123456789abcde").is_none()); // 15 digits
        assert!(WireHash::parse("0123456789abcdeg").is_none()); // non-hex
    }

    #[test]
    fn parse_refuses_uppercase_and_display_round_trips() {
        assert!(WireHash::parse("0123456789ABCDEF").is_none());
        let h = WireHash::parse("0123456789abcdef").unwrap();
        assert_eq!(h.to_string(), "0123456789abcdef");
    }

    #[test]
    fn serde_uses_the_hex_string_form() {
        let h = WireHash::from_u64(42);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"000000000000002a\"");
        assert_eq!(serde_json::from_str::<WireHash>(&json).unwrap(), h);
        assert!(serde_json::from_str::<WireHash>("\"xyz\"").is_err());
    }

    #[test]
    fn of_bytes_is_stable_and_sensitive() {
        assert_eq!(WireHash::of_bytes(b"abc"), WireHash::of_bytes(b"abc"));
        assert_ne!(WireHash::of_bytes(b"abc"), WireHash::of_bytes(b"abd"));
    }
}
