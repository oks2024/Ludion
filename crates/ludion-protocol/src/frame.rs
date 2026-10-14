//! The binary frame-streaming message: a fixed header plus an encoded image payload.
//!
//! Rendered frames are delivered over the `frames` subscription as **binary** WebSocket messages, not
//! JSON events (engine-architecture.md §6): a fixed [`FrameHeader`] followed by the encoded pixels.
//! Keeping frames off the JSON path avoids base64-inflating megabytes of pixels per frame; the
//! `render.screenshot` *command* is the JSON path (it returns a base64 PNG) and exists for the
//! one-shot, exact-frame case.
//!
//! # Byte layout (little-endian, header is exactly [`FRAME_HEADER_LEN`] bytes)
//!
//! | offset | size | field                                    |
//! |--------|------|------------------------------------------|
//! | 0      | 4    | magic [`FRAME_MAGIC`] (`b"LFR1"`)        |
//! | 4      | 1    | format ([`FrameFormat`] as `u8`)         |
//! | 5      | 1    | flags (bit 0 = [`FRAME_FLAG_GI_STALE`])   |
//! | 6      | 2    | reserved, zero                           |
//! | 8      | 8    | `sub_id` (`u64`)                         |
//! | 16     | 8    | `frame_index` (`u64`)                    |
//! | 24     | 4    | `width` (`u32`)                          |
//! | 28     | 4    | `height` (`u32`)                         |
//! | 32     | 4    | `payload_len` (`u32`)                    |
//! | 36     | …    | payload (`payload_len` bytes)            |

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::envelope::SubId;

/// The 4-byte magic that opens every frame message.
pub const FRAME_MAGIC: [u8; 4] = *b"LFR1";

/// The fixed size of a [`FrameHeader`] on the wire, in bytes.
pub const FRAME_HEADER_LEN: usize = 36;

/// Flags-byte bit 0: the frame's global illumination was *held* rather than rebuilt for the world it
/// depicts (see [`FrameHeader::gi_stale`]).
///
/// Unset bits stay reserved and are ignored on decode, so a header written before a flag existed
/// reads back as that flag being off — which is exactly what an encoder that never holds anything
/// means. That tolerance is what lets a flag be added without a frame-format rev.
pub const FRAME_FLAG_GI_STALE: u8 = 1 << 0;

/// The encoding of a streamed frame's pixels (engine-architecture.md §6).
///
/// The JSON form is the uppercase name (`"QOI"` / `"LZ4_RGBA"` / `"RAW_RGBA"`); the binary form is
/// the `u8` in the header. Both are kept in sync by [`FrameFormat::from_u8`] / [`FrameFormat::as_u8`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FrameFormat {
    /// QOI-encoded RGBA (fast to encode, decodes cheaply in a WASM viewer). The default: a
    /// subscriber that names no format receives this one.
    #[default]
    #[serde(rename = "QOI")]
    Qoi,
    /// LZ4-compressed raw RGBA8.
    #[serde(rename = "LZ4_RGBA")]
    Lz4Rgba,
    /// Uncompressed, tightly-packed RGBA8: exactly `width * height * 4` payload bytes, row-major,
    /// top row first. The cheap lane for loopback transport, where payload size is free and codec
    /// time is not — the sender's "encode" is one copy and the receiver's "decode" is none at all.
    #[serde(rename = "RAW_RGBA")]
    RawRgba,
}

impl FrameFormat {
    /// The format's on-the-wire byte tag.
    pub const fn as_u8(self) -> u8 {
        match self {
            FrameFormat::Qoi => 0,
            FrameFormat::Lz4Rgba => 1,
            FrameFormat::RawRgba => 2,
        }
    }

    /// Parses a byte tag, returning `None` for an unknown value.
    pub const fn from_u8(byte: u8) -> Option<FrameFormat> {
        match byte {
            0 => Some(FrameFormat::Qoi),
            1 => Some(FrameFormat::Lz4Rgba),
            2 => Some(FrameFormat::RawRgba),
            _ => None,
        }
    }
}

/// The fixed header of a binary frame message.
///
/// # Example
/// ```
/// use ludion_protocol::frame::{FrameFormat, FrameHeader};
/// use ludion_protocol::envelope::SubId;
/// use ludion_core::time::FrameIndex;
///
/// let header = FrameHeader {
///     sub: SubId::new(7),
///     frame: FrameIndex::new(1203),
///     format: FrameFormat::Qoi,
///     width: 1920,
///     height: 1080,
///     gi_stale: false,
/// };
/// let payload = b"...encoded pixels...";
/// let message = header.encode(payload);
///
/// let (decoded, body) = FrameHeader::decode(&message).unwrap();
/// assert_eq!(decoded, header);
/// assert_eq!(body, payload);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrameHeader {
    /// The subscription this frame belongs to.
    pub sub: SubId,
    /// The sim frame index this image was rendered from.
    #[schemars(with = "u64")]
    pub frame: FrameIndex,
    /// How the payload pixels are encoded.
    pub format: FrameFormat,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Whether this frame's global illumination was *held* — served from a voxelization or probe
    /// bake built under inputs the frame no longer had, because the session's rebake lever is at
    /// `hold` (`render.gi_rebake`). Travels with the image so a viewer, or an agent decoding a
    /// frame, learns it is looking at stale light without asking a second question.
    #[serde(default)]
    pub gi_stale: bool,
}

