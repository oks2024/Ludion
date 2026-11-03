//! Runtime proof for projectile patterns (`Plan/projectile-patterns.md` Wave 1): a `kind: pattern`
//! document played by a [`PatternPlayer`] stands closed-form [`Volley`] entities up on exact frames,
//! and every bullet on the field is a pure function of `(volley, bullet kind, index, now)`.
//!
//! The claims, in the order the tests make them: a timeline fires on the frames it names (`loop`
//! and `play` included); a `shmup-stress`-shaped storm's live census equals its closed form and the
//! extracted sprite count equals the census; [`volley_hits`] finds the bullet crossing a known
//! circle at a known instant and a kill bit takes it out of both the query and the draw; a volley
//! run extracted every frame and one extracted never produce the same `WorldHash` trace and
//! byte-identical recordings; editing a bullet kind re-shapes the field already in flight and rides
//! the recording as a `DocReload`; a player naming an unloaded document degrades with one `L2108`
//! instead of crashing; and a game on `Plane::Xy` lays its bullets in XY.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    BulletKind, DocStore, Egress, EgressChannels, Engine, EngineBuilder, PatternDoc, QualityParams,
    Transform, Volley, advance_volleys, bullet_is_alive, bullet_position, extract_at,
    play_patterns, volley_hits,
};
use ludion_protocol::command::{
    AssetReloadParams, EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Channel, Command, ResponseBody};
use ludion_renderer::SpriteInstance;
use ludion_spatial::Plane;

const TICK: TickRate = TickRate::DEFAULT;
/// The document key every scene below plays.
const PROBE: &str = "patterns/probe.yaml";

/// A camera and one turret at the origin playing [`PROBE`] from frame 0.
const SCENE: &str = "\
format: 1
seed: 5
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 2.0
          - 8.0
      Camera: {}
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: patterns/probe.yaml
        started: 0
";

/// The same turret naming a document the content root does not hold.
const MISSING_SCENE: &str = "\
format: 1
seed: 5
entities:
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: patterns/gone.yaml
        started: 0
";

// --- Harness ---------------------------------------------------------------------------------

/// A content root holding `patterns/<name>.yaml` for each `(name, body)`.
///
/// Every document is authored in **block** style: the content walk skips a flow-style mapping or
/// sequence, so a `{ a: 1 }` document would load as nothing and every assertion below would pass
/// vacuously.
fn root(tag: &str, docs: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-patterns-it-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("patterns")).expect("a temp content root");
    for (name, body) in docs {
        std::fs::write(dir.join("patterns").join(format!("{name}.yaml")), body).unwrap();
    }
    dir
}

/// The game a pattern needs: the `pattern` doc kind with its validator, and the two systems in the
/// order a game registers them — volleys stand up, then the spent ones are reaped.
fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .patterns()
        .system("play_patterns", play_patterns)
        .system("advance_volleys", advance_volleys)
        .project_root(root)
}

/// An engine over a fresh content root with `scene` loaded.
fn playing(tag: &str, docs: &[(&str, &str)], scene: &str) -> (PathBuf, Engine) {
    let dir = root(tag, docs);
    let mut engine = game(&dir).build().expect("the pattern documents validate");
    engine.load_inline_scene(scene).expect("the scene loads");
    (dir, engine)
}

fn step(ticks: u64) -> Command {
    Command::SimStep(SimStepParams { ticks, hash: false })
}

/// Advances `ticks` fixed ticks. A tick of frame *N* runs the systems with `ctx.frame = N` and then
/// makes the counter *N + 1*, so after `n` ticks the schedule has seen frames `0..n`.
fn advance(engine: &mut Engine, ticks: u64) {
    engine.execute(step(ticks)).expect("the sim steps");
}

/// Every live volley with the origin captured at fire time, in ascending entity order.
fn volleys(engine: &Engine) -> Vec<(Volley, Transform)> {
    let world = engine.world();
    world
        .entities()
        .filter_map(|id| {
            let volley = world.get::<Volley>(id).ok().flatten()?.clone();
            let transform = world.get::<Transform>(id).ok().flatten().copied()?;
            Some((volley, transform))
        })
        .collect()
}

