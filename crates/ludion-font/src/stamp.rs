//! Burning a label into a CPU canvas from the embedded 8×8 face.

use crate::FontFace;

/// Side of one embedded glyph cell, in texels.
pub const LABEL_CELL: u32 = 8;
/// Pen advance per character, in texels: the cell plus one texel of tracking.
pub const LABEL_PITCH: u32 = 9;

/// Stamps `text` into the RGBA8 `canvas` (`size` pixels, row-major) with its top-left at `at`.
///
/// `atlas` is the decoded [`EMBEDDED_PNG`](crate::EMBEDDED_PNG). Every texel of a glyph's rect with
/// alpha ≥ 128 becomes a `scale`×`scale` block of `color`, clipped to the canvas. Lowercase draws as
/// its uppercase twin; a character the face lacks draws blank, and the pen advances
/// [`LABEL_PITCH`]` * scale` for every character either way.
///
/// # Panics
/// If `atlas` is not the embedded face's atlas size.
///
/// # Example
/// ```
/// use ludion_font::{EMBEDDED_PNG, LABEL_CELL, stamp_label};
///
/// let (_, _, atlas) = ludion_assets::decode_png(EMBEDDED_PNG, "default.png").unwrap();
/// let mut canvas = vec![0u8; 16 * 16 * 4];
/// stamp_label(&atlas, &mut canvas, [16, 16], [2, 2], "A", 1, [255, 0, 0, 255]);
/// let inked = canvas.chunks(4).filter(|px| px[3] != 0).count();
/// assert!(inked > 0 && inked < (LABEL_CELL * LABEL_CELL) as usize);
/// ```
pub fn stamp_label(
    atlas: &[u8],
    canvas: &mut [u8],
    size: [u32; 2],
    at: [u32; 2],
    text: &str,
    scale: u32,
    color: [u8; 4],
) {
    let face = FontFace::embedded();
    let atlas_doc = &face.document().atlas;
    let atlas_w = atlas_doc.width as usize;
    assert_eq!(
        atlas.len(),
        atlas_w * atlas_doc.height as usize * 4,
        "stamp_label wants the decoded embedded atlas ({}×{} RGBA8)",
        atlas_doc.width,
        atlas_doc.height
    );
    let mut pen = at[0] as u64;
    for ch in text.chars() {
        if let Some(rect) = face.glyph(ch).and_then(|glyph| glyph.rect) {
            for row in 0..rect[3] {
                for col in 0..rect[2] {
                    let src = ((rect[1] + row) as usize * atlas_w + (rect[0] + col) as usize) * 4;
                    if atlas[src + 3] < 128 {
                        continue;
                    }
                    let x0 = pen + u64::from(col * scale);
                    let y0 = u64::from(at[1]) + u64::from(row * scale);
                    for dy in 0..u64::from(scale) {
                        for dx in 0..u64::from(scale) {
                            let (x, y) = (x0 + dx, y0 + dy);
                            if x < u64::from(size[0]) && y < u64::from(size[1]) {
                                let dst = (y as usize * size[0] as usize + x as usize) * 4;
                                canvas[dst..dst + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
        pen += u64::from(LABEL_PITCH * scale);
    }
}
