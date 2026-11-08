//! `render.view`: point the host's stream at a free camera pose instead of the scene's own.
//!
//! One [`Command::RenderView`] verb, three wire shapes: `{}` reads the override without changing it,
//! `{"pose": {...}}` sets it, and `{"pose": null}` clears it. The override is **host render state**,
//! exactly as the render extent (`render.size`) is — it moves the stream, never the world, so it is
//! never hashed, snapshotted or recorded. `render.bounds` and `render.screenshot` only look through it
//! when a call opts in with `"camera": "view"`; every other call, including the same two verbs at
//! their default `"camera": "scene"`, keeps answering through the scene's own camera whether or not a
//! human is flying the viewport somewhere else.
//!
//! This example is headless: `render.bounds` is pure CPU (extract + projection math), so no GPU
//! adapter is needed to see the override change what a query reports.
//!
//! Run it:
//!
//! ```text
//! cargo run -p ludion-engine --example render_view
//! ```

use std::process::ExitCode;

use ludion_engine::{Engine, dispatch};
use ludion_protocol::command::{CameraChoice, RenderBoundsParams, RenderViewParams, ViewPose};
use ludion_protocol::response::ScreenRect;
use ludion_protocol::{Command, ResponseBody};

/// One camera and one mesh, so `render.bounds` has exactly one renderable to report.
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

fn main() -> ExitCode {
    let mut engine = Engine::new().with_render_size(320, 180);
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }

    // `{}` — a pure read. No override is in force yet, so `camera: "view"` and `camera: "scene"`
    // (the default) already agree.
    let scene_rect = mesh_screen_rect(&mut engine, CameraChoice::Scene);
    println!("no override,   camera: scene -> {scene_rect:?}");
    assert_eq!(
        scene_rect,
        mesh_screen_rect(&mut engine, CameraChoice::View),
        "with no override set, \"view\" and \"scene\" answer identically"
    );

    // Set the override — pulled back and shifted from the scene camera, looking the same direction
    // (identity rotation, as the scene camera's own untouched `Transform` also is) and naming its own
    // field of view, so the mesh stays on screen but its projected rect visibly moves and shrinks.
    dispatch(
        &mut engine,
        Command::RenderView(RenderViewParams {
            pose: Some(Some(ViewPose {
                translation: [1.5, 0.5, 10.0],
                rotation: [0.0, 0.0, 0.0],
                fov_y_deg: Some(40.0),
            })),
            ..Default::default()
        }),
    )
    .expect("a well-formed pose is accepted");

    // `camera: "view"` now looks through the flown pose — the mesh has moved in the frame.
    let flown_rect = mesh_screen_rect(&mut engine, CameraChoice::View);
    println!("override set,  camera: view  -> {flown_rect:?}");
    assert_ne!(
        scene_rect, flown_rect,
        "camera: \"view\" must look through the override once one is set"
    );

    // `camera: "scene"` never reads the override, even while one is in force.
    let still_scene_rect = mesh_screen_rect(&mut engine, CameraChoice::Scene);
    println!("override set,  camera: scene -> {still_scene_rect:?}");
    assert_eq!(
        scene_rect, still_scene_rect,
        "camera: \"scene\" answers through the game's own framing no matter who is flying"
    );

    // `{"pose": null}` clears it. `camera: "view"` now falls back to the scene's own camera again —
    // byte-for-byte the same answer as before the override was ever set.
    dispatch(
        &mut engine,
        Command::RenderView(RenderViewParams {
            pose: Some(None),
            ..Default::default()
        }),
    )
    .expect("clearing the override is accepted");
    let cleared_rect = mesh_screen_rect(&mut engine, CameraChoice::View);
    println!("cleared,       camera: view  -> {cleared_rect:?}");
    assert_eq!(
        scene_rect, cleared_rect,
        "a cleared override answers exactly as a scene that never had one"
    );

    ExitCode::SUCCESS
}

/// `render.bounds` for the one mesh in [`SCENE`], projected through `camera`.
fn mesh_screen_rect(engine: &mut Engine, camera: CameraChoice) -> Option<ScreenRect> {
    match dispatch(engine, Command::RenderBounds(RenderBoundsParams { camera }))
        .expect("render.bounds succeeds")
        .body
    {
        ResponseBody::RenderBounds { entities, .. } => {
            entities
                .into_iter()
                .next()
                .expect("the mesh is renderable")
                .screen_rect
        }
        other => panic!("expected RenderBounds, got {other:?}"),
    }
}
