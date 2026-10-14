//! Integration tests for `ludion check-scene`'s sprite-pipeline document paths
//! (`Plan/sprite-engine.md` Wave 1): dispatch on the top-level `kind: sprite` / `kind: atlas-source`
//! / `kind: atlas` keys, the atlas's on-disk pass over the packed image it addresses, and the scene
//! pass that resolves every `atlas` + `frame` pair a scene names. Driven against the built `ludion`
//! binary — the `check_scene_voxel_animation.rs` shape.

use std::path::Path;
use std::process::{Command, Output};

fn run_check_scene(path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(path)
        .output()
        .expect("failed to run the ludion binary")
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sprite")
        .join(name)
}

/// Runs `check-scene` on a fixture and returns `(stdout, stderr)`, asserting the exit code.
fn check(name: &str, expect_success: bool) -> (String, String) {
    let output = run_check_scene(&fixture(name));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.success(),
        expect_success,
        "unexpected exit for {name}\nstdout: {stdout}\nstderr: {stderr}"
    );
    (stdout, stderr)
}

// --- kind: sprite ---------------------------------------------------------------------------

/// A valid sprite document checks clean: a summary naming the extent, palette, frames and effects,
/// then one line per frame carrying the pivot the bake will use (authored, or the frame's center).
#[test]
fn good_sprite_succeeds() {
    let (stdout, _) = check("good.sprite.yaml", true);
    assert!(stdout.contains("OK"), "stdout was: {stdout}");
    assert!(stdout.contains("bolt"), "stdout was: {stdout}");
    assert!(stdout.contains("4x4"), "stdout was: {stdout}");
    assert!(stdout.contains("2 frames"), "stdout was: {stdout}");
    // The authored pivot of `spent`, and the derived center of `idle`.
    assert!(
        stdout.contains("idle: pivot (2, 2)"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("spent: pivot (2, 3)"),
        "stdout was: {stdout}"
    );
}

/// A frame whose row is narrower than `size` declares is `L1153`, with a JSON Pointer at the row.
#[test]
fn broken_sprite_reports_l1153() {
    let (_, stderr) = check("broken.sprite.yaml", false);
    assert!(stderr.contains("error[L1153]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/frames/idle/rows/1"),
        "stderr was: {stderr}"
    );
}

// --- kind: atlas-source ---------------------------------------------------------------------

/// A valid atlas source checks clean and echoes its inputs — the list the bake will read.
#[test]
fn good_atlas_source_succeeds() {
    let (stdout, _) = check("good.atlas-source.yaml", true);
    assert!(stdout.contains("atlas source"), "stdout was: {stdout}");
    assert!(stdout.contains("nearest filter"), "stdout was: {stdout}");
    assert!(stdout.contains("1 flipbook"), "stdout was: {stdout}");
    assert!(
        stdout.contains("input good.sprite.yaml"),
        "stdout was: {stdout}"
    );
}

// --- kind: atlas ----------------------------------------------------------------------------

/// A valid atlas checks clean, and its summary reports the *packed image's* own extent — which is
/// the proof the on-disk half ran at all.
#[test]
fn good_atlas_succeeds() {
    let (stdout, _) = check("ship.atlas.yaml", true);
    assert!(stdout.contains("OK (atlas,"), "stdout was: {stdout}");
    assert!(stdout.contains("4x4 px"), "stdout was: {stdout}");
    assert!(stdout.contains("2 regions"), "stdout was: {stdout}");
    assert!(
        stdout.contains("generated from good.atlas-source.yaml"),
        "stdout was: {stdout}"
    );
}

/// A region whose `rect` runs off the packed image is `L1153` at `/regions/<name>/rect`, with the
/// region name RFC 6901-escaped (an atlas region name carries a `/` by construction).
#[test]
fn a_region_outside_the_packed_image_reports_l1153() {
    let (_, stderr) = check("oversize.atlas.yaml", false);
    assert!(stderr.contains("error[L1153]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/regions/bolt~1spent/rect"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("4x4"), "stderr was: {stderr}");
}

// --- scenes ---------------------------------------------------------------------------------

/// A scene whose `Sprite` names a real region of a real atlas checks clean — the atlas resolves
/// relative to the scene file, the content root a host boots with.
#[test]
fn a_scene_naming_a_good_frame_succeeds() {
    let (stdout, _) = check("scene-good.yaml", true);
    assert!(stdout.contains("1 entity"), "stdout was: {stdout}");
}

/// An unknown `frame` is `L1152` at the component's `frame` field, and the nearest region name is in
/// the report — the whole point of the code.
#[test]
fn a_scene_naming_an_unknown_frame_reports_l1152() {
    let (_, stderr) = check("scene-unknown-frame.yaml", false);
    assert!(stderr.contains("error[L1152]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Sprite/frame"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("did you mean `bolt/idle`?"),
        "stderr was: {stderr}"
    );
}

/// A screen-space `UiRect` resolves its `atlas` + `frame` through the same pass a `Sprite` does, so a
/// HUD icon naming no region is caught at check time (`L1152` at the component's `frame`) rather than
/// failing only at render — the parity `UiRect`'s atlas path owes.
#[test]
fn a_scene_ui_rect_reports_l1152() {
    let (_, stderr) = check("scene-ui-rect-unknown-frame.yaml", false);
    assert!(stderr.contains("error[L1152]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/UiRect/frame"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("did you mean `bolt/idle`?"),
        "stderr was: {stderr}"
    );
}

/// A `Tilemap` legend entry resolves through the same pass, pointed at the glyph's own entry.
#[test]
fn a_scene_tilemap_legend_reports_l1152() {
    let (_, stderr) = check("scene-tilemap-unknown-frame.yaml", false);
    assert!(stderr.contains("error[L1152]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Tilemap/legend/b/frame"),
        "stderr was: {stderr}"
    );
}

/// An `atlas` naming no file is `L1151` at the component's `atlas` field, carrying the read failure
/// as help.
#[test]
fn a_scene_naming_a_missing_atlas_reports_l1151() {
    let (_, stderr) = check("scene-missing-atlas.yaml", false);
    assert!(stderr.contains("error[L1151]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("/entities/0/components/Sprite/atlas"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("absent.atlas.yaml"), "stderr was: {stderr}");
}

// --- kind: palette --------------------------------------------------------------------------

/// A standalone `kind: palette` document — the shared glyph palette many sprites reference instead
/// of inlining the same colours — checks alone, with a summary naming it and its glyph count.
#[test]
fn good_palette_succeeds() {
    let (stdout, _) = check("shared.palette.yaml", true);
    assert!(
        stdout.contains("OK (palette, shared, 2 glyphs)"),
        "stdout was: {stdout}"
    );
}

/// A palette document's own defects are `L1147`, pointed at the offending key and carrying the
/// nearest-match suggestion the sprite family gives.
#[test]
fn a_broken_palette_reports_l1147() {
    let (_, stderr) = check("broken.palette.yaml", false);
    assert!(stderr.contains("error[L1147]"), "stderr was: {stderr}");
    assert!(stderr.contains("--> /palettes"), "stderr was: {stderr}");
    assert!(
        stderr.contains("nearest match is `palette`"),
        "stderr was: {stderr}"
    );
}

/// A sprite that references a shared palette checks clean, and its summary counts the *resolved*
/// palette: two glyphs inherited from the shared document plus the one the include's `overrides`
/// adds, which is what the bake will paint from.
#[test]
fn a_sprite_including_a_shared_palette_succeeds() {
    let (stdout, _) = check("include.sprite.yaml", true);
    assert!(
        stdout.contains("OK (include, 2x2, 3 palette glyphs"),
        "stdout was: {stdout}"
    );
}

/// A `palette.from` naming no readable file is `L1147` at the reference, carrying the path it tried
/// — the include resolves relative to the sprite document's own directory.
#[test]
fn a_missing_palette_include_reports_l1147() {
    let (_, stderr) = check("missing-palette.sprite.yaml", false);
    assert!(stderr.contains("error[L1147]"), "stderr was: {stderr}");
    assert!(stderr.contains("--> /palette/from"), "stderr was: {stderr}");
    assert!(
        stderr.contains("does-not-exist.palette.yaml"),
        "stderr was: {stderr}"
    );
}

/// A `palette.from` pointing at a document that is not a palette is `L1147` too, and the defect is
/// reported at the sprite's own `/palette/from` with the other file's path and pointer in the
/// message — the report is headed with the sprite, so a bare `/kind` would name a key it lacks.
#[test]
fn an_include_naming_a_non_palette_document_reports_l1147() {
    let (_, stderr) = check("wrong-kind-palette.sprite.yaml", false);
    assert!(stderr.contains("error[L1147]"), "stderr was: {stderr}");
    assert!(stderr.contains("--> /palette/from"), "stderr was: {stderr}");
    assert!(stderr.contains("good.sprite.yaml"), "stderr was: {stderr}");
    assert!(
        stderr.contains("a palette document must declare `kind: palette`"),
        "stderr was: {stderr}"
    );
}

/// A glyph the merged palette still does not define is the ordinary `L1153` at the row that uses it:
/// once the include resolves, the sprite has a concrete palette like any inline one.
#[test]
fn a_glyph_undefined_after_the_include_resolves_reports_l1153() {
    let (_, stderr) = check("undefined-glyph-include.sprite.yaml", false);
    assert!(stderr.contains("error[L1153]"), "stderr was: {stderr}");
    assert!(
        stderr.contains("--> /frames/idle/rows/0"),
        "stderr was: {stderr}"
    );
    assert!(stderr.contains("`Z`"), "stderr was: {stderr}");
}

// --- kind: font-source / font -----------------------------------------------------------------

fn gallery_font(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/fonts")
        .join(name)
}

#[test]
fn good_font_source_succeeds() {
    let output = run_check_scene(&gallery_font("silkscreen.font-source.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("OK (font source, silkscreen, bitmap 8 px"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("line height 1250"), "stdout was: {stdout}");
    assert!(stdout.contains("95 chars"), "stdout was: {stdout}");
    assert!(
        stdout.contains("file Silkscreen-Regular.ttf"),
        "stdout was: {stdout}"
    );
}

#[test]
fn good_font_succeeds() {
    let output = run_check_scene(&gallery_font("silkscreen.font.yaml"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.contains("OK (font, bitmap, 95 glyphs, 0 kern pairs, 60x76 atlas"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("from silkscreen.font-source.yaml"),
        "stdout was: {stdout}"
    );
}
