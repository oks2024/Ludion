//! The deterministic atlas packer: baked frames and loose PNGs in, one packed image plus the
//! derived `kind: atlas` document out.
//!
//! Shelf packing over a fixed sort, chosen because it is *explainable*: a region's position follows
//! from its size and its name and nothing else, so a reviewer can read a layout change out of a
//! diff. A cleverer packer that saved texels but reshuffled on an unrelated edit would re-bake every
//! committed atlas for nothing.

use std::collections::BTreeMap;

use ludion_assets::Image;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math;
use ludion_core::suggest;
use ludion_scene::{AtlasDocument, AtlasRegion, AtlasSourceDocument};

use crate::image::FrameImage;
use crate::structure_error;

/// The gutter, in texels, around every packed region on all four sides.
///
/// Filled by extruding the region's own edge texels rather than left transparent, so a `linear`
/// sampler reaching half a texel past a region's edge finds that region's colour and never a
/// neighbour's. `rect` in the written document excludes it.
const GUTTER: u32 = 1;

/// Both atlas axes are rounded up to a multiple of this, which keeps every row's byte stride aligned
/// for the upload path whatever the packing produced. Deliberately *not* a power-of-two requirement:
/// that would waste up to half an atlas to satisfy a constraint no target still has.
const EXTENT_ALIGN: u32 = 4;

/// One input to the packer: everything one source file contributes.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasInput {
    /// The region-name prefix: a sprite document's `name` (or its file stem when it has none), or a
    /// PNG's file stem. The caller resolves it, because only the caller knows the file names.
    pub base: String,
    /// The frames this input contributes, in the order they should mint region names.
    pub frames: Vec<AtlasInputFrame>,
}

/// One frame of an [`AtlasInput`].
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasInputFrame {
    /// `Some(frame)` for a sprite document, giving the region name `base/frame`; `None` for a loose
    /// PNG, whose whole image is the one region named `base`.
    pub name: Option<String>,
    /// The pixels, in the bake's working format.
    pub image: FrameImage,
    /// The pivot in texels from the frame's own top-left, carried onto the region unchanged.
    pub pivot: [f64; 2],
}

/// A packed atlas: the image, the exact bytes to commit, and the document that indexes them.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasBake {
    /// The packed colour map: premultiplied alpha, sRGB-encoded RGB.
    ///
    /// [`Image::hash`] is the content hash of [`Self::png`], so a freshly packed atlas and the
    /// committed file loaded back off disk share one identity — which is what lets the drift gate
    /// compare them at all.
    pub image: Image,
    /// The encoded PNG: the bytes `generate-atlas` writes and `--check` byte-compares.
    pub png: Vec<u8>,
    /// The derived document naming every region's rect and pivot.
    pub doc: AtlasDocument,
}

