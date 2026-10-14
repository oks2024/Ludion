//! Behavioral proof for the Environment/IBL Wave 3 occlusion-fade plumbing (plan decision 6): a
//! `Fade`-driving system mirroring the `occlusion_fade` example, verified two ways — the walls'
//! resolved `Fade` values (the system actually reveals occluders, each its own way, and restores them
//! when the marker moves out from behind them) and a two-run `WorldHash` trace (the policy is
//! deterministic).
//!
//! The dither *pixels* are proven a GPU test in `ludion-renderer` (`fade_dithers_*`); this proves the
//! game-side half — the copyable pattern of writing `(fade, height_min)` per occlusion mode.

use ludion::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
struct Marker {}

#[derive(
    Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq,
)]
#[serde(rename_all = "snake_case")]
enum OcclusionMode {
    #[default]
    Dither,
    TopFade,
    Cutaway,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
struct Occluder {
    #[serde(default)]
    mode: OcclusionMode,
}

const STORY_HEIGHT: f32 = 3.0;
const DITHER_FADE: f32 = 0.35;
const TOP_FADE: f32 = 0.15;
const CUTAWAY_FADE: f32 = 0.04;
const CORRIDOR: f32 = 8.0;

impl OcclusionMode {
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

fn reveal_occluders(world: &mut World, _ctx: &SystemCtx) {
    let Some(cam_z) = world
        .query::<(&Camera, &Transform)>()
        .next()
        .map(|(_, xform)| xform.translation[2])
    else {
        return;
    };
    let Some(marker) = world
        .query::<(&Marker, &Transform)>()
        .next()
        .map(|(_, xform)| xform.translation)
    else {
        return;
    };

    for (occluder, xform, fade) in world.query::<(&Occluder, &Transform, &mut Fade)>() {
        let wall = xform.translation;
        let between_depth = (wall[2] - marker[2]) * (wall[2] - cam_z) <= 0.0;
        let in_corridor = (wall[0] - marker[0]).abs() < CORRIDOR;
        *fade = if between_depth && in_corridor {
            occluder.mode.fade()
        } else {
            Fade::default()
        };
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Marker>("Marker")?;
            reg.register::<Occluder>("Occluder")
        })
        .system("reveal_occluders", reveal_occluders)
}

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
      Marker: {}
"#;

fn engine_with_scene() -> Engine {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the occlusion scene loads");
    engine
}

fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// The wall `Fade` values, keyed by their occlusion mode (each mode appears once).
fn wall_fades(engine: &Engine) -> Vec<(OcclusionMode, Fade)> {
    let world = engine.world();
    let mut out = Vec::new();
    for entity in world.entities() {
        if let Some(occluder) = world.get::<Occluder>(entity).unwrap() {
            let fade = *world
                .get::<Fade>(entity)
                .unwrap()
                .expect("an occluder has a Fade");
            out.push((occluder.mode, fade));
        }
    }
    out
}

fn fade_for(fades: &[(OcclusionMode, Fade)], mode: OcclusionMode) -> Fade {
    fades
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, f)| *f)
        .expect("mode present")
}

/// With the marker behind the walls each occluder fades its own way; move it aside and every wall
/// goes solid again.
#[test]
fn walls_reveal_the_marker_then_restore() {
    let mut engine = engine_with_scene();

    step(&mut engine);
    let occluded = wall_fades(&engine);

    // Whole-building dither: partial fade, no height gate.
    let dither = fade_for(&occluded, OcclusionMode::Dither);
    assert_eq!(dither.fade, DITHER_FADE);
    assert_eq!(dither.height_min, -f32::MAX);

    // Top-fade: small fade *and* a story-height threshold, so the ground floor stays solid.
    let top = fade_for(&occluded, OcclusionMode::TopFade);
    assert_eq!(top.fade, TOP_FADE);
    assert_eq!(top.height_min, STORY_HEIGHT);

    // Cutaway: near-zero fade.
    let cutaway = fade_for(&occluded, OcclusionMode::Cutaway);
    assert_eq!(cutaway.fade, CUTAWAY_FADE);

    // Move the marker out of the corridor; the walls no longer block it and reset to solid.
    let marker = {
        let world = engine.world();
        world.single::<Marker>().expect("the scene has a marker")
    };
    engine
        .execute(Command::ComponentSet(command::ComponentSetParams {
            entity: marker,
            component: "Transform".to_string(),
            value: json!({ "translation": [20.0, 0.6, -6.0] }),
        }))
        .expect("moving the marker succeeds");
    step(&mut engine);

    for (_mode, fade) in wall_fades(&engine) {
        assert_eq!(fade, Fade::default(), "a non-occluding wall is solid again");
    }
}

/// Two identical runs of the reveal policy produce identical per-frame `WorldHash` traces.
#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let commands = || {
        vec![Command::SimStep(command::SimStepParams {
            ticks: 2,
            hash: false,
        })]
    };
    let run = || {
        let mut engine = engine_with_scene();
        ludion_engine::drive_hashes(&mut engine, commands()).expect("the session drives")
    };
    assert_eq!(run(), run(), "the reveal policy must be deterministic");
}
