//! `CameraRig` at runtime: the engine's `camera_rig` system poses a scene camera that rides a
//! `kind: path` track after a target the game moves (here a `PathFollower`). Each case builds a scene
//! over paths written to a temp content root and reads the camera `Transform` and the rig's state
//! back: the track projection end to end, blends and snaps, the held gameplay clock, determinism,
//! snapshot/restore, always-installed and the diagnostics of spec §6.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    Camera, CameraOffset, CameraRig, ChaseRig, DebugCategories, Egress, EgressChannels, Engine,
    Name, RigConfig, RigMode, Transform, add_kick, add_look, add_trauma, set_look, snap_rig,
};
use ludion_protocol::command::{
    EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};

type Systems<'a> = &'a [(&'static str, fn(&mut World, &SystemCtx))];

const DT: f32 = 1.0 / 60.0;

fn fixture_root(tag: &str, files: &[(&str, String)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-camera-rig-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

fn path_yaml(points: &[[f32; 3]], interpolation: &str, closed: bool) -> String {
    let mut text = format!(
        "format: 1\nkind: path\nname: p\ninterpolation: {interpolation}\nclosed: {closed}\npoints:\n"
    );
    for p in points {
        text.push_str(&format!(
            "  - - {:?}\n    - {:?}\n    - {:?}\n",
            p[0], p[1], p[2]
        ));
    }
    text
}

/// Two 20 m legs 4 m apart joined by a hairpin: the nearest-point trap.
const SWITCHBACK: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [20.0, 0.0, 0.0],
    [20.0, 0.0, -4.0],
    [0.0, 0.0, -4.0],
];

fn circle(radius: f32) -> Vec<[f32; 3]> {
    (0..8)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / 8.0;
            [
                radius * ludion_core::math::cos(angle),
                0.0,
                radius * ludion_core::math::sin(angle),
            ]
        })
        .collect()
}

/// The files of a scene whose camera tracks `points` and whose target follows the same line.
fn track_files(
    points: &[[f32; 3]],
    interpolation: &str,
    closed: bool,
) -> Vec<(&'static str, String)> {
    let text = path_yaml(points, interpolation, closed);
    vec![
        ("track.path.yaml", text.clone()),
        ("target.path.yaml", text),
    ]
}

/// The camera at entity 0 and the hero, a follower of `target.path.yaml`, at entity 1.
fn rig_scene(mode: &str, speed: f32, rig_extra: &str) -> String {
    format!(
        "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform: {{}}\n      Camera: {{}}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            track:\n              path: track.path.yaml\n{rig_extra}  - name: hero\n    components:\n      Transform: {{}}\n      PathFollower:\n        path: target.path.yaml\n        speed: {speed}\n        mode: {mode}\n"
    )
}

fn named(world: &World, wanted: &str) -> EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn rig_of(engine: &Engine) -> CameraRig {
    let world = engine.world();
    world
        .get::<CameraRig>(named(world, "cam"))
        .unwrap()
        .unwrap()
        .clone()
}

fn eye_of(engine: &Engine) -> [f32; 3] {
    let world = engine.world();
    world
        .get::<Transform>(named(world, "cam"))
        .unwrap()
        .unwrap()
        .translation
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3)
        .map(|i| (a[i] - b[i]) * (a[i] - b[i]))
        .sum::<f32>()
        .sqrt()
}

fn build(tag: &str, files: &[(&str, String)], systems: Systems) -> Engine {
    let mut builder = Engine::builder().project_root(fixture_root(tag, files));
    for &(name, system) in systems {
        builder = builder.system(name, system);
    }
    builder.build().expect("the game builds")
}

fn engine_for(tag: &str, files: &[(&str, String)], scene: &str, systems: Systems) -> Engine {
    let mut engine = build(tag, files, systems);
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn advance(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

/// The projection parameter and eye after each of `ticks` ticks.
fn trace(engine: &mut Engine, ticks: usize) -> Vec<(f32, [f32; 3])> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            (rig_of(engine).state.param.unwrap_or(0.0), eye_of(engine))
        })
        .collect()
}

// ---- the projection, end to end ----

/// Asserts the eye never jumps (the snap tick aside) and the projection never changes by more than
/// `bound` metres of arc a tick, modulo the path `length` when `closed`.
fn assert_continuous(samples: &[(f32, [f32; 3])], length: Option<f32>, what: &str) {
    for (tick, pair) in samples.windows(2).enumerate() {
        let mut d = (pair[1].0 - pair[0].0).abs();
        if let Some(length) = length {
            d = d.min(length - d);
        }
        assert!(
            d < 0.5,
            "{what}: the projection jumped {d} m at tick {tick}"
        );
        let step = distance(pair[0].1, pair[1].1);
        assert!(step < 0.6, "{what}: the eye jumped {step} m at tick {tick}");
    }
}

#[test]
fn a_follower_on_a_switchback_never_jumps_legs() {
    let files = track_files(&SWITCHBACK, "catmull_rom", false);
    let mut engine = engine_for("switchback", &files, &rig_scene("once", 6.0, ""), &[]);
    let samples = trace(&mut engine, 420);
    assert_continuous(&samples, None, "switchback");
    let travelled = samples.last().unwrap().0;
    assert!(
        travelled > 40.0,
        "the camera rode the whole path: {travelled}"
    );
}

#[test]
fn a_follower_on_a_closed_loop_wraps_without_a_jump() {
    let points = circle(8.0);
    let files = track_files(&points, "catmull_rom", true);
    let mut engine = engine_for("loop", &files, &rig_scene("loop", 6.0, ""), &[]);
    let length = ludion_engine::PathSampler::new(
        &ludion_engine::check_path(&files[0].1).expect("the path parses"),
    )
    .length() as f32;
    let samples = trace(&mut engine, 1100);
    assert_continuous(&samples, Some(length), "loop");
    let wraps = samples.windows(2).filter(|p| p[1].0 < p[0].0).count();
    assert!(wraps >= 2, "two laps wrap the parameter: {wraps}");
}

// ---- the pose, snaps and blends ----

