//! `render_sprite_sheet` / `render_atlas_sheet` — the two CPU-composed sprite instruments.
//!
//! No GPU adapter is requested anywhere in this file: both paths are pure CPU composition, which is
//! the property that lets them answer on a machine with no adapter and lets these tests probe exact
//! pixels. The probes hardcode the sheet's layout constants (they are private to the engine) and
//! show the arithmetic beside each one, so a layout change fails here loudly rather than silently
//! moving a ruler an author is counting texels with.

use std::path::{Path, PathBuf};

use ludion_engine::{SpriteSheetRequest, SpriteSteps, render_atlas_sheet, render_sprite_sheet};
use ludion_scene::{AtlasDocument, AtlasRegion, SceneOverride};
use serde_json::json;

/// The sheet background, and the colour every unpainted pixel carries (`voxel_sheet::BACKGROUND`).
const BACKGROUND: [u8; 4] = [24, 26, 30, 255];
/// `voxel_sheet::RULER_COLOR`'s sprite-sheet twin — the ticks along the first tile's edges.
const RULER_COLOR: [u8; 4] = [120, 124, 134, 255];
/// The sheet header strip's height (`voxel_sheet::SHEET_HEADER_H`).
const SHEET_HEADER_H: u32 = 40;
/// A block title strip's height (`voxel_sheet::BLOCK_TITLE_H`).
const BLOCK_TITLE_H: u32 = 28;
/// The ruler band reserved on every block-A cell's top and left edges.
const RULER_BAND: u32 = 20;
/// The first block-A tile's own upscale for an 8×6 frame: `MIN_TILE_PX(128) / 8`.
const UPSCALE: u32 = 16;

/// Two 8×6 frames over two glyphs, with an `outline` and a `gradient` in that order. The gradient is
/// scoped to `B`, so texel `(0, 0)` — glyph `A`, opaque, outside the outline's transparent target —
/// bakes to exactly its palette colour and can be probed by value.
const SPRITE: &str = "\
format: 1
kind: sprite
name: probe
size:
  - 8
  - 6
seed: 3
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
frames:
  idle:
    rows:
      - AAAABBBB
      - AAAABBBB
      - \"AAA..BBB\"
      - \"AAA..BBB\"
      - AAAABBBB
      - AAAABBBB
  walk:
    rows:
      - BBBBAAAA
      - BBBBAAAA
      - \"BBB..AAA\"
      - \"BBB..AAA\"
      - BBBBAAAA
      - BBBBAAAA
effects:
  - op: outline
    color:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
  - op: gradient
    glyphs:
      - B
    from:
      - 0.0
      - 0.0
      - 0.0
      - 1.0
    to:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
";

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-spritesheet-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the temp directory is creatable");
    dir
}

fn write_sprite(dir: &Path) -> PathBuf {
    let path = dir.join("probe.sprite.yaml");
    std::fs::write(&path, SPRITE).expect("the fixture writes");
    path
}

fn over(spec: &str) -> SceneOverride {
    SceneOverride::parse(spec).expect("the fixture override parses")
}

/// The RGBA at `(x, y)` of a composed sheet.
fn pixel(image: &ludion_assets::Image, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * image.width + x) * 4) as usize;
    image.rgba[index..index + 4]
        .try_into()
        .expect("a four-byte RGBA pixel")
}

/// The first block-A tile's top-left pixel: the grid starts at column 0, and the tile sits one
/// ruler band in from its cell's own origin.
fn first_tile() -> (u32, u32) {
    (RULER_BAND, SHEET_HEADER_H + BLOCK_TITLE_H + RULER_BAND)
}