impl FrameHeader {
    /// Serializes the header followed by `payload` into one binary message.
    ///
    /// A thin wrapper over [`encode_into`](Self::encode_into) for a caller with no buffer of its own
    /// to reuse.
    pub fn encode(&self, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out, payload);
        out
    }

    /// Serializes the header followed by `payload` into `out`, clearing it first.
    ///
    /// Lets a caller building many per-subscriber messages from the same payload (the frame router's
    /// per-format fan-out, one header build per subscriber) keep one scratch `Vec` and reuse its
    /// allocation across calls, rather than paying a fresh allocation for each one.
    pub fn encode_into(&self, out: &mut Vec<u8>, payload: &[u8]) {
        out.clear();
        out.reserve(FRAME_HEADER_LEN + payload.len());
        out.extend_from_slice(&FRAME_MAGIC);
        out.push(self.format.as_u8());
        out.push(if self.gi_stale {
            FRAME_FLAG_GI_STALE
        } else {
            0
        });
        out.extend_from_slice(&[0, 0]); // reserved
        out.extend_from_slice(&self.sub.as_u64().to_le_bytes());
        out.extend_from_slice(&self.frame.as_u64().to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }

    /// Parses a binary frame message into its header and payload slice.
    ///
    /// # Errors
    /// `L3004` if the bytes are shorter than a header, do not start with [`FRAME_MAGIC`], name an
    /// unknown [`FrameFormat`], or carry a `payload_len` that disagrees with the remaining bytes.
    pub fn decode(bytes: &[u8]) -> Result<(FrameHeader, &[u8]), Diagnostic> {
        if bytes.len() < FRAME_HEADER_LEN {
            return Err(malformed(format!(
                "frame message is {} bytes, shorter than the {FRAME_HEADER_LEN}-byte header",
                bytes.len()
            )));
        }
        if bytes[0..4] != FRAME_MAGIC {
            return Err(malformed(
                "frame message does not start with the LFR1 magic",
            ));
        }
        let format = FrameFormat::from_u8(bytes[4])
            .ok_or_else(|| malformed(format!("unknown frame format byte {}", bytes[4])))?;
        // Undefined flag bits are ignored, never rejected: an encoder from before a flag existed
        // wrote zero there, and one from after may set a bit this build has no name for.
        let gi_stale = bytes[5] & FRAME_FLAG_GI_STALE != 0;
        let sub = SubId::new(u64::from_le_bytes(bytes[8..16].try_into().unwrap()));
        let frame = FrameIndex::new(u64::from_le_bytes(bytes[16..24].try_into().unwrap()));
        let width = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let height = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let payload_len = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
        let payload = &bytes[FRAME_HEADER_LEN..];
        if payload.len() != payload_len {
            return Err(malformed(format!(
                "frame message declares a {payload_len}-byte payload but carries {}",
                payload.len()
            )));
        }
        Ok((
            FrameHeader {
                sub,
                frame,
                format,
                width,
                height,
                gi_stale,
            },
            payload,
        ))
    }
}