fn track_scene() -> (Vec<(&'static str, String)>, String) {
    (
        track_files(&SWITCHBACK, "catmull_rom", false),
        rig_scene("once", 6.0, ""),
    )
}

#[test]
fn the_rig_drives_the_camera_transform_and_the_first_tick_snaps() {
    let (files, scene) = track_scene();
    let mut engine = engine_for("pose", &files, &scene, &[]);
    assert_eq!(eye_of(&engine), [0.0; 3], "authored at the origin");
    assert!(rig_of(&engine).state.last_applied.is_none());
    engine.advance_live();
    let rig = rig_of(&engine);
    assert!(
        rig.state.last_applied.is_some(),
        "the rig applied its config"
    );
    assert!(rig.state.blend.is_none(), "a snap starts no blend");
    let pose = rig.state.pose.as_ref().expect("the tick posed");
    assert_eq!(eye_of(&engine), pose.eye, "no trauma: the eye is the pose");
    assert!(
        distance(pose.eye, [0.0; 3]) > 1.0,
        "it left the origin: {:?}",
        pose.eye
    );
    advance(&mut engine, 30);
    assert_eq!(eye_of(&engine), rig_of(&engine).state.pose.unwrap().eye);
}

const EDIT_AT: u64 = 40;
const SNAP_AT: u64 = 60;

fn retarget(world: &mut World) {
    let cam = named(world, "cam");
    let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
    let RigMode::Track(track) = &mut rig.rig.mode else {
        panic!("track mode")
    };
    track.offset = [3.0, 5.0, 9.0];
}

fn edit_rig(world: &mut World, ctx: &SystemCtx) {
    if ctx.game_frame.as_u64() == EDIT_AT {
        retarget(world);
    }
}

fn edit_then_snap(world: &mut World, ctx: &SystemCtx) {
    edit_rig(world, ctx);
    if ctx.frame.as_u64() == SNAP_AT {
        let cam = named(world, "cam");
        snap_rig(world.get_mut::<CameraRig>(cam).unwrap().unwrap());
    }
}

#[test]
fn a_new_rig_blends_in_over_its_seconds_and_a_snap_cuts() {
    let (files, scene) = track_scene();
    let mut engine = engine_for("blend", &files, &scene, &[("edit", edit_rig)]);
    advance(&mut engine, EDIT_AT as usize);
    let mut blending = 0;
    let mut last = eye_of(&engine);
    for tick in 0..120 {
        engine.advance_live();
        let eye = eye_of(&engine);
        let step = distance(last, eye);
        assert!(
            step < 0.6,
            "tick {tick} after the edit moved the eye {step} m"
        );
        last = eye;
        if rig_of(&engine).state.blend.is_some() {
            blending += 1;
        }
    }
    let expected = (1.0 / DT) as i32;
    assert!(
        (expected - 1..=expected + 1).contains(&blending),
        "the default 1 s blend ran {blending} ticks"
    );

    let mut engine = engine_for("snap", &files, &scene, &[("edit", edit_then_snap)]);
    advance(&mut engine, SNAP_AT as usize);
    assert!(
        rig_of(&engine).state.blend.is_some(),
        "mid-blend before the snap"
    );
    engine.advance_live();
    let rig = rig_of(&engine);
    assert!(
        rig.state.blend.is_none(),
        "the snap dropped the blend in one tick"
    );
    assert_eq!(rig.state.eye_velocity, [0.0; 3]);
    assert!(!rig.state.snap, "the flag is consumed");
    let after = eye_of(&engine);
    engine.advance_live();
    assert!(
        distance(after, eye_of(&engine)) < 0.6,
        "the cut leaves the eye at rest"
    );
}

#[test]
fn a_rig_snap_marks_the_camera_cut_for_the_presenter() {
    let (files, scene) = track_scene();
    let mut engine = engine_for("present-cut", &files, &scene, &[("edit", edit_then_snap)]);
    let cam = named(engine.world(), "cam");
    advance(&mut engine, SNAP_AT as usize);
    let (_, before) = engine.extract_present_scene().unwrap();
    assert!(!before.snaps.contains(&cam), "a blending rig is not a cut");
    engine.advance_live();
    let (_, after) = engine.extract_present_scene().unwrap();
    assert_eq!(after.camera.map(|camera| camera.key), Some(cam));
    assert!(after.snaps.contains(&cam), "snap_rig cuts the camera");
}

// ---- the gameplay clock ----

const HOLD_FROM: u64 = 30;
const HOLD_TO: u64 = 34;

fn freeze_window(world: &mut World, ctx: &SystemCtx) {
    if (HOLD_FROM..=HOLD_TO).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

/// The rig and the camera pose after each of `ticks` ticks.
fn states(engine: &mut Engine, ticks: usize) -> Vec<(serde_json::Value, [f32; 3], [f32; 3])> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            let world = engine.world();
            let transform = *world
                .get::<Transform>(named(world, "cam"))
                .unwrap()
                .unwrap();
            (
                serde_json::to_value(rig_of(engine)).unwrap(),
                transform.translation,
                transform.rotation,
            )
        })
        .collect()
}

#[test]
fn a_held_gameplay_clock_freezes_the_rig_and_the_run_resumes_shifted() {
    const TICKS: usize = 120;
    let (files, scene) = track_scene();
    let held_for = (HOLD_TO - HOLD_FROM + 1) as usize;
    let mut unheld = engine_for("unheld", &files, &scene, &[("edit", edit_rig)]);
    let unheld = states(&mut unheld, TICKS);
    let mut held = engine_for(
        "held",
        &files,
        &scene,
        &[("freeze", freeze_window), ("edit", edit_rig)],
    );
    let held = states(&mut held, TICKS + held_for);
    // `hold` called on frame f freezes frames f + 1 ..= HOLD_TO + 1; frame HOLD_FROM still advances.
    let before = HOLD_FROM as usize;
    for frozen in &held[before..=before + held_for] {
        assert_eq!(frozen, &held[before], "the window changes nothing");
    }
    // The edit lands on gameplay frame EDIT_AT in both runs, so the held run is the unheld one shifted.
    for tick in before..TICKS {
        assert_eq!(
            held[tick + held_for],
            unheld[tick],
            "tick {tick}, shifted by the hold"
        );
    }
}

// ---- determinism, hashes, restore ----

fn hashes(tag: &str) -> Vec<u64> {
    let (files, scene) = track_scene();
    let mut engine = engine_for(tag, &files, &scene, &[("edit", edit_rig)]);
    (0..200)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_rig_run_hashes_the_same_twice_and_at_one_and_eight_threads() {
    let first = hashes("hash-a");
    assert_eq!(first, hashes("hash-b"));
    let distinct: std::collections::BTreeSet<u64> = first.iter().copied().collect();
    assert!(distinct.len() > 100, "a flat trace proves nothing");
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "hash-t1"), (8, "hash-t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || hashes(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
    assert_eq!(one, first);
}

#[test]
fn the_world_hash_moves_when_the_rig_state_moves() {
    let (files, scene) = track_scene();
    let mut engine = engine_for("hash-state", &files, &scene, &[]);
    advance(&mut engine, 10);
    let before = engine.world_hash().unwrap();
    let cam = named(engine.world(), "cam");
    engine
        .world_mut()
        .get_mut::<CameraRig>(cam)
        .unwrap()
        .unwrap()
        .state
        .eye[0] += 0.5;
    assert_ne!(engine.world_hash().unwrap(), before);
}

fn stir(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == EDIT_AT {
        retarget(world);
        let cam = named(world, "cam");
        add_trauma(world.get_mut::<CameraRig>(cam).unwrap().unwrap(), 0.8);
    }
}

fn restorable(tag: &str) -> Engine {
    let (files, scene) = track_scene();
    engine_for(tag, &files, &scene, &[("stir", stir)])
}

#[test]
fn a_snapshot_restore_mid_blend_with_trauma_resumes_the_same_hashes() {
    const AT: usize = 55;
    const TICKS: usize = 160;
    let mut engine = restorable("restore-ref");
    let reference: Vec<u64> = (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();

    let mut before = restorable("restore-a");
    advance(&mut before, AT);
    let rig = rig_of(&before);
    assert!(rig.state.blend.is_some(), "tick {AT} is mid-blend");
    assert!(rig.trauma > 0.0, "and shaking: {}", rig.trauma);
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    let mut resumed = restorable("restore-b");
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert!(
        resumed
            .world()
            .get::<CameraRig>(named(resumed.world(), "cam"))
            .is_ok()
    );
    assert!(
        rig_of(&resumed).state.last_applied.is_some(),
        "restore carries the state"
    );
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

// ---- debug draw ----

/// Per-tick hashes, the categories drawn and the snapshot bytes of a run through a blend and a
/// trauma pulse.
fn draw_run(tag: &str, categories: DebugCategories) -> (Vec<u64>, Vec<&'static str>, Vec<u8>) {
    let mut engine = restorable(tag);
    engine.set_debug_draw(categories);
    let mut hashes = Vec::new();
    for _ in 0..120 {
        engine.advance_live();
        hashes.push(engine.world_hash().unwrap().as_u64());
    }
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => serde_json::to_vec(&snapshot).unwrap(),
        other => panic!("expected a snapshot, got {other:?}"),
    };
    (hashes, engine.debug_draw_drawn(), snapshot)
}

#[test]
fn the_camera_category_never_perturbs_the_hash_or_snapshot() {
    let (hashes, drawn, snapshot) = draw_run("draw-on", DebugCategories::parse("camera"));
    let (plain_hashes, plain_drawn, plain_snapshot) = draw_run("draw-off", DebugCategories::None);
    assert!(
        drawn.contains(&"camera"),
        "the camera category drew nothing"
    );
    assert_eq!(
        drawn, plain_drawn,
        "the pen records whatever the host filter"
    );
    assert_eq!(hashes, plain_hashes, "per-tick hashes");
    assert_eq!(snapshot, plain_snapshot, "snapshot bytes");
}

#[test]
fn a_held_rig_draws_nothing() {
    let scene = "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n      CameraRig:\n        target: null\n        rig:\n          mode:\n            track:\n              path: track.path.yaml\n";
    let (files, _) = track_scene();
    let mut engine = engine_for("draw-held", &files, scene, &[]);
    advance(&mut engine, 6);
    assert!(!engine.debug_draw_drawn().contains(&"camera"));
}

// ---- always installed, drift ----

fn describe_systems(engine: &mut Engine) -> Vec<serde_json::Value> {
    match engine
        .execute(Command::EngineDescribe)
        .expect("describes")
        .body
    {
        ResponseBody::EngineDescription { description } => {
            description["systems"].as_array().unwrap().clone()
        }
        other => panic!("{other:?}"),
    }
}

fn noop(_: &mut World, _: &SystemCtx) {}

#[test]
fn describe_lists_camera_rig_after_the_physics_triple_with_and_without_a_rig() {
    let mut engine = engine_for(
        "describe-none",
        &[],
        "format: 1\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n",
        &[("noop", noop)],
    );
    let names = |systems: &[serde_json::Value]| -> Vec<String> {
        systems
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(&describe_systems(&mut engine)),
        ["path_follow", "noop", "camera_rig"],
        "both engine systems are always installed"
    );

    let (files, scene) = track_scene();
    let scene = scene.replacen("seed: 3\n", "seed: 3\nphysics:\n  space: free\n", 1);
    let mut engine = engine_for("describe-rig", &files, &scene, &[("noop", noop)]);
    let systems = describe_systems(&mut engine);
    assert_eq!(
        names(&systems),
        [
            "path_follow",
            "noop",
            "pose_blend",
            ludion_physics::WRITE,
            ludion_physics::STEP,
            ludion_physics::MIRROR,
            "camera_rig"
        ]
    );
    let rig = systems.last().unwrap();
    assert_eq!(rig["order"], 6);
    assert!(rig["registered_at"].is_null());
    assert_eq!(rig["fn_path"], "ludion_engine::camera_rig");

    let scene = scene.replacen("physics:\n  space: free\n", "", 1);
    let mut engine = engine_for("describe-rig-nophys", &files, &scene, &[("noop", noop)]);
    let systems = describe_systems(&mut engine);
    assert_eq!(names(&systems), ["path_follow", "noop", "camera_rig"]);
    assert_eq!(systems[2]["order"], 2);
}

#[test]
fn describe_orders_a_presentation_system_after_the_engine_systems_it_runs_behind() {
    let (files, scene) = track_scene();
    let scene = scene.replacen("seed: 3\n", "seed: 3\nphysics:\n  space: free\n", 1);
    let mut engine = Engine::builder()
        .project_root(fixture_root("describe-present", &files))
        .presentation_system("present", noop)
        .system("noop", noop)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(&scene).expect("the scene loads");
    let systems = describe_systems(&mut engine);
    let names: Vec<&str> = systems
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "path_follow",
            "noop",
            "pose_blend",
            ludion_physics::WRITE,
            ludion_physics::STEP,
            ludion_physics::MIRROR,
            "camera_rig",
            "present"
        ]
    );
    for (i, system) in systems.iter().enumerate() {
        assert_eq!(system["order"], i, "{}", system["name"]);
    }
    assert!(systems[7]["registered_at"]["line"].is_u64());
}

