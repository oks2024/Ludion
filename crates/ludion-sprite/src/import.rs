//! PNG → `kind: sprite`: the inverse of the bake's painting step, for art authored elsewhere.
//!
//! The point is *editability*. A sprite document is the form an agent can read, diff and change one
//! glyph of; a PNG is not. So the import is lossless whenever the image's colours fit the glyph
//! budget, and when they do not it says so and refuses, rather than quietly dropping colours — the
//! author asks for `--quantize N` and gets the loss reported back.

use std::collections::BTreeMap;

use ludion_assets::Image;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{
    SPRITE_EMPTY_GLYPH, SPRITE_GLYPH_BUDGET, SPRITE_MAX_SIZE, SpriteDocument, SpriteFrame,
    SpriteFrameGrid, SpritePaletteEntry,
};

use crate::structure_error;
use ludion_core::color::srgb_to_linear;

/// The glyphs an import hands out, in order: the most-used colour takes the first.
///
/// Exactly the 93 printable-ASCII characters a palette may name — `0x21..=0x7E` minus the reserved
/// [`SPRITE_EMPTY_GLYPH`] — ordered heaviest-looking first, so the grid of a document read as plain
/// text is a rough greyscale of the sprite it draws. Every character appears exactly once; a test
/// pins both the length and the uniqueness, because a duplicate would silently merge two colours.
pub const IMPORT_GLYPHS: &str = "#@%&$0OQ8BWMXZ*+=~^-:;!?/\\|<>()[]{}abcdefghijklmnopqrstuvwxyzACDEFGHIJKLNPRSTUVY12345679'\",_`";

/// The name of the single frame an import writes. One image is one pose; a second pose is a second
/// import into a second document.
const IMPORT_FRAME: &str = "main";

/// What an import's `--quantize` pass cost, when it merged anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuantizeReport {
    /// How many distinct RGBA8 colours the source image held.
    pub colors_before: usize,
    /// How many distinct colours the written palette holds.
    pub colors_after: usize,
    /// The largest per-channel difference any texel suffered, in 8-bit units. The number an author
    /// judges the merge by: single digits are invisible, tens are a different sprite.
    pub max_channel_error: u8,
}

/// An imported sprite document, plus the loss report when colours had to be merged.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedSprite {
    /// The document, ready to emit with [`ludion_scene::emit_sprite`].
    pub doc: SpriteDocument,
    /// `Some` only when quantization actually merged colours; a `--quantize N` run over an image
    /// that already fits `N` is lossless and reports nothing.
    pub quantized: Option<QuantizeReport>,
}

/// Turns a decoded PNG into an editable sprite document.
///
/// Every fully transparent texel becomes [`SPRITE_EMPTY_GLYPH`]; every distinct remaining RGBA8
/// value becomes one palette entry, sorted by how often it occurs so the busiest colour takes the
/// first (heaviest) glyph. Palette colours are sRGB-decoded to linear `f64`, which is exactly what
/// [`crate::bake()`] re-encodes, so a lossless import round-trips back to the input bytes.
///
/// `quantize` clamps to `1..=SPRITE_GLYPH_BUDGET` and never fails: asking to quantize is asking for
/// a document whatever the cost, and the cost comes back in [`ImportedSprite::quantized`].
///
/// # Errors
/// - `L4014` when the image holds more distinct colours than there are glyphs and `quantize` is
///   `None`. The message names both counts and the `--quantize N` fix.
/// - `L1153` when the image is empty, larger than [`SPRITE_MAX_SIZE`] on an axis, or has no opaque
///   texels at all (a document with an empty palette is not a valid sprite).
pub fn import_png(
    image: &Image,
    name: Option<String>,
    quantize: Option<usize>,
) -> Result<ImportedSprite, Diagnostic> {
    check_extent(image)?;

    let mut counts: BTreeMap<[u8; 4], usize> = BTreeMap::new();
    let mut texels: Vec<Option<[u8; 4]>> = Vec::with_capacity(image.rgba.len() / 4);
    for chunk in image.rgba.chunks_exact(4) {
        if chunk[3] == 0 {
            texels.push(None);
            continue;
        }
        let color = [chunk[0], chunk[1], chunk[2], chunk[3]];
        *counts.entry(color).or_insert(0) += 1;
        texels.push(Some(color));
    }

    if counts.is_empty() {
        return Err(structure_error(
            "every texel of the image is fully transparent, so the import has no colour to name",
            "",
            "a sprite document must name at least one palette colour; check the image is not a \
             blank layer",
        ));
    }

    let colors_before = counts.len();
    let (mapping, quantized) = match quantize {
        None if colors_before > SPRITE_GLYPH_BUDGET => {
            return Err(needs_quantize(colors_before));
        }
        None => (identity(&counts), None),
        Some(target) => reduce(&counts, target.clamp(1, SPRITE_GLYPH_BUDGET)),
    };

    let glyphs = assign_glyphs(&counts, &mapping);
    let rows = draw(image.width, image.height, &texels, &mapping, &glyphs);
    let palette = palette(&glyphs);

    Ok(ImportedSprite {
        doc: SpriteDocument {
            name,
            size: [image.width, image.height],
            seed: None,
            palette,
            palette_include: None,
            ramps: Vec::new(),
            frames: vec![SpriteFrame {
                name: IMPORT_FRAME.to_string(),
                pivot: None,
                grid: SpriteFrameGrid::Rows { rows, mirror: None },
            }],
            effects: Vec::new(),
        },
        quantized,
    })
}