/// One loaded bullet kind.
fn bullet_kind(engine: &Engine, pattern: &str, name: &str) -> BulletKind {
    engine
        .world()
        .resource::<DocStore<PatternDoc>>()
        .expect("the pattern store is registered")
        .get(pattern)
        .expect("the document is loaded")
        .bullets
        .get(name)
        .expect("the bullet kind is defined")
        .clone()
}

/// The bullets on the field at `frame`, counted the way the sim counts them.
fn census(engine: &Engine, frame: u64) -> usize {
    let now = SimTime::from_ticks(frame, TICK);
    volleys(engine)
        .iter()
        .map(|(volley, _)| {
            let kind = bullet_kind(engine, &volley.pattern, &volley.bullet);
            (0..volley.count)
                .filter(|&index| bullet_is_alive(volley, &kind, index, now))
                .count()
        })
        .sum()
}

/// The sprites the render boundary draws at `frame` — bullets included.
fn sprites_at(engine: &Engine, frame: u64) -> Vec<SpriteInstance> {
    extract_at(
        engine.world(),
        (320, 180),
        ludion_assets::AssetRoot::content(engine.project_root()),
        SimTime::from_ticks(frame, TICK),
        QualityParams::FULL.particle_density,
    )
    .expect("the scene extracts")
    .sprites
}

