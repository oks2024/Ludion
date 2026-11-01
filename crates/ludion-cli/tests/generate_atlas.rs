//! Integration tests for `ludion generate-atlas` (sprite-engine Wave 1), driven against the built
//! binary. The packer's own layout and per-code coverage live in `ludion-sprite`'s unit tests; these
//! cover the CLI surface: the committed pair actually lands, both files are byte-deterministic, the
//! `--check` gate catches a drift in *either* of them, and a broken `inputs:` entry is `L1153`
//! pointing at the entry that broke.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/atlas")
}

/// A stable (not per-process) scratch dir, cleared each run — same rationale as `tests/import.rs`.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copies the `ship` fixture set into a fresh scratch dir and returns the atlas source's path
/// there. The bake writes beside its source, so working on a copy keeps the committed fixtures
/// clean and exercises the default (no `--out-dir`) layout.
fn staged(name: &str) -> PathBuf {
    let dir = scratch(name);
    for file in ["ship.atlas-source.yaml", "ship.sprite.yaml", "spark.png"] {
        std::fs::copy(fixtures().join(file), dir.join(file)).unwrap();
    }
    dir.join("ship.atlas-source.yaml")
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn generate(source: &Path) -> Output {
    run_ludion(&["generate-atlas", source.to_str().unwrap()])
}

fn check(source: &Path) -> Output {
    run_ludion(&["generate-atlas", "--check", source.to_str().unwrap()])
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The bake writes both committed files, the document parses as a real `kind: atlas`, every input
/// minted the region name it should (`<sprite name>/<frame>` for a document, the file stem for a
/// loose PNG), and the PNG decodes to exactly the extent the document's rects imply.
#[test]
fn generate_atlas_writes_a_parseable_pair() {
    let source = staged("atlas_pair");
    let dir = source.parent().unwrap();
    let output = generate(&source);
    assert!(
        output.status.success(),
        "generate-atlas failed: {}",
        stderr(&output)
    );

    let yaml = std::fs::read_to_string(dir.join("ship.atlas.yaml")).unwrap();
    let png = std::fs::read(dir.join("ship.png")).unwrap();
    let doc = ludion_scene::check_atlas(&yaml).expect("the generated atlas validates");

    assert_eq!(doc.generated_from, "ship.atlas-source.yaml");
    assert_eq!(doc.color_map, "ship.png");
    assert_eq!(doc.pixels_per_unit, 8.0);
    for region in ["ship/idle", "ship/bank-left", "spark"] {
        assert!(
            doc.region(region).is_some(),
            "region `{region}` missing from {:?}",
            doc.regions.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        doc.flipbooks.len(),
        2,
        "the source's flipbooks are carried, in authored order"
    );
    assert_eq!(doc.flipbooks[0].name, "ship/fly");
    assert_eq!(doc.flipbooks[1].name, "ship/dive");

    // The image is exactly as large as the document says every region fits inside.
    let (width, height, _) = ludion_assets::decode_png(&png, "ship.png").expect("the PNG decodes");
    for (name, region) in &doc.regions {
        let [x, y, w, h] = region.rect;
        assert!(
            x + w <= width && y + h <= height,
            "region `{name}` {:?} escapes the {width}x{height} image",
            region.rect
        );
    }

    // The one line an author reads: what was written, how big, how many regions.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("wrote") && stdout.contains(&format!("({width}x{height}, 3 regions)")),
        "stdout was: {stdout}"
    );
}

/// Two runs over the same inputs write byte-identical files — the property the `atlas-drift` gate
/// leans on, asserted on both halves of the pair.
#[test]
fn generate_atlas_is_byte_deterministic() {
    let source = staged("atlas_determinism");
    let dir = source.parent().unwrap();

    assert!(generate(&source).status.success());
    let first_yaml = std::fs::read(dir.join("ship.atlas.yaml")).unwrap();
    let first_png = std::fs::read(dir.join("ship.png")).unwrap();

    assert!(generate(&source).status.success());
    assert_eq!(
        first_yaml,
        std::fs::read(dir.join("ship.atlas.yaml")).unwrap(),
        "two runs must write byte-identical atlas documents"
    );
    assert_eq!(
        first_png,
        std::fs::read(dir.join("ship.png")).unwrap(),
        "two runs must write byte-identical packed images"
    );
}

/// `--check` right after a generate is a no-op pass; flipping a byte of *either* committed file, or
/// deleting one, is a `DRIFT` under `L1153` and a non-zero exit.
#[test]
fn check_passes_when_current_and_reports_drift_per_file() {
    let source = staged("atlas_check");
    let dir = source.parent().unwrap();
    assert!(generate(&source).status.success());

    let clean = check(&source);
    assert!(clean.status.success(), "check failed: {}", stderr(&clean));
    assert!(
        String::from_utf8_lossy(&clean.stdout).contains("up to date"),
        "stdout was: {}",
        String::from_utf8_lossy(&clean.stdout)
    );

    // The image drifts on its own — the half a YAML-only comparison would miss.
    let png_path = dir.join("ship.png");
    let mut png = std::fs::read(&png_path).unwrap();
    let last = png.len() - 1;
    png[last] ^= 0xFF;
    std::fs::write(&png_path, &png).unwrap();
    let drifted = check(&source);
    assert!(!drifted.status.success(), "a drifted PNG must fail --check");
    let text = stderr(&drifted);
    assert!(text.contains("L1153"), "stderr was: {text}");
    assert!(
        text.contains("DRIFT") && text.contains("ship.png"),
        "stderr was: {text}"
    );

    // The document drifts on its own too.
    assert!(generate(&source).status.success());
    let yaml_path = dir.join("ship.atlas.yaml");
    let yaml = std::fs::read_to_string(&yaml_path).unwrap();
    std::fs::write(
        &yaml_path,
        yaml.replace("pixels_per_unit: 8.0", "pixels_per_unit: 4.0"),
    )
    .unwrap();
    let drifted = check(&source);
    assert!(
        !drifted.status.success(),
        "a hand-edited atlas document must fail --check"
    );
    let text = stderr(&drifted);
    assert!(
        text.contains("DRIFT") && text.contains("ship.atlas.yaml"),
        "stderr was: {text}"
    );

    // A missing committed file is drift too, not a silent pass.
    assert!(generate(&source).status.success());
    std::fs::remove_file(&yaml_path).unwrap();
    let missing = check(&source);
    assert!(
        !missing.status.success(),
        "a missing document must fail --check"
    );
    let text = stderr(&missing);
    assert!(
        text.contains("L1153") && text.contains("DRIFT"),
        "stderr was: {text}"
    );
}

/// `--out-dir` moves both files together and re-points `generated_from:` at the source from there,
/// so the pair stays regenerable from wherever it landed.
#[test]
fn out_dir_moves_both_files_and_repoints_the_provenance() {
    let source = staged("atlas_out_dir");
    let dir = source.parent().unwrap().join("baked");
    std::fs::create_dir_all(&dir).unwrap();

    let output = run_ludion(&[
        "generate-atlas",
        source.to_str().unwrap(),
        "--out-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "generate-atlas --out-dir failed: {}",
        stderr(&output)
    );
    assert!(dir.join("ship.png").is_file());
    let doc =
        ludion_scene::check_atlas(&std::fs::read_to_string(dir.join("ship.atlas.yaml")).unwrap())
            .expect("the generated atlas validates");
    assert_eq!(doc.generated_from, "../ship.atlas-source.yaml");
}

/// The document names the PNG, so it lands second: a PNG that cannot be written leaves no document.
#[test]
fn a_failed_png_write_leaves_no_document() {
    let source = staged("atlas_png_fails");
    let dir = source.parent().unwrap();
    std::fs::create_dir(dir.join("ship.png")).unwrap();

    let output = generate(&source);
    assert!(!output.status.success());
    assert!(!dir.join("ship.atlas.yaml").exists());
}

/// Every code this verb can raise is answerable — the promise each diagnostic's `L####` makes.
#[test]
fn the_codes_this_verb_raises_explain() {
    let output = run_ludion(&["explain", "L1153"]);
    assert!(
        output.status.success(),
        "explain L1153 failed: {}",
        stderr(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("L1153 [error]:"),
        "a malformed sprite/atlas document is a hard error"
    );
}

/// An `inputs:` entry naming a file that is not there is `L1153` pointed at that entry — the
/// diagnostic an author fixes by editing one line of the source.
#[test]
fn a_missing_input_is_l1153_at_its_own_pointer() {
    let source = fixtures().join("missing-input.atlas-source.yaml");
    let out = scratch("atlas_missing_input");
    let output = run_ludion(&[
        "generate-atlas",
        source.to_str().unwrap(),
        "--out-dir",
        out.to_str().unwrap(),
    ]);

    assert!(
        !output.status.success(),
        "an unresolvable input must not write an atlas"
    );
    let text = stderr(&output);
    assert!(text.contains("error[L1153]"), "stderr was: {text}");
    assert!(text.contains("/inputs/0"), "stderr was: {text}");
    assert!(
        std::fs::read_dir(&out).unwrap().next().is_none(),
        "a failed bake writes nothing"
    );
}

/// A sprite whose palette lives in a shared `kind: palette` document bakes to the *same bytes* as the
/// same sprite with that palette inlined: `generate-atlas` resolves the include before the bake, so
/// moving a game's thirty sprite documents onto one palette re-blesses no committed image and the
/// `atlas-drift` gate stays byte-exact either way. (`examples/gallery/sprites/emblem.*` is the
/// committed instance of this; here both halves are baked side by side so the equality is asserted,
/// not assumed.)
#[test]
fn an_included_palette_bakes_to_the_inline_bytes() {
    let inline_source = staged("atlas_palette_inline");
    let inline_dir = inline_source.parent().unwrap().to_path_buf();
    assert!(generate(&inline_source).status.success());

    // The same fixture set, with the sprite's inline palette lifted into a palette document it
    // references. Lifting it through the document API rather than by hand keeps the two palettes
    // provably equal — the point under test is the resolution, not a hand-copied colour.
    let include_source = staged("atlas_palette_include");
    let include_dir = include_source.parent().unwrap().to_path_buf();
    let sprite_path = include_dir.join("ship.sprite.yaml");
    let doc = ludion_scene::check_sprite(&std::fs::read_to_string(&sprite_path).unwrap())
        .expect("the fixture sprite is valid");
    std::fs::write(
        include_dir.join("ship.palette.yaml"),
        ludion_scene::emit_palette(&ludion_scene::PaletteDocument {
            name: Some("ship".to_string()),
            palette: doc.palette.clone(),
            ramps: Vec::new(),
        }),
    )
    .unwrap();
    let mut including = doc;
    including.palette = Vec::new();
    including.palette_include = Some(ludion_scene::SpritePaletteInclude {
        from: "ship.palette.yaml".to_string(),
        overrides: Vec::new(),
    });
    std::fs::write(&sprite_path, ludion_scene::emit_sprite(&including)).unwrap();

    let output = generate(&include_source);
    assert!(
        output.status.success(),
        "the including sprite must bake: {}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read(include_dir.join("ship.png")).unwrap(),
        std::fs::read(inline_dir.join("ship.png")).unwrap(),
        "an included palette must pack byte-identical pixels"
    );
    assert_eq!(
        std::fs::read(include_dir.join("ship.atlas.yaml")).unwrap(),
        std::fs::read(inline_dir.join("ship.atlas.yaml")).unwrap(),
        "and a byte-identical atlas document"
    );
}

/// Rewrites the staged atlas source's `inputs:` to `inputs_yaml` (a block of `  - …` lines) and
/// optionally appends `extra` (more top-level keys), keeping the rest of the fixture.
fn with_inputs(source: &Path, inputs_yaml: &str, extra: &str) {
    let text = std::fs::read_to_string(source).unwrap();
    let head = text.split("inputs:").next().unwrap();
    std::fs::write(source, format!("{head}inputs:\n{inputs_yaml}{extra}")).unwrap();
}

/// The texels of the region `name` in the baked `ship.png`.
fn region_texels(dir: &Path, name: &str) -> (u32, u32, Vec<[u8; 4]>) {
    let doc =
        ludion_scene::check_atlas(&std::fs::read_to_string(dir.join("ship.atlas.yaml")).unwrap())
            .expect("the generated atlas validates");
    let [x, y, w, h] = doc
        .region(name)
        .unwrap_or_else(|| panic!("no region {name}"))
        .rect;
    let png = std::fs::read(dir.join("ship.png")).unwrap();
    let (width, _, rgba) = ludion_assets::decode_png(&png, "ship.png").unwrap();
    let mut out = Vec::new();
    for row in y..y + h {
        for col in x..x + w {
            let at = ((row * width + col) * 4) as usize;
            out.push([rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]);
        }
    }
    (w, h, out)
}

/// A variant input mints its own region prefix and recolours only the glyph it overrides: the plain
/// and the recoloured region are the same size, and every texel that differs is a recoloured one.
#[test]
fn a_variant_mints_its_own_prefix_and_recolours() {
    let source = staged("atlas_variant");
    let dir = source.parent().unwrap().to_path_buf();
    // The fixture's outline and shadow effects repaint the `o` texels (they sit on the silhouette
    // edge), so the scratch copy drops the effects to make the recolour exactly visible.
    let sprite_path = dir.join("ship.sprite.yaml");
    let sprite = std::fs::read_to_string(&sprite_path).unwrap();
    std::fs::write(&sprite_path, sprite.split("effects:").next().unwrap()).unwrap();
    with_inputs(
        &source,
        "  - ship.sprite.yaml\n  - path: ship.sprite.yaml\n    name: ship-red\n    palette:\n      o:\n        color: \"#ff0000\"\n        emissive: 2.0\n",
        "flipbooks:\n  - name: ship-red/fly\n    frames:\n      - ship-red/idle\n      - ship-red/bank-left\n    fps: 8.0\n",
    );
    let output = generate(&source);
    assert!(output.status.success(), "{}", stderr(&output));

    let (w, h, plain) = region_texels(&dir, "ship/idle");
    let (rw, rh, red) = region_texels(&dir, "ship-red/idle");
    assert_eq!((w, h), (rw, rh), "a variant is the same size as its base");
    let differing: Vec<usize> = (0..plain.len()).filter(|&i| plain[i] != red[i]).collect();
    let width = w as usize;
    // The idle frame's last row is `..o..o..`: those two texels, and only those, change.
    assert_eq!(
        differing,
        vec![5 * width + 2, 5 * width + 5],
        "exactly the `o` texels recolour"
    );
    let atlas = std::fs::read_to_string(dir.join("ship.atlas.yaml")).unwrap();
    let atlas = ludion_scene::check_atlas(&atlas).unwrap();
    assert!(atlas.flipbook("ship-red/fly").is_some());
}

/// A second input of the same document without a distinct `name` collides on the minted regions.
#[test]
fn a_variant_without_a_name_collides_with_the_plain_input() {
    let source = staged("atlas_variant_collision");
    with_inputs(
        &source,
        "  - ship.sprite.yaml\n  - path: ship.sprite.yaml\n    palette:\n      o:\n        color: \"#ff0000\"\n        emissive: 2.0\n",
        "",
    );
    let output = generate(&source);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("L1153"), "stderr was: {text}");
    assert!(text.contains("/inputs/1"), "stderr was: {text}");
    assert!(text.contains("ship/idle"), "stderr was: {text}");
}

/// A variant recolouring a glyph the sprite's palette lacks is a typo, not an addition.
#[test]
fn a_variant_recolouring_an_absent_glyph_is_refused() {
    let source = staged("atlas_variant_dead_glyph");
    with_inputs(
        &source,
        "  - ship.sprite.yaml\n  - path: ship.sprite.yaml\n    name: ship-red\n    palette:\n      z:\n        color: \"#ff0000\"\n",
        "",
    );
    let output = generate(&source);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("L1153"), "stderr was: {text}");
    assert!(text.contains("/inputs/1/palette/z"), "stderr was: {text}");
}

/// Stages the ship with its effects replaced by one explicitly coloured `light` (so a recoloured
/// source lights its neighbours identically), plus a plain input and a recoloured `ship-red` variant.
fn lit_variant(name: &str, override_yaml: &str) -> PathBuf {
    let source = staged(name);
    let dir = source.parent().unwrap().to_path_buf();
    let sprite_path = dir.join("ship.sprite.yaml");
    let sprite = std::fs::read_to_string(&sprite_path).unwrap();
    let head = sprite.split("effects:").next().unwrap();
    std::fs::write(
        &sprite_path,
        format!("{head}effects:\n  - op: light\n    radius_px: 2.0\n    strength: 1.0\n    color: \"#ffffff\"\n"),
    )
    .unwrap();
    with_inputs(
        &source,
        &format!(
            "  - ship.sprite.yaml\n  - path: ship.sprite.yaml\n    name: ship-red\n    palette:\n      o:\n        color: \"#ff0000\"\n{override_yaml}"
        ),
        "",
    );
    source
}

/// A variant override replaces the whole entry, so recolouring a light source without repeating its
/// `emissive` would silently drop the light: refused at the override.
#[test]
fn a_variant_dropping_an_emissive_glyphs_emissive_is_refused() {
    let source = lit_variant("atlas_variant_drops_emissive", "");
    let output = generate(&source);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("L1153"), "stderr was: {text}");
    assert!(text.contains("/inputs/1/palette/o"), "stderr was: {text}");
    assert!(text.contains("emissive: 2"), "stderr was: {text}");
}

#[test]
fn a_variant_repeating_emissive_keeps_the_base_light() {
    let source = lit_variant("atlas_variant_keeps_emissive", "        emissive: 2.0\n");
    let output = generate(&source);
    assert!(output.status.success(), "{}", stderr(&output));
    let dir = source.parent().unwrap();
    let (w, _, plain) = region_texels(dir, "ship/idle");
    let (_, _, red) = region_texels(dir, "ship-red/idle");
    let differing: Vec<usize> = (0..plain.len()).filter(|&i| plain[i] != red[i]).collect();
    let width = w as usize;
    assert_eq!(
        differing,
        vec![5 * width + 2, 5 * width + 5],
        "only the source texels recolour; the light it casts is the base's"
    );
}

/// `emissive: 0` is the deliberate way to drop a source; with the only source gone the light has
/// nothing to shine from.
#[test]
fn a_variant_dropping_emissive_on_purpose_leaves_the_light_sourceless() {
    let source = lit_variant("atlas_variant_zero_emissive", "        emissive: 0.0\n");
    let output = generate(&source);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("L1154"), "stderr was: {text}");
}
