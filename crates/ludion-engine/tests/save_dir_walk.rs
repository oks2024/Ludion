//! The content-root doc walk skips `<save_root>/save`: a malformed file there reaches the lenient
//! save loads (`L4277` bindings, `L4340` settings) instead of refusing the build, while malformed
//! content anywhere else still refuses it.

use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::{Command, ResponseBody};
use std::path::{Path, PathBuf};

const MAP: &str = "\
format: 1
kind: input-actions
actions:
  jump:
    kind: button
    bindings:
      - key: Space
";

// An unclosed quote defeats the walk's textual kind sniff, so the walk full-parses (and rejects) the
// file; a plain engine-kind file is skipped by the sniff before it is ever parsed.
const MALFORMED: &str = "format: 1\nkind: input-bindings\nactions: [\nx: \"a\n";
const MALFORMED_SETTINGS: &str = "format: 1\nkind: settings\naudio: [\nx: \"a\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-save-dir-walk-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn builder(root: &Path) -> EngineBuilder {
    Engine::builder()
        .input_actions()
        .doc_text("input/actions.yaml", MAP)
        .project_root(root)
        .persist_input_bindings()
        .persist_settings()
}

fn backups(dir: &Path, prefix: &str) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_malformed_save_doc_boots_with_its_lenient_diagnostic() {
    let root = fresh_dir("default");
    write(&root, "save/input-bindings.yaml", MALFORMED);
    write(&root, "save/settings.yaml", MALFORMED_SETTINGS);
    let engine = builder(&root).build().expect("the save dir is not walked");
    let codes: Vec<&str> = engine
        .boot_warnings()
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert!(codes.contains(&"L4277"), "{codes:?}");
    assert!(codes.contains(&"L4340"), "{codes:?}");
    assert_eq!(backups(&root.join("save"), "input-bindings.yaml.bak."), 1);
    assert_eq!(backups(&root.join("save"), "settings.yaml.bak."), 1);
}

#[test]
fn malformed_content_outside_the_save_dir_still_refuses_the_build() {
    let root = fresh_dir("content");
    write(
        &root,
        "content/broken.yaml",
        "format: 1\nkind: weapon\nstats: [\n",
    );
    let Err(diagnostic) = builder(&root).build() else {
        panic!("a malformed content doc must refuse the build");
    };
    assert!(
        diagnostic.code.as_str().starts_with("L100"),
        "{diagnostic:?}"
    );
}

#[test]
fn a_custom_save_root_moves_the_skip_with_it() {
    let root = fresh_dir("custom");
    write(&root, "user/save/settings.yaml", MALFORMED_SETTINGS);
    let engine = builder(&root)
        .save_root(root.join("user"))
        .build()
        .expect("user/save is the save dir");
    assert!(
        engine
            .boot_warnings()
            .iter()
            .any(|d| d.code.as_str() == "L4340")
    );

    let root = fresh_dir("custom-stale");
    write(&root, "save/settings.yaml", MALFORMED_SETTINGS);
    assert!(
        builder(&root).save_root(root.join("user")).build().is_err(),
        "save/ at the project root is content once save_root moved"
    );
}

#[test]
fn a_doc_reload_ignores_the_save_dir() {
    let root = fresh_dir("reload");
    let mut engine = builder(&root).build().unwrap();
    write(&root, "save/input-bindings.yaml", MALFORMED);
    let result = engine.execute(Command::AssetReload(AssetReloadParams {
        path: Some("save/input-bindings.yaml".to_string()),
    }));
    assert!(
        matches!(
            result.as_ref().map(|d| &d.body),
            Ok(ResponseBody::AssetsReloaded { paths }) if paths.is_empty()
        ),
        "{result:?}"
    );
}
