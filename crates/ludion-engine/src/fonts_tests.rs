//! Unit tests for the font library (`fonts.rs`).

use super::*;
use std::path::Path;

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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(same(library.face(None, None), FontFace::embedded()));
    assert!(same(library.face(Some("body"), None), FontFace::embedded()));
    assert_eq!(library.faults().count(), 0);
    assert!(!library.contains("body"));
}

#[test]
fn a_default_entry_replaces_the_embedded_face_for_none() {
    let dir = scratch("default");
    copy_face(&dir, "inter");
    table(&dir, "  default: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(!same(library.face(None, None), FontFace::embedded()));
    assert_eq!(
        library.face(None, None).document().mode,
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert_eq!(
        library.face(Some("title"), None).document().mode,
        ludion_scene::FontMode::Bitmap
    );
    assert_eq!(
        library.paths(Some("title"), None),
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(same(library.face(Some("nope"), None), FontFace::embedded()));
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    let faults: Vec<_> = library.faults().collect();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, "fonts/missing.font.yaml");
    assert_eq!(faults[0].1.code, code::FONT_LOAD);
    assert!(library.contains("gone"));
    assert!(same(library.face(Some("gone"), None), FontFace::embedded()));
    assert!(!same(
        library.face(Some("body"), None),
        FontFace::embedded()
    ));
}

#[test]
fn a_malformed_table_is_one_table_fault() {
    let dir = scratch("badtable");
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    let faults: Vec<_> = library.faults().collect();
    assert_eq!(faults.len(), 1);
    assert_eq!(faults[0].0, FONTS_FILE);
    assert_eq!(faults[0].1.code, code::FONTS_TABLE_STRUCTURE);
    assert!(same(library.face(None, None), FontFace::embedded()));
}

#[test]
fn two_names_on_one_path_load_once() {
    let dir = scratch("shared");
    copy_face(&dir, "inter");
    table(
        &dir,
        "  body: fonts/inter.font.yaml\n  default: fonts/inter.font.yaml\n",
    );
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert_eq!(library.inner.faces.len(), 1);
    assert!(same(
        library.face(None, None),
        library.face(Some("body"), None)
    ));
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(same(
        library.face(Some("typo"), None),
        library.face(None, None)
    ));
    assert!(!same(
        library.face(Some("typo"), None),
        FontFace::embedded()
    ));
    assert_eq!(library.paths(Some("typo"), None), library.paths(None, None));
    assert_eq!(
        library.paths(None, None),
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(same(
        library.face(Some("gone"), None),
        library.face(None, None)
    ));
    assert_eq!(library.paths(Some("gone"), None), library.paths(None, None));
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
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
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
    assert_eq!(
        FontLibrary::load(ludion_assets::AssetRoot::content(&dir)).table_state(),
        TableState::Absent
    );
    let dir = scratch("state-bad");
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    assert_eq!(
        FontLibrary::load(ludion_assets::AssetRoot::content(&dir)).table_state(),
        TableState::Faulted
    );
    let dir = scratch("state-ok");
    table(&dir, "  body: fonts/x.font.yaml\n");
    assert_eq!(
        FontLibrary::load(ludion_assets::AssetRoot::content(&dir)).table_state(),
        TableState::Loaded
    );
}

fn localized_table(dir: &Path, locales: &str) {
    std::fs::write(
        dir.join("fonts.yaml"),
        format!(
            "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  default: \
             fonts/inter.font.yaml\nlocales:\n{locales}"
        ),
    )
    .expect("write table");
}

#[test]
fn a_locale_override_wins_for_its_locale_only() {
    let dir = scratch("locale-override");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    localized_table(&dir, "  fr:\n    body: fonts/silkscreen.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(library.has_locale_overrides());
    let base = library.face(Some("body"), None);
    let fr = library.face(Some("body"), Some("fr"));
    assert!(!same(base, fr));
    assert_eq!(fr.document().mode, ludion_scene::FontMode::Bitmap);
    assert!(same(library.face(Some("body"), Some("en")), base));
    assert_eq!(
        library.paths(Some("body"), Some("fr")),
        Some(("fonts/silkscreen.font.yaml", "fonts/silkscreen.png"))
    );
}

#[test]
fn the_resolution_order_walks_override_base_override_default_base_default() {
    let dir = scratch("locale-order");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    localized_table(&dir, "  fr:\n    default: fonts/silkscreen.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    let silk = library.face(None, Some("fr"));
    assert_eq!(silk.document().mode, ludion_scene::FontMode::Bitmap);
    // `body` has no `fr` override but a base entry: base wins over the locale's default.
    assert_eq!(
        library.face(Some("body"), Some("fr")).document().mode,
        ludion_scene::FontMode::Sdf
    );
    // An unknown name skips both `name` stages and takes the locale's default override.
    assert!(same(library.face(Some("typo"), Some("fr")), silk));
    assert!(!same(library.face(Some("typo"), None), silk));
}

#[test]
fn a_failed_override_falls_through_to_the_base_face() {
    let dir = scratch("locale-failed");
    copy_face(&dir, "inter");
    localized_table(&dir, "  fr:\n    body: fonts/missing.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(same(
        library.face(Some("body"), Some("fr")),
        library.face(Some("body"), None)
    ));
    assert!(
        library
            .faults()
            .any(|(path, d)| path == "fonts/missing.font.yaml" && d.code == code::FONT_LOAD)
    );
}

#[test]
fn holds_sees_an_override_path_and_a_table_without_overrides_reports_none() {
    let dir = scratch("locale-holds");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    localized_table(&dir, "  fr:\n    body: fonts/silkscreen.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(library.holds("fonts/silkscreen.font.yaml"));
    assert!(!library.holds("fonts/other.font.yaml"));
    assert!(library.contains("body") && !library.contains("silkscreen"));
    let plain = scratch("locale-none");
    copy_face(&plain, "inter");
    table(&plain, "  body: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&plain));
    assert!(!library.has_locale_overrides());
}

fn chained_table(dir: &Path, entries: &str, locales: &str) {
    std::fs::write(
        dir.join("fonts.yaml"),
        format!("format: 1\nkind: fonts\nfonts:\n{entries}{locales}"),
    )
    .expect("write table");
}

/// A third distinct face document (a copy of Silkscreen) so a chain can hold three paths.
fn copy_third(dir: &Path) {
    std::fs::copy(
        gallery_fonts().join("silkscreen.font.yaml"),
        dir.join("fonts/third.font.yaml"),
    )
    .expect("copy");
}

fn chain_docs(library: &FontLibrary, name: &str, locale: Option<&str>) -> Vec<String> {
    library
        .chain_paths(Some(name), locale)
        .into_iter()
        .map(|(doc, _)| doc.to_string())
        .collect()
}

#[test]
fn a_fallback_chain_rides_inside_the_face_in_depth_first_order() {
    let dir = scratch("chain-order");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    copy_third(&dir);
    chained_table(
        &dir,
        "  body:\n    path: fonts/inter.font.yaml\n    fallback:\n      - third\n  \
         pixel:\n    path: fonts/silkscreen.font.yaml\n    fallback:\n      - body\n  \
         third: fonts/third.font.yaml\n",
        "",
    );
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    let pixel = library.face(Some("pixel"), None);
    assert_eq!(pixel.chain().count(), 3);
    assert_eq!(
        chain_docs(&library, "pixel", None),
        [
            "fonts/silkscreen.font.yaml",
            "fonts/inter.font.yaml",
            "fonts/third.font.yaml"
        ]
    );
    // `body` has its own two-face chain; `third` has none.
    assert_eq!(library.face(Some("body"), None).chain().count(), 2);
    assert_eq!(library.face(Some("third"), None).chain().count(), 1);
}

#[test]
fn a_face_reached_twice_by_path_joins_the_chain_once() {
    let dir = scratch("chain-dedup");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    chained_table(
        &dir,
        "  alias: fonts/inter.font.yaml\n  body: fonts/inter.font.yaml\n  pixel:\n    path: \
         fonts/silkscreen.font.yaml\n    fallback:\n      - alias\n      - body\n",
        "",
    );
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert_eq!(
        chain_docs(&library, "pixel", None),
        ["fonts/silkscreen.font.yaml", "fonts/inter.font.yaml"]
    );
    assert_eq!(library.face(Some("pixel"), None).chain().count(), 2);
}

#[test]
fn a_per_locale_primary_keeps_the_same_fallbacks() {
    let dir = scratch("chain-locale");
    copy_face(&dir, "inter");
    copy_face(&dir, "silkscreen");
    copy_third(&dir);
    chained_table(
        &dir,
        "  body: fonts/inter.font.yaml\n  pixel:\n    path: fonts/silkscreen.font.yaml\n    \
         fallback:\n      - body\n  third: fonts/third.font.yaml\n",
        "locales:\n  fr:\n    pixel: fonts/third.font.yaml\n",
    );
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert_eq!(
        chain_docs(&library, "pixel", Some("fr")),
        ["fonts/third.font.yaml", "fonts/inter.font.yaml"]
    );
    assert_eq!(
        chain_docs(&library, "pixel", Some("en")),
        ["fonts/silkscreen.font.yaml", "fonts/inter.font.yaml"]
    );
    assert_eq!(
        chain_docs(&library, "pixel", None),
        ["fonts/silkscreen.font.yaml", "fonts/inter.font.yaml"]
    );
    assert_eq!(library.face(Some("pixel"), Some("fr")).chain().count(), 2);
}

#[test]
fn a_failed_fallback_is_skipped() {
    let dir = scratch("chain-failed");
    copy_face(&dir, "silkscreen");
    chained_table(
        &dir,
        "  gone: fonts/missing.font.yaml\n  pixel:\n    path: fonts/silkscreen.font.yaml\n    \
         fallback:\n      - gone\n",
        "",
    );
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert_eq!(library.face(Some("pixel"), None).chain().count(), 1);
    assert_eq!(
        chain_docs(&library, "pixel", None),
        ["fonts/silkscreen.font.yaml"]
    );
    assert!(
        library
            .faults()
            .any(|(path, _)| path == "fonts/missing.font.yaml")
    );
}

#[test]
fn a_table_without_fallback_builds_no_chain() {
    let dir = scratch("chain-none");
    copy_face(&dir, "inter");
    table(&dir, "  body: fonts/inter.font.yaml\n");
    let library = FontLibrary::load(ludion_assets::AssetRoot::content(&dir));
    assert!(library.inner.chains.is_empty());
    assert_eq!(library.face(Some("body"), None).chain().count(), 1);
    assert_eq!(
        chain_docs(&library, "body", None),
        ["fonts/inter.font.yaml"]
    );
    assert!(chain_docs(&FontLibrary::default(), "body", None).is_empty());
}