/// The contact sheet draws the art, the ruler and the footer, and echoes the effect list in
/// application order.
#[test]
fn the_contact_sheet_draws_art_a_ruler_and_the_effect_echo() {
    let dir = temp_dir("contact");
    let sheet = render_sprite_sheet(&SpriteSheetRequest::new(write_sprite(&dir)))
        .expect("the fixture bakes and composes");

    let (tx, ty) = first_tile();
    // Texel (0, 0) of `idle` is glyph `A` — opaque pure red, untouched by either effect. Probed at
    // the texel's centre: its corner sits on a (major) grid line.
    assert_eq!(
        pixel(&sheet.image, tx + UPSCALE / 2, ty + UPSCALE / 2),
        [255, 0, 0, 255],
        "the first tile draws the baked frame"
    );
    // The ruler's first tick is a long one, ending flush against the tile's top edge.
    assert_eq!(
        pixel(&sheet.image, tx, ty - 1),
        RULER_COLOR,
        "the texel ruler's top ticks are drawn"
    );
    assert_eq!(
        pixel(&sheet.image, tx - 1, ty),
        RULER_COLOR,
        "the texel ruler's left ticks are drawn"
    );

    // The footer sits in the last band of the sheet and is never empty: the effect echo is burned
    // in, which is the whole point of echoing it.
    let footer_top = sheet.image.height - 30;
    let inked = (footer_top..sheet.image.height)
        .flat_map(|y| (0..sheet.image.width).map(move |x| (x, y)))
        .any(|(x, y)| pixel(&sheet.image, x, y) != BACKGROUND);
    assert!(inked, "the footer band carries burned-in text");

    assert_eq!(sheet.effects.len(), 2);
    assert!(
        sheet.effects[0].starts_with("0. outline"),
        "{:?}",
        sheet.effects
    );
    assert!(
        sheet.effects[1].starts_with("1. gradient"),
        "{:?}",
        sheet.effects
    );
    assert!(sheet.applied.is_empty());
    assert_eq!(
        sheet.image.width * sheet.image.height * 4,
        sheet.image.rgba.len() as u32
    );
    assert!(sheet.image.width <= 4096 && sheet.image.height <= 4096);
}

/// Two runs over the same document write the same bytes — the pure-CPU determinism a committed
/// instrument needs.
#[test]
fn composing_twice_is_bit_identical() {
    let dir = temp_dir("determinism");
    let request = SpriteSheetRequest::new(write_sprite(&dir));
    let first = render_sprite_sheet(&request).expect("the first compose succeeds");
    let second = render_sprite_sheet(&request).expect("the second compose succeeds");
    assert_eq!(first.image.width, second.image.width);
    assert_eq!(first.image.height, second.image.height);
    assert_eq!(first.image.rgba, second.image.rgba);
    assert_eq!(first.image.hash, second.image.hash);
}

/// A `--set` over the palette changes the pixels it names and is echoed back with the value the file
/// held.
#[test]
fn a_palette_override_repaints_the_sheet_and_is_echoed() {
    let dir = temp_dir("set");
    let doc = write_sprite(&dir);
    let plain = render_sprite_sheet(&SpriteSheetRequest::new(doc.clone())).expect("the base bakes");

    let request = SpriteSheetRequest {
        set: vec![over("/palette/A/color=#000000")],
        ..SpriteSheetRequest::new(doc)
    };
    let overridden = render_sprite_sheet(&request).expect("the override bakes");

    let (tx, ty) = first_tile();
    let (tx, ty) = (tx + UPSCALE / 2, ty + UPSCALE / 2);
    assert_eq!(pixel(&plain.image, tx, ty), [255, 0, 0, 255]);
    assert_eq!(
        pixel(&overridden.image, tx, ty),
        [0, 0, 0, 255],
        "clearing the red channel repaints every `A` texel"
    );
    assert_ne!(plain.image.rgba, overridden.image.rgba);

    assert_eq!(overridden.applied.len(), 1);
    assert_eq!(overridden.applied[0].pointer, "/palette/A/color");
    assert_eq!(overridden.applied[0].from, Some(json!("#ff0000")));
    assert_eq!(overridden.applied[0].to, json!("#000000"));
}