/// The frames on which a volley appeared, over the first `frames` ticks.
fn fire_frames(tag: &str, docs: &[(&str, &str)], frames: u64) -> Vec<u64> {
    let (dir, mut engine) = playing(tag, docs, SCENE);
    let mut fired = Vec::new();
    for frame in 0..frames {
        let before = volleys(&engine).len();
        advance(&mut engine, 1);
        if volleys(&engine).len() > before {
            fired.push(frame);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    fired
}

// --- Documents -------------------------------------------------------------------------------

/// Three volleys ten frames apart. `life: 600` outlives every schedule test, so nothing is reaped
/// underneath the count.
const CADENCE: &str = "\
format: 1
kind: pattern
name: cadence
bullets:
  pellet:
    life: 600
timeline:
  - at: 0
    repeat: 3
    every: 10
    bullet: pellet
    count: 2
    speed: 1.0
";

/// A host whose only entry plays a leaf pattern seven frames in.
const PLAY_HOST: &str = "\
format: 1
kind: pattern
timeline:
  - at: 7
    play: patterns/leaf.yaml
";

/// The leaf: one volley three frames into its own timeline, so the host fires it on frame 10.
const LEAF: &str = "\
format: 1
kind: pattern
bullets:
  spark:
    life: 600
timeline:
  - at: 3
    bullet: spark
    count: 2
    speed: 1.0
";

/// The `shmup-stress` shape: a ring of 25 every four frames, each bullet living two seconds.
const STORM: &str = "\
format: 1
kind: pattern
bullets:
  pellet:
    life: 120
timeline:
  - at: 0
    repeat: 40
    shape: ring
    every: 4
    bullet: pellet
    count: 25
    speed: 3.0
";

/// Four bullets on the axes, bullet 0 straight along the plane's first axis at 2 units a second and
/// bullet 1 along its second — whichever plane the game chose.
const CROSS: &str = "\
format: 1
kind: pattern
bullets:
  pellet:
    life: 600
    hit_radius: 0.1
timeline:
  - at: 0
    shape: ring
    bullet: pellet
    count: 4
    speed: 2.0
    angle_deg: 0.0
";

/// A constant-speed bullet whose speed lives on the **bullet kind**, so a reload re-shapes bullets
/// already in flight (a volley bakes its fire speed in; a kind's motion it reads by name).
fn tuned(speed: &str) -> String {
    format!(
        "\
format: 1
kind: pattern
bullets:
  dart:
    life: 600
    motion:
      - ramp:
          frames: 600
          from: {speed}
          to: {speed}
timeline:
  - at: 0
    bullet: dart
    count: 1
    speed: 2.0
"
    )
}

// --- The schedule ----------------------------------------------------------------------------

/// A timeline stands its volleys up on exactly the frames it names — the repeat cadence, the loop
/// that replays it, and the `play` that offsets a leaf document's own entries.
#[test]
fn a_played_timeline_fires_on_the_frames_it_names() {
    assert_eq!(
        fire_frames("cadence", &[("probe", CADENCE)], 25),
        vec![0, 10, 20],
        "`repeat: 3, every: 10` fires three times and never between"
    );
    assert_eq!(
        fire_frames("looped", &[("probe", &format!("{CADENCE}loop: 30\n"))], 35),
        vec![0, 10, 20, 30],
        "`loop: 30` starts the timeline over on frame 30"
    );
    assert_eq!(
        fire_frames("play", &[("probe", PLAY_HOST), ("leaf", LEAF)], 16),
        vec![10],
        "a `play` at 7 fires the leaf's own `at: 3` entry on frame 10"
    );

    // The played document owns the bullet kind, not the host that reached it.
    let (dir, mut engine) = playing("play-owner", &[("probe", PLAY_HOST), ("leaf", LEAF)], SCENE);
    advance(&mut engine, 12);
    let out = volleys(&engine);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].0.pattern, "patterns/leaf.yaml");
    assert_eq!(out[0].0.bullet, "spark");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- The census ------------------------------------------------------------------------------

/// The closed form of [`STORM`]'s field at frame `N`.
///
/// A volley fires on every frame `4k` and its 25 bullets live 120 frames, so bullet `i` of that
/// volley is on the field at `N` exactly while `0 <= N - 4k < 120`. The fire frames satisfying it
/// are the `k` in `max(0, (N - 119 + 3) / 4) ..= N / 4`, which is `min(N / 4 + 1, 30)` of them
/// (120 / 4 = 30 volleys in flight once the field is saturated).
fn expected_census(frame: u64) -> usize {
    25 * usize::try_from((frame / 4 + 1).min(30)).unwrap()
}

/// The `shmup-stress` shape, frame by frame: the live census equals its closed form, and the
/// renderer draws exactly the bullets the sim counts.
#[test]
fn the_census_of_a_storm_matches_its_closed_form() {
    let (dir, mut engine) = playing("census", &[("probe", STORM)], SCENE);
    for frame in 0..=130u64 {
        advance(&mut engine, 1);
        assert_eq!(
            census(&engine, frame),
            expected_census(frame),
            "the field at frame {frame}"
        );
        if frame % 26 == 0 {
            assert_eq!(
                sprites_at(&engine, frame).len(),
                expected_census(frame),
                "extract draws a different field than the sim counts at frame {frame}"
            );
        }
    }
    assert_eq!(
        expected_census(130),
        750,
        "the saturated field is 30 volleys of 25"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- The query -------------------------------------------------------------------------------

/// `volley_hits` finds the bullet crossing a known circle at a known instant, and a kill bit takes
/// that bullet out of both the query and the draw.
#[test]
fn a_hit_is_found_where_the_bullet_is_drawn_and_a_kill_bit_removes_both() {
    let (dir, mut engine) = playing("hits", &[("probe", CROSS)], SCENE);
    advance(&mut engine, 61); // frames 0..60: the volley fired on 0, so it is one second old at 60
    let now = SimTime::from_ticks(60, TICK);
    let kind = bullet_kind(&engine, PROBE, "pellet");
    let (volley, origin) = volleys(&engine).pop().expect("the volley fired");

    // Bullet 0 launches along the plane's first axis at 2 units a second, so a second in it sits at
    // x = 2 — where a circle of radius 0.2 catches it, and nowhere near one at the origin.
    let hits: Vec<u32> =
        volley_hits(&volley, &origin, &kind, now, Plane::Xz, [2.0, 0.0], 0.2).collect();
    assert_eq!(hits, vec![0], "the bullet on the circle");
    assert!(
        volley_hits(&volley, &origin, &kind, now, Plane::Xz, [0.5, 0.0], 0.2)
            .next()
            .is_none(),
        "a circle the ring has already passed"
    );
    assert_eq!(sprites_at(&engine, 60).len(), 4, "the whole ring draws");

    // The kill bit is the game's write; the engine only reads it.
    let world = engine.world_mut();
    let ids: Vec<_> = world.entities().collect();
    for id in ids {
        if let Ok(Some(volley)) = world.get_mut::<Volley>(id) {
            volley.kill(0);
        }
    }
    let (killed, origin) = volleys(&engine).pop().expect("the volley is still there");
    assert!(
        volley_hits(&killed, &origin, &kind, now, Plane::Xz, [2.0, 0.0], 0.2)
            .next()
            .is_none(),
        "a killed bullet still hits"
    );
    assert_eq!(
        sprites_at(&engine, 60).len(),
        3,
        "a killed bullet is still drawn"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `bullet_position` takes a plain `&Transform` origin and returns a plain `[f32; 3]` — no `glam`
/// reachable from a game — and agrees with the closed form `volley_hits` just proved the hit test
/// and the draw share: bullet 0 of the same ring, one second in, sits at `x = 2`.
#[test]
fn bullet_position_matches_the_closed_form_a_hit_test_would_use() {
    let (dir, mut engine) = playing("position", &[("probe", CROSS)], SCENE);
    advance(&mut engine, 61); // frames 0..60: the volley fired on 0, so it is one second old at 60
    let now = SimTime::from_ticks(60, TICK);
    let kind = bullet_kind(&engine, PROBE, "pellet");
    let (volley, origin) = volleys(&engine).pop().expect("the volley fired");

    let at = bullet_position(&volley, &origin, &kind, 0, now, Plane::Xz);
    assert!(
        (at[0] - 2.0).abs() < 1e-4 && at[1].abs() < 1e-4 && at[2].abs() < 1e-4,
        "bullet 0 at {at:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- The sim is untouched --------------------------------------------------------------------

/// Extracting a volley field every frame moves neither a world hash nor a recorded byte: bullets
/// are computed at the render boundary from hashed volleys and written back nowhere.
#[test]
fn extracting_bullets_changes_neither_the_hashes_nor_the_recording() {
    let dir = root("neutral", &[("probe", STORM)]);

    let run = |tag: &str, extract_each_tick: bool| {
        let lrp = dir.join(format!("{tag}.lrp"));
        let mut engine = game(&dir).build().expect("the documents validate");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        engine
            .set_recording(std::fs::File::create(&lrp).unwrap())
            .expect("recording starts");

        let mut trace = Vec::new();
        let mut drawn = 0usize;
        for _ in 0..40 {
            trace.extend(
                ludion_engine::drive_hashes(&mut engine, vec![step(1)])
                    .expect("the session drives")
                    .into_iter()
                    // `drive_hashes` re-samples the current frame first; keep only the new tick.
                    .skip(1),
            );
            if extract_each_tick {
                drawn += sprites_at(&engine, engine.frame().as_u64()).len();
            }
        }
        engine.stop_recording().expect("recording stops");
        (trace, std::fs::read(&lrp).unwrap(), drawn)
    };

    let (quiet_trace, quiet_bytes, _) = run("quiet", false);
    let (drawn_trace, drawn_bytes, drawn) = run("drawn", true);

    assert_eq!(quiet_trace.len(), 40, "the run actually advanced 40 frames");
    assert!(drawn > 0, "the extracting run actually drew bullets");
    assert_eq!(
        quiet_trace, drawn_trace,
        "extracting bullets must not move a single world hash"
    );
    assert_eq!(
        quiet_bytes, drawn_bytes,
        "extracting bullets must not write a byte into the recording"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `extract` over a world holding live volleys is a deterministic function of `(world, now)` — the
/// `extract_is_deterministic` claim with bullets in the frame.
#[test]
fn extracting_a_volley_field_is_deterministic() {
    let (dir, mut engine) = playing("determinism", &[("probe", STORM)], SCENE);
    advance(&mut engine, 30);
    let a = sprites_at(&engine, 29);
    let b = sprites_at(&engine, 29);
    assert!(!a.is_empty(), "the storm is on the field at frame 29");
    assert_eq!(
        a, b,
        "the same world at the same instant draws the same set"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Hot reload ------------------------------------------------------------------------------

/// Editing a bullet kind's motion re-shapes the bullets already in flight on the next frame, and
/// the swap rides the recording as a `DocReload` so a replay re-applies it off-disk.
#[test]
fn reloading_a_pattern_document_reshapes_the_live_field() {
    let dir = root("reload", &[("probe", &tuned("2.0"))]);
    let lrp = dir.join("session.lrp");
    let mut engine = game(&dir).build().expect("the documents validate");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
        .set_recording(std::fs::File::create(&lrp).unwrap())
        .expect("recording starts");

    advance(&mut engine, 61); // frames 0..60: the dart is one second old
    let before = sprites_at(&engine, 60);
    assert_eq!(before.len(), 1, "one dart");
    assert!(
        (before[0].center.x - 2.0).abs() < 1e-4,
        "a second at 2 units a second is x = 2, not {}",
        before[0].center.x
    );

    std::fs::write(dir.join("patterns/probe.yaml"), tuned("4.0")).unwrap();
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(PROBE.to_string()),
        }))
        .expect("the edited document reloads");
    advance(&mut engine, 1);

    let age = 61.0 / 60.0;
    let after = sprites_at(&engine, 61);
    assert_eq!(
        after.len(),
        1,
        "the volley is untouched — only its kind moved"
    );
    assert!(
        (after[0].center.x - 4.0 * age).abs() < 1e-3,
        "the doubled dart should be at x = {}, not {}",
        4.0 * age,
        after[0].center.x
    );

    engine.stop_recording().expect("recording stops");
    let bytes = std::fs::read(&lrp).unwrap();
    let reloads: Vec<String> = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::DocReload { doc_kind, path, .. } if path == PROBE => Some(doc_kind),
            _ => None,
        })
        .collect();
    assert_eq!(reloads, vec!["pattern".to_string()], "one recorded reload");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Degradation -----------------------------------------------------------------------------

/// The `log`-channel lines a live session emits over `ticks` ticks — the public seam onto
/// `Diagnostics`, which latches once per `(code, key)` and drains at the frame boundary into a
/// `Warn` event. The engine is paused before the loop starts so no tick can report before the
/// subscription exists.
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
    send(step(ticks));

    // The ticks run after the reply lands, so wait for the first line, then drain until the lane
    // goes quiet.
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

/// A player naming a document that is not loaded fires nothing, keeps ticking, and reports `L2108`
/// exactly once however long the run lasts.
#[test]
fn a_missing_pattern_document_degrades_with_one_l2108() {
    let (dir, mut engine) = playing("missing", &[("probe", CADENCE)], MISSING_SCENE);
    advance(&mut engine, 30);
    assert_eq!(
        volleys(&engine).len(),
        0,
        "a missing document fires nothing"
    );
    assert_eq!(engine.frame().as_u64(), 30, "the frames advanced anyway");

    let (_, live) = playing("missing-log", &[("probe", CADENCE)], MISSING_SCENE);
    let reported: Vec<LogEvent> = log_lines(live, 20)
        .into_iter()
        .filter(|line| code_is(line, "L2108"))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "the latch is once per offender, over 20 ticks: {reported:?}"
    );
    assert!(
        reported[0].message.contains("patterns/gone.yaml"),
        "the line names the document: {}",
        reported[0].message
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// --- A pattern system that never ran ----------------------------------------------------------

/// A scene system standing in for "the game has systems, but not the pattern ones".
fn idle(_world: &mut World, _ctx: &SystemCtx) {}

/// A wrapper that owns the call: `play_patterns` runs from the game's own system every tick.
fn wrapped(world: &mut World, ctx: &SystemCtx) {
    play_patterns(world, ctx);
    advance_volleys(world, ctx);
}

fn l1394(lines: &[LogEvent]) -> Vec<&LogEvent> {
    lines.iter().filter(|l| code_is(l, "L1394")).collect()
}

/// An engine over `docs` with the given game systems registered and `scene` loaded.
fn with_systems(tag: &str, scene: &str, build: fn(EngineBuilder) -> EngineBuilder) -> Engine {
    let dir = root(tag, &[("probe", CADENCE)]);
    let mut engine = build(Engine::builder().patterns().project_root(&dir))
        .build()
        .expect("the pattern documents validate");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

#[test]
fn a_player_without_play_patterns_warns_once() {
    let engine = with_systems("unrun", SCENE, |b| b.system("idle", idle));
    let lines = log_lines(engine, 20);
    let warned = l1394(&lines);
    assert_eq!(warned.len(), 1, "once over 20 ticks: {warned:?}");
    assert!(
        warned[0].message.contains("play_patterns"),
        "the line names the fix: {}",
        warned[0].message
    );
}

#[test]
fn volleys_without_advance_volleys_warn_once() {
    let engine = with_systems("no-advance", SCENE, |b| {
        b.system("play_patterns", play_patterns)
    });
    let lines = log_lines(engine, 30);
    let warned = l1394(&lines);
    assert_eq!(warned.len(), 1, "once over 30 ticks: {warned:?}");
    assert!(
        warned[0].message.contains("advance_volleys"),
        "the line names the fix: {}",
        warned[0].message
    );
}

/// A pattern system registered in the wrong phase must not panic on its marker write; it only
/// panics if it really spawns or despawns, which this scene never makes it do.
#[test]
fn pattern_systems_in_the_presentation_phase_do_not_panic_on_their_marker() {
    const BARE: &str = "format: 1\nseed: 5\nentities:\n  - name: camera\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 2.0\n          - 8.0\n      Camera: {}\n";
    let dir = root("presentation", &[("probe", CADENCE)]);
    let mut engine = Engine::builder()
        .patterns()
        .presentation_system("play_patterns", play_patterns)
        .presentation_system("advance_volleys", advance_volleys)
        .project_root(&dir)
        .build()
        .expect("the pattern documents validate");
    engine.load_inline_scene(BARE).expect("the scene loads");
    advance(&mut engine, 5);
    assert_eq!(engine.frame().as_u64(), 5, "the world is still running");
    let lines = log_lines(engine, 3);
    assert!(
        !lines.iter().any(|l| code_is(l, "L4007")),
        "no sim panic: {lines:?}"
    );
}

#[test]
fn registered_pattern_systems_never_warn_across_a_restore() {
    let (_, mut engine) = playing("ran", &[("probe", CADENCE)], SCENE);
    advance(&mut engine, 12);
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .expect("the snapshot restores");
    let lines = log_lines(engine, 5);
    assert!(l1394(&lines).is_empty(), "{:?}", l1394(&lines));
}

#[test]
fn a_paused_correct_game_warns_neither_unticked_nor_after_its_first_step() {
    let (_, engine) = playing("paused", &[("probe", CADENCE)], SCENE);
    let lines = log_lines(engine, 0);
    assert!(l1394(&lines).is_empty(), "no tick, no check: {lines:?}");
    let (_, engine) = playing("paused-step", &[("probe", CADENCE)], SCENE);
    let lines = log_lines(engine, 1);
    assert!(l1394(&lines).is_empty(), "{:?}", l1394(&lines));
}

#[test]
fn a_wrapper_that_calls_the_pattern_systems_never_warns() {
    let engine = with_systems("wrapped", SCENE, |b| b.system("wrapped", wrapped));
    let lines = log_lines(engine, 10);
    assert!(l1394(&lines).is_empty(), "{:?}", l1394(&lines));
}

#[test]
fn a_world_without_game_systems_never_warns() {
    let engine = with_systems("bare", SCENE, |b| b);
    let lines = log_lines(engine, 10);
    assert!(l1394(&lines).is_empty(), "{:?}", l1394(&lines));
}

#[test]
fn an_empty_pattern_name_never_warns() {
    const EMPTY: &str = "\
format: 1
seed: 5
entities:
  - name: turret
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      PatternPlayer:
        pattern: \"\"
        started: 0
";
    let engine = with_systems("empty", EMPTY, |b| b.system("idle", idle));
    let lines = log_lines(engine, 10);
    assert!(l1394(&lines).is_empty(), "{:?}", l1394(&lines));
}

// --- The plane -------------------------------------------------------------------------------

/// A game on `Plane::Xy` lays the same ring in XY: the 90-degree bullet runs along `+y`, and no
/// bullet leaves the plane.
#[test]
fn a_bullet_follows_the_games_plane() {
    let dir = root("plane-xy", &[("probe", CROSS)]);
    let mut engine = game(&dir)
        .plane(Plane::Xy)
        .build()
        .expect("the documents validate");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    advance(&mut engine, 31);

    // Bullet 1 of the ring launches at 90 degrees, which on Xy is +y (on Xz it would be +z).
    let drawn = sprites_at(&engine, 30);
    assert_eq!(drawn.len(), 4, "the whole ring draws");
    let quarter = drawn[1].center;
    assert!(quarter.y > 0.9, "90 degrees on Xy is +y, not {quarter:?}");
    assert!(quarter.x.abs() < 1e-6, "and only +y: {quarter:?}");
    assert!(
        drawn.iter().all(|sprite| sprite.center.z.abs() < 1e-6),
        "no Xy bullet may leave the plane: {:?}",
        drawn.iter().map(|s| s.center).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}
