//! The square-cell text paths the renderer used to run per character, frozen here as oracles:
//! `layout().quads()` on the embedded 8×8 face must reproduce them bit-for-bit, so swapping a
//! renderer onto `layout()` moves no pixel.

use ludion_font::{FontFace, YAxis, layout};

const TEXTS: [&str; 6] = [
    "",
    "HELLO, world 0123",
    "a~b\u{e9}c  d",
    "LINE ONE\n\nLINE THREE\n",
    "\n\nX",
    "THE QUICK BROWN FOX: !?.,'\"-/",
];
const SIZES: [f32; 3] = [8.0, 7.3, 13.37 * 1.5];
const ORIGIN: [f32; 2] = [12.25, 3.1];
/// `u` is the frozen part: the first atlas row keeps its 47 columns, so `col / GLYPH_COUNT` is
/// unchanged; only `v` follows the atlas height.
const GLYPH_COUNT: usize = 47;

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

fn glyph_cell(face: &FontFace, ch: char) -> Option<(usize, usize)> {
    face.glyph(ch)
        .and_then(|g| g.rect)
        .map(|r| ((r[0] / 8) as usize, (r[1] / 8) as usize))
}

/// One oracle quad: `[left, top, right, bottom]` and `[u0, v0, u1, v1]`.
type Quad = ([f32; 4], [f32; 4]);

/// The old screen-space loop (`push_ui_text_verts`): +Y down.
fn legacy_down(face: &FontFace, origin: [f32; 2], cell: f32, text: &str) -> Vec<Quad> {
    let count = GLYPH_COUNT as f32;
    let rows = (face.document().atlas.height / 8) as f32;
    let mut out = Vec::new();
    let mut pen_x = origin[0];
    let mut pen_y = origin[1];
    for ch in text.chars() {
        if ch == '\n' {
            pen_x = origin[0];
            pen_y += cell;
            continue;
        }
        if let Some((col, row)) = glyph_cell(face, ch) {
            let u0 = col as f32 / count;
            let u1 = (col as f32 + 1.0) / count;
            out.push((
                [pen_x, pen_y, pen_x + cell, pen_y + cell],
                [u0, row as f32 / rows, u1, (row as f32 + 1.0) / rows],
            ));
        }
        pen_x += cell;
    }
    out
}

/// The old world-space loop (`build_text_draw`): +Y up, so the quad's top edge is `pen_y`.
fn legacy_up(face: &FontFace, origin: [f32; 2], s: f32, text: &str) -> Vec<Quad> {
    let count = GLYPH_COUNT as f32;
    let rows = (face.document().atlas.height / 8) as f32;
    let mut out = Vec::new();
    let (mut pen_x, mut pen_y) = (origin[0], origin[1]);
    for ch in text.chars() {
        if ch == '\n' {
            pen_x = origin[0];
            pen_y -= s;
            continue;
        }
        if let Some((col, row)) = glyph_cell(face, ch) {
            let u0 = col as f32 / count;
            let u1 = (col as f32 + 1.0) / count;
            out.push((
                [pen_x, pen_y, pen_x + s, pen_y - s],
                [u0, row as f32 / rows, u1, (row as f32 + 1.0) / rows],
            ));
        }
        pen_x += s;
    }
    out
}

fn check(axis: YAxis, oracle: fn(&FontFace, [f32; 2], f32, &str) -> Vec<Quad>) {
    let face = FontFace::embedded();
    assert_eq!(face.document().atlas.width as usize, GLYPH_COUNT * 8);
    for text in TEXTS {
        for size in SIZES {
            let old = oracle(face, ORIGIN, size, text);
            let quads = layout(face, text, size, 0.0).quads(ORIGIN, size, axis);
            assert_eq!(old.len(), quads.len(), "{text:?} @ {size}");
            for ((pos, uv), quad) in old.iter().zip(&quads) {
                assert_eq!(bits(pos), bits(&quad.pos), "{text:?} @ {size}");
                assert_eq!(bits(uv), bits(&quad.uv), "{text:?} @ {size}");
            }
        }
    }
}

#[test]
fn down_quads_match_the_legacy_ui_text_loop() {
    check(YAxis::Down, legacy_down);
}

#[test]
fn up_quads_match_the_legacy_world_text_loop() {
    check(YAxis::Up, legacy_up);
}
