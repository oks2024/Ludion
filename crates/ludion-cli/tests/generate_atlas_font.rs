//! Integration tests for `ludion generate-atlas` over a `kind: font-source` and for
//! `ludion render --font`, driven against the built binary. The bake's own pixels are pinned in
//! `ludion-font`; these cover the CLI surface: the pair lands and checks clean, it is
//! byte-deterministic, `--check` catches drift in *either* file, a char the face lacks is listed as
//! `missing:`, and a corrupt face is `L1252` rather than a panic.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The committed gallery face the fixture sources bake. Copied into each scratch dir at test time
/// (the source's `file` resolves beside it) so no second copy of the TTF is committed.
fn inter() -> PathBuf {
    manifest().join("../../examples/gallery/fonts/Inter-Regular.ttf")
}

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

/// Stages the fixture source and the Inter face into a fresh scratch dir; returns the source path.
fn staged(name: &str) -> PathBuf {
    let dir = scratch(name);
    std::fs::copy(inter(), dir.join("Inter-Regular.ttf")).unwrap();
    std::fs::copy(
        manifest().join("tests/fixtures/font/small.font-source.yaml"),
        dir.join("small.font-source.yaml"),
    )
    .unwrap();
    dir.join("small.font-source.yaml")
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn generate_writes_a_pair_check_scene_accepts() {
    let source = staged("font_pair");
    let dir = source.parent().unwrap();
    let output = generate(&source);
    assert!(
        output.status.success(),
        "generate failed: {}",
        stderr(&output)
    );

    let yaml = dir.join("small.font.yaml");
    let checked = run_ludion(&["check-scene", yaml.to_str().unwrap()]);
    assert!(
        checked.status.success(),
        "check-scene: {}",
        stderr(&checked)
    );
    assert!(
        stdout(&checked).contains("OK (font, sdf, 3 glyphs"),
        "{}",
        stdout(&checked)
    );
    assert!(stdout(&checked).contains("from small.font-source.yaml"));

    let text = std::fs::read_to_string(&yaml).unwrap();
    let doc = ludion_scene::check_font(&text).expect("the baked font validates");
    assert_eq!(doc.image, "small.png");
    assert!(
        doc.kerning.iter().any(|(l, r, _)| (*l, *r) == ('A', 'V')),
        "Inter kerns A/V"
    );
    assert!(dir.join("small.png").exists());
}

#[test]
fn baking_twice_is_byte_identical() {
    let source = staged("font_twice");
    let dir = source.parent().unwrap();
    assert!(generate(&source).status.success());
    let first = (
        std::fs::read(dir.join("small.font.yaml")).unwrap(),
        std::fs::read(dir.join("small.png")).unwrap(),
    );
    assert!(generate(&source).status.success());
    assert_eq!(first.0, std::fs::read(dir.join("small.font.yaml")).unwrap());
    assert_eq!(first.1, std::fs::read(dir.join("small.png")).unwrap());
}

#[test]
fn check_passes_when_current_and_names_the_drifted_file() {
    let source = staged("font_drift");
    let dir = source.parent().unwrap();
    assert!(generate(&source).status.success());
    let current = check(&source);
    assert!(current.status.success(), "{}", stderr(&current));
    assert!(stdout(&current).contains("up to date"));

    for (file, marker) in [
        ("small.font.yaml", "small.font.yaml"),
        ("small.png", "small.png"),
    ] {
        let path = dir.join(file);
        let original = std::fs::read(&path).unwrap();
        let mut altered = original.clone();
        altered.push(b'\n');
        std::fs::write(&path, altered).unwrap();

        let drifted = check(&source);
        assert!(!drifted.status.success(), "{file} drift must fail --check");
        let err = stderr(&drifted);
        assert!(err.contains("DRIFT"), "{err}");
        assert!(err.contains(marker), "{err}");

        std::fs::write(&path, original).unwrap();
    }
}

#[test]
fn a_char_the_face_lacks_is_listed_as_missing() {
    let source = staged("font_missing");
    let dir = source.parent().unwrap();
    let text = std::fs::read_to_string(&source).unwrap();
    std::fs::write(&source, text.replace("\"AVI\"", "\"AVI\u{4e00}\"")).unwrap();

    let output = generate(&source);
    assert!(output.status.success(), "{}", stderr(&output));
    let doc =
        ludion_scene::check_font(&std::fs::read_to_string(dir.join("small.font.yaml")).unwrap())
            .unwrap();
    assert_eq!(doc.missing, vec!['\u{4e00}']);
    assert!(stdout(&output).contains("1 missing"), "{}", stdout(&output));
}

#[test]
fn a_corrupt_face_is_l1252_without_a_panic() {
    let source = staged("font_corrupt");
    let dir = source.parent().unwrap();
    std::fs::write(
        dir.join("Inter-Regular.ttf"),
        b"this is not a font file at all",
    )
    .unwrap();

    let output = generate(&source);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1252"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(
        !dir.join("small.font.yaml").exists(),
        "nothing is written on failure"
    );
}

#[test]
fn a_missing_face_file_is_l1252_at_file() {
    let source = staged("font_nofile");
    std::fs::remove_file(source.parent().unwrap().join("Inter-Regular.ttf")).unwrap();

    let output = generate(&source);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1252"), "{err}");
    assert!(err.contains("/file"), "{err}");
}

#[test]
fn another_kind_is_refused_by_name() {
    let dir = scratch("font_wrong_kind");
    let path = dir.join("x.palette.yaml");
    std::fs::write(&path, "format: 1\nkind: palette\n").unwrap();
    let output = generate(&path);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("kind: font-source"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn render_font_writes_a_deterministic_nonempty_sheet() {
    let source = staged("font_render");
    let dir = source.parent().unwrap();
    assert!(generate(&source).status.success());
    let yaml = dir.join("small.font.yaml");
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    for out in [&a, &b] {
        let output = run_ludion(&[
            "render",
            "--font",
            yaml.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let bytes = std::fs::read(&a).unwrap();
    assert!(bytes.len() > 1000, "sheet PNG is {} bytes", bytes.len());
    assert_eq!(bytes, std::fs::read(&b).unwrap());
}

#[test]
fn the_font_codes_explain() {
    for code in ["L1250", "L1251", "L1252"] {
        let output = run_ludion(&["explain", code]);
        assert!(
            output.status.success(),
            "explain {code}: {}",
            stderr(&output)
        );
        assert!(
            stdout(&output).starts_with(&format!("{code} [error]:")),
            "{code}"
        );
    }
}