fn check_extent(image: &Image) -> Result<(), Diagnostic> {
    for (axis, extent) in [("width", image.width), ("height", image.height)] {
        if extent == 0 || extent > SPRITE_MAX_SIZE {
            return Err(structure_error(
                format!(
                    "the image is {extent} texels {axis}, outside the sprite range \
                     1..={SPRITE_MAX_SIZE}"
                ),
                "/size",
                "a sprite frame is one region of a packed atlas; anything larger than \
                 1024 texels on an axis belongs in its own texture, not an atlas",
            ));
        }
    }
    let expected = (image.width as usize) * (image.height as usize) * 4;
    if image.rgba.len() != expected {
        return Err(structure_error(
            format!(
                "the image declares {}x{} texels but carries {} RGBA bytes, not {expected}",
                image.width,
                image.height,
                image.rgba.len()
            ),
            "",
            "decode the PNG with `ludion_assets::decode_png`, which always yields tightly packed \
             RGBA8",
        ));
    }
    Ok(())
}

/// The `L4014` refusal: too many colours for the glyph budget, and no `--quantize`.
fn needs_quantize(colors: usize) -> Diagnostic {
    Diagnostic::new(
        code::SPRITE_IMPORT_NEEDS_QUANTIZE,
        format!(
            "the image holds {colors} distinct RGBA colours, more than the {SPRITE_GLYPH_BUDGET} \
             glyphs a sprite document has to name them with"
        ),
    )
    .with_help(format!(
        "re-run with `--quantize N` (N up to {SPRITE_GLYPH_BUDGET}) to merge colours down to N and \
         have the loss reported, or reduce the image's palette in your editor first"
    ))
}

/// Every colour maps to itself — the lossless path.
fn identity(counts: &BTreeMap<[u8; 4], usize>) -> BTreeMap<[u8; 4], [u8; 4]> {
    counts.keys().map(|color| (*color, *color)).collect()
}

/// Median-cut in RGBA8 space down to at most `target` boxes.
///
/// Deterministic end to end: boxes are split largest-channel-range first (ties to the lowest channel
/// index, then to the box created earliest), members sort by that channel with the whole colour as
/// tie-break, and the split is at the member median. A box's colour is the rounded per-channel mean
/// of its members. `None` comes back when nothing had to merge.
fn reduce(
    counts: &BTreeMap<[u8; 4], usize>,
    target: usize,
) -> (BTreeMap<[u8; 4], [u8; 4]>, Option<QuantizeReport>) {
    let colors_before = counts.len();
    if colors_before <= target {
        return (identity(counts), None);
    }

    let mut boxes: Vec<Vec<[u8; 4]>> = vec![counts.keys().copied().collect()];
    while boxes.len() < target {
        let Some((chosen, channel)) = widest_box(&boxes) else {
            break;
        };
        let mut members = std::mem::take(&mut boxes[chosen]);
        members.sort_by_key(|color| (color[channel], *color));
        let rest = members.split_off(members.len() / 2);
        boxes[chosen] = members;
        boxes.push(rest);
    }

    let mut mapping = BTreeMap::new();
    let mut max_channel_error = 0u8;
    for members in &boxes {
        let color = mean(members);
        for member in members {
            for channel in 0..4 {
                max_channel_error = max_channel_error.max(member[channel].abs_diff(color[channel]));
            }
            mapping.insert(*member, color);
        }
    }

    let colors_after = mapping
        .values()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    (
        mapping,
        Some(QuantizeReport {
            colors_before,
            colors_after,
            max_channel_error,
        }),
    )
}