/// A pointer into the document's *shape* rather than its values is `L4008`, with the legal shapes in
/// the help — the sheet never bakes a sprite nobody authored.
#[test]
fn an_illegal_override_pointer_is_refused() {
    let dir = temp_dir("illegal");
    let request = SpriteSheetRequest {
        set: vec![over("/frames/idle/rows/0=AAAAAAAA")],
        ..SpriteSheetRequest::new(write_sprite(&dir))
    };
    let diagnostics = render_sprite_sheet(&request).expect_err("rows are shape, not a knob");
    assert_eq!(diagnostics[0].code.as_str(), "L4008");
    let help = diagnostics[0].help.as_deref().unwrap();
    assert!(help.contains("/palette/<glyph>/color"), "{help}");

    for pointer in [
        "/palette/A/color/0=0.5",
        "/effects/1/to/3=0.5",
        "/palette/A/color=red",
        "/palette/A/color=#fff",
    ] {
        let request = SpriteSheetRequest {
            set: vec![over(pointer)],
            ..SpriteSheetRequest::new(write_sprite(&dir))
        };
        let diagnostics = render_sprite_sheet(&request).expect_err(pointer);
        assert_eq!(diagnostics[0].code.as_str(), "L4008", "{pointer}");
        let help = diagnostics[0].help.as_deref().unwrap();
        assert!(
            help.contains("/effects/<i>/<param>/<j>") && help.contains("#3a7bd5"),
            "{help}"
        );
    }
}

/// A sweep draws one cell per value in a near-square grid: three values fill three of a 2×2 grid's
/// cells and leave the fourth at the background.
#[test]
fn a_sweep_draws_one_cell_per_value() {
    let dir = temp_dir("sweep");
    let doc = write_sprite(&dir);
    let single =
        render_sprite_sheet(&SpriteSheetRequest::new(doc.clone())).expect("the base bakes");

    let request = SpriteSheetRequest {
        sweep: Some((
            "/effects/1/to".to_string(),
            vec![json!("#000000"), json!("#808080"), json!("#ffffff")],
        )),
        ..SpriteSheetRequest::new(doc)
    };
    let swept = render_sprite_sheet(&request).expect("three candidates bake");

    // One cell is both frames in a row, plus the one-pixel separators: `2 * (8 * 16 + 1) + 1`.
    let cell_w = 2 * (8 * UPSCALE + 1) + 1;
    // A cell is the frames' height plus the two label lines: `6 * 16 + 2 * 20 + 1`.
    let cell_h = 6 * UPSCALE + 2 * 20 + 1;
    assert!(
        swept.image.width >= 2 * cell_w,
        "three values pack into a two-column grid: {} < {}",
        swept.image.width,
        2 * cell_w
    );
    assert!(
        swept.image.height > single.image.height,
        "a grid of candidates is taller than the two-block sheet"
    );

    for (col, row) in [(0, 0), (1, 0), (0, 1)] {
        let (x, y) = (col * cell_w, SHEET_HEADER_H + row * cell_h);
        assert_eq!(
            pixel(&swept.image, x, y),
            [255, 0, 0, 255],
            "cell ({col}, {row}) drew its candidate"
        );
    }
    assert_eq!(
        pixel(&swept.image, cell_w, SHEET_HEADER_H + cell_h),
        BACKGROUND,
        "the fourth cell of a three-value sweep is unused"
    );

    // The swept value is not a `--set`, and the effect echo is still the base document's.
    assert!(swept.applied.is_empty());
    assert_eq!(swept.effects.len(), 2);
}

