//! Occlusion fade — the copyable pattern for getting a wall out of the camera's way with the
//! render-only [`Fade`] component (Environment/IBL Wave 3, plan decision 6).
//!
//! `Fade` is pure render plumbing: `fade` in `[0, 1]` (1.0 = fully visible) drives a deterministic
//! screen-door (Bayer) dither *discard* on the opaque mesh pass — no blend, no depth-write change —
//! and `height_min` gates it by world height so only fragments **above** that Y fade. The engine
//! never decides *who* fades; that is game policy. This example is the whole pattern: attach a `Fade`
//! to an occluder, and in a system set its two numbers. It shows the three shapes a game reaches for
//! when a building sits between the camera and the thing the player must see:
//!
//! - **Dither** the whole building — `fade < 1`, `height_min = -inf` (a see-through stipple).
//! - **Top-fade** the upper stories — a small `fade` with `height_min` at a story height, so the
//!   ground floor stays solid and only the roof/upper walls dither away.
//! - **Cutaway** — `fade ≈ 0`, which discards nearly every fragment (the wall all but vanishes).
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()`, the
//! `Occluder`/`Marker` components, and the `reveal_occluders` system are the whole pattern. The
//! occlusion *test* here (is the wall between the camera and the marker?) is a deliberately trivial
//! depth+corridor check: a real game would raycast camera→player through [`ludion_spatial`] with
//! hysteresis (G7's job) — but that only changes *which* walls the system decides to fade, not the
//! `Fade` values it writes, which is the part worth copying.
//!
//! Run it headless (this file's `main` steps the scene and prints each wall's resolved fade):
//!
//! ```text
//! cargo run -p ludion --example occlusion_fade
//! ```
//!
//! To see it, host it live for a viewer instead — the body of `main` is just `ludion::run(game())`.

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- The game (copyable) -------------------------------------------------------------------------

/// The entity the camera must keep in view — a real game's player. A bare marker here.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct Marker {}

/// How an occluder gets out of the way once it is judged to be blocking the view of the [`Marker`].
/// A real game might pick per-building (a landmark cathedral cutaways; generic housing dithers).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OcclusionMode {
    /// The whole building becomes a see-through stipple.
    #[default]
    Dither,
    /// Only the upper stories fade; the ground floor stays solid.
    TopFade,
    /// The building all but vanishes.
    Cutaway,
}

/// Tags a wall as an occluder and records how it should reveal the marker behind it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct Occluder {
    #[serde(default)]
    mode: OcclusionMode,
}

/// The wall spans world Y `[0, 6]`; top-fade reveals everything above its mid-height.
const STORY_HEIGHT: f32 = 3.0;
/// Whole-building dither: a coarse stipple that still reads the wall's silhouette.
const DITHER_FADE: f32 = 0.35;
/// Top-fade: the upper stories go mostly away.
const TOP_FADE: f32 = 0.15;
/// Cutaway: near-total discard — the wall is a ghost.
const CUTAWAY_FADE: f32 = 0.04;
/// Half-width of the corridor (in world X, around the marker) a wall must fall within to count as
/// blocking the view. Wide enough that the three demo walls all block the centred marker at once.
const CORRIDOR: f32 = 8.0;

impl OcclusionMode {
    /// The `Fade` this mode writes when its wall is occluding the marker.
    fn fade(self) -> Fade {
        match self {
            OcclusionMode::Dither => Fade {
                fade: DITHER_FADE,
                height_min: -f32::MAX,
            },
            OcclusionMode::TopFade => Fade {
                fade: TOP_FADE,
                height_min: STORY_HEIGHT,
            },
            OcclusionMode::Cutaway => Fade {
                fade: CUTAWAY_FADE,
                height_min: -f32::MAX,
            },
        }
    }
}

/// Fades every occluder that sits between the camera and the marker (per its own mode), and restores
/// any that no longer blocks the view to fully solid. The blocking test is intentionally simple — a
/// depth interval plus a lateral corridor — so the copyable part is the `Fade` write, not the policy.
fn reveal_occluders(world: &mut World, _ctx: &SystemCtx) {
    // Read the camera and marker positions first, copying the values out so the borrows end before
    // the mutable occluder query below.
    let Some(cam_z) = world
        .query::<(&Camera, &Transform)>()
        .next()
        .map(|(_, xform)| xform.translation[2])
    else {
        return; // no camera this frame
    };
    let Some(marker) = world
        .query::<(&Marker, &Transform)>()
        .next()
        .map(|(_, xform)| xform.translation)
    else {
        return; // no marker this frame
    };

    for (occluder, xform, fade) in world.query::<(&Occluder, &Transform, &mut Fade)>() {
        let wall = xform.translation;
        // The wall's Z lies between the marker and the camera (the product of the two signed gaps is
        // non-positive), and it falls within the lateral corridor around the marker.
        let between_depth = (wall[2] - marker[2]) * (wall[2] - cam_z) <= 0.0;
        let in_corridor = (wall[0] - marker[0]).abs() < CORRIDOR;
        *fade = if between_depth && in_corridor {
            occluder.mode.fade()
        } else {
            Fade::default() // solid again
        };
    }
}

