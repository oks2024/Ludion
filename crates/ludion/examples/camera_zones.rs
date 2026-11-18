//! Camera zones and the occluder query — the copyable pattern for a third-person camera that
//! changes framing by area and fades what stands between it and the hero.
//!
//! A [`CameraZone`] on a sensor box swaps the chase rig's config while the hero's origin is inside
//! it; leaving restores the rig in force before. [`camera_occluders`] lists the props between the
//! eye and the hero; what to do with them (here: a `Fade`) is the game's policy.
//!
//! Everything above the `main` divider is copyable as-is. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example camera_zones
//! ```

use ludion::prelude::*;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The hero walks +x at 6 m/s.
fn walk(world: &mut World, ctx: &SystemCtx) {
    let Some(hero) = world.entities().find(|&id| {
        world
            .get::<Name>(id)
            .ok()
            .flatten()
            .is_some_and(|name| name.0 == "hero")
    }) else {
        return;
    };
    if let Ok(Some(transform)) = world.get_mut::<Transform>(hero) {
        transform.translation[0] = 0.1 * ctx.frame.as_u64() as f32;
    }
}

/// Fades every `wall`-layer prop between the camera and the hero, restoring the others.
fn fade_occluders(world: &mut World, _ctx: &SystemCtx) {
    let Some(camera) = world.entities().find(|&id| world.has::<CameraRig>(id)) else {
        return;
    };
    let layers = LayerSet::from(vec!["wall".to_string()]);
    let Ok(hidden) = camera_occluders(world, camera, &layers, 0.2) else {
        return;
    };
    let props: Vec<EntityId> = world.query_ids::<(&Fade,)>().map(|(id, _)| id).collect();
    for prop in props {
        let fade = if hidden.contains(&prop) { 0.2 } else { 1.0 };
        if let Ok(Some(component)) = world.get_mut::<Fade>(prop) {
            component.fade = fade;
        }
    }
}

/// The game: the walk and the fade systems on a default engine.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .system("walk", walk)
        .system("fade_occluders", fade_occluders)
}

/// Two nested zones along +x: a wide one pulling the boom out, and a close one inside it that wins.
const SCENE: &str = r#"format: 1
seed: 1
physics:
  space: free
  layers:
    - name: wall
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            chase:
              distance: 5.0
  - name: hero
    components:
      Transform: {}
  - name: wide
    components:
      Transform:
        translation:
          - 10.0
          - 0.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 5.0
              - 5.0
              - 5.0
        body: static
        sensor: true
      CameraZone:
        priority: 1
        rig:
          mode:
            chase:
              distance: 8.0
  - name: close
    components:
      Transform:
        translation:
          - 12.0
          - 0.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 1.5
              - 5.0
              - 5.0
        body: static
        sensor: true
      CameraZone:
        priority: 2
        rig:
          mode:
            chase:
              distance: 3.0
"#;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Walks the hero through both zones and checks the rig switched and restored. (A real game's
/// `main` is just `ludion::run(game())`.)
fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let mut seen: Vec<Option<EntityId>> = Vec::new();
    for _ in 0..260 {
        engine.advance_live();
        let world = engine.world();
        let cam = world
            .entities()
            .find(|&id| world.has::<CameraRig>(id))
            .expect("the camera");
        let zone = world.get::<CameraRig>(cam).unwrap().unwrap().state.zone;
        if seen.last() != Some(&zone) {
            seen.push(zone);
        }
    }
    let (Some(first), Some(second)) = (
        seen.get(1).copied().flatten(),
        seen.get(2).copied().flatten(),
    ) else {
        eprintln!("the rig never entered a zone: {seen:?}");
        return ExitCode::FAILURE;
    };
    println!("zones entered in order: {first} then {second}");
    if seen.len() != 5 || seen.first() != Some(&None) || seen.last() != Some(&None) {
        eprintln!("expected none, wide, close, wide, none; got {seen:?}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