/// Past the contact sheet's cell cap a sweep is refused rather than silently truncated.
#[test]
fn an_oversized_sweep_is_refused() {
    let dir = temp_dir("sweep-cap");
    let values: Vec<serde_json::Value> = (0..17).map(|i| json!(format!("#0000{i:02x}"))).collect();
    let request = SpriteSheetRequest {
        sweep: Some(("/effects/1/to".to_string(), values)),
        ..SpriteSheetRequest::new(write_sprite(&dir))
    };
    let diagnostics = render_sprite_sheet(&request).expect_err("seventeen cells do not fit");
    assert_eq!(diagnostics[0].code.as_str(), "L4008");
}

/// Writes an 8×8 packed PNG and the atlas document that indexes it, and returns the document's path.
fn write_atlas(dir: &Path) -> PathBuf {
    // Opaque white: premultiplied and straight agree at full coverage, so the composite is a copy.
    let rgba = vec![255u8; 8 * 8 * 4];
    let png = ludion_assets::encode_png(8, 8, &rgba).expect("the fixture PNG encodes");
    std::fs::write(dir.join("probe.png"), png).expect("the fixture PNG writes");

    let mut doc = AtlasDocument {
        generated_from: "probe.atlas-source.yaml".to_string(),
        pixels_per_unit: 16.0,
        color_map: "probe.png".to_string(),
        ..AtlasDocument::default()
    };
    doc.regions.insert(
        "probe/idle".to_string(),
        AtlasRegion {
            rect: [0, 0, 4, 4],
            pivot: [2.0, 2.0],
        },
    );
    doc.regions.insert(
        "probe/walk".to_string(),
        AtlasRegion {
            rect: [4, 0, 4, 4],
            pivot: [2.0, 2.0],
        },
    );

    let path = dir.join("probe.atlas.yaml");
    std::fs::write(&path, ludion_scene::emit_atlas(&doc)).expect("the atlas document writes");
    path
}

/// The overview upscales the packed image, pads to the minimum sheet width, and outlines every
/// region in its own colour.
#[test]
fn the_atlas_overview_outlines_every_region() {
    let dir = temp_dir("atlas");
    let image = render_atlas_sheet(&write_atlas(&dir)).expect("the fixture atlas composes");

    // An 8-texel atlas upscales by the cap (16) to 128 px, then pads out to at least the 512 px
    // minimum — and further still when, as here, the header line is the widest thing on the sheet.
    assert!(image.width >= 512, "{}", image.width);
    assert_eq!(image.height, SHEET_HEADER_H + 8 * 16);

    // Regions walk in name order, so `probe/idle` takes the first outline colour and its rectangle
    // starts at the packed image's own origin.
    assert_eq!(
        pixel(&image, 0, SHEET_HEADER_H),
        [255, 96, 96, 255],
        "the first region's outline is drawn at its own corner"
    );
    // `probe/walk` starts four texels in, in the second colour.
    assert_eq!(
        pixel(&image, 4 * 16, SHEET_HEADER_H),
        [96, 220, 120, 255],
        "the second region takes the next outline colour"
    );
    // Past the packed image the sheet is padded, not stretched.
    assert_eq!(
        pixel(&image, 8 * 16 + 4, SHEET_HEADER_H + 8 * 16 - 1),
        BACKGROUND
    );
}

/// An unreadable atlas path reports the workspace's ordinary document-IO diagnostic, not a panic.
#[test]
fn a_missing_atlas_reports_its_io_diagnostic() {
    let dir = temp_dir("atlas-missing");
    let diagnostics =
        render_atlas_sheet(&dir.join("absent.atlas.yaml")).expect_err("the file does not exist");
    assert_eq!(diagnostics[0].code.as_str(), "L1015");
}

/// A sprite document that does not validate fails before a pixel is written, with its own code.
fn assert_sprite_diagnostic(source: &str, expected: &str) {
    let dir = temp_dir(&format!("invalid-{expected}"));
    let path = dir.join("broken.sprite.yaml");
    std::fs::write(&path, source).expect("the fixture writes");
    let diagnostics =
        render_sprite_sheet(&SpriteSheetRequest::new(path)).expect_err("the document is invalid");
    assert_eq!(diagnostics[0].code.as_str(), expected);
}

