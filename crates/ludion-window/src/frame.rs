//! Decoding a streamed frame's payload into raw RGBA for GPU upload.
//!
//! The server delivers each frame as a binary message: a [`FrameHeader`](ludion_protocol::frame)
//! naming the encoding and dimensions, plus the encoded payload. This module decodes every format
//! the server can produce to tightly packed, top-row-first RGBA8 — exactly what
//! `wgpu::Queue::write_texture` wants. The arms get their dimensions from different places:
//! [`FrameFormat::Qoi`]'s own payload header carries its dimensions (checked against the QOI decode
//! itself), while [`FrameFormat::Lz4Rgba`] is a raw LZ4 block and [`FrameFormat::RawRgba`] is bare
//! pixels — neither carries any — so their dimensions come from the LFR1 header and their length is
//! checked against `width * height * 4`. `ludion-client` intentionally hands frames over
//! still-encoded (the screenshot precedent); decoding pixels is this viewer's job.

use ludion_client::ReceivedFrame;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::frame::FrameFormat;

/// A decoded frame ready to upload: its pixel dimensions and packed RGBA8 bytes (`width * height * 4`,
/// row-major, top row first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Decodes a received frame's payload into RGBA8.
///
/// # Errors
/// `L3004` if the payload the header's format names is malformed: a QOI stream the `qoi` decoder
/// rejects, an LZ4 block that fails to decompress or decompresses to a length other than
/// `width * height * 4`, or a raw payload whose length is not exactly that. The code matches the one
/// `ludion-client` already uses for malformed/unknown frame content.
pub(crate) fn decode_frame(frame: ReceivedFrame) -> Result<DecodedFrame, Diagnostic> {
    match frame.header.format {
        FrameFormat::Qoi => decode_qoi(&frame.payload),
        FrameFormat::Lz4Rgba => decode_lz4(&frame.payload, frame.header.width, frame.header.height),
        FrameFormat::RawRgba => decode_raw(frame.payload, frame.header.width, frame.header.height),
    }
}

/// Decodes a QOI payload; dimensions come from QOI's own embedded header.
fn decode_qoi(payload: &[u8]) -> Result<DecodedFrame, Diagnostic> {
    let (header, pixels) = qoi::decode_to_vec(payload).map_err(|e| {
        Diagnostic::new(
            code::PROTOCOL_MALFORMED_FRAME,
            format!("the frame's QOI payload could not be decoded: {e}"),
        )
    })?;

    // The QOI header carries its own dimensions; the decoder guarantees `pixels.len()` matches them
    // for the reported channel count. Normalize to 4-channel RGBA regardless of what was encoded.
    let rgba = match header.channels {
        qoi::Channels::Rgba => pixels,
        qoi::Channels::Rgb => rgb_to_rgba(&pixels),
    };
    Ok(DecodedFrame {
        width: header.width,
        height: header.height,
        rgba,
    })
}

/// Decodes a raw LZ4 block payload; the block carries no size of its own, so dimensions come from
/// the LFR1 header and the decompressed length is checked against them.
fn decode_lz4(payload: &[u8], width: u32, height: u32) -> Result<DecodedFrame, Diagnostic> {
    let expected = width as usize * height as usize * 4;
    let rgba = lz4_flex::block::decompress(payload, expected).map_err(|e| {
        Diagnostic::new(
            code::PROTOCOL_MALFORMED_FRAME,
            format!("the frame's LZ4 payload could not be decompressed: {e}"),
        )
    })?;
    if rgba.len() != expected {
        return Err(Diagnostic::new(
            code::PROTOCOL_MALFORMED_FRAME,
            format!(
                "the frame's LZ4 payload decompressed to {} bytes but the header's {width}x{height} dimensions require {expected}",
                rgba.len()
            ),
        ));
    }
    Ok(DecodedFrame {
        width,
        height,
        rgba,
    })
}

/// "Decodes" a raw RGBA payload: the payload *is* the pixels, so the only work is validating that
/// its length agrees with the header's dimensions before it is handed on as an image. A short or
/// long payload is rejected rather than padded or truncated — the header and the bytes disagreeing
/// is a framing bug, and a silently padded frame would show as garbage rows instead of an error.
fn decode_raw(payload: Vec<u8>, width: u32, height: u32) -> Result<DecodedFrame, Diagnostic> {
    let expected = width as usize * height as usize * 4;
    if payload.len() != expected {
        return Err(Diagnostic::new(
            code::PROTOCOL_MALFORMED_FRAME,
            format!(
                "the frame's RAW_RGBA payload is {} bytes but the header's {width}x{height} dimensions require {expected}",
                payload.len()
            ),
        ));
    }
    Ok(DecodedFrame {
        width,
        height,
        rgba: payload,
    })
}