#[test]
fn an_empty_component_inserts_a_rig_and_serializes_into_its_own_schema() {
    let engine = Engine::builder().build().unwrap();
    let registry = engine.registry();
    let info = registry
        .iter()
        .find(|info| info.name() == "CameraRig")
        .expect("the rig is registered");
    let mut world = World::new();
    let entity = world.spawn();
    info.deserialize_insert(&serde_json::json!({}), &mut world, entity)
        .expect("`{}` is a complete rig, not a skipped component");
    let value = info.serialize(&world, entity).unwrap().unwrap();
    assert!(
        ludion_scene::check_component("CameraRig", &value, registry).is_empty(),
        "{value}"
    );
    assert_eq!(
        world.get::<CameraRig>(entity).unwrap().unwrap(),
        &CameraRig::default()
    );
}

// ---- diagnostics ----

fn log_lines(mut engine: Engine, ticks: u64) -> Vec<LogEvent> {
    engine
        .execute(Command::SimPause)
        .expect("the engine pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
    let mut lines = Vec::new();
    let mut wait = Duration::from_secs(5);
    while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
        if let Event::Log(log) = event.message.event {
            lines.push(log);
        }
        wait = Duration::from_millis(300);
    }
    shutdown.signal();
    let _ = handle.join();
    lines
}

fn reported<'a>(lines: &'a [LogEvent], code: &str) -> Vec<&'a LogEvent> {
    lines
        .iter()
        .filter(|l| {
            l.diagnostic
                .as_ref()
                .is_some_and(|d| d.code.as_str() == code)
        })
        .collect()
}

/// Whether the camera's eye stops changing from tick `from` on.
fn holds_from(engine: &mut Engine, from: usize) -> bool {
    advance(engine, from);
    let held = eye_of(engine);
    advance(engine, 20);
    eye_of(engine) == held
}

fn despawn_hero(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 8 {
        let hero = named(world, "hero");
        world.despawn(hero).unwrap();
    }
}

#[test]
fn a_despawned_target_reports_l1305_once_and_the_camera_holds() {
    let (files, scene) = track_scene();
    let engine = engine_for("l1305", &files, &scene, &[("despawn", despawn_hero)]);
    let lines = log_lines(engine, 60);
    let found = reported(&lines, "L1305");
    assert_eq!(found.len(), 1, "once: {lines:?}");
    assert!(
        found[0].message.contains("does not exist"),
        "{}",
        found[0].message
    );

    let mut engine = engine_for("l1305-hold", &files, &scene, &[("despawn", despawn_hero)]);
    assert!(holds_from(&mut engine, 12), "the rig holds its last pose");
    assert!(
        distance(eye_of(&engine), [0.0; 3]) > 0.5,
        "after having posed once"
    );
}

#[test]
fn an_unloadable_track_path_reports_l1306_once_with_the_causes() {
    let scene = rig_scene("once", 6.0, "").replace("track.path.yaml", "gone.path.yaml");
    let files = [("target.path.yaml", path_yaml(&SWITCHBACK, "linear", false))];
    let engine = engine_for("l1306", &files, &scene, &[]);
    let lines = log_lines(engine, 30);
    let found = reported(&lines, "L1306");
    assert_eq!(found.len(), 1, "once: {lines:?}");
    assert!(
        found[0].message.contains("gone.path.yaml"),
        "{}",
        found[0].message
    );
    assert!(
        found[0].message.contains("failed to load: "),
        "{}",
        found[0].message
    );

    let mut engine = engine_for("l1306-hold", &files, &scene, &[]);
    assert!(holds_from(&mut engine, 5));
    assert_eq!(eye_of(&engine), [0.0; 3], "it never posed");
}

#[test]
fn a_rig_on_an_entity_without_a_camera_reports_l1307_once_and_holds() {
    let (files, scene) = track_scene();
    let scene = scene.replacen("      Camera: {}\n", "", 1);
    let engine = engine_for("l1307", &files, &scene, &[]);
    let lines = log_lines(engine, 30);
    assert_eq!(reported(&lines, "L1307").len(), 1, "once: {lines:?}");
    let mut engine = engine_for("l1307-hold", &files, &scene, &[]);
    assert!(holds_from(&mut engine, 5));
    assert_eq!(eye_of(&engine), [0.0; 3]);
}

#[test]
fn a_second_rig_reports_l1308_once_and_only_the_lowest_entity_runs() {
    let (files, scene) = track_scene();
    let second = "  - name: other\n    components:\n      Transform: {}\n      Camera: {}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            track:\n              path: track.path.yaml\n";
    let scene = format!("{scene}{second}");
    let engine = engine_for("l1308", &files, &scene, &[]);
    let lines = log_lines(engine, 30);
    let found = reported(&lines, "L1308");
    assert_eq!(found.len(), 1, "once: {lines:?}");

    let mut engine = engine_for("l1308-run", &files, &scene, &[]);
    advance(&mut engine, 30);
    let world = engine.world();
    let other = world
        .get::<Transform>(named(world, "other"))
        .unwrap()
        .unwrap()
        .translation;
    assert_eq!(other, [0.0; 3], "the second rig never poses its camera");
    assert!(
        distance(eye_of(&engine), [0.0; 3]) > 1.0,
        "the first rig does"
    );
}

