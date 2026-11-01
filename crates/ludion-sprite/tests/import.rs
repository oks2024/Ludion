//! PNG → sprite document: the lossless round trip, the quantized one, and the refusal between them.

use ludion_assets::Image;
use ludion_scene::{SPRITE_GLYPH_BUDGET, check_sprite, emit_sprite};
use ludion_sprite::{bake, import_png};

fn image(width: u32, height: u32, texels: &[[u8; 4]]) -> Image {
    let rgba: Vec<u8> = texels.iter().flatten().copied().collect();
    assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
    Image {
        width,
        height,
        hash: ludion_assets::content_hash(&rgba),
        rgba,
    }
}

#[test]
fn a_lossless_import_bakes_back_to_the_source_pixels() {
    let source = image(
        3,
        2,
        &[
            [255, 0, 0, 255],
            [0, 255, 0, 128],
            [0, 0, 0, 0],
            [10, 20, 30, 255],
            [255, 255, 255, 255],
            [0, 0, 0, 0],
        ],
    );

    let imported = import_png(&source, Some("hero".to_string()), None).expect("the PNG imports");
    assert!(imported.quantized.is_none(), "nothing had to merge");
    assert_eq!(imported.doc.size, [3, 2]);
    assert_eq!(imported.doc.frames.len(), 1);
    assert_eq!(imported.doc.frames[0].name, "main");
    assert_eq!(imported.doc.palette.len(), 4);
    assert_eq!(
        imported.doc.frame_rows(&imported.doc.frames[0])[0]
            .chars()
            .nth(2),
        Some('.')
    );

    // The document survives a trip through the canonical text form...
    let text = emit_sprite(&imported.doc);
    assert!(
        text.contains("color: \"#"),
        "palette colours are written as hex:\n{text}"
    );
    let parsed = check_sprite(&text).expect("the imported document is valid");
    assert_eq!(parsed, imported.doc);
    assert_eq!(
        emit_sprite(&parsed),
        text,
        "canonical emit is a fixed point"
    );

    // ...and baking it reproduces the input bytes exactly.
    let baked = bake(&parsed).expect("the imported document bakes");
    assert_eq!(baked[0].image.to_srgb8_straight().rgba, source.rgba);
}

#[test]
fn a_palette_over_the_glyph_budget_is_refused_with_the_quantize_fix() {
    let texels: Vec<[u8; 4]> = (0..=SPRITE_GLYPH_BUDGET)
        .map(|index| [index as u8, 0, 0, 255])
        .collect();
    let source = image(texels.len() as u32, 1, &texels);

    let diagnostic = import_png(&source, None, None).expect_err("94 colours do not fit 93 glyphs");
    assert_eq!(diagnostic.code.as_str(), "L4014");
    assert!(diagnostic.message.contains("94"), "{}", diagnostic.message);
    assert!(diagnostic.message.contains("93"), "{}", diagnostic.message);
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--quantize")),
        "the help names the fix"
    );

    // The same image with a `--quantize` never errors.
    let imported = import_png(&source, None, Some(SPRITE_GLYPH_BUDGET)).expect("quantize imports");
    let report = imported.quantized.expect("colours were merged");
    assert_eq!(report.colors_before, 94);
    assert!(report.colors_after <= SPRITE_GLYPH_BUDGET);
}

#[test]
fn quantizing_reduces_the_palette_and_reports_the_loss() {
    let source = image(
        6,
        1,
        &[
            [0, 0, 0, 255],
            [10, 0, 0, 255],
            [20, 0, 0, 255],
            [200, 0, 0, 255],
            [210, 0, 0, 255],
            [220, 0, 0, 255],
        ],
    );

    let imported = import_png(&source, None, Some(3)).expect("the PNG imports");
    let report = imported.quantized.expect("colours were merged");
    assert_eq!(report.colors_before, 6);
    assert_eq!(report.colors_after, 3);
    // The 200/210/220 box collapses onto 210, so the worst texel moves ten 8-bit units.
    assert_eq!(report.max_channel_error, 10);
    assert_eq!(imported.doc.palette.len(), 3);

    let parsed = check_sprite(&emit_sprite(&imported.doc)).expect("the document is valid");
    assert_eq!(parsed, imported.doc);
}

#[test]
fn a_quantize_that_never_has_to_merge_reports_nothing() {
    let source = image(2, 1, &[[1, 2, 3, 255], [4, 5, 6, 255]]);
    let imported = import_png(&source, None, Some(SPRITE_GLYPH_BUDGET)).expect("the PNG imports");
    assert!(imported.quantized.is_none());
    assert_eq!(imported.doc.palette.len(), 2);
}

#[test]
fn a_quantize_of_zero_clamps_to_one_colour_rather_than_failing() {
    let source = image(2, 1, &[[0, 0, 0, 255], [255, 255, 255, 255]]);
    let imported = import_png(&source, None, Some(0)).expect("a `Some` never errors");
    assert_eq!(imported.doc.palette.len(), 1);
    assert_eq!(
        imported.quantized.map(|report| report.colors_after),
        Some(1)
    );
}

#[test]
fn an_image_with_no_opaque_texels_is_refused() {
    let source = image(2, 1, &[[0, 0, 0, 0], [0, 0, 0, 0]]);
    let diagnostic = import_png(&source, None, None).expect_err("an empty palette is not a sprite");
    assert_eq!(diagnostic.code.as_str(), "L1153");
}

#[test]
fn an_image_larger_than_a_sprite_is_refused_with_the_limit() {
    let texels = vec![[1u8, 2, 3, 255]; 1025];
    let source = image(1025, 1, &texels);
    let diagnostic = import_png(&source, None, None).expect_err("1025 texels is not a sprite");
    assert_eq!(diagnostic.code.as_str(), "L1153");
    assert!(
        diagnostic.message.contains("1024"),
        "{}",
        diagnostic.message
    );
}