/// The game: three occluder walls (one per fade mode), a marker behind them, one reveal system, and a
/// camera looking through the walls at the marker. `default_scene` lets `ludion::run(game())` host it
/// with no `--scene` argument.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Marker>("Marker")?;
            reg.register::<Occluder>("Occluder")
        })
        .system("reveal_occluders", reveal_occluders)
}

/// A camera above and behind three tall walls (dither / top-fade / cutaway, left to right), each with
/// a default `Fade` for the system to drive, and an emissive marker sphere behind them at the origin.
const SCENE: &str = r#"format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 7.0
          - 16.0
        rotation:
          - -20.0
          - 0.0
          - 0.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 45.0
        near: 0.1
        far: 1000.0
  - name: ground
    components:
      Transform:
        scale:
          - 30.0
          - 1.0
          - 30.0
      Mesh:
        shape: plane
        color:
          - 0.30
          - 0.32
          - 0.36
          - 1.0
  - name: wall-dither
    components:
      Transform:
        translation:
          - -5.0
          - 3.0
          - 0.0
        scale:
          - 3.0
          - 6.0
          - 1.0
      Mesh:
        shape: cube
        color:
          - 0.55
          - 0.52
          - 0.5
          - 1.0
      Fade: {}
      Occluder:
        mode: dither
  - name: wall-top-fade
    components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 0.0
        scale:
          - 3.0
          - 6.0
          - 1.0
      Mesh:
        shape: cube
        color:
          - 0.55
          - 0.52
          - 0.5
          - 1.0
      Fade: {}
      Occluder:
        mode: top_fade
  - name: wall-cutaway
    components:
      Transform:
        translation:
          - 5.0
          - 3.0
          - 0.0
        scale:
          - 3.0
          - 6.0
          - 1.0
      Mesh:
        shape: cube
        color:
          - 0.55
          - 0.52
          - 0.5
          - 1.0
      Fade: {}
      Occluder:
        mode: cutaway
  - name: marker
    components:
      Transform:
        translation:
          - 0.0
          - 0.6
          - -6.0
      Mesh:
        shape: sphere
        color:
          - 1.0
          - 0.85
          - 0.3
          - 1.0
        emissive: 3.0
      Marker: {}
"#;

// --- Headless demo `main` ------------------------------------------------------------------------

/// Steps the scene twice — once with the marker behind the walls (all three fade, each its own way),
/// once with the marker moved far aside (the walls no longer block it, so all go solid) — printing
/// each wall's resolved `Fade` both times. No window, deterministic. (A real game's `main` is just
/// `ludion::run(game())`.)
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

    step(&mut engine);
    println!("marker behind the walls (occluded) — each wall reveals it its own way:");
    print_wall_fades(&engine);

    // Move the marker far to the side (out of the corridor); next step, no wall blocks it.
    move_marker_aside(&mut engine);
    step(&mut engine);
    println!("\nmarker moved aside (not occluded) — the walls are solid again:");
    print_wall_fades(&engine);

    ExitCode::SUCCESS
}

/// Advances one fixed tick, running the reveal system.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// Moves the marker out of every wall's corridor with a `component.set` (the same wire command the
/// editor and a networked client would use to nudge an entity).
fn move_marker_aside(engine: &mut Engine) {
    let marker = marker_entity(engine);
    engine
        .execute(Command::ComponentSet(command::ComponentSetParams {
            entity: marker,
            component: "Transform".to_string(),
            value: json!({ "translation": [20.0, 0.6, -6.0] }),
        }))
        .expect("moving the marker succeeds");
}

/// The marker's entity id, found by its component.
fn marker_entity(engine: &Engine) -> EntityId {
    let world = engine.world();
    for entity in world.entities() {
        if world.get::<Marker>(entity).unwrap().is_some() {
            return entity;
        }
    }
    panic!("the scene has a marker");
}

/// Prints each occluder wall's mode and resolved `Fade`, in entity order (deterministic).
fn print_wall_fades(engine: &Engine) {
    let world = engine.world();
    for entity in world.entities() {
        let Some(occluder) = world.get::<Occluder>(entity).unwrap() else {
            continue;
        };
        let fade = world
            .get::<Fade>(entity)
            .unwrap()
            .expect("occluders have a Fade");
        let height = if fade.height_min <= -f32::MAX {
            "-inf".to_string()
        } else {
            format!("{:.1}", fade.height_min)
        };
        println!(
            "  {:?}: fade {:.2}, height_min {}",
            occluder.mode, fade.fade, height
        );
    }
}