/// Builds an `L3004` malformed-frame diagnostic.
fn malformed(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_MALFORMED_FRAME, message).with_help(
        "binary frame messages are a 36-byte little-endian header (magic `LFR1`, format, flags, \
         sub, frame, width, height, payload_len) followed by exactly payload_len encoded bytes",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FrameHeader {
        FrameHeader {
            sub: SubId::new(7),
            frame: FrameIndex::new(1203),
            format: FrameFormat::Lz4Rgba,
            width: 320,
            height: 200,
            gi_stale: false,
        }
    }

    /// [`FrameHeader::encode`] is documented as a thin wrapper over
    /// [`FrameHeader::encode_into`] — this pins that claim: both paths must produce byte-identical
    /// messages, including when `out` starts non-empty (the reused-buffer case `encode_into` exists
    /// for) and carries stale bytes past the new message's length.
    #[test]
    fn encode_and_encode_into_produce_identical_bytes() {
        let header = sample();
        let payload = vec![1u8, 2, 3, 4, 5];

        let via_encode = header.encode(&payload);

        let mut via_encode_into = Vec::new();
        header.encode_into(&mut via_encode_into, &payload);
        assert_eq!(via_encode, via_encode_into);

        // A `Vec` reused from a previous, longer message must not leak trailing bytes into this one.
        let mut reused = vec![0xAAu8; FRAME_HEADER_LEN + payload.len() + 64];
        header.encode_into(&mut reused, &payload);
        assert_eq!(via_encode, reused);
    }

    #[test]
    fn encode_decode_round_trips() {
        let header = sample();
        let payload = vec![1u8, 2, 3, 4, 5];
        let message = header.encode(&payload);
        assert_eq!(message.len(), FRAME_HEADER_LEN + payload.len());
        let (decoded, body) = FrameHeader::decode(&message).unwrap();
        assert_eq!(decoded, header);
        assert_eq!(body, &payload[..]);
    }

    #[test]
    fn format_tags_round_trip() {
        for f in [FrameFormat::Qoi, FrameFormat::Lz4Rgba, FrameFormat::RawRgba] {
            assert_eq!(FrameFormat::from_u8(f.as_u8()), Some(f));
        }
        assert_eq!(FrameFormat::RawRgba.as_u8(), 2);
        assert_eq!(FrameFormat::from_u8(3), None);
        assert_eq!(FrameFormat::from_u8(9), None);
        // JSON form is the uppercase name.
        assert_eq!(
            serde_json::to_value(FrameFormat::Lz4Rgba).unwrap(),
            serde_json::json!("LZ4_RGBA")
        );
        assert_eq!(
            serde_json::to_value(FrameFormat::RawRgba).unwrap(),
            serde_json::json!("RAW_RGBA")
        );
    }

    /// Every variant survives a full header round trip, so the byte tag written by [`encode`] is the
    /// one [`decode`] reads back — the two-line change that is easiest to get wrong.
    #[test]
    fn every_format_round_trips_through_a_header() {
        for format in [FrameFormat::Qoi, FrameFormat::Lz4Rgba, FrameFormat::RawRgba] {
            let header = FrameHeader { format, ..sample() };
            let message = header.encode(&[7u8; 12]);
            let (decoded, body) = FrameHeader::decode(&message).unwrap();
            assert_eq!(decoded, header);
            assert_eq!(body, &[7u8; 12]);
        }
    }

    /// The flags byte round-trips, lands where the layout says it does, and leaves the two bytes
    /// after it alone — the whole reason a flag could be added without a frame-format rev.
    #[test]
    fn the_gi_stale_flag_round_trips_in_byte_five() {
        let held = FrameHeader {
            gi_stale: true,
            ..sample()
        };
        let message = held.encode(&[9u8; 8]);
        assert_eq!(message.len(), FRAME_HEADER_LEN + 8);
        assert_eq!(message[5], FRAME_FLAG_GI_STALE);
        assert_eq!(
            &message[6..8],
            &[0, 0],
            "the rest of the word stays reserved"
        );
        let (decoded, body) = FrameHeader::decode(&message).unwrap();
        assert!(decoded.gi_stale);
        assert_eq!(decoded, held);
        assert_eq!(body, &[9u8; 8]);

        // And the clean case writes a zero byte, exactly as every encoder did before the flag.
        let fresh = sample().encode(&[9u8; 8]);
        assert_eq!(fresh[5], 0);
        assert!(!FrameHeader::decode(&fresh).unwrap().0.gi_stale);
    }

    /// A message from an encoder that predates the flags byte — all three bytes zero — still decodes,
    /// and reads as not stale. This is the backward-compatibility claim the layout change rests on.
    #[test]
    fn a_header_with_the_old_reserved_zeroes_still_decodes() {
        let mut message = FrameHeader {
            gi_stale: true,
            ..sample()
        }
        .encode(&[3u8; 4]);
        message[5] = 0;
        message[6] = 0;
        message[7] = 0;
        let (decoded, body) = FrameHeader::decode(&message).unwrap();
        assert_eq!(decoded, sample());
        assert!(!decoded.gi_stale);
        assert_eq!(body, &[3u8; 4]);
    }

    /// Bits nobody has named yet are ignored rather than rejected, so a *newer* encoder's frame is
    /// still readable by this build (only the flags it knows are reported).
    #[test]
    fn unknown_flag_bits_are_ignored_not_rejected() {
        let mut message = sample().encode(&[1u8; 4]);
        message[5] = 0b1111_1110; // every bit except gi_stale
        let (decoded, _) = FrameHeader::decode(&message).unwrap();
        assert!(!decoded.gi_stale);

        message[5] = 0b1111_1111;
        assert!(FrameHeader::decode(&message).unwrap().0.gi_stale);
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert_eq!(
            FrameHeader::decode(b"short").unwrap_err().code,
            code::PROTOCOL_MALFORMED_FRAME
        );

        let mut bad_magic = sample().encode(&[0u8; 4]);
        bad_magic[0] = b'X';
        assert_eq!(
            FrameHeader::decode(&bad_magic).unwrap_err().code,
            code::PROTOCOL_MALFORMED_FRAME
        );

        let mut bad_format = sample().encode(&[0u8; 4]);
        bad_format[4] = 200;
        assert_eq!(
            FrameHeader::decode(&bad_format).unwrap_err().code,
            code::PROTOCOL_MALFORMED_FRAME
        );

        // Truncated payload (drop the last byte) disagrees with payload_len.
        let mut short_payload = sample().encode(&[0u8; 4]);
        short_payload.pop();
        assert_eq!(
            FrameHeader::decode(&short_payload).unwrap_err().code,
            code::PROTOCOL_MALFORMED_FRAME,
        );
    }
}
