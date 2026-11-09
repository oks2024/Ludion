//! Integration tests for `ludion import <image.png>` — the PNG → `kind: sprite` half of the import
//! verb (sprite-engine Wave 1), driven against the built binary. The median-cut itself is
//! `ludion-sprite`'s to test; these cover the CLI surface: the lossless round trip an author relies
//! on, the `L4014` refusal and its `--quantize` answer, `--out` naming a file or a directory, and
//! the two per-format flag misuses.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

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

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Writes an RGBA8 image as a PNG at `path` and returns its texels back.
fn write_png(path: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Vec<u8> {
    let bytes = ludion_assets::encode_png(width, height, &rgba).expect("the fixture PNG encodes");
    std::fs::write(path, bytes).unwrap();
    rgba
}

/// Three opaque colours plus transparency, in a 4x2 image.
fn three_color_texels() -> Vec<u8> {
    let clear = [0, 0, 0, 0];
    let a = [220, 40, 40, 255];
    let b = [40, 220, 60, 255];
    let c = [30, 30, 200, 255];
    [clear, a, b, c, a, b, c, clear]
        .into_iter()
        .flatten()
        .collect()
}

/// `width * height` distinct opaque colours — more than the 93 glyphs a sprite document has.
fn many_color_texels(width: u32, height: u32) -> Vec<u8> {
    (0..width * height)
        .flat_map(|i| {
            let i = u8::try_from(i % 256).unwrap();
            [i, i.wrapping_mul(7), i.wrapping_mul(13), 255]
        })
        .collect()
}

/// The headline claim: an image whose colours fit the glyph budget imports losslessly. The written
/// document validates, and baking it back reproduces the source PNG's texels exactly.
#[test]
fn a_small_palette_imports_losslessly() {
    let dir = scratch("import_sprite_lossless");
    let input = dir.join("spark.png");
    let expected = write_png(&input, 4, 2, three_color_texels());
    let out = dir.join("spark.sprite.yaml");

    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "import failed: {}",
        stderr(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("wrote") && stdout.contains("(4x2, 3 colours)"),
        "stdout was: {stdout}"
    );
    assert!(
        stderr(&output).is_empty(),
        "a lossless import warns about nothing: {}",
        stderr(&output)
    );

    let doc = ludion_scene::check_sprite(&std::fs::read_to_string(&out).unwrap())
        .expect("the written sprite document validates");
    assert_eq!(doc.name.as_deref(), Some("spark"));
    assert_eq!(doc.size, [4, 2]);

    let frames = ludion_sprite::bake(&doc).expect("the written document bakes");
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0].image.to_srgb8_straight().rgba,
        expected,
        "the import→bake round trip must reproduce the source texels"
    );
}

/// More colours than glyphs is `L4014`, not a silent merge — and nothing is written.
#[test]
fn too_many_colors_is_l4014() {
    let dir = scratch("import_sprite_l4014");
    let input = dir.join("many.png");
    write_png(&input, 20, 10, many_color_texels(20, 10));
    let out = dir.join("many.sprite.yaml");

    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "the import must refuse");
    let text = stderr(&output);
    assert!(text.contains("error[L4014]"), "stderr was: {text}");
    assert!(text.contains("--quantize"), "stderr was: {text}");
    assert!(!out.exists(), "a refused import writes nothing");
}

/// `--quantize N` is the answer `L4014` names: it succeeds, and reports what the merge cost as an
/// `L4015` warning rather than losing colours quietly.
#[test]
fn quantize_succeeds_and_reports_the_loss_as_l4015() {
    let dir = scratch("import_sprite_quantize");
    let input = dir.join("many.png");
    write_png(&input, 20, 10, many_color_texels(20, 10));
    let out = dir.join("many.sprite.yaml");

    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--quantize",
        "8",
    ]);
    assert!(
        output.status.success(),
        "a reported loss is not a failure: {}",
        stderr(&output)
    );
    let text = stderr(&output);
    assert!(text.contains("warning[L4015]"), "stderr was: {text}");
    assert!(
        text.contains("200") && text.contains('8'),
        "stderr was: {text}"
    );

    let doc = ludion_scene::check_sprite(&std::fs::read_to_string(&out).unwrap())
        .expect("the quantized document validates");
    assert_eq!(doc.palette.len(), 8);
}