/// Packs `inputs` into one atlas, deriving the `kind: atlas` document from `source`.
///
/// `generated_from` is the atlas source's path relative to the written document — the provenance
/// marker the drift gate finds the pair through — and the packed image's name is derived from
/// `source.name`, falling back to `generated_from`'s own stem.
///
/// Layout is fully determined by the inputs: regions sort by height descending, then width
/// descending, then name ascending; the width is the smallest multiple of four that holds both the
/// widest padded region and a square of the total padded area; regions fill shelves left to right,
/// each shelf as tall as its first (and therefore tallest) region.
///
/// # Errors
/// `L1153` when two inputs mint the same region name, when a frame has a zero extent, when the
/// inputs produce no regions at all, when a flipbook names a region no input produced (with a
/// nearest-match suggestion), or when the packed atlas is over `ludion_assets::MAX_PNG_SIDE` a side
/// (checked only once every other defect is fixed). Every other defect found is reported, not just
/// the first.
pub fn pack(
    source: &AtlasSourceDocument,
    generated_from: &str,
    inputs: &[AtlasInput],
) -> Result<AtlasBake, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let entries = collect(inputs, &mut diagnostics);

    if entries.is_empty() && diagnostics.is_empty() {
        diagnostics.push(structure_error(
            "the atlas source produced no regions; an atlas with no regions can never be sampled",
            "/inputs",
            "list at least one sprite document or PNG under `inputs:`",
        ));
    }
    check_flipbooks(source, &entries, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let layout = layout(&entries);
    let limit = ludion_assets::MAX_PNG_SIDE;
    if layout.width > limit || layout.height > limit {
        return Err(vec![structure_error(
            format!(
                "the {} regions pack into a {}x{} atlas, over {limit} a side — the largest \
                 texture the engine loads",
                entries.len(),
                layout.width,
                layout.height
            ),
            "/inputs",
            "split the inputs across two atlas sources, or shrink the largest frames",
        )]);
    }
    let (image, png) = compose(&layout, &entries)?;
    let stem = output_stem(source, generated_from);

    let regions = layout
        .placements
        .iter()
        .map(|placement| {
            let entry = &entries[placement.entry];
            (
                entry.name.clone(),
                AtlasRegion {
                    rect: [
                        placement.x + GUTTER,
                        placement.y + GUTTER,
                        entry.frame.image.width,
                        entry.frame.image.height,
                    ],
                    pivot: entry.frame.pivot,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    Ok(AtlasBake {
        image,
        png,
        doc: AtlasDocument {
            generated_from: generated_from.to_string(),
            pixels_per_unit: source.pixels_per_unit,
            color_map: format!("{stem}.png"),
            filter: source.filter,
            regions,
            flipbooks: source.flipbooks.clone(),
        },
    })
}

/// One region awaiting placement: its minted name and the frame it came from.
struct Entry<'a> {
    name: String,
    frame: &'a AtlasInputFrame,
}

/// Where one entry's *padded* box sits in the atlas. The written `rect` is this inset by [`GUTTER`].
struct Placement {
    entry: usize,
    x: u32,
    y: u32,
}

/// Every entry's placement plus the aligned atlas extent they fill.
struct Layout {
    placements: Vec<Placement>,
    width: u32,
    height: u32,
}

/// Mints every region name and rejects the two defects only the inputs can show: a duplicate name
/// and a frame with no texels.
fn collect<'a>(inputs: &'a [AtlasInput], diagnostics: &mut Vec<Diagnostic>) -> Vec<Entry<'a>> {
    let mut entries: Vec<Entry<'a>> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        for frame in &input.frames {
            let name = match &frame.name {
                Some(frame_name) => format!("{}/{}", input.base, frame_name),
                None => input.base.clone(),
            };
            if entries.iter().any(|entry| entry.name == name) {
                diagnostics.push(structure_error(
                    format!("two inputs both produce the region `{name}`"),
                    format!("/inputs/{index}"),
                    "region names are `<sprite document>/<frame>` or a PNG's file stem; rename \
                     one of the two inputs, or give the sprite document a distinct `name:`",
                ));
                continue;
            }
            if frame.image.width == 0 || frame.image.height == 0 {
                diagnostics.push(structure_error(
                    format!(
                        "the region `{name}` is {}x{} texels; a region covers at least one texel \
                         on each axis",
                        frame.image.width, frame.image.height
                    ),
                    format!("/inputs/{index}"),
                    "check the input file is not an empty image",
                ));
                continue;
            }
            entries.push(Entry { name, frame });
        }
    }
    entries
}

/// Checks that every flipbook frame names a region the inputs actually produced — the one
/// cross-check an atlas source cannot do for itself, because it has not read its inputs.
fn check_flipbooks(
    source: &AtlasSourceDocument,
    entries: &[Entry<'_>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (book_index, flipbook) in source.flipbooks.iter().enumerate() {
        for (frame_index, frame) in flipbook.frames.iter().enumerate() {
            if entries.iter().any(|entry| entry.name == *frame) {
                continue;
            }
            let names = entries.iter().map(|entry| entry.name.as_str());
            let help = match suggest::nearest(frame, names) {
                Some(candidate) => format!("nearest region is `{candidate}`"),
                None => format!(
                    "flipbook `{}` names regions of this atlas; a region is \
                     `<sprite document>/<frame>` or a PNG's file stem",
                    flipbook.name
                ),
            };
            diagnostics.push(structure_error(
                format!(
                    "flipbook `{}` frame `{frame}` names no region these inputs produce",
                    flipbook.name
                ),
                format!("/flipbooks/{book_index}/frames/{frame_index}"),
                help,
            ));
        }
    }
}

/// Places every entry on shelves, in the packer's fixed order, and returns the atlas extent.
fn layout(entries: &[Entry<'_>]) -> Layout {
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| {
        let (left, right) = (&entries[a], &entries[b]);
        right
            .frame
            .image
            .height
            .cmp(&left.frame.image.height)
            .then(right.frame.image.width.cmp(&left.frame.image.width))
            // Names are unique by construction, so this tie-break makes the order total: no two
            // entries can compare equal, and the sort's stability never has to be relied on.
            .then(left.name.cmp(&right.name))
    });

    let width = atlas_width(entries);
    let sizes: Vec<[u32; 2]> = order
        .iter()
        .map(|&entry| {
            let (padded_width, padded_height) = padded(entries[entry].frame);
            [padded_width, padded_height]
        })
        .collect();
    let shelves = ludion_assets::shelf::place(&sizes, width);
    let placements = order
        .iter()
        .zip(&shelves.origins)
        .map(|(&entry, origin)| Placement {
            entry,
            x: origin[0],
            y: origin[1],
        })
        .collect();
    Layout {
        placements,
        width,
        height: align_up(shelves.height),
    }
}

/// The padded extent of one frame: its own size plus a [`GUTTER`] on every side.
fn padded(frame: &AtlasInputFrame) -> (u32, u32) {
    (
        frame.image.width + 2 * GUTTER,
        frame.image.height + 2 * GUTTER,
    )
}

