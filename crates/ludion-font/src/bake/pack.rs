//! Deterministic shelf packing of glyph rects and the RGBA atlas compose.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// Transparent texels around each rect.
const GUTTER: u32 = 1;
/// Atlas extents round up to this.
const EXTENT_ALIGN: u32 = 4;
/// The largest atlas side the bake accepts.
pub(super) const MAX_SIDE: u32 = 4096;
// Stricter than the loader's limit on purpose: it bounds raster work before any pixel is drawn.
const _: () = assert!(MAX_SIDE <= ludion_assets::MAX_PNG_SIDE);

/// Where each entry landed: the rect's top-left (gutter already added) and the atlas extent.
pub(super) struct Packing {
    pub width: u32,
    pub height: u32,
    pub origins: Vec<[u32; 2]>,
}

fn align_up(v: u32) -> u32 {
    v.div_ceil(EXTENT_ALIGN) * EXTENT_ALIGN
}

/// Packs `sizes` (`[w, h]` per entry) in the given order: shelves fill left to right, a new shelf
/// opens when the next rect does not fit, and a shelf is as tall as its tallest rect.
pub(super) fn pack(sizes: &[[u32; 2]]) -> Result<Packing, Vec<Diagnostic>> {
    let padded = |s: &[u32; 2]| (s[0] + 2 * GUTTER, s[1] + 2 * GUTTER);
    let widest = sizes.iter().map(|s| padded(s).0).max().unwrap_or(0);
    let area: u64 = sizes
        .iter()
        .map(|s| {
            let (w, h) = padded(s);
            u64::from(w) * u64::from(h)
        })
        .sum();
    let square = u32::try_from(area.isqrt()).unwrap_or(u32::MAX);
    let width = align_up(widest.max(square));

    let boxes: Vec<[u32; 2]> = sizes
        .iter()
        .map(|s| {
            let (w, h) = padded(s);
            [w, h]
        })
        .collect();
    let shelves = ludion_assets::shelf::place(&boxes, width);
    let origins = shelves
        .origins
        .iter()
        .map(|o| [o[0] + GUTTER, o[1] + GUTTER])
        .collect();
    let height = align_up(shelves.height);
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(vec![
            Diagnostic::new(
                code::FONT_SOURCE_DOCUMENT_STRUCTURE,
                format!("the atlas would be {width}x{height}, over {MAX_SIDE}"),
            )
            .with_path("/em_px")
            .with_help("lower `em_px` or narrow `charset`"),
        ]);
    }
    Ok(Packing {
        width,
        height,
        origins,
    })
}

/// Composes the atlas pixels: RGB 255, the glyph value in alpha, everything else transparent.
pub(super) fn compose(packing: &Packing, glyphs: &[(&[u32; 2], &[u8])]) -> Vec<u8> {
    let (width, height) = (packing.width as usize, packing.height as usize);
    let mut rgba = vec![255u8; width * height * 4];
    for px in rgba.chunks_exact_mut(4) {
        px[3] = 0;
    }
    for (origin, (size, alpha)) in packing.origins.iter().zip(glyphs) {
        let w = size[0] as usize;
        for (row, line) in alpha.chunks(w).enumerate() {
            let at = ((origin[1] as usize + row) * width + origin[0] as usize) * 4;
            for (col, &a) in line.iter().enumerate() {
                rgba[at + col * 4 + 3] = a;
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_fit_without_overlap_and_the_width_is_aligned() {
        let sizes: Vec<[u32; 2]> = (0..40).map(|i| [5 + i % 7, 9 + i % 5]).collect();
        let p = pack(&sizes).unwrap();
        assert_eq!(p.width % 4, 0);
        assert_eq!(p.height % 4, 0);
        let rects: Vec<[u32; 4]> = p
            .origins
            .iter()
            .zip(&sizes)
            .map(|(o, s)| [o[0], o[1], s[0], s[1]])
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a[0] + a[2] <= p.width && a[1] + a[3] <= p.height);
            for b in &rects[i + 1..] {
                let apart = a[0] + a[2] <= b[0]
                    || b[0] + b[2] <= a[0]
                    || a[1] + a[3] <= b[1]
                    || b[1] + b[3] <= a[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn an_oversized_atlas_is_l1251() {
        let sizes = vec![[4000u32, 4000u32]; 2];
        let err = pack(&sizes).err().expect("over 4096");
        assert_eq!(err[0].code, code::FONT_SOURCE_DOCUMENT_STRUCTURE);
        assert_eq!(err[0].path.as_deref(), Some("/em_px"));
    }
}
