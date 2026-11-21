//! A turret that aims at the cursor (or the right stick), the copyable twin-stick pattern.
//!
//! This is the runnable proof of Phase 13's input wave: it reads the mouse position and the
//! gamepad right stick from [`InputState`], turns the cursor into a world-space ground point with
//! [`math::camera_ray`] → [`math::ray_ground_plane`], and rotates a turret's [`Transform`] to face
//! the aim. The two aim sources form the twin-stick shape: the **right stick**, past a small
//! deadzone, aims directly along its own deflection (manual aim); otherwise the turret tracks the
//! **cursor** on the ground.
//!
//! Everything above the `main` divider is copyable into a real game as-is — `game()` and the
//! `aim_turret` system are the whole pattern. Aim is derived, hashed sim state, so all of its
//! trigonometry goes through [`ludion_core::math`](crate::math) (re-exported as `math`), never the
//! `f32` inherent methods — that is what keeps a scripted/replayed aim bit-identical across OSes.
//!
//! Run it headless (this file's `main` drives a scripted mouse + stick and prints the resolved yaw):
//!
//! ```text
//! cargo run -p ludion --example cursor_aimed_turret
//! ```
//!
//! To host it live for a viewer or a windowed play instead, the body of `main` is just
//! `ludion::run(game())` — the same three-line shape every Ludion game's binary uses.

use ludion::prelude::*;
use std::process::ExitCode;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- The game (copyable) -------------------------------------------------------------------------

/// Marks the entity the aim system points. A real game would hang a barrel, cooldown, etc. off it;
/// here it is a bare marker so the scene stays minimal.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct Turret {}

/// Radians → degrees as a plain multiply (deterministic, unlike `f32::to_degrees`'s trig cousins).
const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;

/// The render-target the mouse position is reported in. A game uses its own configured resolution;
/// the cursor pixels [`MouseState::position`] carries are in this space (origin top-left, +Y down).
const VIEWPORT: (u32, u32) = (1280, 720);

/// Points every [`Turret`] at the current aim: the right stick if it is deflected past the deadzone,
/// otherwise wherever the cursor falls on the ground plane.
fn aim_turret(world: &mut World, _ctx: &SystemCtx) {
    // Read input first and copy out the two values we need, so the resource borrow ends before the
    // mutable query below.
    let input = world.resource::<InputState>().unwrap();
    let cursor = input.mouse().position();
    let stick = input.gamepad().right_stick_dz(STICK_DEADZONE);

    // The scene's camera pose + projection, needed to unproject the cursor. Copy it out (both are
    // `Copy`) so the query's borrow ends too.
    let Some((camera, cam_xform)) = world
        .query::<(&Camera, &Transform)>()
        .next()
        .map(|(cam, xform)| (cam.clone(), *xform))
    else {
        return; // no camera — nothing to unproject against this frame
    };

    // Resolve the cursor's ground point once (it is the same for every turret).
    let cursor_aim = cursor_ground_point(cursor, &camera, &cam_xform);
    let stick_deflected = stick != [0.0, 0.0];

    for (_turret, xform) in world.query::<(&Turret, &mut Transform)>() {
        let yaw_deg = if stick_deflected {
            // Twin-stick manual aim: the stick's direction *is* the heading. gilrs normalises the
            // stick to +Y up, so pushing up (+y) should aim away from the camera (world −Z) — hence
            // the `-stick[1]`.
            heading_deg([stick[0], -stick[1]])
        } else if let Some([ax, az]) = cursor_aim {
            // Cursor aim: face from this turret toward the ground point under the cursor.
            heading_deg([ax - xform.translation[0], az - xform.translation[2]])
        } else {
            continue; // cursor off the ground (e.g. above the horizon) — hold the last heading
        };
        xform.rotation = [0.0, yaw_deg, 0.0];
    }
}

/// The yaw (degrees about +Y) that makes an entity whose local forward is −Z point along the ground
/// direction `dir = [dx, dz]`. Uses the deterministic `atan2`, since the result feeds hashed state.
fn heading_deg(dir: [f32; 2]) -> f32 {
    math::atan2(-dir[0], -dir[1]) * RAD_TO_DEG
}