/// The atlas width: the smallest multiple of [`EXTENT_ALIGN`] that holds both the widest padded
/// region and a square of the total padded area.
///
/// The square-of-the-area term is what keeps an atlas roughly square as content grows, instead of
/// one long strip whose height a texture limit eventually refuses.
fn atlas_width(entries: &[Entry<'_>]) -> u32 {
    let mut widest = 0;
    let mut area: u64 = 0;
    for entry in entries {
        let (width, height) = padded(entry.frame);
        widest = widest.max(width);
        area += u64::from(width) * u64::from(height);
    }
    align_up(widest.max(ceil_sqrt(area) as u32))
}

/// `ceil(sqrt(value))`, exactly.
///
/// The floating-point square root only seeds the answer; the loops below correct it, so the result
/// is an integer fact rather than a rounding one and cannot differ between platforms.
fn ceil_sqrt(value: u64) -> u64 {
    let mut root = math::f64::sqrt(value as f64) as u64;
    while root.saturating_mul(root) < value {
        root += 1;
    }
    while root > 0 && (root - 1) * (root - 1) >= value {
        root -= 1;
    }
    root
}

/// Rounds `value` up to a multiple of [`EXTENT_ALIGN`], never below one alignment unit.
fn align_up(value: u32) -> u32 {
    let aligned = value.div_ceil(EXTENT_ALIGN) * EXTENT_ALIGN;
    aligned.max(EXTENT_ALIGN)
}

/// Blits every placed region — and its extruded gutter — into one RGBA8 buffer, then encodes it.
fn compose(layout: &Layout, entries: &[Entry<'_>]) -> Result<(Image, Vec<u8>), Vec<Diagnostic>> {
    let Layout {
        placements,
        width,
        height,
    } = layout;
    let (width, height) = (*width, *height);

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for placement in placements {
        let frame = entries[placement.entry].frame;
        let source = frame.image.to_srgb8_premultiplied();
        let (padded_width, padded_height) = padded(frame);
        for y in 0..padded_height {
            for x in 0..padded_width {
                // Clamping the read into the region *is* the extrude: the gutter ring and its four
                // corners all resolve to the nearest real texel.
                let sx = (x as i32 - GUTTER as i32).clamp(0, frame.image.width as i32 - 1) as u32;
                let sy = (y as i32 - GUTTER as i32).clamp(0, frame.image.height as i32 - 1) as u32;
                let from = ((sy * frame.image.width + sx) as usize) * 4;
                let to = (((placement.y + y) * width + placement.x + x) as usize) * 4;
                rgba[to..to + 4].copy_from_slice(&source.rgba[from..from + 4]);
            }
        }
    }

    let png = ludion_assets::encode_png(width, height, &rgba).map_err(|error| {
        vec![structure_error(
            format!("the packed atlas could not be encoded as a PNG: {error}"),
            "/inputs",
            "the packed extent or its pixel buffer was rejected by the encoder; check the input \
             frame sizes",
        )]
    })?;
    let image = Image {
        width,
        height,
        rgba,
        hash: ludion_assets::content_hash(&png),
    };
    Ok((image, png))
}

/// The output file stem: the source's declared `name`, else `generated_from`'s own stem.
///
/// The fallback mirrors what the CLI does for an unnamed source — derive from the file's name —
/// and `generated_from` *is* that file's path, so this crate can honour the rule without doing IO.
fn output_stem(source: &AtlasSourceDocument, generated_from: &str) -> String {
    if let Some(name) = &source.name
        && !name.is_empty()
    {
        return name.clone();
    }
    let file = generated_from
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(generated_from);
    match file.split_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_sqrt_is_exact_around_perfect_squares() {
        assert_eq!(ceil_sqrt(0), 0);
        assert_eq!(ceil_sqrt(1), 1);
        assert_eq!(ceil_sqrt(15), 4);
        assert_eq!(ceil_sqrt(16), 4);
        assert_eq!(ceil_sqrt(17), 5);
        assert_eq!(ceil_sqrt(1_000_000), 1000);
        assert_eq!(ceil_sqrt(1_000_001), 1001);
    }

    #[test]
    fn align_up_never_returns_zero() {
        assert_eq!(align_up(0), 4);
        assert_eq!(align_up(1), 4);
        assert_eq!(align_up(4), 4);
        assert_eq!(align_up(5), 8);
    }

    #[test]
    fn the_output_stem_falls_back_to_the_source_path() {
        let mut source = AtlasSourceDocument::default();
        assert_eq!(output_stem(&source, "art/ship.atlas-source.yaml"), "ship");
        assert_eq!(output_stem(&source, "ship.atlas-source.yaml"), "ship");
        source.name = Some("fleet".to_string());
        assert_eq!(output_stem(&source, "art/ship.atlas-source.yaml"), "fleet");
    }
}
