//! The embedded 8×8 face: its two-row atlas, the punctuation row pinned against the bitmaps it
//! replaced, and the CPU label stamp over it.

use ludion_font::{EMBEDDED_PNG, FontFace, LABEL_PITCH, stamp_label};

/// The bitmaps `sprite_sheet.rs` once kept for the characters the first atlas row lacked: row 0 at
/// the top, bit `0x80` the leftmost column.
const RETIRED_EXTRA_GLYPHS: [(char, [u8; 8]); 15] = [
    ('"', [0b01101100, 0b01101100, 0b01001000, 0, 0, 0, 0, 0]),
    (
        '#',
        [
            0b00100100, 0b00100100, 0b01111110, 0b00100100, 0b01111110, 0b00100100, 0b00100100, 0,
        ],
    ),
    (
        '%',
        [
            0b01100010, 0b01100100, 0b00001000, 0b00010000, 0b00100110, 0b01000110, 0, 0,
        ],
    ),
    ('\'', [0b00011000, 0b00011000, 0b00110000, 0, 0, 0, 0, 0]),
    (
        '*',
        [
            0, 0b01000010, 0b00100100, 0b00011000, 0b00100100, 0b01000010, 0, 0,
        ],
    ),
    (
        ';',
        [
            0, 0b00011000, 0b00011000, 0, 0b00011000, 0b00011000, 0b00110000, 0,
        ],
    ),
    (
        '<',
        [
            0b00001100, 0b00011000, 0b00110000, 0b01100000, 0b00110000, 0b00011000, 0b00001100, 0,
        ],
    ),
    ('=', [0, 0, 0b01111110, 0, 0b01111110, 0, 0, 0]),
    (
        '>',
        [
            0b00110000, 0b00011000, 0b00001100, 0b00000110, 0b00001100, 0b00011000, 0b00110000, 0,
        ],
    ),
    (
        '@',
        [
            0b00111100, 0b01000010, 0b01011110, 0b01011110, 0b01000000, 0b00111100, 0, 0,
        ],
    ),
    (
        '[',
        [
            0b00111100, 0b00110000, 0b00110000, 0b00110000, 0b00110000, 0b00110000, 0b00111100, 0,
        ],
    ),
    (
        '\\',
        [
            0b01100000, 0b00110000, 0b00110000, 0b00011000, 0b00001100, 0b00000110, 0b00000110, 0,
        ],
    ),
    (
        ']',
        [
            0b00111100, 0b00001100, 0b00001100, 0b00001100, 0b00001100, 0b00001100, 0b00111100, 0,
        ],
    ),
    ('_', [0, 0, 0, 0, 0, 0, 0b01111110, 0]),
    ('~', [0, 0, 0b00110010, 0b01011010, 0b01001100, 0, 0, 0]),
];

fn atlas() -> (u32, Vec<u8>) {
    let (w, _, rgba) = ludion_assets::decode_png(EMBEDDED_PNG, "default.png").unwrap();
    (w, rgba)
}

/// Whether the face's glyph for `ch` has texel `(col, row)` inked.
fn inked(atlas: &(u32, Vec<u8>), ch: char, col: u32, row: u32) -> bool {
    let rect = FontFace::embedded().glyph(ch).unwrap().rect.unwrap();
    let at = (((rect[1] + row) * atlas.0 + rect[0] + col) * 4 + 3) as usize;
    atlas.1[at] >= 128
}

fn stamp(text: &str, size: [u32; 2], scale: u32) -> Vec<u8> {
    let mut canvas = vec![0u8; (size[0] * size[1] * 4) as usize];
    stamp_label(
        &atlas().1,
        &mut canvas,
        size,
        [0, 0],
        text,
        scale,
        [1, 2, 3, 255],
    );
    canvas
}

fn set(canvas: &[u8], size: [u32; 2], x: u32, y: u32) -> bool {
    canvas[((y * size[0] + x) * 4 + 3) as usize] == 255
}

#[test]
fn the_new_punctuation_matches_the_retired_table() {
    let atlas = atlas();
    for (ch, bits) in RETIRED_EXTRA_GLYPHS {
        for row in 0..8u32 {
            for col in 0..8u32 {
                let want = bits[row as usize] & (0x80 >> col) != 0;
                assert_eq!(
                    inked(&atlas, ch, col, row),
                    want,
                    "{ch:?} at ({col}, {row})"
                );
            }
        }
    }
}

#[test]
fn every_embedded_glyph_is_one_full_8x8_cell() {
    let doc = FontFace::embedded().document();
    assert_eq!(doc.glyphs.len(), 62);
    for glyph in &doc.glyphs {
        let rect = glyph.rect.unwrap();
        assert_eq!(glyph.advance, 8, "{:?}", glyph.ch);
        assert_eq!(glyph.bounds, Some([0, 0, 8, 8]), "{:?}", glyph.ch);
        assert_eq!((rect[2], rect[3]), (8, 8), "{:?}", glyph.ch);
    }
}

#[test]
fn stamping_a_glyph_at_scale_two_draws_two_by_two_blocks() {
    let atlas = atlas();
    let size = [32, 32];
    let canvas = stamp("#", size, 2);
    for row in 0..8u32 {
        for col in 0..8u32 {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                assert_eq!(
                    set(&canvas, size, col * 2 + dx, row * 2 + dy),
                    inked(&atlas, '#', col, row),
                    "texel ({col}, {row})"
                );
            }
        }
    }
    assert_eq!(&canvas[..4], &[0u8; 4], "the corner texel is blank");
}

#[test]
fn lowercase_stamps_as_its_uppercase_twin() {
    assert_eq!(stamp("a", [16, 16], 1), stamp("A", [16, 16], 1));
}

#[test]
fn an_unmapped_char_leaves_a_blank_pitch() {
    let size = [40, 16];
    let canvas = stamp("éA", size, 2);
    let lone = stamp("A", size, 2);
    let shift = (LABEL_PITCH * 2) as usize;
    for y in 0..16usize {
        for x in 0..size[0] as usize {
            let got = canvas[(y * size[0] as usize + x) * 4 + 3];
            let want = if x >= shift {
                lone[(y * size[0] as usize + x - shift) * 4 + 3]
            } else {
                0
            };
            assert_eq!(got, want, "({x}, {y})");
        }
    }
    assert!(lone.chunks(4).any(|px| px[3] == 255));
}

#[test]
fn a_label_past_the_canvas_edge_is_clipped() {
    let size = [10, 6];
    let canvas = stamp("HELLO WORLD", size, 3);
    assert_eq!(canvas.len(), (size[0] * size[1] * 4) as usize);
    let mut off_canvas = vec![0u8; 16];
    stamp_label(
        &atlas().1,
        &mut off_canvas,
        [2, 2],
        [100, 100],
        "A",
        2,
        [9; 4],
    );
    assert!(off_canvas.iter().all(|&b| b == 0));
}
