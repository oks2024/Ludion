//! Behavioral proof for the Phase 13 input wave (E2): a scripted mouse + gamepad session driven
//! through a cursor-/stick-aimed turret system, verified two ways — the turret's resolved
//! `Transform` (it actually aims) and a two-run `WorldHash` trace (the aim is deterministic).
//!
//! Input has no visible surface in a static frame, so this — not a golden — is the feature's proof
//! (plan decision 9). The `aim_turret` system here mirrors the `cursor_aimed_turret` example: read
//! `input.mouse()`/`input.gamepad()`, unproject the cursor with `math::camera_ray` →
//! `math::ray_ground_plane`, and face the turret at the aim (right stick past a deadzone wins).

use ludion::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
struct Turret {}

const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;
const VIEWPORT: (u32, u32) = (1280, 720);

fn aim_turret(world: &mut World, _ctx: &SystemCtx) {
    let input = world.resource::<InputState>().unwrap();
    let cursor = input.mouse().position();
    let stick = input.gamepad().right_stick_dz(STICK_DEADZONE);

    let Some((camera, cam_xform)) = world
        .query::<(&Camera, &Transform)>()
        .next()
        .map(|(cam, xform)| (cam.clone(), *xform))
    else {
        return;
    };
    let cursor_aim = cursor_ground_point(cursor, &camera, &cam_xform);
    let stick_deflected = stick != [0.0, 0.0];

    for (_turret, xform) in world.query::<(&Turret, &mut Transform)>() {
        let yaw_deg = if stick_deflected {
            math::atan2(-stick[0], stick[1]) * RAD_TO_DEG
        } else if let Some([ax, az]) = cursor_aim {
            math::atan2(-(ax - xform.translation[0]), -(az - xform.translation[2])) * RAD_TO_DEG
        } else {
            continue;
        };
        xform.rotation = [0.0, yaw_deg, 0.0];
    }
}

fn cursor_ground_point(
    cursor: [f32; 2],
    camera: &Camera,
    cam_xform: &Transform,
) -> Option<[f32; 2]> {
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

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Turret>("Turret"))
        .system("aim_turret", aim_turret)
}

/// A follow camera above/behind the origin (its centre ray hits the ground back at the origin) and a
/// turret at the origin.
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

fn engine_with_scene() -> Engine {
    let mut engine = game().build().expect("the game builds");
    engine
        .load_inline_scene(SCENE)
        .expect("the turret scene loads");
    engine
}

/// The scripted session: a mouse move to a right-of-centre pixel with an awkward exact position,
/// then a right-stick deflection (which overrides the cursor), each followed by a tick.
fn session_commands() -> Vec<Command> {
    let mouse_move = |x: f32, y: f32| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            // Analog fields cross the wire as their IEEE-754 bit pattern (see the input model), so a
            // scripted position is exact; hand-writing the bits keeps this a plain wire event.
            event: json!({"kind": "mouse_move", "position_bits": [x.to_bits(), y.to_bits()]}),
        })
    };
    let stick = |axis: &str, v: f32| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({"kind": "gamepad_axis", "axis": axis, "value_bits": v.to_bits()}),
        })
    };
    let step = Command::SimStep(command::SimStepParams {
        ticks: 1,
        hash: false,
    });
    vec![
        mouse_move(953.7, 360.25),
        step.clone(),
        stick("right_stick_x", -0.8125),
        step,
    ]
}

fn turret_yaw(engine: &Engine) -> f32 {
    let world = engine.world();
    for entity in world.entities() {
        if world.get::<Turret>(entity).unwrap().is_some() {
            return world.get::<Transform>(entity).unwrap().unwrap().rotation[1];
        }
    }
    panic!("the scene has a turret");
}

/// The turret actually tracks its aim: the cursor right of centre points it at +X (yaw ≈ −90°), and
/// once the right stick deflects left it overrides the cursor and faces −X (yaw ≈ +90°).
#[test]
fn turret_tracks_cursor_then_stick() {
    let mut engine = engine_with_scene();

    // Frame 0: the mouse-move + one tick. Cursor at +X → turret faces +X → yaw ≈ −90°.
    ludion_engine::drive_hashes(
        &mut engine,
        vec![
            Command::InputInject(command::InputInjectParams {
                frame: None,
                event: json!({
                    "kind": "mouse_move",
                    "position_bits": [953.7f32.to_bits(), 360.25f32.to_bits()],
                }),
            }),
            Command::SimStep(command::SimStepParams {
                ticks: 1,
                hash: false,
            }),
        ],
    )
    .expect("driving the cursor session succeeds");
    let cursor_yaw = turret_yaw(&engine);
    assert!(
        (cursor_yaw + 90.0).abs() < 2.0,
        "cursor right of centre should aim the turret near −90° (+X), got {cursor_yaw}"
    );

    // Now deflect the right stick left; it wins over the cursor → turret faces −X → yaw ≈ +90°.
    ludion_engine::drive_hashes(
        &mut engine,
        vec![
            Command::InputInject(command::InputInjectParams {
                frame: None,
                event: json!({"kind": "gamepad_axis", "axis": "right_stick_x", "value_bits": (-0.8125f32).to_bits()}),
            }),
            Command::SimStep(command::SimStepParams {
                ticks: 1,
                hash: false,
            }),
        ],
    )
    .expect("driving the stick session succeeds");
    let stick_yaw = turret_yaw(&engine);
    assert!(
        (stick_yaw - 90.0).abs() < 1e-3,
        "a full-left right stick should aim the turret at exactly −X (yaw +90°), got {stick_yaw}"
    );
}

/// Two identical scripted runs produce identical per-frame `WorldHash` traces: the whole aim path —
/// the bit-encoded analog input, `camera_ray`, `ray_ground_plane`, and the `atan2` heading — is
/// deterministic, so the turret's resolved `Transform` is reproducible.
#[test]
fn two_runs_produce_identical_worldhash_traces() {
    let run = || {
        let mut engine = engine_with_scene();
        ludion_engine::drive_hashes(&mut engine, session_commands()).expect("the session drives")
    };
    assert_eq!(run(), run(), "the aim path must be deterministic");
}
