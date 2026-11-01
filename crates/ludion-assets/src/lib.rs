#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-assets` — CPU-only asset data: file bytes, content hashes, and decoded images.
//!
//! This crate is **GPU-free by boundary rule** (`engine-architecture.md` §2): it deals in bytes and
//! plain RGBA pixels, never a `wgpu` type. The renderer owns GPU upload and keys its texture cache on
//! the [content hash](content_hash) this crate computes, so a `wgpu` type here would drag wgpu into
//! the sim-side dependency tree and wreck compile times for content work.
//!
//! Asset paths are **relative to a project root** the caller supplies — for the CLI, the directory
//! that contains the scene file. Every load takes `(root, relative_path)` and joins them; the crate
//! never invents a search path of its own.
//!
//! # What this crate does *not* do
//!
//! No reload policy, no atlas packing. This is the minimal loader the renderer needs: read a
//! file to bytes + hash, and decode a PNG to RGBA8. Behind the opt-in `watch` cargo feature it also
//! provides the debounced filesystem watcher (`watch`, `WatchHandle`) that `ludion dev` and
//! `ludion-server`'s asset reload are built on — deciding *what* to reload is theirs, not this
//! crate's, and only those two hosts enable the feature, so a build for a device without a desktop
//! file watcher never carries `notify`. Atlas packing is deferred (see `CLAUDE.md`).
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! // Load a texture referenced by a scene, resolving it against the scene's directory.
//! let image = ludion_assets::load_png(ludion_assets::AssetRoot::content(Path::new("examples/gallery")), "assets/brick.png")?;
//! assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
//! # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
//! ```

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use xxhash_rust::xxh3::xxh3_64;

pub mod cooked;
pub mod import_sync;
pub mod mips;
pub mod shelf;

#[cfg(feature = "watch")]
mod watch;
#[cfg(feature = "watch")]
pub use watch::{WatchHandle, watch};

/// The raw bytes of an asset file plus their content hash.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AssetBytes {
    /// The file's bytes, exactly as read.
    pub bytes: Vec<u8>,
    /// XXH3-64 of [`bytes`](AssetBytes::bytes) — a stable, portable content identity.
    pub hash: u64,
}

/// A decoded image in tightly packed, top-to-bottom RGBA8 (no row padding).
///
/// [`hash`](Image::hash) is the content hash of the *source file bytes* (not the decoded pixels), so
/// two references to the same file on disk share an identity the renderer can dedup on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed RGBA8 bytes, `width * height * 4` long, row-major, top row first.
    pub rgba: Vec<u8>,
    /// XXH3-64 of the source PNG file's bytes.
    pub hash: u64,
}

impl Image {
    /// Converts straight alpha to **premultiplied** alpha in place, in the colour space the
    /// renderer's texture format implies.
    ///
    /// Every texture the sprite pass samples is premultiplied: an atlas is baked that way by
    /// `generate-atlas`, and a loose PNG is converted here, so an author never meets the convention.
    /// The blend downstream is `src + dst * (1 - src.a)`, which needs the colour already weighted by
    /// its own coverage.
    ///
    /// A screen-space `UiRect`'s texture rides the same convention — sprites and UI rects resolve
    /// through the one texture cache, so one path cannot hold a different one — and its pass blends
    /// premultiplied to match. Text does not: a glyph atlas is coverage in a single channel, built
    /// by the text rasterizer rather than read through here.
    ///
    /// Pixels upload as `Rgba8UnormSrgb`, so the sampler decodes each byte with the sRGB transfer
    /// function before the blend sees it. The weighting therefore happens in **linear** light —
    /// decode, multiply by alpha, re-encode — not on the stored bytes; multiplying the encoded byte
    /// directly would darken a half-covered texel by roughly its own gamma. Alpha itself is a
    /// coverage fraction, never encoded, and is left alone.
    ///
    /// An opaque texel (`a == 255`) is returned unchanged byte-for-byte, so a fully opaque PNG is
    /// identical after this call. The transfer functions go through `ludion_core::math`, so the same
    /// PNG converts to the same bytes on x86 and arm64.
    ///
    /// # Example
    /// ```
    /// let mut image = ludion_assets::Image {
    ///     width: 1,
    ///     height: 1,
    ///     rgba: vec![255, 255, 255, 0],
    ///     hash: 0,
    /// };
    /// image.premultiply();
    /// assert_eq!(image.rgba, vec![0, 0, 0, 0]); // zero coverage contributes nothing
    /// ```
    pub fn premultiply(&mut self) {
        for texel in self.rgba.chunks_exact_mut(4) {
            let alpha = texel[3];
            if alpha == u8::MAX {
                continue;
            }
            let coverage = f32::from(alpha) / 255.0;
            for channel in &mut texel[..3] {
                *channel = encode_srgb(decode_srgb(*channel) * coverage);
            }
        }
    }