#[test]
fn a_parented_rig_reports_l1309_once_and_holds() {
    let (files, scene) = track_scene();
    let scene = scene.replacen(
        "      Camera: {}\n",
        "      Camera: {}\n      Parent:\n        index: 1\n        generation: 0\n",
        1,
    );
    let engine = engine_for("l1309", &files, &scene, &[]);
    let lines = log_lines(engine, 30);
    assert_eq!(reported(&lines, "L1309").len(), 1, "once: {lines:?}");
    let mut engine = engine_for("l1309-hold", &files, &scene, &[]);
    assert!(holds_from(&mut engine, 5));
    assert_eq!(eye_of(&engine), [0.0; 3]);
}

fn spoil_half_life(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 8 {
        let cam = named(world, "cam");
        world
            .get_mut::<CameraRig>(cam)
            .unwrap()
            .unwrap()
            .rig
            .springs
            .position_half_life = -1.0;
    }
}

#[test]
fn a_rust_built_negative_half_life_reports_l1304_once_and_holds() {
    let (files, scene) = track_scene();
    let engine = engine_for("l1304", &files, &scene, &[("spoil", spoil_half_life)]);
    let lines = log_lines(engine, 40);
    let found = reported(&lines, "L1304");
    assert_eq!(found.len(), 1, "once: {lines:?}");
    assert!(
        found[0].message.contains("/rig/springs/position_half_life"),
        "{}",
        found[0].message
    );
    let mut engine = engine_for("l1304-hold", &files, &scene, &[("spoil", spoil_half_life)]);
    assert!(holds_from(&mut engine, 12));
}