/// Where the cursor pixel falls on the `y = 0` ground plane, or `None` if the ray misses it (e.g.
/// the cursor is above the horizon or the camera is degenerate).
fn cursor_ground_point(
    cursor: [f32; 2],
    camera: &Camera,
    cam_xform: &Transform,
) -> Option<[f32; 2]> {
    // Map the camera component's projection onto the math helper's (a distinct, engine-free enum).
    let projection = match camera.projection {
        Projection::Perspective { fov_y_deg } => math::Projection::Perspective { fov_y_deg },
        Projection::Orthographic { height, .. } => math::Projection::Orthographic { height },
    };
    let ray = math::camera_ray(
        cam_xform.translation,
        cam_xform.rotation,
        projection,
        camera.near,
        camera.far,
        cursor,
        VIEWPORT,
    )?;
    math::ray_ground_plane(&ray)
}

/// The game: a turret at the origin, one aim system, a follow camera looking down at it (defined in
/// the scene below). `default_scene` lets `ludion::run(game())` host it with no `--scene` argument.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Turret>("Turret"))
        .system("aim_turret", aim_turret)
}

/// A camera above and behind the origin, tilted down at the turret, plus the turret itself. The
/// camera's centre ray hits the ground back at the origin, so a cursor right-of-centre aims at +X.
const SCENE: &str = concat!(
    "format: 1\n",
    "seed: 1\n",
    "entities:\n",
    "  - name: camera\n",
    "    components:\n",
    "      Transform:\n",
    "        translation:\n",
    "          - 0.0\n",
    "          - 25.44\n",
    "          - 15.9\n",
    "        rotation:\n",
    "          - -58.0\n",
    "          - 0.0\n",
    "          - 0.0\n",
    "      Camera:\n",
    "        projection:\n",
    "          type: perspective\n",
    "          fov_y_deg: 50.0\n",
    "        near: 0.1\n",
    "        far: 1000.0\n",
    "  - name: turret\n",
    "    components:\n",
    "      Transform: {}\n",
    "      Turret: {}\n",
);

// --- Headless demo `main` ------------------------------------------------------------------------

/// Drives the aim system headlessly for two scripted inputs and prints the turret's resolved yaw —
/// no window, no gamepad, deterministic. (A real game's `main` is just `ludion::run(game())`.)
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

    // Cursor aim: move the mouse right of screen centre; the ground point is at +X, so the turret
    // (local forward −Z) turns to face +X, which is a yaw of −90° about +Y.
    inject(
        &mut engine,
        InputEvent::MouseMove {
            position: [960.0, 360.0],
        },
    );
    step(&mut engine);
    println!("cursor right-of-centre -> yaw {:+.1}°", turret_yaw(&engine));

    // Twin-stick aim: deflect the right stick left. Past the deadzone the stick wins over the
    // cursor, and its −X heading faces the turret at −X, a yaw of +90°.
    inject(
        &mut engine,
        InputEvent::GamepadAxis {
            axis: GamepadAxis::RightStickX,
            value: -1.0,
        },
    );
    step(&mut engine);
    println!(
        "right stick pushed left -> yaw {:+.1}°",
        turret_yaw(&engine)
    );

    ExitCode::SUCCESS
}

/// Injects one input event for the next tick over the wire command the window/editor also use.
fn inject(engine: &mut Engine, event: InputEvent) {
    let event = serde_json::to_value(event).expect("an InputEvent always serializes");
    engine
        .execute(Command::InputInject(command::InputInjectParams {
            frame: None,
            event,
        }))
        .expect("input.inject succeeds");
}

/// Advances one fixed tick, running the aim system.
fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .expect("the sim steps");
}

/// The turret's current yaw (degrees), read back from its `Transform`.
fn turret_yaw(engine: &Engine) -> f32 {
    let world = engine.world();
    for entity in world.entities() {
        if world.get::<Turret>(entity).unwrap().is_some() {
            return world.get::<Transform>(entity).unwrap().unwrap().rotation[1];
        }
    }
    panic!("the scene has a turret");
}
