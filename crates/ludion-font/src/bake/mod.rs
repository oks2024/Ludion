//! The bake's reading half (feature `bake`): glyph metrics, flattened outlines and pair kerning
//! from a `.ttf`/`.otf` face, in the face's own font units.
//!
//! Outlines are read unscaled and unhinted, curves are flattened with a fixed subdivision count
//! using only `+ - *` on `f32`, and every collection is sorted — so [`read_face`] is a pure,
//! byte-reproducible function of the face bytes and the char list. Scaling to texels, coverage and
//! SDF belong to [`bake`], which turns a source document and a face into a `kind: font` document and
//! its atlas PNG. Only the CLI enables this feature; the engine never parses a font.

mod coverage;
mod geometry;
mod kerning;
mod outline;
mod pack;
mod sdf;

pub use outline::MAX_GLYPH_SEGMENTS;

use ludion_assets::Image;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{FontAtlas, FontDocument, FontGlyph, FontMode, FontNotdef, FontSourceDocument};
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::{FileRef, TableProvider};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// One glyph read from a face.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    /// Horizontal advance in font units.
    pub advance: i32,
    /// Ink bounds `[x0, y0, x1, y1]` in font units, rounded outward; `None` for an empty outline.
    pub bounds: Option<[i32; 4]>,
    /// Closed polylines in font units, y up.
    pub contours: Vec<Vec<[f32; 2]>>,
}

/// Everything the bake needs from one face for a char list.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceOutlines {
    /// Font units per em.
    pub units_per_em: u32,
    /// Distance from the baseline up to the line top (`hhea`).
    pub ascender: i32,
    /// Distance from the baseline to the line bottom (`hhea`; zero or negative).
    pub descender: i32,
    /// Extra space between lines (`hhea`).
    pub line_gap: i32,
    /// Glyph 0, always extracted.
    pub notdef: GlyphOutline,
    /// The face's glyph per requested char, ascending by char.
    pub glyphs: Vec<(char, GlyphOutline)>,
    /// `(left, right, adjust)` pair kerning in font units, sorted; nonzero adjusts only.
    pub kerning: Vec<(char, char, i32)>,
    /// Requested chars the face has no glyph for, ascending.
    pub missing: Vec<char>,
}

fn file_diag(message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::FONT_FILE, message).with_help("pass a single .ttf/.otf face")]
}

fn round_i32(v: f32) -> i32 {
    v.round() as i32
}

/// Reads `bytes` as a single face and extracts metrics, outlines and kerning for `chars`
/// (sorted, deduplicated — see [`ludion_scene::FontCharset::resolve`]).
///
/// # Errors
/// `L1252` when the bytes are not a single TrueType/OpenType face, have no usable `cmap`, or hold a
/// glyph outline that cannot be read or that flattens past [`MAX_GLYPH_SEGMENTS`]. Never panics on
/// malformed input.
pub fn read_face(bytes: &[u8], chars: &[char]) -> Result<FaceOutlines, Vec<Diagnostic>> {
    let font = match FileRef::new(bytes) {
        Ok(FileRef::Font(font)) => font,
        Ok(FileRef::Collection(_)) => {
            return Err(file_diag(
                "the file is a font collection (.ttc), not a single face",
            ));
        }
        Err(e) => return Err(file_diag(format!("the file is not a font face: {e}"))),
    };
    read_font(&font, chars, MAX_GLYPH_SEGMENTS)
}