#[test]
fn an_invalid_document_never_reaches_composition() {
    assert_sprite_diagnostic(&SPRITE.replacen("      - AAAABBBB\n", "", 1), "L1153");
}

// --- Shared-palette includes ---------------------------------------------------------------------

/// The shared palette `SPRITE`'s include form references: `A` is the red the sprite inherits, `B` a
/// green the include overrides to blue.
const SHARED_PALETTE: &str = "\
format: 1
kind: palette
name: shared
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 1.0
      - 0.0
      - 1.0
";

/// Writes `SPRITE` with its inline palette replaced by an include of `SHARED_PALETTE`, plus the
/// palette document itself, and returns the sprite's path. `A` is inherited, `B` overridden — so the
/// resolved palette is exactly the inline document's.
fn write_include_sprite(dir: &Path) -> PathBuf {
    std::fs::write(dir.join("shared.palette.yaml"), SHARED_PALETTE).expect("the palette writes");
    let inline_palette = "\
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
  B:
    color:
      - 0.0
      - 0.0
      - 1.0
      - 1.0
";
    let include_palette = "\
palette:
  from: shared.palette.yaml
  overrides:
    B:
      color:
        - 0.0
        - 0.0
        - 1.0
        - 1.0
";
    let source = SPRITE.replacen(inline_palette, include_palette, 1);
    assert!(
        source.contains("from: shared.palette.yaml"),
        "the inline palette block must be the one that was replaced"
    );
    let path = dir.join("include.sprite.yaml");
    std::fs::write(&path, source).expect("the fixture writes");
    path
}

/// The contact sheet resolves a `palette: { from }` include before it bakes, so a sprite that
/// references a shared palette composes to bytes identical to the same sprite authored inline — the
/// property that lets a game move thirty documents onto one palette without re-blessing an image.
#[test]
fn an_included_palette_composes_identically_to_the_inline_one() {
    let dir = temp_dir("include");
    let inline = render_sprite_sheet(&SpriteSheetRequest::new(write_sprite(&dir)))
        .expect("the inline sprite composes");
    let included = render_sprite_sheet(&SpriteSheetRequest::new(write_include_sprite(&dir)))
        .expect("the including sprite composes");
    assert_eq!(included.image.rgba, inline.image.rgba);
    assert_eq!(included.image.hash, inline.image.hash);
}

/// `--set` addresses the *resolved* palette, so a glyph inherited from the shared document is as
/// tunable as a local one — and the shared file on disk is never touched, because the edit lives in
/// the in-memory document like every other override.
#[test]
fn a_set_over_an_inherited_glyph_repaints_the_sheet() {
    let dir = temp_dir("include-set");
    let doc = write_include_sprite(&dir);
    let request = SpriteSheetRequest {
        set: vec![over("/palette/A/color=#000000")],
        ..SpriteSheetRequest::new(doc)
    };
    let overridden = render_sprite_sheet(&request).expect("the override bakes");

    let (tx, ty) = first_tile();
    let (tx, ty) = (tx + UPSCALE / 2, ty + UPSCALE / 2);
    assert_eq!(
        pixel(&overridden.image, tx, ty),
        [0, 0, 0, 255],
        "clearing the red channel repaints every `A` texel, glyph `A` being the shared palette's"
    );
    assert_eq!(overridden.applied[0].from, Some(json!("#ff0000")));
    assert_eq!(
        std::fs::read_to_string(dir.join("shared.palette.yaml")).expect("the palette still reads"),
        SHARED_PALETTE,
        "a `--set` never writes the shared palette"
    );
}

