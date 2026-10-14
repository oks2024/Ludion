//! The bake's pixel half against the committed test faces: determinism, document round-trip,
//! Silkscreen's near-binary coverage, and the cross-arch PNG hash canary.
#![cfg(feature = "bake")]

use ludion_font::bake::{FontBake, bake};
use ludion_scene::{CharsetRange, FontCharset, FontMode, FontSourceDocument};

fn face_bytes(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/gallery/fonts/");
    std::fs::read(format!("{path}{name}")).expect("a committed test face")
}

fn source(
    mode: FontMode,
    em_px: u32,
    spread_px: Option<u32>,
    ranges: Vec<CharsetRange>,
    chars: &str,
) -> FontSourceDocument {
    FontSourceDocument {
        name: None,
        file: String::new(),
        mode,
        em_px,
        spread_px,
        line_height: None,
        charset: FontCharset {
            ranges,
            chars: chars.to_string(),
        },
    }
}

fn inter(ranges: Vec<CharsetRange>, chars: &str) -> FontBake {
    let src = source(FontMode::Sdf, 56, Some(6), ranges, chars);
    bake(
        &src,
        &face_bytes("Inter-Regular.ttf"),
        "inter.font-source.yaml",
        "inter",
    )
    .expect("bake")
}

#[test]
fn an_sdf_bake_is_deterministic_and_round_trips() {
    let a = inter(vec![], "AVI");
    let b = inter(vec![], "AVI");
    assert_eq!(a.png, b.png);
    assert_eq!(
        ludion_scene::emit_font(&a.doc),
        ludion_scene::emit_font(&b.doc)
    );
    let back = ludion_scene::check_font(&ludion_scene::emit_font(&a.doc)).expect("valid");
    assert_eq!(back, a.doc);
    assert!(a.doc.kerning.iter().any(|(l, r, _)| (*l, *r) == ('A', 'V')));
    assert!(a.doc.notdef.rect.is_some());
    assert_eq!(a.doc.atlas.spread_px, Some(6));
    assert_eq!(a.image.hash, ludion_assets::content_hash(&a.png));
}

#[test]
fn silkscreen_bitmap_is_near_binary() {
    // One-texel counters are inset on both sides, so they read ~20 (not ~10) and ~235.
    let src = source(FontMode::Bitmap, 8, None, vec![CharsetRange::Ascii], "");
    let b = bake(
        &src,
        &face_bytes("Silkscreen-Regular.ttf"),
        "s.font-source.yaml",
        "s",
    )
    .expect("bake");
    let alpha: Vec<u8> = b.image.rgba.chunks_exact(4).map(|p| p[3]).collect();
    let odd: Vec<u8> = alpha
        .iter()
        .copied()
        .filter(|a| (21..235).contains(a))
        .collect();
    assert!(odd.is_empty(), "{odd:?}");
    let width = b.doc.atlas.width as usize;
    for ch in ['I', 'L', 'T'] {
        let g = b.doc.glyphs.iter().find(|g| g.ch == ch).expect("glyph");
        let [x, y, w, h] = g.rect.expect("inked");
        for row in y..y + h {
            for col in x..x + w {
                let a = alpha[row as usize * width + col as usize];
                assert!(a == 0 || a == 255, "{ch} has alpha {a}");
            }
        }
    }
    let a = b.doc.glyphs.iter().find(|g| g.ch == 'A').expect("A");
    assert!(a.rect.expect("inked")[3] <= 8);
    let truly_lacking: Vec<char> = b.doc.missing.clone();
    assert!(
        truly_lacking.is_empty(),
        "missing from Silkscreen ASCII: {truly_lacking:?}"
    );
}

#[test]
fn inter_ascii_sdf_png_hash_is_the_cross_arch_canary() {
    // The cross-arch bake canary (`Plan/font-assets.md` decision 17): the M4 runs this too.
    let b = inter(vec![CharsetRange::Ascii], "");
    assert_eq!(b.image.hash, 0x9f16_ddd3_f980_edd6);
}

#[test]
fn a_char_the_face_lacks_is_listed_missing_and_not_baked() {
    let b = inter(vec![], "A\u{0E01}");
    assert_eq!(b.doc.missing, vec!['\u{0E01}']);
    assert!(b.doc.glyphs.iter().all(|g| g.ch != '\u{0E01}'));
}

fn silkscreen(
    line_height: Option<u32>,
) -> Result<FontBake, Vec<ludion_core::diagnostics::Diagnostic>> {
    let mut src = source(FontMode::Bitmap, 8, None, vec![CharsetRange::Ascii], "");
    src.line_height = line_height;
    bake(
        &src,
        &face_bytes("Silkscreen-Regular.ttf"),
        "s.font-source.yaml",
        "s",
    )
}

#[test]
fn line_height_moves_only_the_ascender() {
    let plain = silkscreen(None).expect("bake");
    assert_eq!(plain.doc.ascender, 1030);
    let tall = silkscreen(Some(1250)).expect("bake");
    assert_eq!(tall.doc.ascender, 1000);
    assert_eq!(tall.doc.descender, -250);
    assert_eq!(tall.doc.line_gap, 0);
    assert_eq!(tall.png, plain.png);
}

#[test]
fn a_line_height_with_no_room_above_the_baseline_is_rejected() {
    let errs = silkscreen(Some(100)).expect_err("rejected");
    assert_eq!(
        errs[0].code,
        ludion_core::codes::code::FONT_SOURCE_DOCUMENT_STRUCTURE
    );
    assert_eq!(errs[0].path.as_deref(), Some("/line_height"));
}

#[test]
fn silkscreen_ascii_bitmap_png_hash_is_the_cross_arch_canary() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/gallery/fonts/");
    let text = std::fs::read_to_string(format!("{path}silkscreen.font-source.yaml")).unwrap();
    let src = ludion_scene::check_font_source(&text).expect("the committed source");
    let b = bake(
        &src,
        &face_bytes("Silkscreen-Regular.ttf"),
        "silkscreen.font-source.yaml",
        "silkscreen",
    )
    .expect("bake");
    assert_eq!(b.image.hash, 6_230_192_689_813_741_942);
}

#[test]
fn an_oversize_glyph_is_rejected_before_any_rasterizing() {
    let src = source(FontMode::Sdf, 60_000, Some(6), vec![], "AB");
    let errs = bake(&src, &face_bytes("Inter-Regular.ttf"), "i.yaml", "i").expect_err("rejected");
    assert_eq!(
        errs[0].code,
        ludion_core::codes::code::FONT_SOURCE_DOCUMENT_STRUCTURE
    );
    assert_eq!(errs[0].path.as_deref(), Some("/em_px"));
    assert!(errs[0].message.contains("texels, over 4096 a side"));
}