fn read_font(
    font: &FontRef<'_>,
    chars: &[char],
    limit: usize,
) -> Result<FaceOutlines, Vec<Diagnostic>> {
    let charmap = font.charmap();
    if charmap.mappings().next().is_none() {
        return Err(file_diag("the face has no usable `cmap` table"));
    }
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    if metrics.units_per_em == 0 {
        return Err(file_diag("the face declares zero units per em"));
    }
    let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let outlines = font.outline_glyphs();
    let read_glyph = |gid: GlyphId, ch: Option<char>| -> Result<GlyphOutline, Vec<Diagnostic>> {
        let advance = glyph_metrics.advance_width(gid).map_or(0, round_i32);
        let contours = outline::extract(&outlines, gid, limit).map_err(|e| match e {
            outline::ExtractError::Unreadable(e) => file_diag(format!(
                "glyph {} has an unreadable outline: {e}",
                gid.to_u32()
            )),
            outline::ExtractError::TooComplex => {
                let who = match ch {
                    Some(c) => format!("{} (U+{:04X} {c:?})", gid.to_u32(), c as u32),
                    None => format!("{} (.notdef)", gid.to_u32()),
                };
                vec![
                    Diagnostic::new(
                        code::FONT_FILE,
                        format!(
                            "glyph {who} flattens to more than {limit} segments, the most a bake draws"
                        ),
                    )
                    .with_path(if ch.is_some() { "/charset" } else { "/file" })
                    .with_help(
                        "remove the character from `charset`, or use a face with simpler outlines",
                    ),
                ]
            }
        })?;
        Ok(GlyphOutline {
            advance,
            bounds: outline::bounds(&contours),
            contours,
        })
    };

    let glyph_count = font
        .maxp()
        .map_err(|e| file_diag(format!("the face has no usable `maxp` table: {e}")))?
        .num_glyphs()
        .into();
    let notdef = read_glyph(GlyphId::NOTDEF, None)?;
    let mut glyphs = Vec::new();
    let mut missing = Vec::new();
    let mut mapped: Vec<(char, GlyphId)> = Vec::new();
    for &ch in chars {
        match charmap.map(ch) {
            Some(gid) if outline::is_real_glyph(gid, glyph_count) => {
                glyphs.push((ch, read_glyph(gid, Some(ch))?));
                mapped.push((ch, gid));
            }
            _ => missing.push(ch),
        }
    }

    let advances: Vec<i32> = glyphs.iter().map(|(_, g)| g.advance).collect();
    let kerning = kerning::pairs(font, &mapped, &advances);
    Ok(FaceOutlines {
        units_per_em: u32::from(metrics.units_per_em),
        ascender: round_i32(metrics.ascent),
        descender: round_i32(metrics.descent),
        line_gap: round_i32(metrics.leading),
        notdef,
        glyphs,
        kerning,
        missing,
    })
}

/// A baked face: the document, the atlas image and the exact PNG bytes to commit.
#[derive(Clone, Debug, PartialEq)]
pub struct FontBake {
    /// The derived `kind: font` document.
    pub doc: FontDocument,
    /// The atlas pixels: RGBA8, RGB 255, the glyph value in alpha.
    ///
    /// [`Image::hash`] is the content hash of [`Self::png`].
    pub image: Image,
    /// The encoded PNG.
    pub png: Vec<u8>,
}

/// Bakes `font_bytes` per `source` into a `kind: font` document and its atlas PNG.
///
/// `generated_from` is the source's path relative to the written document; the atlas image is
/// named `{stem}.png`. A pure function of its arguments: the same inputs give identical bytes on
/// every machine.
///
/// # Errors
/// `L1252` for an unreadable face or a glyph past [`MAX_GLYPH_SEGMENTS`], `L1251` when a glyph or the
/// atlas would exceed 4096 on a side or the charset has nothing to draw.
pub fn bake(
    source: &FontSourceDocument,
    font_bytes: &[u8],
    generated_from: &str,
    stem: &str,
) -> Result<FontBake, Vec<Diagnostic>> {
    let chars = source.charset.resolve();
    let face = read_face(font_bytes, &chars)?;
    bake_outlines(source, &face, generated_from, stem)
}

fn structure_diag(message: String, path: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::new(code::FONT_SOURCE_DOCUMENT_STRUCTURE, message).with_path(path)]
}

/// One glyph with its texel box, awaiting rasterization.
struct Boxed<'a> {
    tbox: geometry::TexelBox,
    contours: &'a [Vec<[f32; 2]>],
}

