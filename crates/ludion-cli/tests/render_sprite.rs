//! Integration tests for `ludion render --sprite <DOC>` and `ludion render --atlas <DOC>`
//! (sprite-engine Wave 1), driven against the built binary.
//!
//! Both modes are CPU-only — no adapter is ever requested — so unlike `render.rs`/`render_model.rs`
//! nothing here skips on a machine with no GPU. The sheet *composition* is `ludion-engine`'s and is
//! unit-tested there; these cover the CLI surface: the flag wiring (conflicts, the whole-number
//! `--scale` refusal), that `--set`/`--sweep` reach the sprite document's own override domain, the
//! stdout echo a terminal-only session reads instead of the image, and the defaulted `--out` — plus
//! the same three for `--atlas --clip`'s flipbook instruments (the instant, the strip, the onion
//! skin), including their four `L4008` refusals and their three separate defaulted names.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/atlas")
}

fn sprite_doc() -> PathBuf {
    fixtures().join("ship.sprite.yaml")
}

/// A stable (not per-process) scratch dir, cleared each run — `generate_atlas.rs`'s rationale.
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

/// Arguments go through as a `Vec`, never a shell string: a sprite override pointer is full of
/// characters (`/`, `#`) a shell would rewrite.
fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn run_ludion_in(dir: &Path, args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Decodes a written sheet, asserting it is a real PNG, and returns its raw pixels.
fn decoded(path: &Path) -> (u32, u32, Vec<u8>) {
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("sheet {path:?} was not written: {e}"));
    ludion_assets::decode_png(&bytes, &path.display().to_string()).expect("the sheet decodes")
}

