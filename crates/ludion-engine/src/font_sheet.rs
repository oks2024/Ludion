//! `ludion render --font` — the CPU-composed **specimen sheet** of a `kind: font` document.
//!
//! The same "zero pixels" shape as [`crate::sprite_sheet`]'s atlas overview: every byte is written
//! by integer and basic `f32` arithmetic into an RGBA8 buffer, no adapter is requested and no clock
//! is read, so two runs over the same face write identical bytes. The sample text goes through
//! [`ludion_font::layout`] — the layout the game draws with — and is rasterized here by sampling the
//! face's own atlas, so the sheet answers "what did this bake draw" and not "what would a model of
//! it draw". The labels are burned with the embedded 8×8 face ([`crate::voxel_sheet::stamp_text`]).
//!
//! Top to bottom: a header; the atlas image; a metric band (ascender, baseline and descender rules
//! over a mixed-case sample); a kerning sample drawn with the face and again with its kerning
//! removed; the charset; and a pangram at three sizes.

use std::path::Path;

use ludion_assets::Image;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_font::{FontFace, GlyphQuad, YAxis};
use ludion_scene::{FontDocument, FontMode};

use crate::voxel_sheet::{BACKGROUND, fill_block, stamp_text};

/// The sheet's width: the atlas block plus a margin each side.
const SHEET_W: u32 = ATLAS_MAX_W + 2 * MARGIN;
const MARGIN: u32 = 16;
/// The widest the atlas block is drawn.
const ATLAS_MAX_W: u32 = 1024;
/// The largest integer upscale of a small atlas.
const ATLAS_MAX_UPSCALE: u32 = 16;
/// Burned label glyph scale (8×8 cell, plus a one-pixel gap).
const LABEL_SCALE: u32 = 1;
const GAP: u32 = 12;

const TITLE_COLOR: [u8; 4] = [255, 255, 255, 255];
const LABEL_COLOR: [u8; 4] = [150, 156, 168, 255];
const INK: [u8; 4] = [236, 238, 242, 255];
const RULE_ASCENDER: [u8; 4] = [90, 170, 255, 255];
const RULE_BASELINE: [u8; 4] = [255, 196, 64, 255];
const RULE_DESCENDER: [u8; 4] = [255, 110, 110, 255];

const METRIC_SAMPLE: &str = "Hxgp ÀÉ";
const KERNING_SAMPLE: &str = "AV To Wa Yo LT";
const PANGRAM: &str = "The quick brown fox jumps over the lazy dog 0123456789";

/// Composes the specimen sheet of the font document at `doc`: its atlas PNG, resolved beside it,
/// then the metric, kerning, charset and pangram samples.
///
/// # Errors
/// - `L1015` if the document cannot be read.
/// - Every diagnostic [`ludion_scene::check_font`] reports.
/// - `L1101`/`L1102` if the atlas image beside it cannot be read or decoded.
pub fn render_font_sheet(doc: &Path) -> Result<Image, Vec<Diagnostic>> {
    let source = std::fs::read_to_string(doc).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read font document `{}`: {error}", doc.display()),
            )
            .with_help("pass a readable `kind: font` document (check the path and permissions)"),
        ]
    })?;
    let parsed = ludion_scene::check_font(&source)?;
    let root = doc.parent().unwrap_or_else(|| Path::new("."));
    let atlas = ludion_assets::load_png(root, &parsed.image).map_err(|d| vec![d])?;
    if (atlas.width, atlas.height) != (parsed.atlas.width, parsed.atlas.height) {
        return Err(vec![
            Diagnostic::new(
                code::FONT_LOAD,
                format!(
                    "the atlas `{}` is {}x{} but `{}` declares {}x{}",
                    parsed.image,
                    atlas.width,
                    atlas.height,
                    doc.display(),
                    parsed.atlas.width,
                    parsed.atlas.height
                ),
            )
            .with_help("re-run `ludion generate-atlas` on its source"),
        ]);
    }
    let name = doc.file_name().map_or_else(
        || doc.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    compose_font_sheet(&name, &parsed, &atlas)
}

/// A growing RGBA8 canvas of fixed width: rows are appended on demand so the sheet is exactly as
/// tall as its contents.
struct Canvas {
    rgba: Vec<u8>,
    height: u32,
}

impl Canvas {
    fn new() -> Self {
        Canvas {
            rgba: Vec::new(),
            height: 0,
        }
    }

    fn ensure_height(&mut self, height: u32) {
        if height > self.height {
            let rows = (height - self.height) as usize;
            for _ in 0..rows * SHEET_W as usize {
                self.rgba.extend_from_slice(&BACKGROUND);
            }
            self.height = height;
        }
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SHEET_W + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }

    fn set(&mut self, x: u32, y: u32, color: [u8; 4]) {
        let i = ((y * SHEET_W + x) * 4) as usize;
        self.rgba[i..i + 4].copy_from_slice(&color);
    }