/// The splittable box with the largest channel range, and the channel to split it on.
fn widest_box(boxes: &[Vec<[u8; 4]>]) -> Option<(usize, usize)> {
    let mut best: Option<(u8, usize, usize)> = None;
    for (index, members) in boxes.iter().enumerate() {
        if members.len() < 2 {
            continue;
        }
        for channel in 0..4 {
            let mut low = u8::MAX;
            let mut high = u8::MIN;
            for member in members {
                low = low.min(member[channel]);
                high = high.max(member[channel]);
            }
            let range = high - low;
            // Strictly greater keeps the earliest box and the lowest channel index on a tie.
            if best.is_none_or(|(best_range, _, _)| range > best_range) {
                best = Some((range, index, channel));
            }
        }
    }
    best.filter(|(range, _, _)| *range > 0)
        .map(|(_, index, channel)| (index, channel))
}

/// The rounded per-channel mean of a box's members.
fn mean(members: &[[u8; 4]]) -> [u8; 4] {
    let count = members.len() as u32;
    let mut color = [0u8; 4];
    for (channel, slot) in color.iter_mut().enumerate() {
        let sum: u32 = members
            .iter()
            .map(|member| u32::from(member[channel]))
            .sum();
        *slot = ((sum + count / 2) / count) as u8;
    }
    color
}

/// Hands out [`IMPORT_GLYPHS`] to the final colours, busiest first.
///
/// The order is `(count descending, then R, G, B, A ascending)` — a total order, so two colours with
/// the same count always land the same way round.
fn assign_glyphs(
    counts: &BTreeMap<[u8; 4], usize>,
    mapping: &BTreeMap<[u8; 4], [u8; 4]>,
) -> BTreeMap<[u8; 4], char> {
    let mut totals: BTreeMap<[u8; 4], usize> = BTreeMap::new();
    for (source, count) in counts {
        let target = mapping.get(source).copied().unwrap_or(*source);
        *totals.entry(target).or_insert(0) += count;
    }

    let mut ranked: Vec<([u8; 4], usize)> = totals.into_iter().collect();
    ranked.sort_by(|(left_color, left_count), (right_color, right_count)| {
        right_count
            .cmp(left_count)
            .then(left_color.cmp(right_color))
    });
    ranked
        .into_iter()
        .zip(IMPORT_GLYPHS.chars())
        .map(|((color, _), glyph)| (color, glyph))
        .collect()
}

/// The frame's rows: one glyph per texel, `.` where the source was fully transparent.
fn draw(
    width: u32,
    height: u32,
    texels: &[Option<[u8; 4]>],
    mapping: &BTreeMap<[u8; 4], [u8; 4]>,
    glyphs: &BTreeMap<[u8; 4], char>,
) -> Vec<String> {
    (0..height as usize)
        .map(|y| {
            (0..width as usize)
                .map(|x| match texels[y * (width as usize) + x] {
                    None => SPRITE_EMPTY_GLYPH,
                    Some(color) => mapping
                        .get(&color)
                        .and_then(|target| glyphs.get(target))
                        .copied()
                        .unwrap_or(SPRITE_EMPTY_GLYPH),
                })
                .collect()
        })
        .collect()
}

/// The palette entries, sorted by glyph — the canonical order `emit_sprite` writes.
fn palette(glyphs: &BTreeMap<[u8; 4], char>) -> Vec<SpritePaletteEntry> {
    let mut entries: Vec<SpritePaletteEntry> = glyphs
        .iter()
        .map(|(color, glyph)| SpritePaletteEntry {
            glyph: glyph.to_string(),
            color: [
                f64::from(srgb_to_linear(f32::from(color[0]) / 255.0)),
                f64::from(srgb_to_linear(f32::from(color[1]) / 255.0)),
                f64::from(srgb_to_linear(f32::from(color[2]) / 255.0)),
                f64::from(f32::from(color[3]) / 255.0),
            ],
            emissive: None,
        })
        .collect();
    entries.sort_by(|left, right| left.glyph.cmp(&right.glyph));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyph_alphabet_is_the_whole_budget_exactly_once() {
        let glyphs: Vec<char> = IMPORT_GLYPHS.chars().collect();
        assert_eq!(glyphs.len(), SPRITE_GLYPH_BUDGET);

        let unique: std::collections::BTreeSet<char> = glyphs.iter().copied().collect();
        assert_eq!(unique.len(), SPRITE_GLYPH_BUDGET, "a glyph appears twice");

        let expected: std::collections::BTreeSet<char> = (0x21u8..=0x7e)
            .map(char::from)
            .filter(|glyph| *glyph != SPRITE_EMPTY_GLYPH)
            .collect();
        assert_eq!(
            unique, expected,
            "the alphabet is not the palette's own set"
        );
    }

    #[test]
    fn the_mean_of_a_box_rounds() {
        assert_eq!(mean(&[[0, 0, 0, 0], [3, 3, 3, 3]]), [2, 2, 2, 2]);
        assert_eq!(mean(&[[10, 20, 30, 40]]), [10, 20, 30, 40]);
    }
}