/// A `palette.from` naming no readable file is `L1147` at the reference, before a pixel is written.
#[test]
fn an_unresolvable_include_reports_l1147() {
    let dir = temp_dir("include-missing");
    let path = dir.join("dangling.sprite.yaml");
    std::fs::write(
        &path,
        "\
format: 1
kind: sprite
size:
  - 1
  - 1
palette:
  from: absent.palette.yaml
frames:
  idle:
    rows:
      - A
",
    )
    .expect("the fixture writes");
    let diagnostics = render_sprite_sheet(&SpriteSheetRequest::new(path))
        .expect_err("the palette file does not exist");
    assert_eq!(diagnostics[0].code.as_str(), "L1147");
    assert_eq!(diagnostics[0].path.as_deref(), Some("/palette/from"));
}

/// A one-frame, effect-free sprite `width`×`height` texels: a row of opaque red `A`, then `.`.
fn plain_sprite(width: u32, height: u32, first_row: &str) -> String {
    let rows: String = (0..height)
        .map(|y| {
            let row = if y == 0 {
                first_row.to_string()
            } else {
                "A".repeat(width as usize)
            };
            format!("      - \"{row}\"\n")
        })
        .collect();
    format!(
        "format: 1\nkind: sprite\nsize:\n  - {width}\n  - {height}\npalette:\n  A:\n    color:\n      \
         - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nframes:\n  idle:\n    rows:\n{rows}"
    )
}

fn write_plain(dir: &Path, source: &str) -> PathBuf {
    let path = dir.join("plain.sprite.yaml");
    std::fs::write(&path, source).expect("the fixture writes");
    path
}

const CHECKER_DARK: [u8; 4] = [44, 46, 52, 255];
const CHECKER_LIGHT: [u8; 4] = [62, 65, 72, 255];

/// A transparent texel in block A shows both checker colours, never the flat background.
#[test]
fn a_transparent_texel_shows_a_checkerboard() {
    let dir = temp_dir("checker");
    // 8×6 → upscale 16, checker squares of 8 px; texel (1, 0) is transparent.
    let source = plain_sprite(8, 6, "A.AAAAAA");
    let sheet = render_sprite_sheet(&SpriteSheetRequest::new(write_plain(&dir, &source)))
        .expect("the fixture composes");
    let (tx, ty) = first_tile();
    // Off the grid lines (x = 16 is a texel boundary): offsets 2 and 10 land in different squares.
    let left = pixel(&sheet.image, tx + UPSCALE + 2, ty + 2);
    let right = pixel(&sheet.image, tx + UPSCALE + 10, ty + 2);
    assert_eq!(left, CHECKER_DARK);
    assert_eq!(right, CHECKER_LIGHT);
    for dx in 2..UPSCALE {
        assert_ne!(
            pixel(&sheet.image, tx + UPSCALE + dx, ty + 2),
            BACKGROUND,
            "a transparent texel is never the flat background"
        );
    }
}

/// The grid darkens each texel boundary a quarter, every 8th a half; below upscale 6 there is none.
#[test]
fn the_texel_grid_is_minor_and_major_and_absent_when_small() {
    let dir = temp_dir("grid");
    // 16×4 → upscale 8: grid on.
    let source = plain_sprite(16, 4, &"A".repeat(16));
    let sheet = render_sprite_sheet(&SpriteSheetRequest::new(write_plain(&dir, &source)))
        .expect("the fixture composes");
    let (tx, ty) = first_tile();
    assert_eq!(
        pixel(&sheet.image, tx + 8, ty + 3),
        [191, 0, 0, 255],
        "minor line"
    );
    assert_eq!(
        pixel(&sheet.image, tx + 64, ty + 3),
        [127, 0, 0, 255],
        "major line"
    );
    assert_eq!(
        pixel(&sheet.image, tx + 12, ty + 3),
        [255, 0, 0, 255],
        "texel body"
    );

    // 32×4 → upscale 4: no grid, the boundary pixel is the plain texel.
    let source = plain_sprite(32, 4, &"A".repeat(32));
    let sheet = render_sprite_sheet(&SpriteSheetRequest::new(write_plain(&dir, &source)))
        .expect("the fixture composes");
    assert_eq!(pixel(&sheet.image, tx + 4, ty + 1), [255, 0, 0, 255]);
    assert_eq!(pixel(&sheet.image, tx + 32, ty + 1), [255, 0, 0, 255]);
}

