//! **A lamp instanced twice from a scene** — prefab instance entries end to end: a `kind: prefab`
//! file placed by two `prefab:` entries (one with an override), edited live, applied back into the
//! prefab, reverted, unpacked, and saved as an entry each time.
//!
//! The `LAMP` prefab is a `post` and a `bulb` parented to it. The scene places it as `lamp-a` and
//! `lamp-b`; `lamp-b` overrides its bulb's `PointLight.intensity`. Each instance expands to parts
//! named `<instance>/<part>`, and `scene_text` collapses the live parts back into the same entries.
//!
//! Everything above the `main` divider is copyable into a real game. The prefab and the scene are
//! content — files under the game's content root (`ludion docs prefab-instances`).
//!
//! Run it headless (prints one line per step, self-checks every save):
//!
//! ```text
//! cargo run -p ludion --example prefab_scene
//! ```

use ludion::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The prefab, by content-root-relative path.
const LAMP_PATH: &str = "prefabs/lamp.prefab.yaml";

/// The game: no systems, the scene's instances are the content. A real game's `main` is
/// `ludion::run(game())`.
pub fn game() -> EngineBuilder {
    Engine::builder()
}

/// A post and a bulb parented to it; `Parent.index` is the post's position in this list.
const LAMP: &str = "\
format: 1
kind: prefab
name: lamp
entities:
  - name: post
    components:
      Transform: {}
  - name: bulb
    components:
      Parent:
        index: 0
        generation: 0
      Transform:
        translation:
          - 0.0
          - 2.0
          - 0.0
      PointLight:
        intensity: 1.0
";

/// A camera and two lamps; `lamp-b` carries an override. The canonical form `scene_text` writes.
const SCENE: &str = "\
format: 1
name: lamps
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 10.0
        rotation:
          - 0.0
          - 0.0
          - 0.0
        scale:
          - 1.0
          - 1.0
          - 1.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 60.0
        near: 0.10000000149011612
        far: 1000.0
  - name: lamp-a
    prefab: prefabs/lamp.prefab.yaml
    transform:
      translation:
        - -3.0
        - 0.0
        - 0.0
  - name: lamp-b
    prefab: prefabs/lamp.prefab.yaml
    transform:
      translation:
        - 3.0
        - 0.0
        - 0.0
    overrides:
      bulb:
        PointLight:
          intensity: 9.0
";

// --- Headless demo `main` ------------------------------------------------------------------------

/// Runs every step, printing one line each. Deterministic, no window.
fn main() -> ExitCode {
    let root = demo_content_root();
    if let Err(error) = write_prefab(&root) {
        eprintln!("could not write the demo prefab: {error}");
        return ExitCode::FAILURE;
    }
    let result = run(&root);
    let _ = std::fs::remove_dir_all(&root);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Fails with `message` unless `ok`, otherwise prints `step`.
fn step(ok: bool, step: &str, message: &str) -> Result<(), String> {
    if ok {
        println!("ok  {step}");
        Ok(())
    } else {
        Err(format!("FAILED {step}: {message}"))
    }
}

/// Loads `scene` over `root`.
fn load(root: &Path, scene: &str) -> Result<Engine, String> {
    let mut engine = game()
        .project_root(root)
        .build()
        .map_err(|diagnostic| diagnostic.to_string())?;
    engine.load_inline_scene(scene).map_err(|diagnostics| {
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    Ok(engine)
}

/// Every named entity, by name.
fn named(engine: &mut Engine) -> BTreeMap<String, EntityId> {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .map(|(id, (name,))| (name.0.clone(), id))
        .collect()
}

/// The light intensity of the part called `name`.
fn intensity(engine: &mut Engine, name: &str) -> Option<f32> {
    let id = *named(engine).get(name)?;
    let light = engine.world_mut().get::<PointLight>(id).ok()??;
    Some(light.intensity)
}

fn diagnostics_text(diagnostics: &[ludion_core::diagnostics::Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(root: &Path) -> Result<(), String> {
    let mut engine = load(root, SCENE)?;
    let names: Vec<String> = named(&mut engine).into_keys().collect();
    step(
        ["lamp-a/post", "lamp-a/bulb", "lamp-b/post", "lamp-b/bulb"]
            .iter()
            .all(|part| names.iter().any(|name| name == part)),
        "two instances expand to four named parts",
        &format!("{names:?}"),
    )?;
    let saved = engine.scene_text().map_err(|d| d.to_string())?;
    step(
        saved == SCENE,
        "scene_text collapses them back to the source",
        &saved,
    )?;

    // Edit lamp-a's bulb live, then write the edit back into the prefab file.
    let ids = named(&mut engine);
    let brighter = PointLight {
        intensity: 3.0,
        ..PointLight::default()
    };
    engine
        .world_mut()
        .insert(ids["lamp-a/bulb"], brighter)
        .map_err(|error| error.to_string())?;
    let applied = engine
        .apply_prefab(ids["lamp-a/post"], None)
        .map_err(|d| diagnostics_text(&d))?;
    let prefab = std::fs::read_to_string(root.join(LAMP_PATH)).map_err(|e| e.to_string())?;
    step(
        applied.parts == ["bulb"] && prefab.contains("intensity: 3.0"),
        "apply writes the bulb edit into the prefab file",
        &prefab,
    )?;
    let saved = engine.scene_text().map_err(|d| d.to_string())?;
    step(
        saved == SCENE,
        "after the apply lamp-a saves without overrides, lamp-b keeps its own",
        &saved,
    )?;

    // A reload reads the applied prefab: lamp-a is clean, lamp-b's override still wins.
    let mut reloaded = load(root, &saved)?;
    step(
        intensity(&mut reloaded, "lamp-a/bulb") == Some(3.0)
            && intensity(&mut reloaded, "lamp-b/bulb") == Some(9.0),
        "a reload gives lamp-a the applied value and lamp-b its override",
        "unexpected intensities",
    )?;

    // Revert lamp-b: its override goes, the prefab's value returns.
    let ids = named(&mut reloaded);
    revert_prefab(reloaded.world_mut(), ids["lamp-b/post"], None)
        .map_err(|d| diagnostics_text(&d))?;
    let saved = reloaded.scene_text().map_err(|d| d.to_string())?;
    step(
        intensity(&mut reloaded, "lamp-b/bulb") == Some(3.0) && !saved.contains("overrides"),
        "revert drops lamp-b's override",
        &saved,
    )?;

    // Unpack lamp-a: plain entities now, saved as such; lamp-b stays an entry.
    unpack_prefab(reloaded.world_mut(), ids["lamp-a/post"]).map_err(|d| diagnostics_text(&d))?;
    let saved = reloaded.scene_text().map_err(|d| d.to_string())?;
    step(
        saved.contains("name: lamp-a/post")
            && saved.contains("name: lamp-a/bulb")
            && saved.matches("prefab:").count() == 1,
        "unpack saves lamp-a as plain entities and lamp-b as an entry",
        &saved,
    )
}

/// A fresh, process-unique temp directory for the demo's content (removed first, so a re-run starts
/// clean).
fn demo_content_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-prefab-scene-example-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Writes the lamp prefab under `root/prefabs/`.
fn write_prefab(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("prefabs"))?;
    std::fs::write(root.join(LAMP_PATH), LAMP)
}