    fn rule(&mut self, y: u32, color: [u8; 4]) {
        self.ensure_height(y + 1);
        fill_block(
            &mut self.rgba,
            SHEET_W,
            self.height,
            0,
            y,
            SHEET_W,
            1,
            color,
        );
    }

    /// Burns `text` with the embedded 8×8 face and returns the row below it.
    fn label(&mut self, x: u32, y: u32, text: &str, color: [u8; 4]) -> u32 {
        let h = 8 * LABEL_SCALE;
        self.ensure_height(y + h);
        stamp_text(
            &mut self.rgba,
            SHEET_W,
            self.height,
            x,
            y,
            text,
            LABEL_SCALE,
            color,
        );
        y + h + 4
    }

    /// Draws `text` with `face` at `size`, wrapped to the sheet, and returns the row below it.
    fn text(&mut self, face: &FontFace, atlas: &Image, text: &str, size: f32, y: u32) -> u32 {
        let wrap = (SHEET_W - 2 * MARGIN) as f32;
        let laid = ludion_font::layout(face, text, size, wrap);
        let bottom = y + (laid.line_count() as f32 * size).ceil() as u32 + 1;
        self.ensure_height(bottom);
        for quad in laid.quads([MARGIN as f32, y as f32], size, YAxis::Down) {
            self.ink_quad(face, atlas, &quad);
        }
        bottom
    }

    /// Rasterizes one glyph quad by sampling the atlas at each covered pixel centre.
    fn ink_quad(&mut self, face: &FontFace, atlas: &Image, quad: &GlyphQuad) {
        let doc = face.document();
        let [left, top, right, bottom] = quad.pos;
        let (aw, ah) = (atlas.width as f32, atlas.height as f32);
        let (u0, v0, u1, v1) = (
            quad.uv[0] * aw,
            quad.uv[1] * ah,
            quad.uv[2] * aw,
            quad.uv[3] * ah,
        );
        let (quad_w, quad_h) = (right - left, bottom - top);
        // A zero-width UV span has no texels to sample.
        if quad_w <= 0.0 || quad_h <= 0.0 || u1 <= u0 {
            return;
        }
        // Pixels of screen per texel, along x: scales the SDF's texel distance to pixels.
        let px_per_texel = quad_w / (u1 - u0);
        let spread = doc.atlas.spread_px.unwrap_or(1) as f32;
        let x_start = left.floor().max(0.0) as u32;
        let x_end = (right.ceil().max(0.0) as u32).min(SHEET_W);
        let y_start = top.floor().max(0.0) as u32;
        let y_end = (bottom.ceil().max(0.0) as u32).min(self.height);
        for py in y_start..y_end {
            let cy = py as f32 + 0.5;
            if cy < top || cy >= bottom {
                continue;
            }
            for px in x_start..x_end {
                let cx = px as f32 + 0.5;
                if cx < left || cx >= right {
                    continue;
                }
                let tx = u0 + (cx - left) / quad_w * (u1 - u0);
                let ty = v0 + (cy - top) / quad_h * (v1 - v0);
                let coverage = match doc.mode {
                    FontMode::Bitmap => {
                        let x = (tx.floor() as i64).clamp(0, atlas.width as i64 - 1) as u32;
                        let y = (ty.floor() as i64).clamp(0, atlas.height as i64 - 1) as u32;
                        alpha_at(atlas, x, y)
                    }
                    FontMode::Sdf => {
                        let a = bilinear_alpha(atlas, tx, ty);
                        let distance_px = (a - 0.5) * 2.0 * spread * px_per_texel;
                        (distance_px + 0.5).clamp(0.0, 1.0)
                    }
                };
                if coverage > 0.0 {
                    let under = self.pixel(px, py);
                    self.set(px, py, blend(under, INK, coverage));
                }
            }
        }
    }
}

fn alpha_at(atlas: &Image, x: u32, y: u32) -> f32 {
    f32::from(atlas.rgba[((y * atlas.width + x) * 4 + 3) as usize]) / 255.0
}

/// The atlas alpha at texel-space `(x, y)`, bilinearly filtered with edge clamping.
fn bilinear_alpha(atlas: &Image, x: f32, y: f32) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |dx: i64, dy: i64| {
        let sx = (x0 as i64 + dx).clamp(0, atlas.width as i64 - 1) as u32;
        let sy = (y0 as i64 + dy).clamp(0, atlas.height as i64 - 1) as u32;
        alpha_at(atlas, sx, sy)
    };
    let top = at(0, 0) * (1.0 - tx) + at(1, 0) * tx;
    let bottom = at(0, 1) * (1.0 - tx) + at(1, 1) * tx;
    top * (1.0 - ty) + bottom * ty
}

fn blend(under: [u8; 4], ink: [u8; 4], coverage: f32) -> [u8; 4] {
    let mix =
        |u: u8, i: u8| (f32::from(u) + (f32::from(i) - f32::from(u)) * coverage).round() as u8;
    [
        mix(under[0], ink[0]),
        mix(under[1], ink[1]),
        mix(under[2], ink[2]),
        255,
    ]
}