    /// [`premultiply`](Image::premultiply), by value — for a decode-then-convert chain.
    #[must_use]
    pub fn premultiplied(mut self) -> Image {
        self.premultiply();
        self
    }
}

/// One sRGB-encoded byte as a linear value in `[0, 1]` (the IEC 61966-2-1 transfer function, the one
/// `Rgba8UnormSrgb` sampling applies).
///
/// This and [`encode_srgb`] are the engine's single definition of the transfer function, so every
/// tool that blends committed texels — [`Image::premultiply`] here, a mesh-edit paint op in
/// `ludion-mesh-edit` — agrees to the last bit on both pinned platforms.
///
/// # Example
/// ```
/// assert_eq!(ludion_assets::decode_srgb(0), 0.0);
/// assert_eq!(ludion_assets::decode_srgb(255), 1.0);
/// ```
pub fn decode_srgb(byte: u8) -> f32 {
    let value = f32::from(byte) / 255.0;
    if value <= 0.040_449_936 {
        value / 12.92
    } else {
        ludion_core::math::powf((value + 0.055) / 1.055, 2.4)
    }
}

/// The linear-light RGBA mean of tightly packed `Rgba8` pixels: R, G, B decoded through
/// [`decode_srgb`], alpha as `byte / 255`. This is the GI voxelizer's albedo input for an asset mesh
/// (the base-colour map's mean), computed at decode and at cook so both texture axes agree.
///
/// One pass fills a 256-bin histogram per channel, then each channel sums `count × value` in bin
/// order, so the result does not depend on pixel order or thread count. An empty slice is white.
///
/// # Example
/// ```
/// let mean = ludion_assets::srgb_mean(&[0, 0, 0, 255, 255, 255, 255, 255]);
/// assert_eq!(mean, [0.5, 0.5, 0.5, 1.0]);
/// ```
pub fn srgb_mean(rgba: &[u8]) -> [f32; 4] {
    let mut bins = [[0u64; 256]; 4];
    let mut texels = 0u64;
    for texel in rgba.chunks_exact(4) {
        for (channel, byte) in texel.iter().enumerate() {
            bins[channel][usize::from(*byte)] += 1;
        }
        texels += 1;
    }
    if texels == 0 {
        return [1.0; 4];
    }
    let mut mean = [0.0f32; 4];
    for (channel, histogram) in bins.iter().enumerate() {
        let mut sum = 0.0f64;
        for (bin, count) in histogram.iter().enumerate() {
            let value = if channel == 3 {
                bin as f32 / 255.0
            } else {
                decode_srgb(bin as u8)
            };
            sum += *count as f64 * f64::from(value);
        }
        mean[channel] = (sum / texels as f64) as f32;
    }
    mean
}

/// A linear value in `[0, 1]` back to one sRGB-encoded byte — the exact inverse of [`decode_srgb`],
/// rounded to nearest so an unchanged linear value round-trips to its own byte.
///
/// # Example
/// ```
/// let byte = 188;
/// assert_eq!(ludion_assets::encode_srgb(ludion_assets::decode_srgb(byte)), byte);
/// ```
pub fn encode_srgb(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * ludion_core::math::powf(linear, 1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// The XXH3-64 content hash of a byte slice.
///
/// A pure function of the bytes — identical input hashes identically on every platform and every
/// run, which is what lets the renderer key its GPU texture cache on it.
///
/// # Example
/// ```
/// assert_eq!(ludion_assets::content_hash(b"abc"), ludion_assets::content_hash(b"abc"));
/// assert_ne!(ludion_assets::content_hash(b"abc"), ludion_assets::content_hash(b"abd"));
/// ```
pub fn content_hash(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

/// The largest asset file a load reads, 512 MiB; a bigger file or a device that never ends (a
/// payload naming `/dev/zero`) is refused instead of filling memory.
pub const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// The directory an asset path is joined to, and whether the join is confined to it.
///
/// [`content`](AssetRoot::content) is the engine's content root: every path that came from content
/// or a payload must stay below it. [`local_document_unconfined`](AssetRoot::local_document_unconfined)
/// is for a file the local author named, whose document-relative paths may climb above its own
/// directory. Both refuse an absolute or prefixed path and keep the bounded read.
#[derive(Clone, Copy, Debug)]
pub struct AssetRoot<'a> {
    dir: &'a Path,
    confined: bool,
}

impl<'a> AssetRoot<'a> {
    /// The engine's content root: a path that climbs out of `dir` is refused.
    pub fn content(dir: &'a Path) -> Self {
        AssetRoot {
            dir,
            confined: true,
        }
    }

    /// A file the local author named on the command line: its document-relative paths may climb
    /// above `dir` with `..`. For CLI harnesses only, never the engine, the server or the wire.
    pub fn local_document_unconfined(dir: &'a Path) -> Self {
        AssetRoot {
            dir,
            confined: false,
        }
    }

    /// The directory paths are joined to.
    pub fn dir(&self) -> &'a Path {
        self.dir
    }
}

/// Resolves `relative` against `root`, refusing a path that is not a plain relative path once its
/// `..` segments are folded ([`ludion_core::paths::content_refusal`]); an unconfined root lets a
/// climbing `..` through and refuses everything else.
///
/// # Errors
/// `L1321` for an absolute, prefixed, reserved-name or (confined) escaping path.
///
/// # Example
/// ```
/// use std::path::Path;
/// use ludion_assets::AssetRoot;
/// let dir = Path::new("project");
/// let root = AssetRoot::content(dir);
/// assert!(ludion_assets::resolve(root, "meshes/../a.png").is_ok());
/// assert!(ludion_assets::resolve(root, "../a.png").is_err());
/// assert!(ludion_assets::resolve(AssetRoot::local_document_unconfined(dir), "../a.png").is_ok());
/// ```
pub fn resolve(root: AssetRoot<'_>, relative: &str) -> Result<PathBuf, Diagnostic> {
    let refusal = if root.confined {
        ludion_core::paths::content_refusal(relative)
    } else {
        ludion_core::paths::sibling_refusal(relative)
    };
    match refusal {
        None => Ok(root.dir.join(ludion_core::paths::normalize(relative))),
        Some(refusal) => Err(Diagnostic::new(
            code::CONTENT_PATH_OUTSIDE_ROOT,
            format!("asset path `{relative}` {}", refusal.describe()),
        )
        .with_help(
            "name the file relative to the content root (the directory of the scene the engine \
             was started with); a document's own paths may climb with `..` only while they stay \
             inside it",
        )),
    }
}

/// Reads `reader` to its end, or `Ok(None)` when it holds more than `cap` bytes. The read is cut at
/// `cap + 1` so a source whose length metadata lies, or that never ends, cannot fill memory.
fn read_capped(reader: impl std::io::Read, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= cap).then_some(bytes))
}