/// [`bake`] over an already-read face. Every glyph's texel box is sized and packed before any
/// pixel is drawn, so the raster work is bounded by the atlas limit.
fn bake_outlines<'a>(
    source: &FontSourceDocument,
    face: &'a FaceOutlines,
    generated_from: &str,
    stem: &str,
) -> Result<FontBake, Vec<Diagnostic>> {
    let upem = face.units_per_em;
    let em_px = source.em_px;
    let sdf = source.mode == FontMode::Sdf;
    let spread = source.spread_px.unwrap_or(1).max(1);
    let pad = if sdf { spread } else { 0 };
    let s = upem as f32 / em_px as f32;

    let boxed = |g: &'a GlyphOutline, label: &dyn Fn() -> String| {
        let Some(bounds) = g.bounds else {
            return Ok(None);
        };
        match geometry::TexelBox::new(bounds, upem, em_px, pad) {
            Ok(tbox) => Ok(tbox.map(|tbox| Boxed {
                tbox,
                contours: &g.contours,
            })),
            Err([w, h]) => Err(vec![
                Diagnostic::new(
                    code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                    format!(
                        "glyph {} would be {w}x{h} texels, over {} a side",
                        label(),
                        pack::MAX_SIDE
                    ),
                )
                .with_path("/em_px")
                .with_help(
                    "the face's metrics are out of range for this `em_px`; lower `em_px`, or the font file is malformed",
                ),
            ]),
        }
    };
    // Ascending code point, `.notdef` last.
    let boxed_glyphs = face
        .glyphs
        .iter()
        .map(|(ch, g)| boxed(g, &|| format!("U+{:04X}", *ch as u32)))
        .collect::<Result<Vec<_>, _>>()?;
    let boxed_notdef = boxed(&face.notdef, &|| ".notdef".to_string())?;
    let entries: Vec<&Boxed> = boxed_glyphs
        .iter()
        .flatten()
        .chain(boxed_notdef.as_ref())
        .collect();
    if entries.is_empty() {
        return Err(structure_diag(
            "nothing in the charset has ink to bake".to_string(),
            "/charset",
        ));
    }

    let sizes: Vec<[u32; 2]> = entries
        .iter()
        .map(|e| [e.tbox.width(), e.tbox.height()])
        .collect();
    let packing = pack::pack(&sizes)?;
    let alphas: Vec<Vec<u8>> = entries
        .iter()
        .map(|e| {
            if sdf {
                sdf::sdf_glyph(e.contours, &e.tbox, s, spread)
            } else {
                coverage::coverage_glyph(e.contours, &e.tbox, em_px, upem)
            }
        })
        .collect();
    let glyph_refs: Vec<(&[u32; 2], &[u8])> = sizes
        .iter()
        .zip(&alphas)
        .map(|(size, alpha)| (size, alpha.as_slice()))
        .collect();
    let rgba = pack::compose(&packing, &glyph_refs);
    let png = ludion_assets::encode_png(packing.width, packing.height, &rgba).map_err(|e| {
        structure_diag(
            format!("the atlas could not be encoded as a PNG: {e}"),
            "/em_px",
        )
    })?;

    let mut next = 0;
    let mut place = |inked: &Option<Boxed<'_>>| -> (Option<[i32; 4]>, Option<[u32; 4]>) {
        let Some(inked) = inked else {
            return (None, None);
        };
        let [x, y] = packing.origins[next];
        next += 1;
        let rect = [x, y, inked.tbox.width(), inked.tbox.height()];
        (Some(inked.tbox.font_bounds(upem, em_px)), Some(rect))
    };
    let glyphs: Vec<FontGlyph> = face
        .glyphs
        .iter()
        .zip(&boxed_glyphs)
        .map(|((ch, g), inked)| {
            let (bounds, rect) = place(inked);
            FontGlyph {
                ch: *ch,
                advance: g.advance,
                bounds,
                rect,
            }
        })
        .collect();
    let (nd_bounds, nd_rect) = place(&boxed_notdef);

    let ascender = match source.line_height {
        None => face.ascender,
        Some(h) => {
            let ascender =
                i32::try_from(h).expect("validated ≤ 65535") + face.descender - face.line_gap;
            if ascender <= 0 {
                return Err(vec![
                    Diagnostic::new(
                        code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                        format!(
                            "`line_height` {h} leaves no room above the baseline: this face's descender is {} and line gap {}, so it needs more than {}",
                            face.descender,
                            face.line_gap,
                            face.line_gap - face.descender
                        ),
                    )
                    .with_path("/line_height"),
                ]);
            }
            ascender
        }
    };
    let doc = FontDocument {
        generated_from: Some(generated_from.to_string()),
        image: format!("{stem}.png"),
        mode: source.mode,
        units_per_em: upem,
        ascender,
        descender: face.descender,
        line_gap: face.line_gap,
        atlas: FontAtlas {
            width: packing.width,
            height: packing.height,
            em_px,
            spread_px: sdf.then_some(spread),
        },
        notdef: FontNotdef {
            advance: face.notdef.advance,
            bounds: nd_bounds,
            rect: nd_rect,
        },
        glyphs,
        aliases: Vec::new(),
        kerning: face.kerning.clone(),
        missing: face.missing.clone(),
    };
    let problems = ludion_scene::validate_font(&doc);
    if !problems.is_empty() {
        return Err(problems);
    }
    let image = Image {
        width: packing.width,
        height: packing.height,
        rgba,
        hash: ludion_assets::content_hash(&png),
    };
    Ok(FontBake { doc, image, png })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_scene::{CharsetRange, FontCharset};

    fn blank() -> GlyphOutline {
        GlyphOutline {
            advance: 500,
            bounds: None,
            contours: Vec::new(),
        }
    }

    fn inter() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/gallery/fonts/Inter-Regular.ttf"
        ))
        .expect("a committed test face")
    }

    #[test]
    fn a_glyph_past_the_segment_limit_is_an_l1252_naming_it() {
        let bytes = inter();
        let font = FontRef::new(&bytes).expect("Inter");
        let full = read_font(&font, &['@'], MAX_GLYPH_SEGMENTS).expect("under the cap");
        let points = |g: &GlyphOutline| g.contours.iter().map(Vec::len).sum::<usize>();
        let limit = points(&full.glyphs[0].1) - 1;
        assert!(points(&full.notdef) <= limit, "only `@` is over the limit");
        let errs = read_font(&font, &['@'], limit).expect_err("over the limit");
        assert_eq!(errs[0].code, code::FONT_FILE);
        assert_eq!(errs[0].path.as_deref(), Some("/charset"));
        assert!(errs[0].message.contains("U+0040"));
        assert!(errs[0].message.contains(&limit.to_string()));
        assert!(read_font(&font, &['@'], MAX_GLYPH_SEGMENTS).is_ok());
    }

    #[test]
    fn a_notdef_past_the_segment_limit_points_at_the_file() {
        let bytes = inter();
        let font = FontRef::new(&bytes).expect("Inter");
        let errs = read_font(&font, &[], 3).expect_err("over the limit");
        assert_eq!(errs[0].path.as_deref(), Some("/file"));
        assert!(errs[0].message.contains(".notdef"));
    }

    #[test]
    fn a_face_with_no_ink_anywhere_is_an_l1251_at_the_charset() {
        let face = FaceOutlines {
            units_per_em: 1000,
            ascender: 800,
            descender: -200,
            line_gap: 0,
            notdef: blank(),
            glyphs: vec![(' ', blank())],
            kerning: Vec::new(),
            missing: Vec::new(),
        };
        let source = FontSourceDocument {
            name: None,
            file: String::new(),
            mode: FontMode::Bitmap,
            em_px: 8,
            spread_px: None,
            line_height: None,
            charset: FontCharset {
                ranges: vec![CharsetRange::Ascii],
                chars: String::new(),
            },
        };
        let errs = bake_outlines(&source, &face, "s.yaml", "s").expect_err("nothing to bake");
        assert_eq!(errs[0].code, code::FONT_SOURCE_DOCUMENT_STRUCTURE);
        assert_eq!(errs[0].path.as_deref(), Some("/charset"));
    }
}
