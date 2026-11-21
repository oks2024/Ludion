//! Unit tests for the content-root string loader and the template helpers.

use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-loc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("text")).expect("scratch dir");
    dir
}

fn write(dir: &std::path::Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).expect("write");
}

const EN: &str = "format: 1\nkind: strings\nlocale: en\nstrings:\n  hud.ammo: \"Ammo {count}\"\n  hud.title: Neon\n";
const FR: &str = "format: 1\nkind: strings\nlocale: fr\nstrings:\n  hud.title: Néon\n";

#[test]
fn walking_a_root_installs_both_stores() {
    let dir = scratch("walk");
    write(&dir, "text/en.yaml", EN);
    write(&dir, "text/fr.yaml", FR);
    write(
        &dir,
        "locales.yaml",
        "format: 1\nkind: locales\ndefault: fr\nlocales:\n  en:\n    name: English\n  fr:\n    name: Français\n",
    );
    let mut world = World::new();
    assert!(install_content_docs(&mut world, &dir));
    assert!(world.resource::<LocalizationEnabled>().is_some());
    assert_eq!(default_locale(&world), "fr");
    assert_eq!(string_in(&world, "en", "hud.title"), Some("Neon"));
    assert_eq!(string_in(&world, "fr", "hud.title"), Some("Néon"));
}

#[test]
fn a_malformed_file_is_skipped_and_no_strings_reports_false() {
    let dir = scratch("malformed");
    write(
        &dir,
        "text/bad.yaml",
        "format: 1\nkind: strings\nlocale: en\nstrings: 3\n",
    );
    write(&dir, "text/other.yaml", "format: 1\nkind: dialogue\n");
    let mut world = World::new();
    assert!(!install_content_docs(&mut world, &dir));
    assert_eq!(string_in(&world, "en", "hud.title"), None);
    assert_eq!(default_locale(&world), "en");
    write(&dir, "text/en.yaml", EN);
    assert!(install_content_docs(&mut world, &dir));
}

#[test]
fn a_missing_key_or_locale_is_none_without_fallback() {
    let dir = scratch("missing");
    write(&dir, "text/en.yaml", EN);
    write(&dir, "text/fr.yaml", FR);
    let mut world = World::new();
    install_content_docs(&mut world, &dir);
    assert_eq!(string_in(&world, "fr", "hud.ammo"), None);
    assert_eq!(string_in(&world, "de", "hud.title"), None);
    assert_eq!(string_in(&World::new(), "en", "hud.title"), None);
}

#[test]
fn literals_drops_placeholders_and_unescapes_braces() {
    assert_eq!(literals("Ammo {count}/{max}!"), "Ammo /!");
    assert_eq!(literals("{{braces}} {x}"), "{braces} ");
    assert_eq!(literals("plain"), "plain");
    assert_eq!(literals("{}"), "{}");
}