/// Opens `path` and reads it whole with the [`MAX_ASSET_BYTES`] bound; `label` names it in errors.
fn read_bounded(path: &Path, label: &str) -> Result<AssetBytes, Diagnostic> {
    let io_error = |why: String| {
        Diagnostic::new(
            code::ASSET_FILE_IO,
            format!(
                "could not read asset `{label}` (at `{}`): {why}",
                path.display()
            ),
        )
        .with_help("check the path is correct and names a file that exists relative to the project")
    };
    let file = std::fs::File::open(path).map_err(|error| io_error(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error(error.to_string()))?;
    if !metadata.is_file() {
        return Err(io_error("not a regular file".to_string()));
    }
    match read_capped(file, MAX_ASSET_BYTES) {
        Ok(Some(bytes)) => {
            let hash = content_hash(&bytes);
            Ok(AssetBytes { bytes, hash })
        }
        Ok(None) => Err(io_error(format!(
            "larger than the {} MiB asset limit",
            MAX_ASSET_BYTES / (1024 * 1024)
        ))),
        Err(error) => Err(io_error(error.to_string())),
    }
}

/// Reads the asset at `root`/`relative` to bytes and computes its content hash. `relative` is
/// resolved by [`resolve`] and the read is bounded ([`MAX_ASSET_BYTES`]).
///
/// # Errors
/// - `L1321` if `relative` is absolute, prefixed or (on a confined root) climbs out of it.
/// - `L1101` if the file cannot be read (missing, not a regular file, over the size limit, or
///   permission-denied).
pub fn load_bytes(root: AssetRoot<'_>, relative: &str) -> Result<AssetBytes, Diagnostic> {
    read_bounded(&resolve(root, relative)?, relative)
}

/// Reads and decodes the PNG at `root`/`relative` to a top-to-bottom RGBA8 [`Image`]; the path is
/// resolved by [`resolve`].
///
/// Accepts 8-bit grayscale, grayscale+alpha, RGB, RGBA, and palette PNGs; 16-bit channels are
/// narrowed to 8-bit. The decoded pixels are always RGBA8, so the renderer has one texture format to
/// upload.
///
/// # Errors
/// - `L1101` if the file cannot be read.
/// - `L1102` if the bytes are not a decodable PNG, or declare a side over 8192.
pub fn load_png(root: AssetRoot<'_>, relative: &str) -> Result<Image, Diagnostic> {
    let asset = load_bytes(root, relative)?;
    decode_png(&asset.bytes, relative).map(|(width, height, rgba)| Image {
        width,
        height,
        rgba,
        hash: asset.hash,
    })
}

/// The longest side a PNG may declare in its header: 8192, the largest texture the engine's device
/// takes (`ludion_render_graph::MAX_TEXTURE_DIMENSION_2D`, repeated here because this crate must not
/// depend on the render graph). It bounds the output buffer at 256 MiB as RGBA8. Every packer that
/// writes a PNG checks its extent against this.
pub const MAX_PNG_SIDE: u32 = 8192;

/// Decodes PNG `bytes` (named `label` for diagnostics) into `(width, height, rgba8)`.
///
/// Split out from [`load_png`] so it can be unit-tested on in-memory bytes without touching disk.
///
/// # Errors
/// `L1102` if the bytes are not a decodable PNG, or the header declares a side over 8192 (checked
/// before the pixel buffer is allocated).
pub fn decode_png(bytes: &[u8], label: &str) -> Result<(u32, u32, Vec<u8>), Diagnostic> {
    // EXPAND turns palette into RGB and sub-8-bit grayscale up to 8-bit; STRIP_16 narrows 16-bit
    // channels to 8-bit. After these, the color type is one of Grayscale / GrayscaleAlpha / Rgb /
    // Rgba at 8 bits, which the match below folds into RGBA8.
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|error| png_error(label, format!("not a readable PNG: {error}")))?;
    let declared = reader.info();
    if declared.width > MAX_PNG_SIDE || declared.height > MAX_PNG_SIDE {
        return Err(png_error(
            label,
            format!(
                "{}x{} pixels: a side is over {MAX_PNG_SIDE}, the largest texture the engine's device takes",
                declared.width, declared.height
            ),
        )
        .with_help("downscale the image or split it into tiles of at most 8192x8192 pixels"));
    }
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| png_error(label, format!("could not decode PNG pixels: {error}")))?;
    buffer.truncate(info.buffer_size());

    let (width, height) = (info.width, info.height);
    let pixels = (width as usize) * (height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => expand(&buffer, pixels, 3, |src, dst| {
            dst[0..3].copy_from_slice(&src[0..3]);
            dst[3] = 255;
        }),
        png::ColorType::GrayscaleAlpha => expand(&buffer, pixels, 2, |src, dst| {
            dst[0..3].fill(src[0]);
            dst[3] = src[1];
        }),
        png::ColorType::Grayscale => expand(&buffer, pixels, 1, |src, dst| {
            dst[0..3].fill(src[0]);
            dst[3] = 255;
        }),
        other => {
            return Err(png_error(
                label,
                format!("unsupported PNG color type {other:?} after expansion"),
            ));
        }
    };
    Ok((width, height, rgba))
}

