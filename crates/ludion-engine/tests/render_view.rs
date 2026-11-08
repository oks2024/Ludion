//! `render.view` determinism (`Plan/editor-stage-camera.md` W2 decision 9): the override is host
//! render state, exactly as the render extent is — never hashed, snapshotted or recorded, and never
//! read by anything but the stream it moves. Two guarantees follow, each its own test:
//!
//! - flying the override, or not, never perturbs the `WorldHash` sequence;
//! - a frame rendered after the override is set and then cleared is byte-identical to one rendered
//!   before it was ever set.
//!
//! The frame test needs a GPU adapter and graceful-skips on an adapterless machine (the `L2003`
//! pattern from `gpu_dispatch.rs`); the hash test needs none.

use ludion_engine::{Engine, dispatch};
use ludion_protocol::Command;
use ludion_protocol::command::{CullFrom, RenderViewParams, ViewPose};

/// A small scene with a camera and visible geometry, so a render is non-trivial.
const SCENE: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        near: 0.1
        far: 100.0
  - components:
      Mesh: {}
";

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn pose() -> ViewPose {
    ViewPose {
        translation: [3.0, 4.0, 12.0],
        rotation: [-20.0, 35.0, 0.0],
        fov_y_deg: Some(50.0),
    }
}

/// `WorldHash` is a pure function of the world, and the override lives entirely on the `Engine`
/// outside it — so a run that flies the override every frame hashes identically, frame for frame, to
/// one that never touches it.
#[test]
fn the_view_override_never_perturbs_the_world_hash() {
    fn run(fly: bool) -> Vec<u64> {
        let mut engine = engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for i in 0..5 {
            if fly {
                let mut moved = pose();
                moved.translation[0] += i as f32;
                dispatch(
                    &mut engine,
                    Command::RenderView(RenderViewParams {
                        pose: Some(Some(moved)),
                        cull_from: Some([CullFrom::View, CullFrom::Game, CullFrom::Frozen][i % 3]),
                    }),
                )
                .expect("a well-formed pose is accepted");
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(
        run(true),
        run(false),
        "render.view must not perturb the WorldHash sequence"
    );
}

/// A frame rendered after the override is set and then cleared (`render.view` with `{"pose": null}`)
/// is byte-identical to one rendered before the override was ever set — nothing the override touched
/// survives the clear.
#[test]
fn needs_gpu_a_frame_after_clearing_the_override_matches_one_from_before_it_was_ever_set() {
    let mut engine = engine();

    // `use_view: true` is the `camera: "view"` path, but with no override yet in force it resolves
    // to the scene's own camera exactly as `camera: "scene"` would — this is the baseline.
    let before = match engine.render_png_through(160, 90, true) {
        Ok(png) => png,
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!("[render_view] SKIPPED: no GPU adapter available");
            return;
        }
        Err(diag) => panic!("render failed: {diag}"),
    };

    dispatch(
        &mut engine,
        Command::RenderView(RenderViewParams {
            pose: Some(Some(pose())),
            ..Default::default()
        }),
    )
    .expect("a well-formed pose is accepted");
    // The override must actually move the frame, or the byte-identity below would hold just as
    // well for a `render.view` that did nothing at all — the test would pass without testing.
    let flown = engine
        .render_png_through(160, 90, true)
        .expect("a GPU adapter was already confirmed available");
    assert_ne!(
        before, flown,
        "with an override in force, camera \"view\" must render through the new pose"
    );

    dispatch(
        &mut engine,
        Command::RenderView(RenderViewParams {
            pose: Some(None),
            ..Default::default()
        }),
    )
    .expect("clearing the override is accepted");

    let after = engine
        .render_png_through(160, 90, true)
        .expect("a GPU adapter was already confirmed available");

    assert_eq!(
        before, after,
        "a frame after render.view is set and cleared must match one rendered before it was ever set"
    );
}

fn set_view(engine: &mut Engine, pose: Option<Option<ViewPose>>, cull_from: Option<CullFrom>) {
    dispatch(
        engine,
        Command::RenderView(RenderViewParams { pose, cull_from }),
    )
    .expect("a well-formed render.view is accepted");
}

/// The game camera at z = 5 looks down -Z at a cube at the origin; a second cube at z = 20 stands
/// behind it, outside the game camera's frustum. `with_far` drops that second cube.
fn cull_scene(with_far: bool) -> String {
    let far = if with_far {
        "  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 20.0
      Mesh: {}
"
    } else {
        ""
    };
    format!(
        "format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        near: 0.1
        far: 100.0
  - components:
      Mesh: {{}}
{far}"
    )
}

/// A view looking down -Z from z = 30: the far cube is in front of it, the game camera behind it.
fn behind_view() -> ViewPose {
    ViewPose {
        translation: [0.0, 0.0, 30.0],
        rotation: [0.0, 0.0, 0.0],
        fov_y_deg: None,
    }
}

fn render_or_skip(engine: &mut Engine) -> Option<Vec<u8>> {
    match engine.render_png_through(160, 90, true) {
        Ok(png) => Some(png),
        Err(diag) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diag)) => {
            eprintln!("[render_view] SKIPPED: no GPU adapter available");
            None
        }
        Err(diag) => panic!("render failed: {diag}"),
    }
}

/// With no override in force `game` is the rendered camera already, so it takes the default path.
#[test]
fn needs_gpu_cull_from_game_without_an_override_renders_as_view() {
    let mut engine = engine();
    let Some(plain) = render_or_skip(&mut engine) else {
        return;
    };
    set_view(&mut engine, None, Some(CullFrom::Game));
    assert_eq!(render_or_skip(&mut engine).unwrap(), plain);
}

/// Culling from the game camera drops a mesh the flown view looks straight at; the image is the one
/// of a scene that never had that mesh, and `view` still draws it.
#[test]
fn needs_gpu_cull_from_game_culls_what_the_flown_view_sees() {
    let mut engine = Engine::new();
    engine.load_inline_scene(&cull_scene(true)).unwrap();
    let mut absent = Engine::new();
    absent.load_inline_scene(&cull_scene(false)).unwrap();

    set_view(&mut engine, Some(Some(behind_view())), None);
    set_view(&mut absent, Some(Some(behind_view())), None);
    let Some(seen) = render_or_skip(&mut engine) else {
        return;
    };
    let without = render_or_skip(&mut absent).unwrap();
    assert_ne!(seen, without, "the far cube is visible from the flown view");

    set_view(&mut engine, None, Some(CullFrom::Game));
    assert_eq!(
        render_or_skip(&mut engine).unwrap(),
        without,
        "culled against the game camera, the far cube's pixels are the background"
    );

    set_view(&mut engine, None, Some(CullFrom::View));
    assert_eq!(render_or_skip(&mut engine).unwrap(), seen);
}