/// The scene of a camera and a track follower, with no rig: a game system adds it at frame 1.
fn rigless_scene() -> (Vec<(&'static str, String)>, String) {
    let (files, scene) = track_scene();
    let at = scene.find("      CameraRig:").unwrap();
    let end = scene.find("  - name: hero").unwrap();
    let scene = format!("{}{}", &scene[..at], &scene[end..]);
    (files, scene)
}

/// Adds the track rig to the camera at frame 1, into a world that held none at load.
fn add_rig(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let (cam, hero) = (named(world, "cam"), named(world, "hero"));
    let mut rig = CameraRig {
        target: Some(hero),
        ..CameraRig::default()
    };
    let RigMode::Track(track) = &mut rig.rig.mode else {
        panic!("track mode")
    };
    track.path = "track.path.yaml".to_string();
    world.insert(cam, rig).unwrap();
}

fn late_rig(tag: &str) -> Engine {
    let (files, scene) = rigless_scene();
    engine_for(tag, &files, &scene, &[("add_rig", add_rig)])
}

#[test]
fn a_rig_added_at_runtime_poses_its_camera() {
    let mut engine = late_rig("late-rig");
    advance(&mut engine, 30);
    assert!(rig_of(&engine).state.last_applied.is_some());
    assert!(
        distance(eye_of(&engine), [0.0; 3]) > 1.0,
        "the camera moved"
    );
}

#[test]
fn a_snapshot_restore_after_a_rig_was_added_at_runtime_resumes_the_same_hashes() {
    const AT: usize = 20;
    const TICKS: usize = 60;
    let mut engine = late_rig("late-ref");
    let reference: Vec<u64> = (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();

    let mut before = late_rig("late-a");
    advance(&mut before, AT);
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = late_rig("late-b");
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

#[test]
fn a_rig_with_no_target_is_inert_and_reports_nothing() {
    let (files, scene) = track_scene();
    let scene = scene.replacen(
        "        target:\n          index: 1\n          generation: 0\n",
        "",
        1,
    );
    let mut engine = engine_for("inert", &files, &scene, &[]);
    advance(&mut engine, 10);
    assert_eq!(eye_of(&engine), [0.0; 3]);
    assert!(rig_of(&engine).state.last_applied.is_none());
    let lines = log_lines(engine, 5);
    assert!(
        lines.iter().all(|l| l
            .diagnostic
            .as_ref()
            .is_none_or(|d| !d.code.as_str().starts_with("L13"))),
        "{lines:?}"
    );
}

// ---- the presentation camera offset ----

/// Spawns the offset entity on the first tick and animates it every tick after.
fn drive_offset(world: &mut World, ctx: &SystemCtx) {
    let existing = world.presentation_entities().next();
    let id = existing.unwrap_or_else(|| world.spawn_presentation());
    let t = ctx.frame.as_u64() as f32;
    let offset = CameraOffset {
        translation: [0.01 * t, 0.0, 3.0 - 0.02 * t],
        rotation_deg: [0.0, 0.5 * t, 0.0],
        fov_delta: -0.05 * t,
    };
    world.insert(id, offset).unwrap();
}

/// A sim system reading the presentation offset entity: the crossing is `L0028`.
fn peek_offset(world: &mut World, _: &SystemCtx) {
    let _ = world.get::<CameraOffset>(EntityId::new(ludion_ecs::PRESENTATION_BASE, 0));
}

fn offset_engine(tag: &str, offset: bool, peek: bool) -> Engine {
    let (files, scene) = track_scene();
    let mut builder = Engine::builder().project_root(fixture_root(tag, &files));
    if peek {
        builder = builder.system("peek_offset", peek_offset);
    }
    if offset {
        builder = builder.presentation_system("drive_offset", drive_offset);
    }
    let mut engine = builder.build().expect("the game builds");
    engine.load_inline_scene(&scene).expect("the scene loads");
    engine
}

fn camera_of(engine: &Engine) -> Camera {
    let world = engine.world();
    *world.get::<Camera>(named(world, "cam")).unwrap().unwrap()
}

/// T1: the offset moves nothing the sim owns. Over 120 ticks of a moving rig the world hash, the
/// camera `Transform` and `Camera`, and the rig's yaw equal the run without it; the offset survives
/// `engine.restore` without moving the hash.
#[test]
fn an_animated_offset_moves_no_hash_transform_camera_or_rig() {
    let mut plain = offset_engine("offset-plain", false, false);
    let mut moved = offset_engine("offset-moved", true, false);
    for tick in 0..120 {
        plain.advance_live();
        moved.advance_live();
        assert_eq!(
            moved.world_hash().unwrap(),
            plain.world_hash().unwrap(),
            "hash at tick {tick}"
        );
        assert_eq!(eye_of(&moved), eye_of(&plain), "eye at tick {tick}");
        assert_eq!(
            camera_of(&moved),
            camera_of(&plain),
            "camera at tick {tick}"
        );
        assert_eq!(
            rig_of(&moved).yaw_deg(),
            rig_of(&plain).yaw_deg(),
            "yaw at tick {tick}"
        );
    }
    assert!(moved.world().presentation_entities().next().is_some());
    ludion_testkit::assert_no_boundary_crossings(moved.world());

    let snapshot = match moved.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let hash = moved.world_hash().unwrap();
    let mut resumed = offset_engine("offset-resumed", true, false);
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let id = resumed
        .world()
        .presentation_entities()
        .next()
        .expect("survives");
    assert!(
        !resumed
            .world()
            .get::<CameraOffset>(id)
            .unwrap()
            .unwrap()
            .is_identity()
    );
    assert_eq!(resumed.world_hash().unwrap(), hash);
}

/// A `snap_rig` cut keeps the offset: the camera is marked a snap, the blend returns the newer
/// tick's camera whole, and that camera differs from the same tick's without the offset.
#[test]
fn a_snap_rig_cut_keeps_the_offset() {
    let build_with = |tag: &str, offset: bool| {
        let (files, scene) = track_scene();
        let mut builder = Engine::builder()
            .project_root(fixture_root(tag, &files))
            .system("edit", edit_then_snap);
        if offset {
            builder = builder.presentation_system("drive_offset", drive_offset);
        }
        let mut engine = builder.build().expect("the game builds");
        engine.load_inline_scene(&scene).expect("the scene loads");
        engine
    };
    let mut moved = build_with("offset-snap-moved", true);
    let mut plain = build_with("offset-snap-plain", false);
    advance(&mut moved, SNAP_AT as usize);
    advance(&mut plain, SNAP_AT as usize);
    let (prev, prev_data) = moved.extract_present_scene().unwrap();
    moved.advance_live();
    plain.advance_live();
    let (next, next_data) = moved.extract_present_scene().unwrap();
    let cam = named(moved.world(), "cam");
    assert!(next_data.snaps.contains(&cam), "snap_rig cuts the camera");
    assert_eq!(
        ludion_renderer::blend(&prev, &prev_data, &next, &next_data, 0.5).camera,
        next.camera,
        "the cut shows the newer tick's camera"
    );
    let (without, _) = plain.extract_present_scene().unwrap();
    assert_ne!(
        next.camera, without.camera,
        "the offset is kept through the cut"
    );
}

/// A sim system reading the offset entity is an `L0028` crossing.
#[test]
fn a_sim_read_of_the_offset_entity_is_l0028() {
    let mut engine = offset_engine("offset-peek", true, true);
    advance(&mut engine, 3);
    let crossings = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .map(|d| {
            d.reported(ludion_core::codes::code::PRESENTATION_BOUNDARY)
                .count()
        })
        .unwrap_or(0);
    assert!(
        crossings > 0 || !engine.world().pending_boundary_crossings().is_empty(),
        "the sim read crossed the boundary"
    );
}

/// The `camera` debug category draws the rig's own pose: an offset leaves every segment as it was.
#[test]
fn the_camera_debug_category_ignores_the_offset() {
    let lines = |tag: &str, offset: bool| {
        let mut engine = offset_engine(tag, offset, false);
        engine.set_debug_draw(DebugCategories::parse("camera"));
        let mut out = Vec::new();
        for _ in 0..30 {
            engine.advance_live();
            out.push(engine.extract_present_scene().unwrap().0.debug_lines);
        }
        out
    };
    let plain = lines("offset-draw-plain", false);
    assert!(plain.iter().any(|tick| !tick.is_empty()), "the rig drew");
    assert_eq!(lines("offset-draw-moved", true), plain);
}

// ---- first person ----

fn fp_scene() -> String {
    "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            first_person: {}\n  - name: hero\n    components:\n      Transform:\n        translation:\n          - 1.0\n          - 0.0\n          - -3.0\n".to_string()
}

fn look(world: &mut World, ctx: &SystemCtx) {
    let cam = named(world, "cam");
    let k = ctx.frame.as_u64() as f32;
    add_look(
        world.get_mut::<CameraRig>(cam).unwrap().unwrap(),
        ludion_core::math::sin(k * 0.3) * 3.0,
        ludion_core::math::cos(k * 0.1),
    );
}

fn look_once_more(world: &mut World, ctx: &SystemCtx) {
    look(world, ctx);
    if ctx.frame.as_u64() == 20 {
        let cam = named(world, "cam");
        add_look(world.get_mut::<CameraRig>(cam).unwrap().unwrap(), 0.5, 0.0);
    }
}

fn fp_engine(tag: &str, system: fn(&mut World, &SystemCtx)) -> Engine {
    engine_for(tag, &[], &fp_scene(), &[("look", system)])
}

fn fp_hashes(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn first_person_runs_without_a_path_and_matches_aim() {
    let mut engine = fp_engine("fp-aim", look);
    advance(&mut engine, 40);
    let rig = rig_of(&engine);
    let aim = rig.aim().expect("posed");
    assert!((aim.eye[0] - 1.0).abs() < 1e-4 && (aim.eye[1] - 1.6).abs() < 1e-4);
    let world = engine.world();
    let transform = world
        .get::<Transform>(named(world, "cam"))
        .unwrap()
        .unwrap();
    assert_eq!(transform.rotation, aim.rotation_deg());
    assert_eq!(transform.translation, aim.eye);
}

#[test]
fn first_person_hashes_repeat_and_cover_the_look() {
    let a = fp_hashes(&mut fp_engine("fp-a", look), 60);
    let b = fp_hashes(&mut fp_engine("fp-b", look), 60);
    assert_eq!(a, b);
    let c = fp_hashes(&mut fp_engine("fp-c", look_once_more), 60);
    assert_eq!(a[..20], c[..20]);
    assert_ne!(a[20..], c[20..], "one extra look moves the hash");
}

#[test]
fn first_person_snapshot_restore_resumes_bit_exact() {
    const AT: usize = 25;
    let reference = fp_hashes(&mut fp_engine("fp-ref", look), 80);
    let mut before = fp_engine("fp-pre", look);
    advance(&mut before, AT);
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = fp_engine("fp-post", look);
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

fn look_and_kick_at_20(world: &mut World, ctx: &SystemCtx) {
    look(world, ctx);
    if ctx.frame.as_u64() == 20 {
        let cam = named(world, "cam");
        add_kick(
            world.get_mut::<CameraRig>(cam).unwrap().unwrap(),
            0.5,
            -2.0,
            0.3,
        );
    }
}

#[test]
fn a_kick_hashes_while_it_lasts_and_rejoins_the_unkicked_run() {
    let plain = fp_hashes(&mut fp_engine("kick-plain", look), 120);
    let a = fp_hashes(&mut fp_engine("kick-a", look_and_kick_at_20), 120);
    let b = fp_hashes(&mut fp_engine("kick-b", look_and_kick_at_20), 120);
    assert_eq!(a, b);
    assert_eq!(a[..20], plain[..20]);
    assert_ne!(a[20], plain[20], "the kick moves the hash");
    assert_eq!(a[119], plain[119], "the kick has rested and is unhashed");
}

#[test]
fn a_kick_snapshot_restore_resumes_bit_exact() {
    const AT: usize = 25;
    let reference = fp_hashes(&mut fp_engine("kick-ref", look_and_kick_at_20), 80);
    let mut before = fp_engine("kick-pre", look_and_kick_at_20);
    advance(&mut before, AT);
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = fp_engine("kick-post", look_and_kick_at_20);
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

// ---- chase and follow ----

const CHASE_SCENE: &str = "format: 1\nseed: 3\nentities:\n  - name: cam\n    components:\n      Transform: {}\n      Camera: {}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            chase:\n              distance: 5.0\n  - name: hero\n    components:\n      Transform: {}\n";
const CHASE_SWITCH_AT: u64 = 40;

/// The hero walks a diagonal and the camera is turned a little every tick; at `CHASE_SWITCH_AT` the
/// rig switches to a one-second blend into `follow`.
fn walk_and_turn(world: &mut World, ctx: &SystemCtx) {
    let hero = named(world, "hero");
    let t = ctx.frame.as_u64() as f32;
    world
        .get_mut::<Transform>(hero)
        .unwrap()
        .unwrap()
        .translation = [0.05 * t, 0.0, -0.03 * t];
    let cam = named(world, "cam");
    let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
    add_look(rig, 0.7, 0.3);
    if ctx.frame.as_u64() == CHASE_SWITCH_AT {
        rig.rig.mode = RigMode::Follow(Default::default());
    }
}

fn chase_engine(tag: &str) -> Engine {
    engine_for(tag, &[], CHASE_SCENE, &[("walk_and_turn", walk_and_turn)])
}

#[test]
fn a_chase_rig_poses_the_camera_on_its_boom_and_the_first_tick_snaps() {
    let mut engine = engine_for("chase-pose", &[], CHASE_SCENE, &[]);
    assert_eq!(eye_of(&engine), [0.0; 3], "authored at the origin");
    engine.advance_live();
    let rig = rig_of(&engine);
    assert!(rig.state.blend.is_none(), "a snap starts no blend");
    assert_eq!(
        rig.state.pivot,
        Some([0.0; 3]),
        "the pivot rests on the hero"
    );
    let pose = rig.state.pose.as_ref().expect("the tick posed");
    assert_eq!(eye_of(&engine), pose.eye);
    assert!(
        (distance(pose.eye, [0.0; 3]) - 5.0).abs() < 1e-4,
        "{:?}",
        pose.eye
    );
    assert_eq!((pose.yaw_deg, pose.pitch_deg), (0.0, 20.0));
}

fn chase_hashes(tag: &str) -> Vec<u64> {
    let mut engine = chase_engine(tag);
    (0..160)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_chase_run_hashes_the_same_twice_and_at_one_and_eight_threads() {
    let first = chase_hashes("chase-hash-a");
    assert_eq!(first, chase_hashes("chase-hash-b"));
    let distinct: std::collections::BTreeSet<u64> = first.iter().copied().collect();
    assert!(distinct.len() > 100, "a flat trace proves nothing");
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "chase-hash-t1"), (8, "chase-hash-t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || chase_hashes(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
    assert_eq!(one, first);
}

#[test]
fn a_snapshot_restore_mid_blend_from_follow_to_chase_resumes_the_same_hashes() {
    const AT: usize = 55;
    const TICKS: usize = 140;
    let mut engine = chase_engine("chase-restore-ref");
    let reference: Vec<u64> = (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();

    let mut before = chase_engine("chase-restore-a");
    advance(&mut before, AT);
    let rig = rig_of(&before);
    assert!(rig.state.blend.is_some(), "tick {AT} is mid-blend");
    assert!(matches!(rig.rig.mode, RigMode::Follow(_)));
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = chase_engine("chase-restore-b");
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

#[test]
fn the_camera_category_draws_a_chase_rig_without_perturbing_the_hash() {
    let run = |tag: &str, categories: DebugCategories| {
        let mut engine = chase_engine(tag);
        engine.set_debug_draw(categories);
        let hashes: Vec<u64> = (0..100)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect();
        (hashes, engine.debug_draw_drawn())
    };
    let (hashes, drawn) = run("chase-draw-on", DebugCategories::parse("camera"));
    let (plain, _) = run("chase-draw-off", DebugCategories::None);
    assert!(
        drawn.contains(&"camera"),
        "the camera category drew nothing"
    );
    assert_eq!(hashes, plain);
}

// ---- collision and framing ----

const RADIUS: f32 = 0.3;

/// A chase rig 6 m from a hero at the origin (entity 1), with `collision` fields (12-space
/// indented lines under `collision:`) and `walls` as extra entities. `physics` is the scene's
/// physics block, empty for none.
fn collide_scene(physics: &str, pitch: f32, distance: f32, collision: &str, walls: &str) -> String {
    format!(
        "format: 1\nseed: 3\n{physics}entities:\n  - name: cam\n    components:\n      Transform: {{}}\n      Camera: {{}}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            chase:\n              distance: {distance:?}\n              pitch_deg: {pitch:?}\n          collision:\n            enabled: true\n{collision}  - name: hero\n    components:\n      Transform: {{}}\n{walls}"
    )
}

const PHYSICS: &str = "physics:\n  space: free\n  layers:\n    - name: wall\n    - name: ghost\n";

/// A block-style three-number sequence, each item at `indent` spaces.
fn block3(indent: usize, v: [f32; 3]) -> String {
    v.iter()
        .map(|n| format!("{}- {n:?}\n", " ".repeat(indent)))
        .collect()
}

/// A static box collider entity.
fn wall(name: &str, centre: [f32; 3], half: [f32; 3], layer: &str) -> String {
    let layer = if layer.is_empty() {
        String::new()
    } else {
        format!("        layer: {layer}\n")
    };
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n{layer}",
        block3(10, centre),
        block3(14, half),
    )
}

/// The wall on the hero's +X side, its near face at x = 4.
fn side_wall(layer: &str) -> String {
    wall("wall", [4.5, 1.0, 0.0], [0.5, 5.0, 10.0], layer)
}

/// The metres from `point` to the box centred `centre` with half extents `half`.
fn gap_to_box(point: [f32; 3], centre: [f32; 3], half: [f32; 3]) -> f32 {
    let d = |i: usize| ((point[i] - centre[i]).abs() - half[i]).max(0.0);
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
}

fn spin(world: &mut World, _: &SystemCtx) {
    let cam = named(world, "cam");
    add_look(world.get_mut::<CameraRig>(cam).unwrap().unwrap(), 4.0, 0.0);
}

/// Yaw 90 (the eye on the +X side, into the wall) until frame 60, then yaw 0.
fn park_then_leave(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    if frame == 0 || frame == 60 {
        let cam = named(world, "cam");
        let yaw = if frame == 0 { 90.0 } else { 0.0 };
        set_look(world.get_mut::<CameraRig>(cam).unwrap().unwrap(), yaw, 20.0);
    }
}

fn collide_engine(tag: &str, scene: &str, systems: Systems) -> Engine {
    engine_for(tag, &[], scene, systems)
}

#[test]
fn an_orbit_past_a_wall_never_puts_the_eye_sphere_inside_it() {
    let scene = collide_scene(PHYSICS, 20.0, 6.0, "", &side_wall(""));
    let mut engine = collide_engine("collide-orbit", &scene, &[("spin", spin)]);
    let mut pulled = 0;
    for tick in 0..200 {
        engine.advance_live();
        let gap = gap_to_box(eye_of(&engine), [4.5, 1.0, 0.0], [0.5, 5.0, 10.0]);
        assert!(
            gap >= RADIUS - 1e-3,
            "tick {tick}: the eye is {gap} m from the wall"
        );
        pulled += usize::from(rig_of(&engine).state.boom.is_some());
    }
    assert!(
        pulled > 20,
        "the orbit crossed the wall ({pulled} ticks pulled in)"
    );
}

#[test]
fn the_boom_recovers_to_its_distance_after_leaving_the_wall() {
    let scene = collide_scene(PHYSICS, 20.0, 6.0, "", &side_wall(""));
    let mut engine = collide_engine("collide-recover", &scene, &[("leave", park_then_leave)]);
    advance(&mut engine, 50);
    let boom = rig_of(&engine).state.boom.expect("pulled in at the wall");
    assert!(boom < 4.0, "{boom}");
    advance(&mut engine, 500);
    let rig = rig_of(&engine);
    assert_eq!((rig.state.boom, rig.state.boom_hold), (None, 0.0));
    assert!((distance(eye_of(&engine), [0.0; 3]) - 6.0).abs() < 1e-3);
}

#[test]
fn a_four_metre_ceiling_holds_a_twelve_metre_boom_at_fifty_five_degrees() {
    let ceiling = wall("ceiling", [0.0, 4.5, 0.0], [30.0, 0.5, 30.0], "");
    let scene = collide_scene(PHYSICS, 55.0, 12.0, "", &ceiling);
    let mut engine = collide_engine("collide-ceiling", &scene, &[("spin", spin)]);
    for tick in 0..120 {
        engine.advance_live();
        let eye = eye_of(&engine);
        assert!(eye[1] + RADIUS <= 4.0 + 1e-3, "tick {tick}: eye {eye:?}");
        assert!(rig_of(&engine).state.boom.is_some(), "tick {tick}");
    }
}

#[test]
fn a_pivot_and_target_inside_geometry_report_l1552_once_and_hold_min_distance() {
    let room = wall("room", [0.0, 0.0, 0.0], [3.0, 3.0, 3.0], "");
    let scene = collide_scene(PHYSICS, 20.0, 6.0, "", &room);
    let mut engine = collide_engine("collide-stuck", &scene, &[]);
    advance(&mut engine, 5);
    assert_eq!(rig_of(&engine).state.boom, Some(0.6));
    let lines = log_lines(collide_engine("collide-stuck-log", &scene, &[]), 30);
    assert_eq!(reported(&lines, "L1552").len(), 1, "{lines:?}");
}

#[test]
fn an_undeclared_layer_reports_l1550_once_and_a_world_without_physics_l1551() {
    let scene = collide_scene(
        PHYSICS,
        20.0,
        6.0,
        "            ignore_layers:\n              - nowhere\n",
        &side_wall(""),
    );
    let lines = log_lines(
        collide_engine("collide-l1550", &scene, &[("spin", spin)]),
        30,
    );
    let found = reported(&lines, "L1550");
    assert_eq!(found.len(), 1, "{lines:?}");
    let diagnostic = found[0].diagnostic.as_ref().unwrap();
    assert_eq!(diagnostic.path.as_deref(), Some("/rig/collision"));
    assert!(found[0].message.contains("nowhere"), "{}", found[0].message);

    let scene = collide_scene("", 20.0, 6.0, "", "");
    let lines = log_lines(collide_engine("collide-l1551", &scene, &[]), 30);
    assert_eq!(reported(&lines, "L1551").len(), 1, "{lines:?}");
    let mut engine = collide_engine("collide-l1551-pose", &scene, &[]);
    advance(&mut engine, 3);
    assert!(
        (distance(eye_of(&engine), [0.0; 3]) - 6.0).abs() < 1e-3,
        "runs uncollided"
    );
}

#[test]
fn an_ignored_layer_lets_the_camera_through() {
    let walls = side_wall("ghost");
    let blocked = collide_scene(PHYSICS, 20.0, 6.0, "", &walls);
    let mut engine = collide_engine("collide-blocked", &blocked, &[("leave", park_then_leave)]);
    advance(&mut engine, 30);
    assert!(rig_of(&engine).state.boom.is_some());

    let through = collide_scene(
        PHYSICS,
        20.0,
        6.0,
        "            ignore_layers:\n              - ghost\n",
        &walls,
    );
    let mut engine = collide_engine("collide-through", &through, &[("leave", park_then_leave)]);
    advance(&mut engine, 30);
    assert_eq!(rig_of(&engine).state.boom, None);
    assert!((distance(eye_of(&engine), [0.0; 3]) - 6.0).abs() < 1e-3);
}

/// Spins the camera, and from frame 40 asks for a framed shoulder peek.
fn spin_and_frame(world: &mut World, ctx: &SystemCtx) {
    spin(world, ctx);
    if ctx.frame.as_u64() == 40 {
        let cam = named(world, "cam");
        let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
        rig.framing.weight = 1.0;
        rig.framing.pivot_offset = [0.8, 0.3, 0.0];
        rig.framing.yaw_offset_deg = 10.0;
    }
}

fn collide_hero_scene() -> String {
    collide_scene(PHYSICS, 20.0, 6.0, "", &side_wall(""))
}

fn collide_hashes(tag: &str) -> Vec<u64> {
    let mut engine = collide_engine(tag, &collide_hero_scene(), &[("spin", spin_and_frame)]);
    (0..160)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_collided_framed_run_hashes_the_same_twice_and_at_one_and_eight_threads() {
    let first = collide_hashes("collide-hash-a");
    assert_eq!(first, collide_hashes("collide-hash-b"));
    let distinct: std::collections::BTreeSet<u64> = first.iter().copied().collect();
    assert!(distinct.len() > 100, "a flat trace proves nothing");
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "collide-hash-t1"), (8, "collide-hash-t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || collide_hashes(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
    assert_eq!(one, first);
}

/// Yaw 90 into the wall from frame 0, and a framed peek from frame 40.
fn park_and_frame(world: &mut World, ctx: &SystemCtx) {
    park_then_leave_start(world, ctx);
    if ctx.frame.as_u64() == 40 {
        let cam = named(world, "cam");
        let rig = world.get_mut::<CameraRig>(cam).unwrap().unwrap();
        rig.framing.weight = 1.0;
        rig.framing.pivot_offset = [0.8, 0.3, 0.0];
        rig.framing.yaw_offset_deg = 10.0;
    }
}

fn park_then_leave_start(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 0 {
        let cam = named(world, "cam");
        set_look(
            world.get_mut::<CameraRig>(cam).unwrap().unwrap(),
            90.0,
            20.0,
        );
    }
}

#[test]
fn a_snapshot_restore_mid_pull_in_and_mid_framing_resumes_the_same_hashes() {
    const AT: usize = 43;
    const TICKS: usize = 120;
    let scene = collide_hero_scene();
    let make = |tag: &str| collide_engine(tag, &scene, &[("drive", park_and_frame)]);
    let mut engine = make("collide-restore-ref");
    let reference: Vec<u64> = (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();

    let mut before = make("collide-restore-a");
    advance(&mut before, AT);
    let rig = rig_of(&before);
    assert!(rig.state.boom.is_some(), "mid pull-in");
    assert!(
        rig.state.framing_weight > 0.0 && rig.state.framing_weight < 1.0,
        "mid framing: {}",
        rig.state.framing_weight
    );
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = make("collide-restore-b");
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[AT - 1]);
    for (i, expected) in reference[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

// ---- camera zones (L1553-L1555) and the occluder query ----

/// A chase rig 5 m behind a hero at x = `hero_x`, with `zones` as extra entities.
fn zone_scene(physics: &str, hero_x: f32, zones: &str) -> String {
    format!(
        "format: 1\nseed: 3\n{physics}entities:\n  - name: cam\n    components:\n      Transform: {{}}\n      Camera: {{}}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            chase:\n              distance: 5.0\n  - name: hero\n    components:\n      Transform:\n        translation:\n          - {hero_x:?}\n          - 0.0\n          - 0.0\n{zones}"
    )
}

/// A sensor box covering x in `[from, to]` whose rig is a chase at `distance`.
fn zone(name: &str, from: f32, to: f32, priority: i32, distance: f32) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - {:?}\n              - 5.0\n              - 5.0\n        body: static\n        sensor: true\n      CameraZone:\n        priority: {priority}\n        rig:\n          mode:\n            chase:\n              distance: {distance:?}\n",
        (from + to) / 2.0,
        (to - from) / 2.0,
    )
}

/// The chase distance of `rig`'s authored config.
fn chase_distance(rig: &CameraRig) -> f32 {
    match &rig.rig.mode {
        RigMode::Chase(chase) => chase.distance,
        other => panic!("expected a chase rig, got {other:?}"),
    }
}

/// The hero walks +x at 6 m/s and parks at x = 20.
fn walk_to_twenty(world: &mut World, ctx: &SystemCtx) {
    let hero = named(world, "hero");
    world
        .get_mut::<Transform>(hero)
        .unwrap()
        .unwrap()
        .translation = [(0.1 * ctx.frame.as_u64() as f32).min(20.0), 0.0, 0.0];
}

fn zone_engine(tag: &str, scene: &str, systems: Systems) -> Engine {
    engine_for(tag, &[], scene, systems)
}

fn nested_zones() -> String {
    zone("outer", 5.0, 15.0, 1, 8.0) + &zone("inner", 10.5, 13.5, 2, 3.0)
}

#[test]
fn walking_through_nested_zones_hits_each_in_turn() {
    let scene = zone_scene(PHYSICS, 0.0, &nested_zones());
    let mut engine = zone_engine("zone-nest", &scene, &[("walk", walk_to_twenty)]);
    let (outer, inner) = (
        named(engine.world(), "outer"),
        named(engine.world(), "inner"),
    );
    let mut seen = Vec::new();
    for _ in 0..260 {
        engine.advance_live();
        let zone = rig_of(&engine).state.zone;
        if seen.last() != Some(&zone) {
            seen.push(zone);
        }
    }
    assert_eq!(
        seen,
        [None, Some(outer), Some(inner), Some(outer), None],
        "enter, nest, back out through the lower zone, back to the base"
    );
    advance(&mut engine, 200);
    let rig = rig_of(&engine);
    assert_eq!(chase_distance(&rig), 5.0, "the base is back");
    assert!(rig.state.base.is_none());
    let hero = [20.0, 0.0, 0.0];
    assert!(
        (distance(eye_of(&engine), hero) - 5.0).abs() < 1e-2,
        "the eye settles on the base boom"
    );
}

#[test]
fn equal_priority_zones_report_l1555_once_and_the_lower_id_wins() {
    let zones = zone("a", 5.0, 15.0, 1, 8.0) + &zone("b", 5.0, 15.0, 1, 3.0);
    let scene = zone_scene(PHYSICS, 8.0, &zones);
    let engine = zone_engine("zone-tie", &scene, &[]);
    let a = named(engine.world(), "a");
    let lines = log_lines(engine, 30);
    assert_eq!(reported(&lines, "L1555").len(), 1, "{lines:?}");
    let mut engine = zone_engine("zone-tie-run", &scene, &[]);
    advance(&mut engine, 5);
    assert_eq!(rig_of(&engine).state.zone, Some(a));
}

fn write_fov_at_80(world: &mut World, ctx: &SystemCtx) {
    walk_to_twenty(world, ctx);
    if ctx.frame.as_u64() == 80 {
        let cam = named(world, "cam");
        world.get_mut::<CameraRig>(cam).unwrap().unwrap().rig = RigConfig {
            mode: RigMode::Chase(ChaseRig {
                distance: 6.0,
                ..ChaseRig::default()
            }),
            fov_y_deg: Some(33.0),
            ..RigConfig::default()
        };
    }
}

#[test]
fn a_game_write_inside_a_zone_survives_and_becomes_the_base() {
    let scene = zone_scene(PHYSICS, 0.0, &zone("z", 2.0, 15.0, 1, 8.0));
    let mut engine = zone_engine("zone-write", &scene, &[("walk", write_fov_at_80)]);
    advance(&mut engine, 90);
    let rig = rig_of(&engine);
    assert!(rig.state.zone.is_some());
    assert_eq!(rig.rig.fov_y_deg, Some(33.0), "the write is not fought");
    assert_eq!(rig.state.base.as_ref().unwrap().fov_y_deg, Some(33.0));
    advance(&mut engine, 120);
    let rig = rig_of(&engine);
    assert_eq!(rig.state.zone, None);
    assert_eq!(rig.rig.fov_y_deg, Some(33.0), "the write is the base");
    assert_eq!(chase_distance(&rig), 6.0);
}

fn teleport_in_and_snap(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 20 {
        let hero = named(world, "hero");
        world
            .get_mut::<Transform>(hero)
            .unwrap()
            .unwrap()
            .translation = [10.0, 0.0, 0.0];
        let cam = named(world, "cam");
        snap_rig(world.get_mut::<CameraRig>(cam).unwrap().unwrap());
    }
}

#[test]
fn a_teleport_with_a_snap_into_a_zone_cuts_straight_to_it() {
    let scene = zone_scene(PHYSICS, 0.0, &zone("z", 5.0, 15.0, 1, 8.0));
    let mut engine = zone_engine("zone-snap", &scene, &[("tp", teleport_in_and_snap)]);
    let z = named(engine.world(), "z");
    advance(&mut engine, 21);
    let rig = rig_of(&engine);
    assert_eq!(rig.state.zone, Some(z));
    assert!(rig.state.blend.is_none(), "a snap starts no blend");
    assert!(
        (distance(eye_of(&engine), [10.0, 0.0, 0.0]) - 8.0).abs() < 1e-3,
        "the eye is on the zone's boom at once"
    );
}

#[test]
fn a_load_with_the_target_inside_a_zone_starts_on_its_rig() {
    let scene = zone_scene(PHYSICS, 10.0, &zone("z", 5.0, 15.0, 1, 8.0));
    let mut engine = zone_engine("zone-load", &scene, &[]);
    let z = named(engine.world(), "z");
    advance(&mut engine, 1);
    let rig = rig_of(&engine);
    assert_eq!(rig.state.zone, Some(z));
    assert!(rig.state.blend.is_none());
    assert!((distance(eye_of(&engine), [10.0, 0.0, 0.0]) - 8.0).abs() < 1e-3);
}

fn despawn_zone_at_60(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 60 {
        let zone = named(world, "z");
        world.despawn(zone).expect("the zone despawns");
    }
}

#[test]
fn a_despawned_active_zone_falls_back_to_the_base() {
    let scene = zone_scene(PHYSICS, 8.0, &zone("z", 5.0, 15.0, 1, 8.0));
    let mut engine = zone_engine("zone-despawn", &scene, &[("kill", despawn_zone_at_60)]);
    advance(&mut engine, 50);
    assert!(rig_of(&engine).state.zone.is_some());
    advance(&mut engine, 20);
    let rig = rig_of(&engine);
    assert_eq!(rig.state.zone, None);
    assert_eq!(chase_distance(&rig), 5.0);
}

#[test]
fn zones_without_physics_report_l1553_once_and_the_rig_runs_on_its_own_config() {
    let scene = zone_scene(
        "",
        8.0,
        "  - name: z\n    components:\n      CameraZone: {}\n",
    );
    let lines = log_lines(zone_engine("zone-nophys-log", &scene, &[]), 30);
    assert_eq!(reported(&lines, "L1553").len(), 1, "{lines:?}");
    let mut engine = zone_engine("zone-nophys", &scene, &[]);
    advance(&mut engine, 5);
    assert_eq!(rig_of(&engine).state.zone, None);
}

fn zone_hashes(tag: &str) -> Vec<u64> {
    let scene = zone_scene(PHYSICS, 0.0, &nested_zones());
    let mut engine = zone_engine(tag, &scene, &[("walk", walk_to_twenty)]);
    (0..260)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_zone_run_hashes_the_same_twice_and_at_one_and_eight_threads() {
    let first = zone_hashes("zone-hash-a");
    assert_eq!(first, zone_hashes("zone-hash-b"));
    let distinct: std::collections::BTreeSet<u64> = first.iter().copied().collect();
    assert!(distinct.len() > 100, "a flat trace proves nothing");
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "zone-hash-t1"), (8, "zone-hash-t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || zone_hashes(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
    assert_eq!(one, first);
}

#[test]
fn a_snapshot_restore_inside_a_zone_mid_blend_resumes_the_same_hashes() {
    const AT: usize = 70;
    let scene = zone_scene(PHYSICS, 0.0, &nested_zones());
    let build = |tag| zone_engine(tag, &scene, &[("walk", walk_to_twenty)]);
    let mut reference = build("zone-restore-ref");
    let hashes: Vec<u64> = (0..200)
        .map(|_| {
            reference.advance_live();
            reference.world_hash().unwrap().as_u64()
        })
        .collect();

    let mut before = build("zone-restore-a");
    advance(&mut before, AT);
    let rig = rig_of(&before);
    assert!(
        rig.state.zone.is_some() && rig.state.blend.is_some(),
        "mid-blend in a zone"
    );
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = build("zone-restore-b");
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(resumed.world_hash().unwrap().as_u64(), hashes[AT - 1]);
    for (i, expected) in hashes[AT..].iter().enumerate() {
        resumed.advance_live();
        assert_eq!(
            resumed.world_hash().unwrap().as_u64(),
            *expected,
            "tick {} after the restore",
            i + 1
        );
    }
}

#[test]
fn camera_occluders_returns_the_sorted_props_between_the_eye_and_the_target() {
    let probe = zone_engine("occluder-probe", &zone_scene(PHYSICS, 0.0, ""), &[]);
    let mut probe = probe;
    advance(&mut probe, 3);
    let eye = eye_of(&probe);
    let mid = [eye[0] / 2.0, eye[1] / 2.0, eye[2] / 2.0];
    let props = wall("prop_b", mid, [0.3, 0.3, 0.3], "wall")
        + &wall(
            "prop_a",
            [mid[0], mid[1] + 0.1, mid[2]],
            [0.3, 0.3, 0.3],
            "wall",
        )
        + &wall(
            "ghost",
            [mid[0] + 0.1, mid[1], mid[2]],
            [0.3, 0.3, 0.3],
            "ghost",
        )
        + &wall("far", [30.0, 0.0, 30.0], [0.3, 0.3, 0.3], "wall");
    let mut engine = zone_engine("occluder", &zone_scene(PHYSICS, 0.0, &props), &[]);
    advance(&mut engine, 3);
    let world = engine.world();
    let cam = named(world, "cam");
    let layers = ludion_physics::LayerSet::from(vec!["wall".to_string()]);
    let found = ludion_engine::camera_occluders(world, cam, &layers, 0.2).unwrap();
    let mut expected = vec![named(world, "prop_a"), named(world, "prop_b")];
    expected.sort_unstable();
    assert_eq!(
        found, expected,
        "the ghost layer and the far prop are ignored"
    );

    let bad = ludion_physics::LayerSet::from(vec!["nowhere".to_string()]);
    assert!(ludion_engine::camera_occluders(world, cam, &bad, 0.2).is_err());
    let none = ludion_engine::camera_occluders(world, named(world, "hero"), &layers, 0.2);
    assert_eq!(none.unwrap(), vec![], "no rig on the hero");
}