/// Encodes tightly packed RGBA8 `rgba` (`width * height * 4` bytes, row-major, top row first) as an
/// in-memory PNG.
///
/// **Deterministic:** the `png` encoder writes only IHDR / IDAT / IEND — no `tIME` or other
/// timestamped ancillary chunks — and deflate is a pure function of its input, so identical pixels
/// always produce byte-identical files. This is the one PNG encoder in the engine: `ludion render`'s
/// readback writer and `ludion import`'s texture re-encode both go through it, so a re-encoded image
/// and a freshly rendered one share an identity (`ludion-render-graph` and `ludion-import` delegate
/// here — the encoder lives in the GPU-free assets crate so the importer never depends on wgpu).
///
/// Returns the encoder's own reason string on failure (a caller wraps it in its domain's diagnostic
/// code — `L2006` for a render write, `L1203` for an import write); encoding into a `Vec` has no IO
/// to fail, so a failure here means the dimensions or buffer length were rejected by the encoder.
///
/// # Example
/// ```
/// let a = ludion_assets::encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
/// let b = ludion_assets::encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
/// assert_eq!(a, b, "PNG encoding is deterministic");
/// assert_eq!(&a[1..4], b"PNG");
/// ```
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("could not write PNG header: {error}"))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| format!("could not write PNG pixel data: {error}"))?;
    }
    Ok(bytes)
}

