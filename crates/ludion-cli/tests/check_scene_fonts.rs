//! Integration tests for `ludion check-scene`'s font pass (`Plan/font-assets.md` decision 8): an
//! unknown `font` name is `L1254`, characters a table face lacks are an `L1256` warning, and the
//! embedded face is never coverage-checked. Driven against the built `ludion` binary, over a scratch
//! content root holding a copy of the gallery's Inter face.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-check-fonts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).expect("scratch dir");
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in ["inter.font.yaml", "inter.png"] {
        std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).expect("copy face");
    }
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n",
    )
    .expect("table");
    dir
}

fn check(dir: &Path, scene: &str) -> Output {
    let path = dir.join("scene.yaml");
    std::fs::write(&path, format!("format: 1\nentities:\n{scene}")).expect("scene");
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("check-scene")
        .arg(&path)
        .output()
        .expect("failed to run the ludion binary")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_unknown_font_name_is_l1254_with_a_suggestion() {
    let dir = root("unknown");
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: HI\n        font: bdoy\n",
    );
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1254"), "stderr was: {err}");
    assert!(err.contains("did you mean `body`"), "stderr was: {err}");
    assert!(
        err.contains("/entities/0/components/UiText/font"),
        "stderr was: {err}"
    );
}

#[test]
fn a_font_without_a_table_points_at_fonts_yaml() {
    let dir = root("notable");
    std::fs::remove_file(dir.join("fonts.yaml")).unwrap();
    let output = check(
        &dir,
        "  - name: label\n    components:\n      Text:\n        text: HI\n        font: body\n",
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("add a `fonts.yaml`"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn uncovered_chars_on_a_table_face_warn_and_pass() {
    let dir = root("coverage");
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: \"HI \u{2603}\"\n        font: body\n",
    );
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    let err = stderr(&output);
    assert!(err.contains("L1256"), "stderr was: {err}");
    assert!(err.contains("charset.chars"), "stderr was: {err}");
}

#[test]
fn the_embedded_face_is_never_coverage_checked() {
    let dir = root("embedded");
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: \"a ~ b\"\n",
    );
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    assert!(!stderr(&output).contains("L1256"), "{}", stderr(&output));
}

#[test]
fn a_malformed_table_keeps_l1253() {
    let dir = root("badtable");
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: HI\n",
    );
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1253"), "{err}");
    assert!(err.contains("fonts.yaml: 1 error found"), "{err}");
    assert!(!err.contains("scene.yaml"), "{err}");
}

#[test]
fn a_bad_glyph_rect_names_the_face_file_with_its_pointer() {
    let dir = root("badface");
    let face = dir.join("fonts/inter.font.yaml");
    let doc = std::fs::read_to_string(&face).unwrap();
    std::fs::write(
        &face,
        doc.replace("\nascender: ", "\nascender: 2147483647 #"),
    )
    .unwrap();
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: HI\n",
    );
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("L1255"), "{err}");
    assert!(err.contains("/ascender"), "{err}");
    assert!(err.contains("inter.font.yaml: 1 error found"), "{err}");
    assert!(!err.contains("scene.yaml"), "{err}");
}

#[test]
fn the_generate_atlas_hint_is_root_relative() {
    let dir = root("hint");
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: \"\u{2603}\"\n        font: body\n",
    );
    assert!(
        stderr(&output).contains("generate-atlas fonts/inter.font-source.yaml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn explain_knows_the_font_codes() {
    for code in ["L1253", "L1254", "L1255", "L1256"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["explain", code])
            .output()
            .expect("run explain");
        assert!(output.status.success(), "explain {code} failed");
    }
}

#[test]
fn line_breaks_and_tabs_are_not_uncovered_chars() {
    let dir = root("breaks");
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: \"HI\\r\\nYOU\\tTOO\"\n        font: body\n",
    );
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    assert!(
        !stderr(&output).contains("L1256"),
        "stderr was: {}",
        stderr(&output)
    );
}

/// A root whose `body` face is the ASCII-only bitmap Silkscreen, with English and French tables;
/// `fr_override` is the `fonts.yaml` `locales:` entry for `fr`, if any.
fn keyed_root(name: &str, fr_override: Option<&str>) -> PathBuf {
    let dir = root(name);
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in ["silkscreen.font.yaml", "silkscreen.png"] {
        std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).expect("copy face");
    }
    let locales = fr_override.map_or(String::new(), |path| {
        format!("locales:\n  fr:\n    body: {path}\n")
    });
    std::fs::write(
        dir.join("fonts.yaml"),
        format!("format: 1\nkind: fonts\nfonts:\n  body: fonts/silkscreen.font.yaml\n{locales}"),
    )
    .expect("table");
    std::fs::create_dir_all(dir.join("strings")).expect("strings dir");
    std::fs::write(
        dir.join("strings/en.yaml"),
        "format: 1\nkind: strings\nlocale: en\nstrings:\n  hud.start: Start {count}\n",
    )
    .expect("en");
    std::fs::write(
        dir.join("strings/fr.yaml"),
        "format: 1\nkind: strings\nlocale: fr\nstrings:\n  hud.start: \"D\u{e9}marrer {count}\"\n",
    )
    .expect("fr");
    dir
}

const KEYED: &str =
    "  - name: label\n    components:\n      UiText:\n        key: hud.start\n        font: body\n";

#[test]
fn a_key_missing_from_the_default_locale_is_l1715() {
    let dir = keyed_root("keyed-missing", None);
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        key: hud.nope\n",
    );
    assert!(!output.status.success(), "stderr was: {}", stderr(&output));
    let err = stderr(&output);
    assert!(err.contains("L1715"), "stderr was: {err}");
    assert!(err.contains("hud.nope"), "stderr was: {err}");
    assert!(
        err.contains("/entities/0/components/UiText/key"),
        "stderr was: {err}"
    );
}

#[test]
fn a_locale_string_the_locales_face_lacks_warns_l1256_naming_the_locale() {
    let dir = keyed_root("keyed-glyph", None);
    let output = check(&dir, KEYED);
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    let err = stderr(&output);
    assert!(err.contains("L1256"), "stderr was: {err}");
    assert!(err.contains("in locale `fr`"), "stderr was: {err}");
    assert!(!err.contains("in locale `en`"), "stderr was: {err}");
    assert!(err.contains("\u{e9}"), "stderr was: {err}");
}

#[test]
fn a_locale_face_override_with_the_glyph_clears_the_warning() {
    let dir = keyed_root("keyed-override", Some("fonts/inter.font.yaml"));
    let output = check(&dir, KEYED);
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    assert!(!stderr(&output).contains("L1256"), "{}", stderr(&output));
}

#[test]
fn a_scene_without_keys_ignores_the_string_tables() {
    let dir = keyed_root("keyed-none", None);
    let output = check(
        &dir,
        "  - name: label\n    components:\n      UiText:\n        text: HI\n        font: body\n",
    );
    assert!(output.status.success(), "stderr was: {}", stderr(&output));
    let err = stderr(&output);
    assert!(!err.contains("L1715") && !err.contains("L1256"), "{err}");
}
