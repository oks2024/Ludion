//! Pair kerning through the face's own GPOS, via a two-character shape per ordered pair.

use harfrust::{Direction, ShapeOptions, ShaperData, UnicodeBuffer, script};
use skrifa::{FontRef, GlyphId};

/// `(left, right, adjust)` for every ordered pair of `mapped` chars whose two-char shape keeps
/// both glyphs and moves the first one's advance; `advances[i]` is the hmtx advance of
/// `mapped[i]`. Direction and script are set explicitly (never guessed) so every pair takes one
/// path. Pairs that ligate, substitute or drop a glyph are skipped: they are not pair kerning.
pub(super) fn pairs(
    font: &FontRef<'_>,
    mapped: &[(char, GlyphId)],
    advances: &[i32],
) -> Vec<(char, char, i32)> {
    let data = ShaperData::new(font);
    let shaper = data.shaper(font).build();
    let mut out = Vec::new();
    let mut buffer = UnicodeBuffer::new();
    let mut text = String::with_capacity(8);
    for (li, &(left, left_gid)) in mapped.iter().enumerate() {
        for &(right, right_gid) in mapped {
            text.clear();
            text.push(left);
            text.push(right);
            buffer.push_str(&text);
            buffer.set_direction(Direction::LeftToRight);
            buffer.set_script(script::LATIN);
            let shaped = shaper.shape(buffer, ShapeOptions::new());
            let infos = shaped.glyph_infos();
            if let [a, b] = infos
                && a.glyph_id == left_gid.to_u32()
                && b.glyph_id == right_gid.to_u32()
            {
                let adjust = shaped.glyph_positions()[0].x_advance - advances[li];
                if adjust != 0 {
                    out.push((left, right, adjust));
                }
            }
            buffer = shaped.clear();
        }
    }
    out
}
