//! Unit tests for the font library (`fonts.rs`).

use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-fonts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).expect("scratch dir");
    dir
}

fn gallery_fonts() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts")
}

fn copy_face(dir: &Path, stem: &str) {
    for ext in ["font.yaml", "png"] {
        let file = format!("{stem}.{ext}");
        std::fs::copy(gallery_fonts().join(&file), dir.join("fonts").join(&file)).expect("copy");
    }
}

fn table(dir: &Path, body: &str) {
    std::fs::write(
        dir.join("fonts.yaml"),
        format!("format: 1\nkind: fonts\nfonts:\n{body}"),
    )
    .expect("write table");
}

fn same(a: &FontFace, b: &FontFace) -> bool {
    std::ptr::eq(a, b)
}

#[test]
fn no_table_is_the_embedded_face_without_faults() {
    let dir = scratch("none");
    let library = FontLibrary::load(&dir);
    assert!(same(library.face(None), FontFace::embedded()));
    assert!(same(library.face(Some("body")), FontFace::embedded()));
    assert_eq!(library.faults().count(), 0);
    assert!(!library.contains("body"));
}

#[test]
fn a_default_entry_replaces_the_embedded_face_for_none() {
    let dir = scratch("default");
    copy_face(&dir, "inter");
    table(&dir, "  default: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(&dir);
    assert!(!same(library.face(None), FontFace::embedded()));
    assert_eq!(
        library.face(None).document().mode,
        ludion_scene::FontMode::Sdf
    );
    assert_eq!(library.faults().count(), 0);
}

#[test]
fn a_named_face_resolves_and_paths_are_root_relative() {
    let dir = scratch("named");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    table(
        &dir,
        "  body: fonts/inter.font.yaml\n  title: fonts/silkscreen.font.yaml\n",
    );
    let library = FontLibrary::load(&dir);
    assert_eq!(
        library.face(Some("title")).document().mode,
        ludion_scene::FontMode::Bitmap
    );
    assert_eq!(
        library.paths(Some("title")),
        Some(("fonts/silkscreen.font.yaml", "fonts/silkscreen.png"))
    );
    assert_eq!(library.names().collect::<Vec<_>>(), ["body", "title"]);
    assert!(library.contains("body"));
}

#[test]
fn an_unknown_name_falls_back() {
    let dir = scratch("unknown");
    copy_face(&dir, "inter");
    table(&dir, "  body: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(&dir);
    assert!(same(library.face(Some("nope")), FontFace::embedded()));
    assert!(!library.contains("nope"));
}

#[test]
fn a_broken_entry_is_a_fault_with_the_fallback() {
    let dir = scratch("broken");
    copy_face(&dir, "inter");
    table(
        &dir,
        "  body: fonts/inter.font.yaml\n  gone: fonts/missing.font.yaml\n",
    );
    let library = FontLibrary::load(&dir);
    let faults: Vec<_> = library.faults().collect();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, "fonts/missing.font.yaml");
    assert_eq!(faults[0].1.code, code::FONT_LOAD);
    assert!(library.contains("gone"));
    assert!(same(library.face(Some("gone")), FontFace::embedded()));
    assert!(!same(library.face(Some("body")), FontFace::embedded()));
}

#[test]
fn a_malformed_table_is_one_table_fault() {
    let dir = scratch("badtable");
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    let library = FontLibrary::load(&dir);
    let faults: Vec<_> = library.faults().collect();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, FONTS_FILE);
    assert_eq!(faults[0].1.code, code::FONTS_TABLE_STRUCTURE);
    assert!(same(library.face(None), FontFace::embedded()));
}

#[test]
fn two_names_on_one_path_load_once() {
    let dir = scratch("shared");
    copy_face(&dir, "inter");
    table(
        &dir,
        "  body: fonts/inter.font.yaml\n  default: fonts/inter.font.yaml\n",
    );
    let library = FontLibrary::load(&dir);
    assert_eq!(library.inner.faces.len(), 1);
    assert!(same(library.face(None), library.face(Some("body"))));
}

#[test]
fn reloading_a_face_document_emits_asset_reloaded() {
    let dir = scratch("reload-event");
    copy_face(&dir, "inter");
    table(&dir, "  body: fonts/inter.font.yaml\n");
    let mut engine = crate::Engine::builder()
        .project_root(&dir)
        .build()
        .expect("builds");
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("loads");
    engine.take_events();
    engine
        .execute(ludion_protocol::Command::AssetReload(
            ludion_protocol::command::AssetReloadParams {
                path: Some("fonts/inter.font.yaml".to_string()),
            },
        ))
        .expect("reloads");
    let events = engine.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ludion_protocol::event::Event::AssetReloaded(d) if d.path == "fonts/inter.font.yaml")),
        "{events:?}"
    );
}

const WITH_DEFAULT: &str =
    "  default: fonts/inter.font.yaml\n  title: fonts/silkscreen.font.yaml\n";

#[test]
fn an_unknown_name_falls_back_to_the_default_face() {
    let dir = scratch("unknown-default");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    table(&dir, WITH_DEFAULT);
    let library = FontLibrary::load(&dir);
    assert!(same(library.face(Some("typo")), library.face(None)));
    assert!(!same(library.face(Some("typo")), FontFace::embedded()));
    assert_eq!(library.paths(Some("typo")), library.paths(None));
    assert_eq!(
        library.paths(None),
        Some(("fonts/inter.font.yaml", "fonts/inter.png"))
    );
}

#[test]
fn a_broken_entry_falls_back_to_the_default_face() {
    let dir = scratch("broken-default");
    copy_face(&dir, "inter");
    table(
        &dir,
        "  default: fonts/inter.font.yaml\n  gone: fonts/missing.font.yaml\n",
    );
    let library = FontLibrary::load(&dir);
    assert!(same(library.face(Some("gone")), library.face(None)));
    assert_eq!(library.paths(Some("gone")), library.paths(None));
}

#[test]
fn a_face_that_fails_its_checks_keeps_its_pointer() {
    let dir = scratch("bad-face");
    copy_face(&dir, "inter");
    let doc = std::fs::read_to_string(dir.join("fonts/inter.font.yaml")).unwrap();
    let bad = doc.replace("\nascender: ", "\nascender: 2147483647 #");
    assert_ne!(bad, doc);
    std::fs::write(dir.join("fonts/inter.font.yaml"), bad).unwrap();
    table(&dir, "  default: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(&dir);
    let faults: Vec<_> = library.faults().collect();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, "fonts/inter.font.yaml");
    assert_eq!(faults[0].1.code, code::FONT_LOAD);
    assert_eq!(faults[0].1.path.as_deref(), Some("/ascender"));
    assert!(faults[0].1.message.contains("fonts/inter.font.yaml"));
}

#[test]
fn the_table_state_distinguishes_absent_faulted_and_loaded() {
    let dir = scratch("state-none");
    assert_eq!(FontLibrary::load(&dir).table_state(), TableState::Absent);
    let dir = scratch("state-bad");
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    assert_eq!(FontLibrary::load(&dir).table_state(), TableState::Faulted);
    let dir = scratch("state-ok");
    table(&dir, "  body: fonts/x.font.yaml\n");
    assert_eq!(FontLibrary::load(&dir).table_state(), TableState::Loaded);
}
