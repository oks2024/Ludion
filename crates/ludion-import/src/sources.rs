//! Resolving glTF buffer and image *sources* to raw bytes, by hand.
//!
//! We deliberately do **not** enable `gltf`'s `import` feature: it drags in the whole `image` crate
//! (a JPEG decoder included), which decision 3 rules out — our `png` dep decodes PNGs and JPEG is an
//! `L1202` lossy-path error, not a second decoder. So we resolve buffer/image sources ourselves:
//! the GLB binary blob, external files relative to the input, and base64 `data:` URIs (the only
//! thing we needed `base64` for). This keeps the dependency tree lean (see the crate's dep-tree
//! audit) while giving us exact, deterministic control over what bytes reach the encoders.

use std::io::Read;

use gltf::buffer;
use gltf::image;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::relative_refusal;

use crate::ExternalFiles;
use crate::diagnostics::{invalid_gltf, uri_refused};

/// Every buffer's resolved bytes, indexed by `buffer.index()`.
pub(crate) struct Buffers(Vec<Vec<u8>>);

impl Buffers {
    /// Resolves every buffer the document declares, in index order, against `external` (where
    /// external `.bin` URIs may be read from) and `blob` (a GLB's embedded binary chunk).
    pub(crate) fn resolve(
        document: &gltf::Document,
        external: ExternalFiles<'_>,
        blob: Option<&[u8]>,
    ) -> Result<Buffers, Diagnostic> {
        let mut data = Vec::with_capacity(document.buffers().len());
        for buffer in document.buffers() {
            let bytes = match buffer.source() {
                buffer::Source::Bin => blob
                    .ok_or_else(|| {
                        invalid_gltf(format!(
                            "buffer {} names the binary chunk, but the file has no GLB blob",
                            buffer.index()
                        ))
                    })?
                    .to_vec(),
                buffer::Source::Uri(uri) => resolve_uri(uri, external, "buffer", buffer.index())?,
            };
            data.push(bytes);
        }
        Ok(Buffers(data))
    }

    /// The whole resolved bytes of buffer `index` (the mesh reader's `get_buffer_data` closure).
    pub(crate) fn get(&self, index: usize) -> Option<&[u8]> {
        self.0.get(index).map(Vec::as_slice)
    }

    /// The bytes of the buffer view `[offset, offset + length)`.
    pub(crate) fn view_slice(&self, view: &buffer::View) -> Result<&[u8], Diagnostic> {
        let buffer = &self.0[view.buffer().index()];
        let start = view.offset();
        let range = start
            .checked_add(view.length())
            .and_then(|end| buffer.get(start..end));
        range.ok_or_else(|| {
            invalid_gltf(format!(
                "buffer view runs past the end of buffer {}",
                view.buffer().index()
            ))
        })
    }
}

/// An image's source bytes plus whether they are a PNG (vs. a JPEG we must reject).
pub(crate) struct ImageBytes {
    /// The raw encoded image bytes (PNG or JPEG).
    pub(crate) bytes: Vec<u8>,
    /// `true` if the bytes are a PNG (by magic), `false` if a JPEG.
    pub(crate) is_png: bool,
}

/// Resolves one image's source (a buffer view, an external file, or a base64 `data:` URI) to its
/// raw encoded bytes, and classifies them as PNG or JPEG by magic number.
///
/// # Errors
/// `L1201` if the source cannot be resolved, or the bytes are neither a PNG nor a JPEG (an image
/// format the importer does not recognize at all).
pub(crate) fn resolve_image(
    image: &image::Image,
    buffers: &Buffers,
    external: ExternalFiles<'_>,
) -> Result<ImageBytes, Diagnostic> {
    let bytes = match image.source() {
        image::Source::View { view, .. } => buffers.view_slice(&view)?.to_vec(),
        image::Source::Uri { uri, .. } => resolve_uri(uri, external, "image", image.index())?,
    };
    let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    let is_jpeg = bytes.starts_with(&[0xFF, 0xD8]);
    if !is_png && !is_jpeg {
        return Err(invalid_gltf(format!(
            "image {} is neither a PNG nor a JPEG (unrecognized magic bytes)",
            image.index()
        )));
    }
    Ok(ImageBytes { bytes, is_png })
}

/// The most an external file may hold, the same bound as `ludion-gen`'s download cap.
const MAX_EXTERNAL_BYTES: u64 = 512 << 20;

/// Resolves a glTF URI (`kind`/`index` name it for diagnostics) to bytes: a base64 `data:` URI is
/// decoded in place; anything else is a relative path read at or below the directory `external`
/// names, and must be a regular file within [`MAX_EXTERNAL_BYTES`].
fn resolve_uri(
    uri: &str,
    external: ExternalFiles<'_>,
    kind: &str,
    index: usize,
) -> Result<Vec<u8>, Diagnostic> {
    if let Some(rest) = uri.strip_prefix("data:") {
        return decode_data_uri(rest, kind, index);
    }
    let refused = |reason: &str| uri_refused(kind, index, uri, reason);
    let ExternalFiles::Beside(base_dir) = external else {
        return Err(refused(
            "is external; only embedded sources (the `.glb` binary chunk or `data:` URIs) are \
             accepted here",
        ));
    };
    let relative = percent_decode(uri);
    // Both forms: the raw text is what an exporter wrote, the decoded text is what the file system
    // is asked for (`%2e%2e/`, `%3a`).
    if let Some(refusal) = relative_refusal(uri).or_else(|| relative_refusal(&relative)) {
        return Err(refused(refusal.describe()));
    }
    let path = base_dir.join(&relative);
    let unreadable = |error: std::io::Error| {
        invalid_gltf(format!(
            "{kind} {index} references `{relative}`, which could not be read (at `{}`): {error}",
            path.display()
        ))
    };
    // Metadata first: opening a directory fails on Windows, and opening a pipe can block.
    if !std::fs::metadata(&path).map_err(unreadable)?.is_file() {
        return Err(refused("is not a regular file"));
    }
    let file = std::fs::File::open(&path).map_err(unreadable)?;
    let mut bytes = Vec::new();
    file.take(MAX_EXTERNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_EXTERNAL_BYTES {
        return Err(refused("exceeds the 512 MiB external-file cap"));
    }
    Ok(bytes)
}

/// Decodes the part of a `data:` URI after `data:` — `[<mediatype>][;base64],<payload>`. Only
/// base64 payloads are supported (what every glTF exporter emits for embedded buffers/images).
fn decode_data_uri(rest: &str, kind: &str, index: usize) -> Result<Vec<u8>, Diagnostic> {
    let (header, payload) = rest.split_once(',').ok_or_else(|| {
        invalid_gltf(format!(
            "{kind} {index} has a malformed `data:` URI (no comma)"
        ))
    })?;
    if !header.contains(";base64") {
        return Err(invalid_gltf(format!(
            "{kind} {index} uses a non-base64 `data:` URI, which the importer does not decode"
        )));
    }
    base64::decode(payload).map_err(|error| {
        invalid_gltf(format!(
            "{kind} {index}'s base64 `data:` payload could not be decoded: {error}"
        ))
    })
}

/// Minimal percent-decoding for external URIs (`%20` → space, etc.). glTF URIs are percent-encoded;
/// this covers the common cases without pulling a URL crate. An invalid escape passes through as a
/// literal `%`.
fn percent_decode(input: &str) -> String {
    let raw = input.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && i + 2 < raw.len()
            && let (Some(hi), Some(lo)) = (
                (raw[i + 1] as char).to_digit(16),
                (raw[i + 2] as char).to_digit(16),
            )
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