/// Widens `pixels` source samples of `stride` bytes each into RGBA8 via `fill`.
fn expand(src: &[u8], pixels: usize, stride: usize, fill: impl Fn(&[u8], &mut [u8; 4])) -> Vec<u8> {
    let mut out = vec![0u8; pixels * 4];
    for i in 0..pixels {
        let s = &src[i * stride..i * stride + stride];
        let mut px = [0u8; 4];
        fill(s, &mut px);
        out[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    out
}

fn png_error(label: &str, detail: String) -> Diagnostic {
    Diagnostic::new(
        code::ASSET_PNG_DECODE,
        format!("asset `{label}` is not a valid PNG: {detail}"),
    )
    .with_help("re-export the image as a standard 8-bit PNG (grayscale, RGB, RGBA, or palette)")
}

/// Decoded PCM audio: interleaved `f32` samples in `[-1.0, 1.0]`, plus the source's sample rate and
/// channel count.
///
/// This is CPU-only sound data — the audio analog of [`Image`]. The playback sink (`ludion-audio`)
/// consumes it; this crate stays free of any audio device or codec beyond the minimal WAV reader.
#[derive(Clone, PartialEq, Debug)]
pub struct AudioPcm {
    /// Samples per second of the source clip (e.g. `44100`).
    pub sample_rate: u32,
    /// Channel count: `1` (mono) or `2` (stereo). Samples are interleaved by channel.
    pub channels: u16,
    /// Interleaved samples normalized from PCM16 to `f32` in `[-1.0, 1.0]`, `frames * channels` long.
    pub samples: Vec<f32>,
}

/// Reads and decodes the WAV file at `root`/`relative` into [`AudioPcm`].
///
/// Accepts uncompressed 16-bit PCM WAV, mono or stereo — the minimal format a small committed sound
/// effect needs. See [`decode_wav`] for the accepted shape and errors.
///
/// # Errors
/// - `L1101` if the file cannot be read.
/// - `L1104` if the bytes are not a decodable 16-bit PCM WAV.
pub fn load_wav(root: AssetRoot<'_>, relative: &str) -> Result<AudioPcm, Diagnostic> {
    let asset = load_bytes(root, relative)?;
    decode_wav(&asset.bytes, relative)
}

/// Decodes WAV `bytes` (named `label` for diagnostics) into [`AudioPcm`].
///
/// Split out from [`load_wav`] so it can be unit-tested on in-memory bytes without touching disk.
/// Only the minimal subset a committed sound effect uses is accepted: a RIFF/WAVE container whose
/// `fmt ` chunk declares uncompressed PCM (`audio_format == 1`, or `WAVE_FORMAT_EXTENSIBLE` with a
/// PCM SubFormat), 16 bits per sample, and 1 or 2 channels, followed by a `data` chunk. PCM16
/// samples are normalized to `f32` in `[-1.0, 1.0]`.
///
/// # Errors
/// `L1104` for any deviation: not a RIFF/WAVE container, a missing/short `fmt ` or `data` chunk, a
/// non-PCM codec, a bit depth other than 16, an unsupported channel count, or a sample rate below
/// 8000 Hz.
pub fn decode_wav(bytes: &[u8], label: &str) -> Result<AudioPcm, Diagnostic> {
    // RIFF header: "RIFF" <u32 size> "WAVE", then a sequence of <4-byte id><u32 size><payload>
    // chunks. We only need `fmt ` (format description) and `data` (the samples).
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(wav_error(label, "not a RIFF/WAVE container"));
    }

    let mut fmt: Option<WavFmt> = None;
    let mut data: Option<&[u8]> = None;
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(size)
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| {
                wav_error(label, "a chunk declares more bytes than the file contains")
            })?;
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => fmt = Some(parse_wav_fmt(body, label)?),
            b"data" => data = Some(body),
            _ => {} // ignore chunks we do not need (e.g. `LIST`, `fact`)
        }
        // Chunks are word-aligned: an odd size is followed by one padding byte.
        pos = body_end + (size & 1);
    }

    let fmt = fmt.ok_or_else(|| wav_error(label, "no `fmt ` chunk"))?;
    let data = data.ok_or_else(|| wav_error(label, "no `data` chunk"))?;

    let frame_bytes = fmt.channels as usize * 2; // 2 bytes per 16-bit sample
    if frame_bytes == 0 || data.len() % frame_bytes != 0 {
        return Err(wav_error(
            label,
            "the `data` chunk length is not a whole number of sample frames",
        ));
    }
    let samples = data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0)
        .collect();

    Ok(AudioPcm {
        sample_rate: fmt.sample_rate,
        channels: fmt.channels,
        samples,
    })
}

/// The lowest sample rate a WAV or a streamed Ogg track may declare (the telephony rate,
/// inclusive). The playback sink resamples everything to the device rate, so a rate near zero would
/// expand a clip by a factor of the device rate; at the floor the worst case is 6× on a 48 kHz
/// device.
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// The fields of a WAV `fmt ` chunk this loader uses.
struct WavFmt {
    channels: u16,
    sample_rate: u32,
}

/// Parses (and validates) a WAV `fmt ` chunk body: PCM, 16-bit, mono or stereo, at least
/// [`MIN_SAMPLE_RATE`].
fn parse_wav_fmt(body: &[u8], label: &str) -> Result<WavFmt, Diagnostic> {
    if body.len() < 16 {
        return Err(wav_error(label, "the `fmt ` chunk is too short"));
    }
    let audio_format = u16::from_le_bytes([body[0], body[1]]);
    let channels = u16::from_le_bytes([body[2], body[3]]);
    let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);

    if audio_format == WAVE_FORMAT_EXTENSIBLE {
        check_extensible_pcm(body, label)?;
    } else if audio_format != 1 {
        return Err(wav_error(
            label,
            &format!("audio format {audio_format} is not uncompressed PCM (format 1)"),
        ));
    }
    if bits_per_sample != 16 {
        return Err(wav_error(
            label,
            &format!("{bits_per_sample}-bit samples are not supported (only 16-bit PCM)"),
        ));
    }
    if channels != 1 && channels != 2 {
        return Err(wav_error(
            label,
            &format!("{channels} channels are not supported (only mono or stereo)"),
        ));
    }
    if sample_rate < MIN_SAMPLE_RATE {
        return Err(wav_error(
            label,
            &format!("sample rate {sample_rate} Hz is below the {MIN_SAMPLE_RATE} Hz floor"),
        ));
    }
    Ok(WavFmt {
        channels,
        sample_rate,
    })
}

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// The SubFormat GUID's tail shared by every `KSDATAFORMAT_SUBTYPE_*`; its first two bytes are the
/// plain format tag (1 = PCM, 3 = IEEE float).
const SUBFORMAT_GUID_TAIL: [u8; 14] = [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71];

