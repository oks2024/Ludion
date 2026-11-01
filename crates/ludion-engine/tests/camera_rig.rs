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
    CameraRig, DebugCategories, Egress, EgressChannels, Engine, Name, RigMode, Transform,
    add_trauma, snap_rig,
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
    let RigMode::Track(track) = &mut rig.rig.mode;
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
            ludion_physics::WRITE,
            ludion_physics::STEP,
            ludion_physics::MIRROR,
            "camera_rig"
        ]
    );
    let rig = systems.last().unwrap();
    assert_eq!(rig["order"], 5);
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
    assert!(systems[6]["registered_at"]["line"].is_u64());
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
    let RigMode::Track(track) = &mut rig.rig.mode;
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
