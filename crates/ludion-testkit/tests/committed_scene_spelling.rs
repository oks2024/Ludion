//! The example projects' scenes and prefabs keep their spelling through a save
//! (`ludion_testkit::assert_saves_respell_nothing`, audit scene-text S1).

use std::path::PathBuf;

use ludion::prelude::Engine;
use ludion_testkit::assert_saves_respell_nothing;

fn example(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(path)
}

#[test]
fn gallery_scenes_respell_nothing_on_save() {
    assert_saves_respell_nothing(&example("gallery"), &["."], Engine::builder());
}

#[test]
fn example_scenes_respell_nothing_on_save() {
    assert_saves_respell_nothing(&example("scenes"), &["."], Engine::builder());
}

#[test]
fn import_example_respells_nothing_on_save() {
    assert_saves_respell_nothing(&example("import/expected"), &["."], Engine::builder());
}

#[test]
fn import_textured_example_respells_nothing_on_save() {
    assert_saves_respell_nothing(
        &example("import-textured/expected"),
        &["."],
        Engine::builder(),
    );
}

/// Stands in for the template game's `Player` component (`templates/src/components.rs`).
#[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone)]
struct Player {
    #[serde(default)]
    x: f32,
}

/// The `ludion new` scenes carry a `{{name}}` placeholder, so the gate runs on a copy with it filled.
#[test]
fn project_template_scenes_respell_nothing_on_save() {
    let templates =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ludion-cli/templates/scenes");
    let scratch =
        std::env::temp_dir().join(format!("ludion-template-scenes-{}", std::process::id()));
    let scenes = scratch.join("scenes");
    std::fs::create_dir_all(&scenes).unwrap();
    for entry in std::fs::read_dir(templates).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("{{name}}", "probe");
        std::fs::write(scenes.join(path.file_name().unwrap()), text).unwrap();
    }
    let outcome = std::panic::catch_unwind(|| {
        assert_saves_respell_nothing(
            &scratch,
            &["scenes"],
            Engine::builder().components(|registry| registry.register::<Player>("Player")),
        );
    });
    std::fs::remove_dir_all(&scratch).ok();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