fn steps_request(path: PathBuf, frame: Option<&str>) -> SpriteSheetRequest {
    SpriteSheetRequest {
        steps: Some(SpriteSteps {
            frame: frame.map(str::to_string),
        }),
        ..SpriteSheetRequest::new(path)
    }
}

/// The steps sheet has one row per frame, each `paint` then every applicable effect's index.
#[test]
fn the_steps_sheet_draws_a_row_per_frame() {
    let dir = temp_dir("steps");
    let path = write_sprite(&dir);
    let sheet = render_sprite_sheet(&steps_request(path.clone(), None)).expect("steps compose");
    assert_eq!(
        sheet.steps,
        vec![
            "idle: paint, 0, 1".to_string(),
            "walk: paint, 0, 1".to_string()
        ]
    );
    assert_eq!(sheet.effects.len(), 2);
    // The paint cell is the first cell of the first row, drawn at the upscale of 16.
    assert_eq!(
        pixel(&sheet.image, UPSCALE / 2, SHEET_HEADER_H + UPSCALE / 2),
        [255, 0, 0, 255]
    );
    let again = render_sprite_sheet(&steps_request(path.clone(), None)).expect("steps compose");
    assert_eq!(sheet.image, again.image, "composing twice is bit-identical");

    let walk = render_sprite_sheet(&steps_request(path, Some("walk"))).expect("one row composes");
    assert_eq!(walk.steps, vec!["walk: paint, 0, 1".to_string()]);
    assert!(walk.image.height < sheet.image.height);
}

/// Steps honour `--set`: a palette override changes the paint cell.
#[test]
fn steps_honour_set() {
    let dir = temp_dir("steps-set");
    let request = SpriteSheetRequest {
        set: vec![over("/palette/A/color=#ffff00")],
        ..steps_request(write_sprite(&dir), None)
    };
    let sheet = render_sprite_sheet(&request).expect("steps compose");
    assert_eq!(
        pixel(&sheet.image, UPSCALE / 2, SHEET_HEADER_H + UPSCALE / 2),
        [255, 255, 0, 255]
    );
    assert_eq!(sheet.applied.len(), 1);
}

/// An unknown frame, steps with a sweep, and a document of more than sixteen cells are `L4008`.
#[test]
fn the_steps_refusals_are_l4008() {
    let dir = temp_dir("steps-refuse");
    let path = write_sprite(&dir);

    let unknown =
        render_sprite_sheet(&steps_request(path.clone(), Some("jump"))).expect_err("no such frame");
    assert_eq!(unknown[0].code.as_str(), "L4008");
    assert!(
        unknown[0].help.as_deref().unwrap_or("").contains("idle"),
        "the help lists the frames: {:?}",
        unknown[0].help
    );

    let both = SpriteSheetRequest {
        sweep: Some(("/effects/1/to".to_string(), vec![json!("#808080")])),
        ..steps_request(path, None)
    };
    let both = render_sprite_sheet(&both).expect_err("steps and a sweep");
    assert_eq!(both[0].code.as_str(), "L4008");

    let mut source = plain_sprite(8, 6, "AAAAAAAA");
    source.push_str("effects:\n");
    for _ in 0..16 {
        source.push_str("  - op: noise\n    amount: 0.1\n");
    }
    let overflow = render_sprite_sheet(&steps_request(write_plain(&dir, &source), None))
        .expect_err("seventeen cells do not fit");
    assert_eq!(overflow[0].code.as_str(), "L4008");
    assert!(
        overflow[0].message.contains("17"),
        "{}",
        overflow[0].message
    );
}