/// Accepts a `WAVE_FORMAT_EXTENSIBLE` `fmt ` body only when its SubFormat is PCM; the bit depth,
/// channel and rate checks then apply as for a plain format-1 file.
fn check_extensible_pcm(body: &[u8], label: &str) -> Result<(), Diagnostic> {
    let cb_size = body
        .get(16..18)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    if cb_size < 22 || body.len() < 40 {
        return Err(wav_error(
            label,
            &format!(
                "a WAVE_FORMAT_EXTENSIBLE header needs cbSize >= 22 and a 40-byte `fmt ` chunk \
                 (found cbSize {cb_size}, {} bytes)",
                body.len()
            ),
        ));
    }
    let tag = u16::from_le_bytes([body[24], body[25]]);
    if tag == 1 && body[26..40] == SUBFORMAT_GUID_TAIL {
        return Ok(());
    }
    let subformat = match tag {
        3 if body[26..40] == SUBFORMAT_GUID_TAIL => "IEEE float".to_string(),
        _ => format!("subformat {tag}"),
    };
    Err(wav_error(
        label,
        &format!(
            "WAVE_FORMAT_EXTENSIBLE with {subformat} is not uncompressed PCM; export as 16-bit \
             signed PCM"
        ),
    ))
}

fn wav_error(label: &str, detail: &str) -> Diagnostic {
    Diagnostic::new(
        code::ASSET_WAV_DECODE,
        format!("asset `{label}` is not a valid WAV: {detail}"),
    )
    .with_help("re-export the sound as a 16-bit PCM WAV (mono or stereo)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a tiny RGBA8 image to PNG bytes for round-trip tests (mirrors the renderer's writer).
    fn tiny_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(rgba).unwrap();
        }
        bytes
    }

    #[test]
    fn srgb_mean_of_one_texel_decodes_each_channel() {
        let mean = srgb_mean(&[255, 0, 128, 64]);
        assert_eq!(mean, [1.0, 0.0, decode_srgb(128), 64.0 / 255.0]);
    }

    #[test]
    fn srgb_mean_averages_in_linear_light() {
        let mean = srgb_mean(&[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(mean, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn srgb_mean_ignores_pixel_order() {
        let a = [10, 20, 30, 40, 200, 150, 100, 50, 7, 99, 250, 128];
        let b = [7, 99, 250, 128, 10, 20, 30, 40, 200, 150, 100, 50];
        let bits = |m: [f32; 4]| m.map(f32::to_bits);
        assert_eq!(bits(srgb_mean(&a)), bits(srgb_mean(&b)));
        assert_eq!(srgb_mean(&[]), [1.0; 4]);
    }

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        assert_eq!(content_hash(b"ludion"), content_hash(b"ludion"));
        assert_ne!(content_hash(b"ludion"), content_hash(b"Ludion"));
    }

    #[test]
    fn decodes_rgba_png_round_trip() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128];
        let png = tiny_png(2, 1, &rgba);
        let (w, h, out) = decode_png(&png, "test.png").unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, rgba);
    }

    #[test]
    fn bad_png_bytes_are_l1102() {
        let error = decode_png(b"not a png at all", "junk.png").unwrap_err();
        assert_eq!(error.code.as_str(), "L1102");
        assert!(error.message.contains("junk.png"));
    }

    /// CRC-32 (IEEE) as PNG chunks use it; bitwise, since this is test-only.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn png_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut chunk = (body.len() as u32).to_be_bytes().to_vec();
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(body);
        chunk.extend_from_slice(&crc_input);
        chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        chunk
    }

    #[test]
    fn oversized_png_header_is_l1102_before_allocating() {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&100_000u32.to_be_bytes());
        ihdr.extend_from_slice(&100_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(png_chunk(b"IHDR", &ihdr));
        png.extend(png_chunk(b"IDAT", &[]));
        png.extend(png_chunk(b"IEND", &[]));

        // A header-sized allocation would be 40 GB; returning at all is the proof it never happens.
        let error = decode_png(&png, "huge.png").unwrap_err();
        assert_eq!(error.code.as_str(), "L1102");
        assert!(error.message.contains("100000"), "{}", error.message);
    }

    #[test]
    fn a_side_over_8192_is_l1102() {
        let png = encode_png(16384, 1, &vec![0u8; 16384 * 4]).unwrap();
        let error = decode_png(&png, "wide.png").unwrap_err();
        assert_eq!(error.code.as_str(), "L1102");
        assert!(error.message.contains("16384"), "{}", error.message);
    }

    #[test]
    fn escaping_and_absolute_paths_are_refused_with_the_content_code() {
        let root = std::env::temp_dir();
        for path in [
            "../x.png",
            "a/../../x.png",
            "/dev/zero",
            "C:x.png",
            "//h/s/x.png",
        ] {
            let error = load_bytes(AssetRoot::content(&root), path).unwrap_err();
            assert_eq!(error.code, code::CONTENT_PATH_OUTSIDE_ROOT, "{path}");
            assert!(error.message.contains(path), "{}", error.message);
        }
        assert!(resolve(AssetRoot::content(&root), "meshes/../t.png").is_ok());
    }

    #[test]
    fn a_directory_is_l1101() {
        let dir = std::env::temp_dir().join(format!("ludion-assets-dir-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let error = load_bytes(AssetRoot::content(&dir), "sub").unwrap_err();
        assert_eq!(error.code, code::ASSET_FILE_IO);
        assert!(error.message.contains("regular file"), "{}", error.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_read_past_the_cap_is_refused_and_one_at_it_is_kept() {
        assert_eq!(read_capped(&[7u8; 4][..], 4).unwrap(), Some(vec![7; 4]));
        assert_eq!(read_capped(&[7u8; 5][..], 4).unwrap(), None);
        // A source that never ends stops at the cap instead of filling memory.
        assert_eq!(read_capped(std::io::repeat(0), 16).unwrap(), None);
    }

    #[test]
    fn an_unconfined_root_climbs_but_still_refuses_absolute_paths() {
        let dir = std::env::temp_dir().join(format!("ludion-assets-rf-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("x.bin"), b"abc").unwrap();
        let sub = dir.join("a");
        let root = AssetRoot::local_document_unconfined(&sub);
        assert_eq!(load_bytes(root, "../x.bin").unwrap().bytes, b"abc");
        for path in ["/dev/zero", "C:x.bin", "..\\..\\nul.bin"] {
            let error = load_bytes(root, path).unwrap_err();
            assert_eq!(error.code, code::CONTENT_PATH_OUTSIDE_ROOT, "{path}");
        }
        assert_eq!(
            load_bytes(AssetRoot::content(&sub), "../x.bin")
                .unwrap_err()
                .code,
            code::CONTENT_PATH_OUTSIDE_ROOT
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_l1101() {
        let root = std::env::temp_dir();
        let error = load_png(
            AssetRoot::content(&root),
            "ludion-definitely-missing-42.png",
        )
        .unwrap_err();
        assert_eq!(error.code.as_str(), "L1101");
    }

    /// Builds a minimal 16-bit PCM WAV from interleaved i16 samples, for round-trip tests.
    fn tiny_wav(channels: u16, sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for &s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn decodes_mono_wav_round_trip() {
        let wav = tiny_wav(1, 22050, &[0, 16384, -16384, i16::MAX, i16::MIN]);
        let pcm = decode_wav(&wav, "blip.wav").unwrap();
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.sample_rate, 22050);
        assert_eq!(pcm.samples.len(), 5);
        assert_eq!(pcm.samples[0], 0.0);
        assert!((pcm.samples[1] - 0.5).abs() < 1e-6);
        assert!((pcm.samples[2] + 0.5).abs() < 1e-6);
        assert!(pcm.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn decodes_stereo_wav_round_trip() {
        // Two frames, L/R interleaved.
        let wav = tiny_wav(2, 44100, &[100, -100, 200, -200]);
        let pcm = decode_wav(&wav, "stereo.wav").unwrap();
        assert_eq!(pcm.channels, 2);
        assert_eq!(pcm.sample_rate, 44100);
        assert_eq!(pcm.samples.len(), 4);
    }

    #[test]
    fn bad_wav_bytes_are_l1104() {
        let error = decode_wav(b"not a wav at all!!!!", "junk.wav").unwrap_err();
        assert_eq!(error.code.as_str(), "L1104");
        assert!(error.message.contains("junk.wav"));
    }

    #[test]
    fn non_pcm_wav_is_l1104() {
        let mut wav = tiny_wav(1, 8000, &[0, 1, 2]);
        // Overwrite the audio-format field (offset 20) with 3 (IEEE float) — not accepted.
        wav[20] = 3;
        let error = decode_wav(&wav, "float.wav").unwrap_err();
        assert_eq!(error.code.as_str(), "L1104");
    }

    /// `tiny_wav` re-headed as `WAVE_FORMAT_EXTENSIBLE`: a 40-byte `fmt ` chunk with `cb_size` and
    /// the SubFormat GUID's leading two bytes (`01 00` is PCM, `03 00` IEEE float).
    fn extensible_wav(cb_size: u16, subformat: [u8; 2], samples: &[i16]) -> Vec<u8> {
        let plain = tiny_wav(1, 44100, samples);
        let mut bytes = plain[..20].to_vec(); // RIFF header, `fmt ` id and size placeholder
        bytes[16..20].copy_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&0xFFFEu16.to_le_bytes());
        bytes.extend_from_slice(&plain[22..36]); // channels .. bits per sample
        bytes.extend_from_slice(&cb_size.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        bytes.extend_from_slice(&4u32.to_le_bytes()); // channel mask: front centre
        bytes.extend_from_slice(&subformat);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
        bytes.extend_from_slice(&plain[36..]); // `data` chunk
        bytes
    }

    #[test]
    fn extensible_pcm16_wav_decodes_like_the_plain_file() {
        let samples = [0, 16384, -16384, i16::MAX, i16::MIN];
        let plain = decode_wav(&tiny_wav(1, 44100, &samples), "plain.wav").unwrap();
        let extensible = decode_wav(&extensible_wav(22, [1, 0], &samples), "ext.wav").unwrap();
        assert_eq!(extensible.samples, plain.samples);
        assert_eq!((extensible.channels, extensible.sample_rate), (1, 44100));
    }

    #[test]
    fn extensible_float_wav_is_rejected_naming_the_subformat() {
        let error = decode_wav(&extensible_wav(22, [3, 0], &[0, 1]), "ext.wav").unwrap_err();
        assert_eq!(error.code.as_str(), "L1104");
        assert!(
            error.message.contains("WAVE_FORMAT_EXTENSIBLE"),
            "{}",
            error.message
        );
        assert!(error.message.contains("float"), "{}", error.message);
        assert!(error.help.as_deref().unwrap_or("").contains("16-bit"));
    }

    #[test]
    fn extensible_wav_with_a_short_cb_size_is_rejected_readably() {
        let error = decode_wav(&extensible_wav(0, [1, 0], &[0, 1]), "ext.wav").unwrap_err();
        assert_eq!(error.code.as_str(), "L1104");
        assert!(
            error.message.contains("WAVE_FORMAT_EXTENSIBLE"),
            "{}",
            error.message
        );
        assert!(error.message.contains("cbSize"), "{}", error.message);
    }

    #[test]
    fn sample_rate_below_floor_is_l1104() {
        for rate in [1, 7999] {
            let error = decode_wav(&tiny_wav(1, rate, &[0, 1, 2]), "slow.wav").unwrap_err();
            assert_eq!(error.code.as_str(), "L1104");
            assert!(error.message.contains("8000 Hz floor"), "{}", error.message);
        }
    }

    #[test]
    fn sample_rate_at_floor_decodes() {
        let pcm = decode_wav(&tiny_wav(1, 8000, &[0, 1, 2]), "floor.wav").unwrap();
        assert_eq!(pcm.sample_rate, 8000);
    }

    /// The convention's no-op case: an opaque texel is already its own premultiplied form, so a
    /// fully opaque PNG must survive the conversion byte-for-byte (what keeps the shipped goldens
    /// standing).
    #[test]
    fn premultiplying_an_opaque_image_changes_nothing() {
        let rgba = vec![10, 20, 30, 255, 200, 0, 128, 255];
        let mut image = Image {
            width: 2,
            height: 1,
            rgba: rgba.clone(),
            hash: 0,
        };
        image.premultiply();
        assert_eq!(image.rgba, rgba);
    }

    /// Half coverage weights the colour in **linear** light, not on the stored byte: mid-grey sRGB
    /// `188` decodes to linear `0.503`, halves to `0.252`, and re-encodes to `138`. Multiplying the
    /// stored byte instead would give `94` — visibly darker, the defect this test pins.
    #[test]
    fn premultiplying_weights_colour_in_linear_light() {
        let mut image = Image {
            width: 1,
            height: 1,
            rgba: vec![188, 188, 188, 128],
            hash: 0,
        };
        image.premultiply();
        assert_eq!(image.rgba, vec![138, 138, 138, 128]);
    }

    /// Zero coverage contributes nothing under `src + dst * (1 - src.a)`, so the colour must be
    /// exactly zero — a stale colour behind alpha 0 is what fringes a scaled sprite.
    #[test]
    fn premultiplying_zero_alpha_zeroes_the_colour() {
        let mut image = Image {
            width: 1,
            height: 1,
            rgba: vec![255, 128, 7, 0],
            hash: 0,
        };
        assert_eq!(image.clone().premultiplied().rgba, vec![0, 0, 0, 0]);
        image.premultiply();
        assert_eq!(image.rgba, vec![0, 0, 0, 0]);
    }

    #[test]
    fn load_png_hash_matches_file_bytes() {
        let dir = std::env::temp_dir();
        let png = tiny_png(1, 1, &[10, 20, 30, 40]);
        std::fs::write(dir.join("ludion_hash_probe.png"), &png).unwrap();
        let image = load_png(AssetRoot::content(&dir), "ludion_hash_probe.png").unwrap();
        assert_eq!(image.hash, content_hash(&png));
        assert_eq!(image.rgba, vec![10, 20, 30, 40]);
    }
}