/// Composes the sheet for a parsed font document and its decoded atlas. Pure and CPU-only.
fn compose_font_sheet(
    name: &str,
    doc: &FontDocument,
    atlas: &Image,
) -> Result<Image, Vec<Diagnostic>> {
    let face = FontFace::new(doc.clone())?;
    let unkerned = {
        let mut bare = doc.clone();
        bare.kerning.clear();
        FontFace::new(bare)?
    };
    let mut canvas = Canvas::new();

    let header = format!(
        "{name}  {}  {} glyphs  {} kern pairs  {} missing  atlas {}x{}",
        doc.mode.keyword(),
        doc.glyphs.len(),
        doc.kerning.len(),
        doc.missing.len(),
        atlas.width,
        atlas.height,
    );
    let mut y = canvas.label(MARGIN, MARGIN, &header, TITLE_COLOR) + GAP;

    // The atlas, nearest-scaled to fit the block, composited over the background.
    let dest_w = if atlas.width >= ATLAS_MAX_W {
        ATLAS_MAX_W
    } else {
        atlas.width * (ATLAS_MAX_W / atlas.width.max(1)).clamp(1, ATLAS_MAX_UPSCALE)
    };
    let dest_h =
        (u64::from(atlas.height) * u64::from(dest_w) / u64::from(atlas.width.max(1))).max(1) as u32;
    canvas.ensure_height(y + dest_h);
    for dy in 0..dest_h {
        let sy = (u64::from(dy) * u64::from(atlas.height) / u64::from(dest_h)) as u32;
        for dx in 0..dest_w {
            let sx = (u64::from(dx) * u64::from(atlas.width) / u64::from(dest_w)) as u32;
            let under = canvas.pixel(MARGIN + dx, y + dy);
            canvas.set(
                MARGIN + dx,
                y + dy,
                blend(under, INK, alpha_at(atlas, sx, sy)),
            );
        }
    }
    y += dest_h + GAP;

    // Metric band: ascender / baseline / descender rules behind a mixed-case sample.
    y = canvas.label(
        MARGIN,
        y,
        "metrics  96 px: ascender, baseline, descender",
        LABEL_COLOR,
    );
    let size = 96.0_f32;
    let unit = size / face.line_units() as f32;
    let ascender_y = y;
    let baseline_y = y + (doc.ascender as f32 * unit).round() as u32;
    let descender_y = y + ((doc.ascender - doc.descender) as f32 * unit).round() as u32;
    canvas.ensure_height(descender_y + 1);
    canvas.rule(ascender_y, RULE_ASCENDER);
    canvas.rule(baseline_y, RULE_BASELINE);
    canvas.rule(descender_y, RULE_DESCENDER);
    let band_bottom = canvas.text(&face, atlas, METRIC_SAMPLE, size, y);
    y = band_bottom.max(descender_y + 1) + GAP;

    y = canvas.label(MARGIN, y, "kerned", LABEL_COLOR);
    y = canvas.text(&face, atlas, KERNING_SAMPLE, 64.0, y);
    y = canvas.label(MARGIN, y + 4, "unkerned", LABEL_COLOR);
    y = canvas.text(&unkerned, atlas, KERNING_SAMPLE, 64.0, y) + GAP;

    y = canvas.label(MARGIN, y, "charset  32 px", LABEL_COLOR);
    let charset: String = doc
        .glyphs
        .iter()
        .map(|g| g.ch)
        .filter(|ch| !ch.is_control())
        .collect();
    y = canvas.text(&face, atlas, &charset, 32.0, y) + GAP;

    for size in [16.0_f32, 32.0, 64.0] {
        y = canvas.label(MARGIN, y, &format!("pangram  {size} px"), LABEL_COLOR);
        y = canvas.text(&face, atlas, PANGRAM, size, y) + 4;
    }
    canvas.ensure_height(y + MARGIN);

    let rgba = canvas.rgba;
    Ok(Image {
        width: SHEET_W,
        height: canvas.height,
        hash: ludion_assets::content_hash(&rgba),
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> (FontDocument, Image) {
        let doc = FontFace::embedded().document().clone();
        let (width, height, rgba) =
            ludion_assets::decode_png(ludion_font::EMBEDDED_PNG, "embedded").expect("embedded png");
        let image = Image {
            width,
            height,
            hash: ludion_assets::content_hash(ludion_font::EMBEDDED_PNG),
            rgba,
        };
        (doc, image)
    }

    #[test]
    fn the_embedded_face_composes_a_deterministic_nonblank_sheet() {
        let (doc, atlas) = embedded();
        let first = compose_font_sheet("default.font.yaml", &doc, &atlas).expect("sheet");
        let second = compose_font_sheet("default.font.yaml", &doc, &atlas).expect("sheet");
        assert_eq!(first, second);
        assert_eq!(first.width, SHEET_W);
        assert!(first.height > 400);
        let background = BACKGROUND;
        let inked = first
            .rgba
            .chunks_exact(4)
            .filter(|px| px[..3] != background[..3])
            .count();
        assert!(
            inked > 1000,
            "only {inked} pixels differ from the background"
        );
    }
}
