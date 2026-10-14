//! The bake's reading half against the committed test faces: kerning, missing chars, outline
//! shape, determinism, and a malformed-input corpus that must never panic.
#![cfg(feature = "bake")]

use std::sync::OnceLock;

use ludion_font::bake::{FaceOutlines, read_face, resolve_charset};
use ludion_scene::{CharsetRange, FontCharset};

fn face_bytes(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/gallery/fonts/");
    std::fs::read(format!("{path}{name}")).expect("a committed test face")
}

fn charset() -> Vec<char> {
    resolve_charset(&FontCharset {
        ranges: vec![CharsetRange::Ascii],
        chars: "\u{0E01}".to_string(),
    })
}

fn inter() -> &'static FaceOutlines {
    static INTER: OnceLock<FaceOutlines> = OnceLock::new();
    INTER.get_or_init(|| read_face(&face_bytes("Inter-Regular.ttf"), &charset()).expect("Inter"))
}

#[test]
fn inter_kerns_a_v_negative() {
    let kern = inter()
        .kerning
        .iter()
        .find(|(l, r, _)| (*l, *r) == ('A', 'V'))
        .expect("an A/V pair");
    assert!(kern.2 < 0, "A/V adjust is {}", kern.2);
}

#[test]
fn inter_reports_a_missing_char_and_notdef() {
    let face = inter();
    assert_eq!(face.missing, vec!['\u{0E01}']);
    assert!(!face.notdef.contours.is_empty());
    assert_eq!(face.units_per_em, 2048);
    let space = face
        .glyphs
        .iter()
        .find(|(c, _)| *c == ' ')
        .expect("space")
        .1
        .clone();
    assert!(space.bounds.is_none() && space.contours.is_empty() && space.advance > 0);
    let a = &face.glyphs.iter().find(|(c, _)| *c == 'A').expect("A").1;
    assert!(a.bounds.is_some() && !a.contours.is_empty());
}

#[test]
fn reading_twice_is_equal() {
    let again = read_face(&face_bytes("Inter-Regular.ttf"), &charset()).expect("Inter");
    assert_eq!(&again, inter());
}

/// Silkscreen's pixels are 125 font units; the counters of `0 6 8 9 @ A B D O P Q R a b d o p q r`
/// are inset by a 5-unit overlap, so a point is on the grid within that tolerance.
#[test]
fn silkscreen_points_sit_on_the_125_unit_pixel_grid() {
    let face = read_face(&face_bytes("Silkscreen-Regular.ttf"), &charset()).expect("Silkscreen");
    assert_eq!(face.units_per_em, 1000);
    let off_grid = |v: f32| {
        let r = v.rem_euclid(125.0);
        r.min(125.0 - r)
    };
    for (ch, glyph) in &face.glyphs {
        for p in glyph.contours.iter().flatten() {
            assert!(
                off_grid(p[0]) <= 5.0 && off_grid(p[1]) <= 5.0,
                "{ch:?} has a point off the pixel grid: {p:?}"
            );
        }
    }
}

fn codes_ok(result: Result<FaceOutlines, Vec<ludion_core::diagnostics::Diagnostic>>) {
    if let Err(diagnostics) = result {
        assert!(!diagnostics.is_empty());
        for d in diagnostics {
            assert_eq!(d.code.as_str(), "L1252");
        }
    }
}

#[test]
fn malformed_input_never_panics() {
    let small: Vec<char> = "AVa ".chars().collect();
    let inter = face_bytes("Inter-Regular.ttf");
    codes_ok(read_face(&[], &small));
    codes_ok(read_face(&inter[..12], &small));
    for i in 0..64usize {
        codes_ok(read_face(&inter[..inter.len() * i / 64], &small));
    }
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for _ in 0..256 {
        let mut bytes = inter.clone();
        let at = next() % bytes.len();
        bytes[at] ^= (next() % 255 + 1) as u8;
        codes_ok(read_face(&bytes, &small));
    }
    let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x01\x00\x00\x00\x10".to_vec();
    ttc.extend_from_slice(&[0; 32]);
    let err = read_face(&ttc, &small).expect_err("a collection is refused");
    assert_eq!(err[0].code.as_str(), "L1252");
}

#[test]
fn charset_resolves_sorted_and_deduped() {
    let ascii = FontCharset {
        ranges: vec![CharsetRange::Ascii],
        chars: String::new(),
    };
    assert_eq!(resolve_charset(&ascii).len(), 95);
    let merged = FontCharset {
        ranges: vec![CharsetRange::Ascii, CharsetRange::Ascii],
        chars: "zA\u{2026}\u{2026}".to_string(),
    };
    let chars = resolve_charset(&merged);
    assert_eq!(chars.len(), 96);
    assert!(chars.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(chars.last(), Some(&'\u{2026}'));
}