/// `--out` may name the destination *directory*, in which case the document takes the image's stem;
/// `--name` overrides what goes in the document (and therefore an atlas's region prefix).
#[test]
fn out_may_be_a_directory_and_name_overrides_the_stem() {
    let dir = scratch("import_sprite_out_dir");
    let input = dir.join("spark.png");
    write_png(&input, 4, 2, three_color_texels());

    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
        "--name",
        "ember",
    ]);
    assert!(
        output.status.success(),
        "import failed: {}",
        stderr(&output)
    );

    let written = dir.join("spark.sprite.yaml");
    assert!(written.is_file(), "the document takes the image's stem");
    let doc = ludion_scene::check_sprite(&std::fs::read_to_string(&written).unwrap()).unwrap();
    assert_eq!(doc.name.as_deref(), Some("ember"));
}

/// A PNG import is an editable document (its palette is meant to be hand-fixed), so a second import
/// onto the same path refuses with `L0030` and the hand edit survives — also through a directory
/// `--out`.
#[test]
fn a_second_import_never_overwrites_the_document() {
    let dir = scratch("import_sprite_never_overwrites");
    let input = dir.join("spark.png");
    write_png(&input, 4, 2, three_color_texels());
    let out = dir.join("spark.sprite.yaml");
    let import = |out: &Path| {
        run_ludion(&[
            "import",
            input.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
    };

    let first = import(&out);
    assert!(first.status.success(), "import failed: {}", stderr(&first));
    let edited = format!(
        "{}# hand-fixed palette
",
        std::fs::read_to_string(&out).unwrap()
    );
    std::fs::write(&out, &edited).unwrap();

    for target in [out.clone(), dir.clone()] {
        let again = import(&target);
        assert!(!again.status.success(), "a re-import must refuse");
        let text = stderr(&again);
        assert!(text.contains("error[L0030]"), "stderr was: {text}");
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            edited,
            "the hand edit must survive"
        );
    }
}

/// A file where `--out`'s parent folder belongs is an output I/O error (`L1038`), never the
/// "document exists" refusal (`L0030`). The codes are asserted, not the wording: Windows reports
/// this case as os error 183, whose own text says "already exists".
#[test]
fn a_file_where_a_parent_folder_belongs_is_not_reported_as_the_document_existing() {
    let dir = scratch("import_png_file_parent");
    let png = dir.join("a.png");
    write_png(&png, 4, 2, three_color_texels());
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "a file").unwrap();
    let out = blocker.join("a.sprite.yaml");
    let result = run_ludion(&[
        "import",
        png.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    let err = stderr(&result);
    assert!(err.contains("L1038"), "{err}");
    assert!(!err.contains("L0030"), "{err}");
}

/// Both import codes are answerable, and `ludion explain` states which one stops a build: the
/// refusal is an error, the reported merge only a warning.
#[test]
fn the_import_codes_explain_with_their_severities() {
    for (code, severity) in [("L4014", "error"), ("L4015", "warning")] {
        let output = run_ludion(&["explain", code]);
        assert!(
            output.status.success(),
            "explain {code} failed: {}",
            stderr(&output)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).starts_with(&format!("{code} [{severity}]:")),
            "expected {code} to be a {severity}"
        );
    }
}

/// The two flag sets are per-format, and a flag belonging to the other format is refused rather
/// than ignored.
#[test]
fn per_format_flags_are_refused_against_the_other_format() {
    let dir = scratch("import_sprite_flags");
    let input = dir.join("spark.png");
    write_png(&input, 4, 2, three_color_texels());

    let lossy_png = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        dir.join("spark.sprite.yaml").to_str().unwrap(),
        "--allow-lossy",
    ]);
    assert!(
        !lossy_png.status.success(),
        "--allow-lossy is a glTF flag and must be refused on a PNG"
    );
    assert!(
        stderr(&lossy_png).contains("--allow-lossy"),
        "stderr was: {}",
        stderr(&lossy_png)
    );

    let gltf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import/scene.gltf");
    let quantize_gltf = run_ludion(&[
        "import",
        gltf.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
        "--quantize",
        "8",
    ]);
    assert!(
        !quantize_gltf.status.success(),
        "--quantize is a PNG flag and must be refused on a glTF"
    );
    assert!(
        stderr(&quantize_gltf).contains("--quantize"),
        "stderr was: {}",
        stderr(&quantize_gltf)
    );
}