/// Expands packed RGB8 to RGBA8 with an opaque alpha. Only reached if the server ever encodes a
/// 3-channel frame; the current server always encodes RGBA, so this is a robustness fallback.
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for chunk in rgb.chunks_exact(3) {
        out.extend_from_slice(chunk);
        out.push(255);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::time::FrameIndex;
    use ludion_protocol::envelope::SubId;
    use ludion_protocol::frame::FrameHeader;

    fn received(format: FrameFormat, payload: Vec<u8>, width: u32, height: u32) -> ReceivedFrame {
        ReceivedFrame {
            header: FrameHeader {
                sub: SubId::new(1),
                frame: FrameIndex::new(0),
                format,
                width,
                height,
                gi_stale: false,
            },
            payload,
        }
    }

    #[test]
    fn decodes_qoi_rgba_round_trip() {
        // A 2x2 RGBA image with four distinct pixels, encoded exactly as the server encodes frames.
        let src = vec![
            10, 20, 30, 255, //
            40, 50, 60, 255, //
            70, 80, 90, 255, //
            100, 110, 120, 255,
        ];
        let payload = qoi::encode_to_vec(&src, 2, 2).unwrap();
        let decoded = decode_frame(received(FrameFormat::Qoi, payload, 2, 2)).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba, src);
    }

    #[test]
    fn decodes_lz4_rgba_round_trip() {
        // A 2x2 RGBA image with four distinct pixels, compressed exactly as the server compresses
        // frames: a raw LZ4 block with no prepended size.
        let src = vec![
            10, 20, 30, 255, //
            40, 50, 60, 255, //
            70, 80, 90, 255, //
            100, 110, 120, 255,
        ];
        let payload = lz4_flex::block::compress(&src);
        let decoded = decode_frame(received(FrameFormat::Lz4Rgba, payload, 2, 2)).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba, src);
    }

    #[test]
    fn decodes_raw_rgba_round_trip() {
        // A 2x2 RGBA image sent exactly as the server sends it in the raw lane: the payload is the
        // pixels, byte for byte, with no codec on either end.
        let src = vec![
            10, 20, 30, 255, //
            40, 50, 60, 255, //
            70, 80, 90, 255, //
            100, 110, 120, 255,
        ];
        let decoded = decode_frame(received(FrameFormat::RawRgba, src.clone(), 2, 2)).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.rgba, src);
    }

    #[test]
    fn rejects_raw_rgba_length_mismatch() {
        // One pixel of payload against 2x2 dimensions (16 bytes expected): a raw payload that
        // under-fills what its header promises must be rejected, never zero-padded.
        let short =
            decode_frame(received(FrameFormat::RawRgba, vec![1, 2, 3, 4], 2, 2)).unwrap_err();
        assert_eq!(short.code, code::PROTOCOL_MALFORMED_FRAME);
        // And an over-long one is equally a disagreement, not a frame with slack to ignore.
        let long = decode_frame(received(FrameFormat::RawRgba, vec![0; 20], 2, 2)).unwrap_err();
        assert_eq!(long.code, code::PROTOCOL_MALFORMED_FRAME);
    }

    #[test]
    fn rejects_garbage_lz4_payload() {
        let err = decode_frame(received(FrameFormat::Lz4Rgba, vec![0xff; 16], 1, 1)).unwrap_err();
        assert_eq!(err.code, code::PROTOCOL_MALFORMED_FRAME);
    }

    #[test]
    fn rejects_lz4_length_mismatch() {
        // Compress a payload for one pixel (4 bytes) but claim 2x2 dimensions (16 bytes expected):
        // a block that under-fills what its header promises must be rejected, not zero-padded.
        let payload = lz4_flex::block::compress(&[1, 2, 3, 4]);
        let err = decode_frame(received(FrameFormat::Lz4Rgba, payload, 2, 2)).unwrap_err();
        assert_eq!(err.code, code::PROTOCOL_MALFORMED_FRAME);
    }

    #[test]
    fn rejects_garbage_payload() {
        let err = decode_frame(received(FrameFormat::Qoi, vec![1, 2, 3, 4, 5], 1, 1)).unwrap_err();
        assert_eq!(err.code, code::PROTOCOL_MALFORMED_FRAME);
    }

    #[test]
    fn expands_rgb_to_opaque_rgba() {
        let src = vec![1, 2, 3, 4, 5, 6]; // two RGB pixels
        let payload = qoi::encode_to_vec(&src, 2, 1).unwrap();
        let decoded = decode_frame(received(FrameFormat::Qoi, payload, 2, 1)).unwrap();
        assert_eq!(decoded.rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