/// The plain contact sheet: it lands, it decodes, and its stdout names the effects the document
/// applied — with no `set` line, because nothing was overridden.
#[test]
fn contact_sheet_writes_a_png_and_echoes_the_effects() {
    let dir = scratch("render-sprite-plain");
    let out = dir.join("sheet.png");
    let output = run_ludion(&[
        "render",
        "--sprite",
        sprite_doc().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let (width, height, _) = decoded(&out);
    assert!(width > 0 && height > 0);

    let text = stdout(&output);
    assert!(text.contains("wrote"), "stdout: {text}");
    assert!(
        text.contains("effect") && text.contains("outline"),
        "the fixture's first effect is an outline; stdout: {text}"
    );
    assert!(
        !text.contains("set /"),
        "nothing was overridden, so nothing may be echoed as a set; stdout: {text}"
    );
}

/// `--set` reaches the sprite document's palette: the override is echoed with its prior value, and
/// the sheet is a different image than the un-set one.
#[test]
fn set_edits_the_palette_and_is_echoed() {
    let dir = scratch("render-sprite-set");
    let plain = dir.join("plain.png");
    let edited = dir.join("edited.png");
    let doc = sprite_doc();

    let first = run_ludion(&[
        "render",
        "--sprite",
        doc.to_str().unwrap(),
        "--out",
        plain.to_str().unwrap(),
    ]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));

    let second = run_ludion(&[
        "render",
        "--sprite",
        doc.to_str().unwrap(),
        "--out",
        edited.to_str().unwrap(),
        "--set",
        "/palette/#/color/0=0.0",
    ]);
    assert!(second.status.success(), "stderr: {}", stderr(&second));

    let text = stdout(&second);
    assert!(text.contains("set /palette/#/color/0"), "stdout: {text}");

    let (_, _, before) = decoded(&plain);
    let (_, _, after) = decoded(&edited);
    assert_ne!(
        before, after,
        "zeroing the hull's red channel must change the sheet"
    );
}

/// `--sweep` bakes one candidate per value. The pointer is a real parameter of the fixture's first
/// effect (`outline`'s colour), not a strength — the op has none.
#[test]
fn sweep_renders_one_cell_per_value() {
    let dir = scratch("render-sprite-sweep");
    let out = dir.join("sweep.png");
    let output = run_ludion(&[
        "render",
        "--sprite",
        sprite_doc().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--sweep",
        "/effects/0/color/0=0.1,0.9",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let (width, height, _) = decoded(&out);
    assert!(width > 0 && height > 0);
}

/// A pointer outside the supported shapes is the override layer's own `L4008`, not a panic and not
/// a silently ignored flag.
#[test]
fn an_unsupported_pointer_is_l4008() {
    let dir = scratch("render-sprite-bad-pointer");
    let out = dir.join("never.png");
    let output = run_ludion(&[
        "render",
        "--sprite",
        sprite_doc().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--set",
        "/frames/idle/rows/0=x",
    ]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("L4008"),
        "stderr: {}",
        stderr(&output)
    );
}

/// `--scale` is the block-B game scale here: whole sheet pixels per texel. A fraction is refused
/// before anything bakes; a whole number renders.
#[test]
fn scale_must_be_a_whole_number() {
    let dir = scratch("render-sprite-scale");
    let doc = sprite_doc();

    let fractional = run_ludion(&[
        "render",
        "--sprite",
        doc.to_str().unwrap(),
        "--out",
        dir.join("frac.png").to_str().unwrap(),
        "--scale",
        "2.5",
    ]);
    assert!(!fractional.status.success());
    assert!(
        stderr(&fractional).contains("whole number"),
        "stderr: {}",
        stderr(&fractional)
    );

    let whole = run_ludion(&[
        "render",
        "--sprite",
        doc.to_str().unwrap(),
        "--out",
        dir.join("whole.png").to_str().unwrap(),
        "--scale",
        "2",
    ]);
    assert!(whole.status.success(), "stderr: {}", stderr(&whole));
    decoded(&dir.join("whole.png"));
}

/// The render targets are one `ArgGroup`, so naming two is clap's own usage error (exit 2), never a
/// silently preferred one.
#[test]
fn sprite_conflicts_with_scene() {
    let output = run_ludion(&[
        "render",
        "--sprite",
        sprite_doc().to_str().unwrap(),
        "--scene",
        "examples/scenes/empty.yaml",
        "--out",
        "unused.png",
    ]);
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
}

/// The atlas overview: bake the fixture set into a scratch dir first (so no second PNG is committed),
/// then draw the packed image beside its own index.
#[test]
fn atlas_overview_writes_a_png() {
    let dir = scratch("render-sprite-atlas");
    let baked = run_ludion(&[
        "generate-atlas",
        fixtures().join("ship.atlas-source.yaml").to_str().unwrap(),
        "--out-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(baked.status.success(), "stderr: {}", stderr(&baked));

    let out = dir.join("atlas.png");
    let output = run_ludion(&[
        "render",
        "--atlas",
        dir.join("ship.atlas.yaml").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("wrote"), "stdout: {text}");
    assert!(
        !text.contains("clip "),
        "with no `--clip` this is the packing overview, not a flipbook; stdout: {text}"
    );

    let (width, _, _) = decoded(&out);
    assert!(
        width >= 512,
        "the overview pads to at least its minimum width; got {width}"
    );
}

/// Bakes the fixture set into a fresh scratch dir and returns it — the starting point for every
/// flipbook test, since the instruments read the packed pair rather than the source.
fn baked_atlas(name: &str) -> PathBuf {
    let dir = scratch(name);
    let baked = run_ludion(&[
        "generate-atlas",
        fixtures().join("ship.atlas-source.yaml").to_str().unwrap(),
        "--out-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(baked.status.success(), "stderr: {}", stderr(&baked));
    dir
}

/// One instant of a flipbook: the entry showing at `--time`, written to the defaulted
/// `<stem>.atlas-frame.png` and echoed with the clip's own shape.
#[test]
fn atlas_clip_renders_one_instant_at_a_defaulted_out() {
    let dir = baked_atlas("render-sprite-clip-frame");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--atlas",
            "ship.atlas.yaml",
            "--clip",
            "ship/fly",
            "--time",
            "0.13",
            "--scale",
            "4",
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("clip ship/fly"), "stdout: {text}");
    assert!(
        text.contains("8 fps") && text.contains("loop"),
        "stdout: {text}"
    );
    assert!(
        text.contains("phase 0") && text.contains("ship/bank-left"),
        "0.13 s at 8 fps is the second entry; stdout: {text}"
    );

    let (width, height, _) = decoded(&dir.join("ship.atlas-frame.png"));
    assert!(width > 0 && height > 0);
}

/// A strip lays one cell out per phase: four echoed phases, and an image wider than the same clip's
/// two-phase strip. The cell arithmetic itself is `ludion-engine`'s unit test.
#[test]
fn atlas_clip_strip_draws_one_cell_per_phase() {
    let dir = baked_atlas("render-sprite-clip-strip");
    let strip = |phases: &str, out: &str| {
        let output = run_ludion(&[
            "render",
            "--atlas",
            dir.join("ship.atlas.yaml").to_str().unwrap(),
            "--clip",
            "ship/fly",
            "--strip",
            phases,
            "--scale",
            "4",
            "--out",
            dir.join(out).to_str().unwrap(),
        ]);
        assert!(output.status.success(), "stderr: {}", stderr(&output));
        output
    };

    let four = stdout(&strip("4", "four.png"));
    assert_eq!(
        four.matches("phase ").count(),
        4,
        "one echoed phase per cell; stdout: {four}"
    );
    let sixteen = stdout(&strip("16", "sixteen.png"));
    assert_eq!(sixteen.matches("phase ").count(), 16, "stdout: {sixteen}");

    // Sixteen cells of the same clip are wider than four of it. (A four-cell strip of this small
    // fixture is still as wide as its own header line, so the four-to-sixteen step is the one that
    // shows the cells rather than the caption; the exact per-cell arithmetic is `ludion-engine`'s
    // unit test.)
    let (wide, _, _) = decoded(&dir.join("sixteen.png"));
    let (narrow, _, _) = decoded(&dir.join("four.png"));
    assert!(
        wide > narrow,
        "sixteen cells must be wider than four; {wide} vs {narrow}"
    );
}

/// A one-shot strip shows both endpoints, so its last phase is the clip's last entry — the `n − 1`
/// spacing, driven end to end.
#[test]
fn a_one_shot_strip_ends_on_the_clips_last_entry() {
    let dir = baked_atlas("render-sprite-clip-one-shot");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--atlas",
            "ship.atlas.yaml",
            "--clip",
            "ship/dive",
            "--strip",
            "3",
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert!(text.contains("one-shot"), "stdout: {text}");
    let last = text
        .lines()
        .rfind(|line| line.trim_start().starts_with("phase "))
        .expect("three echoed phases");
    assert!(
        last.contains("t=0.250s") && last.contains("ship/bank-left"),
        "the final phase is the clip's own end; got: {last}"
    );
    decoded(&dir.join("ship.atlas-strip.png"));
}

/// The onion skin writes its own defaulted name and echoes every overlaid entry, the last one
/// showing.
#[test]
fn atlas_clip_onion_overlays_every_entry() {
    let dir = baked_atlas("render-sprite-clip-onion");
    let output = run_ludion_in(
        &dir,
        &[
            "render",
            "--atlas",
            "ship.atlas.yaml",
            "--clip",
            "ship/fly",
            "--onion",
            "--scale",
            "4",
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let text = stdout(&output);
    assert_eq!(text.matches("onion ").count(), 2, "stdout: {text}");
    assert!(
        text.contains("ghost") && text.contains("showing"),
        "stdout: {text}"
    );
    decoded(&dir.join("ship.atlas-onion.png"));
}

/// The four ways a flipbook request is refused, each `L4008` rather than a silently ignored flag or
/// a panic: a modifier with no clip, both layouts at once, a phase count out of range, and a clip
/// name the atlas does not carry.
#[test]
fn the_flipbook_refusals_are_l4008() {
    let dir = baked_atlas("render-sprite-clip-refusals");
    let atlas = dir.join("ship.atlas.yaml");
    let out = dir.join("never.png");
    let refused = |extra: &[&str]| {
        let mut args = vec![
            "render",
            "--atlas",
            atlas.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let output = run_ludion(&args);
        assert!(
            !output.status.success(),
            "expected a refusal for {extra:?}; stdout: {}",
            stdout(&output)
        );
        assert!(
            stderr(&output).contains("L4008"),
            "for {extra:?}; stderr: {}",
            stderr(&output)
        );
    };

    refused(&["--time", "0.2"]);
    refused(&["--clip", "ship/fly", "--strip", "4", "--onion"]);
    refused(&["--clip", "ship/fly", "--strip", "1"]);
    refused(&["--clip", "ship/nope"]);
    assert!(!out.exists(), "a refused request writes nothing");
}

/// Omitting `--out` defaults it the way `--model` does: `<stem minus .sprite>.sheet.png` in the
/// working directory.
#[test]
fn out_defaults_to_the_document_stem() {
    let dir = scratch("render-sprite-default-out");
    let output = run_ludion_in(
        &dir,
        &["render", "--sprite", sprite_doc().to_str().unwrap()],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    decoded(&dir.join("ship.sheet.png"));
}

/// `ship.sprite.yaml` and the `ship.atlas.yaml` baked from it share a stem, so the two modes must
/// default to two different names — otherwise the second sheet silently replaces the first.
#[test]
fn the_two_default_names_do_not_collide_on_a_shared_stem() {
    let dir = scratch("render-sprite-default-out-pair");
    let baked = run_ludion(&[
        "generate-atlas",
        fixtures().join("ship.atlas-source.yaml").to_str().unwrap(),
        "--out-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(baked.status.success(), "stderr: {}", stderr(&baked));

    let sprite = run_ludion_in(
        &dir,
        &["render", "--sprite", sprite_doc().to_str().unwrap()],
    );
    assert!(sprite.status.success(), "stderr: {}", stderr(&sprite));

    let atlas = run_ludion_in(&dir, &["render", "--atlas", "ship.atlas.yaml"]);
    assert!(atlas.status.success(), "stderr: {}", stderr(&atlas));

    let contact = dir.join("ship.sheet.png");
    let overview = dir.join("ship.atlas-sheet.png");
    let (_, _, contact_pixels) = decoded(&contact);
    let (_, _, overview_pixels) = decoded(&overview);
    assert_ne!(
        contact_pixels, overview_pixels,
        "the two defaults must name two files, so neither sheet is the other"
    );
}

/// `--steps` writes `<stem>.steps.png` by default and echoes one `steps` line per row.
#[test]
fn steps_writes_the_steps_sheet_at_its_default_name() {
    let dir = scratch("render-sprite-steps");
    let doc = sprite_doc();
    let output = run_ludion_in(
        &dir,
        &["render", "--sprite", doc.to_str().unwrap(), "--steps"],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    decoded(&dir.join("ship.steps.png"));
    let text = stdout(&output);
    assert!(text.contains("steps idle: paint, 0, 1"), "stdout: {text}");
    assert!(
        text.contains("steps bank-left: paint, 0, 1"),
        "stdout: {text}"
    );

    let one = run_ludion_in(
        &dir,
        &[
            "render",
            "--sprite",
            doc.to_str().unwrap(),
            "--steps",
            "--frame",
            "idle",
        ],
    );
    assert!(one.status.success(), "stderr: {}", stderr(&one));
    decoded(&dir.join("ship-idle.steps.png"));
    assert!(
        !stdout(&one).contains("bank-left"),
        "stdout: {}",
        stdout(&one)
    );
}

/// `--frame` alone is `L4008`; `--steps` with `--sweep` is clap's conflict.
#[test]
fn steps_flag_misuse_is_refused() {
    let dir = scratch("render-sprite-steps-refuse");
    let doc = sprite_doc();
    let orphan = run_ludion_in(
        &dir,
        &[
            "render",
            "--sprite",
            doc.to_str().unwrap(),
            "--frame",
            "idle",
        ],
    );
    assert!(!orphan.status.success());
    assert!(
        stderr(&orphan).contains("L4008"),
        "stderr: {}",
        stderr(&orphan)
    );

    let both = run_ludion_in(
        &dir,
        &[
            "render",
            "--sprite",
            doc.to_str().unwrap(),
            "--steps",
            "--sweep",
            "/effects/1/offset/0=1,2",
        ],
    );
    assert_eq!(both.status.code(), Some(2), "stderr: {}", stderr(&both));
    assert!(
        stderr(&both).contains("cannot be used with"),
        "stderr: {}",
        stderr(&both)
    );
}
