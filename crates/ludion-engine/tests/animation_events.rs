//! Runtime proof for `AnimationEvents` (`Plan/animation-clips.md` W2 decisions 11, 14, 15): the
//! `animation_events` system reports every `(entity, clip, event)` the clip clock crossed, in a
//! deterministic order, for assemblies and characters, and only for the incoming clip of a
//! crossfade. Everything goes through the public surface — a scene document, `EngineBuilder`, the
//! ordinary sim step — over a fixture project written to a temp directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::par;
use ludion_core::time::GameClock;
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    Animation, AnimationEvents, ClipEvent, Egress, EgressChannels, Engine, EngineBuilder, Mesh,
    PrefabInstance, Sprite, animation_events, verify_replay_keyframes,
};
use ludion_protocol::command::{StreamSubscribeParams, SubscribeMode};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody, command};

/// A one-part assembly binding the one-shot `swing` clip.
const ASSEMBLY: &str = "\
format: 1
kind: assembly
name: fixture
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  swing: anims/swing.anim.yaml
  recover: anims/recover.anim.yaml
  lap: anims/lap.anim.yaml
";

/// Half a second, one-shot: `vulnerable` at the start.
const RECOVER: &str = "\
format: 1
kind: animation
name: recover
duration: 0.5
loop: false
events:
  - t: 0.0
    name: vulnerable
tracks:
  body:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 0.5
        v:
          - 90.0
          - 0.0
          - 0.0
";

/// One second, looping: `mark` at 0.51.
const LAP: &str = "\
format: 1
kind: animation
name: lap
duration: 1.0
loop: true
events:
  - t: 0.51
    name: mark
tracks:
  body:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 90.0
          - 0.0
          - 0.0
";

/// Half a second, one-shot: `swing` at the start, `hit` at a quarter second.
const SWING: &str = "\
format: 1
kind: animation
name: swing
duration: 0.5
loop: false
events:
  - t: 0.0
    name: swing
  - t: 0.25
    name: hit
tracks:
  body:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 0.5
        v:
          - 90.0
          - 0.0
          - 0.0
";

/// The gallery mannequin's `run` (0.8 s loop) at overlay speed 2 with two footstep events, and
/// its `jump` (0.8 s one-shot) with a `land` event.
const CHARACTER: &str = "\
format: 1
kind: character
name: fast
mesh: \"../meshes/skin-mannequin.mesh.yaml\"
clips:
  jump:
    clip: \"../clips/jump.anim.yaml\"
    events:
      - t: 0.2
        name: land
  run:
    clip: \"../clips/run.anim.yaml\"
    speed: 2.0
    events:
      - t: 0.1
        name: foot_l
      - t: 0.4
        name: foot_r
";

const SWING_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: swinger
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: swing
";

const RUN_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: runner
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: run
";

/// `fading` crosses from `jump` into `run`; `outgoing` plays `jump` on its own.
const FADE_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: fading
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: run
        from: jump
        fade: 1.0
  - name: outgoing
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: jump
";

/// Every kind at once, for the thread-count comparison.
const MIXED_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: swinger
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: swing
  - name: runner
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: run
  - name: fading
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: run
        from: jump
        fade: 1.0
  - name: jumper
    components:
      Mesh:
        shape:
          character: characters/fast.character.yaml
      Animation:
        clip: jump
";

const SPRITE_FADE_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: hero
    components:
      Sprite:
        atlas: atlas.yaml
        frame: idle
      Animation:
        clip: run
        from: walk
        fade: 0.5
";

/// The events every tick fired, `(tick, event)`, in firing order.
#[derive(Default)]
struct EventLog(Vec<(u64, ClipEvent)>);

fn record(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let fired: Vec<ClipEvent> = world
        .resource::<AnimationEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<EventLog>().is_none() {
        world.insert_resource(EventLog::default());
    }
    let log = world.resource_mut::<EventLog>().expect("just inserted");
    log.0
        .extend(fired.into_iter().map(|event| (ctx.frame.as_u64(), event)));
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("record", record)
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a fixture path has a parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A fresh project under the system temp dir: the assembly fixture plus the gallery mannequin and
/// its clips, with the `fast` character.
fn fixture_project(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-anim-events-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "models/fixture.assembly.yaml", ASSEMBLY);
    write(
        &dir,
        "models/parts/body.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
    );
    write(&dir, "models/anims/swing.anim.yaml", SWING);
    write(&dir, "models/anims/recover.anim.yaml", RECOVER);
    write(&dir, "models/anims/lap.anim.yaml", LAP);
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery");
    for file in [
        "meshes/skin-mannequin.mesh.yaml",
        "clips/jump.anim.yaml",
        "clips/run.anim.yaml",
    ] {
        write(
            &dir,
            file,
            &std::fs::read_to_string(gallery.join(file)).unwrap(),
        );
    }
    write(&dir, "characters/fast.character.yaml", CHARACTER);
    dir
}

fn engine_with(name: &str, scene: &str) -> Engine {
    let dir = fixture_project(name);
    let mut engine = game(&dir).build().expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn step(ticks: u64) -> Command {
    Command::SimStep(command::SimStepParams { ticks, hash: false })
}

/// The event log after `ticks` ticks.
fn log_after(engine: &mut Engine, ticks: u64) -> Vec<(u64, ClipEvent)> {
    engine.execute(step(ticks)).expect("the sim steps");
    engine
        .world()
        .resource::<EventLog>()
        .map(|log| log.0.clone())
        .unwrap_or_default()
}

/// The ticks on which `name` fired in `log`.
fn ticks_of(log: &[(u64, ClipEvent)], name: &str) -> Vec<u64> {
    log.iter()
        .filter(|(_, event)| event.name == name)
        .map(|(tick, _)| *tick)
        .collect()
}

#[test]
fn an_assembly_one_shot_fires_its_events_then_end_once_each() {
    let mut engine = engine_with("swing", SWING_SCENE);
    let log = log_after(&mut engine, 90);
    let names: Vec<&str> = log.iter().map(|(_, e)| e.name.as_str()).collect();
    assert_eq!(names, ["swing", "hit", "end"], "{log:?}");
    assert!(log.iter().all(|(_, event)| event.clip == "swing"));
    assert_eq!(
        ticks_of(&log, "swing"),
        [0],
        "an event at t = 0 fires on the start tick"
    );
    // 0.25 s and 0.5 s at 60 Hz are ticks 15 and 30, give or take the last bit of a float.
    assert!((15..=16).contains(&ticks_of(&log, "hit")[0]), "{log:?}");
    assert!((30..=31).contains(&ticks_of(&log, "end")[0]), "{log:?}");
}

#[test]
fn a_character_loop_fires_overlay_events_at_the_overlay_speed() {
    let mut engine = engine_with("run", RUN_SCENE);
    let log = log_after(&mut engine, 60);
    // Speed 2 makes the 0.8 s clip a 0.4 s cycle: 24 ticks. Two events per cycle, 2.5 cycles.
    let left = ticks_of(&log, "foot_l");
    let right = ticks_of(&log, "foot_r");
    assert_eq!(left.len(), 3, "{log:?}");
    assert_eq!(right.len(), 2, "{log:?}");
    assert!((3..=4).contains(&left[0]), "{left:?}");
    // A cycle is 24 ticks; the last float bit of `T` can land an event a tick late.
    assert!((24..=25).contains(&(left[1] - left[0])), "{left:?}");
    assert!((24..=25).contains(&(right[1] - right[0])), "{right:?}");
    assert!(left[0] < right[0] && right[0] < left[1], "{log:?}");
    assert!(
        log.iter().all(|(_, event)| event.name != "end"),
        "a loop has no end"
    );
}

#[test]
fn during_a_crossfade_only_the_incoming_clip_emits() {
    let mut engine = engine_with("fade", FADE_SCENE);
    let log = log_after(&mut engine, 60);
    let fading = |event: &ClipEvent| event.entity.index == 0;
    let fading_events: Vec<&str> = log
        .iter()
        .filter(|(_, event)| fading(event))
        .map(|(_, event)| event.name.as_str())
        .collect();
    assert!(fading_events.contains(&"foot_l"), "{log:?}");
    assert!(
        fading_events.iter().all(|name| name.starts_with("foot")),
        "the outgoing jump's `land`/`end` must not fire on the fading entity: {log:?}"
    );
    let outgoing: Vec<&str> = log
        .iter()
        .filter(|(_, event)| !fading(event))
        .map(|(_, event)| event.name.as_str())
        .collect();
    assert_eq!(
        outgoing,
        ["land", "end"],
        "the plain jump still emits: {log:?}"
    );
}

/// The per-tick event log and the `WorldHash` trace of the mixed scene at `threads` threads.
fn trace(threads: usize) -> (Vec<(u64, ClipEvent)>, Vec<u64>) {
    let mut engine = engine_with(&format!("threads-{threads}"), MIXED_SCENE);
    let hashes: Vec<u64> = (0..60)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect();
    let log = engine
        .world()
        .resource::<EventLog>()
        .map(|log| log.0.clone())
        .unwrap_or_default();
    (log, hashes)
}

#[test]
fn the_event_log_and_hashes_are_identical_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [1usize, 2, 8]
            .map(|n| scope.spawn(move || par::with_threads(n, || trace(n))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert!(!one.0.is_empty(), "the trace must actually carry events");
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

/// The `log`-channel lines a live session emits over `ticks` ticks (`patterns.rs`'s `log_lines`).
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

#[test]
fn a_runtime_sprite_crossfade_latches_l1209_once() {
    let engine = engine_with("sprite", SPRITE_FADE_SCENE);
    let reported: Vec<LogEvent> = log_lines(engine, 20)
        .into_iter()
        .filter(|line| code_is(line, "L1209"))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "once per entity over 20 ticks: {reported:?}"
    );
}

#[test]
fn a_character_whose_model_fails_to_load_latches_l1210_once() {
    // The scene loads with the model intact; the library reads it lazily on the first tick, so
    // corrupting the file in between is the runtime failure.
    let dir = fixture_project("unloaded");
    let mut engine = game(&dir).build().expect("the game builds");
    engine
        .load_inline_scene(SWING_SCENE)
        .expect("the scene loads");
    write(&dir, "models/fixture.assembly.yaml", "format: 1\nkind: [\n");
    let reported: Vec<LogEvent> = log_lines(engine, 20)
        .into_iter()
        .filter(|line| code_is(line, "L1210"))
        .collect();
    assert_eq!(
        reported.len(),
        1,
        "once per entity over 20 ticks: {reported:?}"
    );
    assert!(
        reported[0].message.contains("e0v0")
            && reported[0].message.contains("models/fixture.assembly.yaml"),
        "names the entity and the model: {}",
        reported[0].message
    );
    let diagnostic = reported[0].diagnostic.as_ref().expect("structured");
    assert!(
        !diagnostic.message.contains(": L1"),
        "{}",
        diagnostic.message
    );
    let oracle = ludion_scene::check_assembly("format: 1\nkind: [\n").expect_err("corrupt");
    assert_eq!(
        diagnostic.related.first().map(|d| d.code),
        oracle.first().map(|d| d.code),
        "the loader's first diagnostic rides along as related: {diagnostic:?}"
    );
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}

type Driver = fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx);

/// The game with an optional driver system registered before `animation_events` (an early starter)
/// and one after `record` (a reaction to this tick's events).
fn game_around(root: &Path, early: Option<Driver>, late: Option<Driver>) -> EngineBuilder {
    let mut builder = Engine::builder().project_root(root);
    if let Some(driver) = early {
        builder = builder
            .system("driver_early", driver)
            .query::<(&mut Animation,)>();
    }
    builder = builder
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("record", record);
    if let Some(driver) = late {
        builder = builder
            .system("driver_late", driver)
            .query::<(&mut Animation,)>();
    }
    builder
}

/// Crossfades into `recover` on the tick `swing` ends: stamped after this tick's walk.
fn react_on_end(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let events = world
        .resource::<AnimationEvents>()
        .cloned()
        .unwrap_or_default();
    for (id, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        if events.fired(id, "swing", "end") {
            ludion_engine::crossfade(animation, "recover", 0.1, ctx.game_frame);
        }
    }
}

/// Crossfades into `recover` at game frame 10: stamped before this tick's walk.
fn start_at_ten(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.game_frame.as_u64() == 10 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            ludion_engine::crossfade(animation, "recover", 0.1, ctx.game_frame);
        }
    }
}

/// Speed 0.5 at game frame 20, then 3.0 at frame 50.
fn ramp(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let speed = match ctx.game_frame.as_u64() {
        20 => 0.5,
        50 => 3.0,
        _ => return,
    };
    for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
        ludion_engine::set_speed(animation, speed, ctx.game_time);
    }
}

/// Speed 0.25 at game frame 60.
fn slow_at_sixty(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.game_frame.as_u64() == 60 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            ludion_engine::set_speed(animation, 0.25, ctx.game_time);
        }
    }
}

/// Speed 0.5 at game frame 59: an anchor whose elapsed time is not a short decimal.
fn slow_at_59(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.game_frame.as_u64() == 59 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            ludion_engine::set_speed(animation, 0.5, ctx.game_time);
        }
    }
}

fn built(
    name: &str,
    scene: &str,
    early: Option<Driver>,
    late: Option<Driver>,
) -> (Engine, PathBuf) {
    let dir = fixture_project(name);
    let mut engine = game_around(&dir, early, late)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    (engine, dir)
}

fn fires_of(log: &[(u64, ClipEvent)], clip: &str, name: &str) -> Vec<u64> {
    log.iter()
        .filter(|(_, event)| event.clip == clip && event.name == name)
        .map(|(tick, _)| *tick)
        .collect()
}

#[test]
fn a_clip_started_after_the_walk_fires_its_zero_event_on_the_next_tick_once() {
    let (mut engine, _) = built("late-start", SWING_SCENE, None, Some(react_on_end));
    let log = log_after(&mut engine, 90);
    let end = fires_of(&log, "swing", "end");
    assert!((30..=31).contains(&end[0]), "{log:?}");
    assert_eq!(fires_of(&log, "recover", "vulnerable"), [end[0] + 1]);
}

#[test]
fn a_clip_started_before_the_walk_fires_its_zero_event_on_the_start_tick_once() {
    let (mut engine, _) = built("early-start", SWING_SCENE, Some(start_at_ten), None);
    let log = log_after(&mut engine, 90);
    assert_eq!(fires_of(&log, "recover", "vulnerable"), [10], "{log:?}");
}

/// A snapshot taken right after the start (late: after the stamp, early: on the start tick) and
/// restored into a fresh engine continues with the tail an uninterrupted run fires.
#[test]
fn a_restore_neither_loses_nor_repeats_a_zero_event() {
    for (name, early, late) in [
        ("restore-late", None, Some(react_on_end as Driver)),
        ("restore-early", Some(start_at_ten as Driver), None),
    ] {
        let (mut reference, dir) = built(name, SWING_SCENE, early, late);
        let full = log_after(&mut reference, 90);
        let fired = fires_of(&full, "recover", "vulnerable");
        assert_eq!(fired.len(), 1, "{name}: {full:?}");
        // The late starter's stamp lands on the tick before its first walk.
        let switch = if late.is_some() {
            fired[0] - 1
        } else {
            fired[0]
        };

        let (mut engine, _) = built(name, SWING_SCENE, early, late);
        engine.execute(step(switch + 1)).unwrap();
        let ResponseBody::Snapshot { snapshot } =
            engine.execute(Command::EngineSnapshot).unwrap().body
        else {
            panic!("expected a snapshot");
        };
        let mut restored = game_around(&dir, early, late).build().unwrap();
        restored
            .execute(Command::EngineRestore(command::EngineRestoreParams {
                snapshot,
            }))
            .expect("the snapshot restores");
        restored.execute(step(90 - (switch + 1))).unwrap();
        let tail = restored
            .world()
            .resource::<EventLog>()
            .map(|log| log.0.clone())
            .unwrap_or_default();
        let expected: Vec<_> = full.iter().filter(|(t, _)| *t > switch).cloned().collect();
        assert_eq!(tail, expected, "{name}");
        // The early start fired on the snapshot tick itself, so only the late one is in the tail.
        assert_eq!(
            fires_of(&tail, "recover", "vulnerable").len(),
            usize::from(late.is_some()),
            "{name}"
        );
    }
}

/// Clip time of the `fast` character's `run` (overlay 2.0) under [`ramp`], in 1/120 s: speed 1 to
/// frame 20, 0.5 to frame 50, then 3.0.
fn ramped_run_time(tick: u64) -> u64 {
    match tick {
        0..=20 => 4 * tick,
        21..=50 => 80 + 2 * (tick - 20),
        _ => 140 + 12 * (tick - 50),
    }
}

#[test]
fn set_speed_mid_loop_fires_each_crossing_exactly_once_on_either_side_of_the_walk() {
    for (name, early, late) in [
        ("ramp-early", Some(ramp as Driver), None),
        ("ramp-late", None, Some(ramp as Driver)),
    ] {
        let (mut engine, _) = built(name, RUN_SCENE, early, late);
        let log = log_after(&mut engine, 101);
        // The 0.8 s loop is 96 units; `foot_l` sits at 0.1 s (12), `foot_r` at 0.4 s (48). The last
        // tick's clip time (740) is 20 units clear of the event before it.
        let last = ramped_run_time(100);
        let events: Vec<(&str, u64)> = (0..)
            .flat_map(|cycle| [("foot_l", 96 * cycle + 12), ("foot_r", 96 * cycle + 48)])
            .take_while(|(_, at)| *at <= last + 40)
            .filter(|(_, at)| *at <= last)
            .collect();
        let fired: Vec<(&str, u64)> = log
            .iter()
            .map(|(tick, event)| (event.name.as_str(), *tick))
            .collect();
        assert_eq!(
            fired.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            events.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            "{name}: alternating, none lost or doubled: {log:?}"
        );
        for ((_, at), (_, tick)) in events.iter().zip(&fired) {
            let first = (0..=100).find(|&n| ramped_run_time(n) >= *at).unwrap();
            let tie = ramped_run_time(first) == *at;
            assert!(
                *tick == first || (tie && *tick == first + 1),
                "{name}: event at {at} fired at {tick}, due {first}: {log:?}"
            );
        }
    }
}

const LAP_SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: lapper
    components:
      Mesh:
        shape:
          assembly: models/fixture.assembly.yaml
      Animation:
        clip: lap
";

/// The audit's own scenario: a 1 s loop with an event at 0.51, slowed to a quarter at tick 60.
#[test]
fn slowing_a_loop_past_a_cycle_boundary_does_not_fire_an_event_twice() {
    for (name, early, late) in [
        ("lap-early", Some(slow_at_sixty as Driver), None),
        ("lap-late", None, Some(slow_at_sixty as Driver)),
    ] {
        let (mut engine, _) = built(name, LAP_SCENE, early, late);
        let log = log_after(&mut engine, 200);
        // Cycle 0 at 0.51 s (tick 31); then 1 + (n − 60)/240 reaches 1.51 at tick 183.
        assert_eq!(fires_of(&log, "lap", "mark"), [31, 183], "{name}");
    }
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `set_speed` anchor holds an `f64` in hashed state; it must survive JSON text bit for bit, in a
/// snapshot sent over the wire and in a replay keyframe verified by a fresh engine.
#[test]
fn a_set_speed_anchor_survives_snapshot_text_and_a_keyframe_verify() {
    let (mut engine, dir) = built("anchor-text", RUN_SCENE, Some(slow_at_59), None);
    engine.execute(step(60)).unwrap();
    let ResponseBody::Snapshot { snapshot } = engine.execute(Command::EngineSnapshot).unwrap().body
    else {
        panic!("expected a snapshot");
    };
    let text = serde_json::to_string(&snapshot).unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&text).unwrap();
    let mut restored = game_around(&dir, Some(slow_at_59), None).build().unwrap();
    restored
        .execute(Command::EngineRestore(command::EngineRestoreParams {
            snapshot,
        }))
        .expect("the snapshot restores");
    assert_eq!(restored.world_hash(), engine.world_hash());
    engine.execute(step(30)).unwrap();
    restored.execute(step(30)).unwrap();
    assert_eq!(restored.world_hash(), engine.world_hash());

    let sink = SharedSink::default();
    let mut recorded = game_around(&dir, Some(slow_at_59), None).build().unwrap();
    recorded.load_inline_scene(RUN_SCENE).unwrap();
    recorded.set_recording(sink.clone()).unwrap();
    for _ in 0..90 {
        recorded.execute(step(1)).unwrap();
    }
    recorded.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    let report =
        verify_replay_keyframes(game_around(&dir, Some(slow_at_59), None), &bytes, &dir).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert!(report.keyframes >= 2);
    assert_eq!(report.verified, report.keyframes);
}

/// A raw `speed` write (no `set_speed`, no restamp) of 0.25 at game frame 60.
fn raw_slow_at_sixty(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.game_frame.as_u64() == 60 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            animation.speed = 0.25;
        }
    }
}

/// A raw write that lowers the speed re-bases nothing, so clip time jumps back: the walk fires
/// nothing until clip time passes the point already walked, instead of repeating the crossings.
#[test]
fn a_raw_speed_write_that_lowers_the_speed_repeats_nothing() {
    for (name, early, late) in [
        ("raw-early", Some(raw_slow_at_sixty as Driver), None),
        ("raw-late", None, Some(raw_slow_at_sixty as Driver)),
    ] {
        let (mut engine, _) = built(name, LAP_SCENE, early, late);
        let log = log_after(&mut engine, 400);
        // 0.51 s at tick 31; the clock re-passes the walked point, then 1.51 = n/240 at n = 362.4.
        assert_eq!(fires_of(&log, "lap", "mark"), [31, 363], "{name}");
    }
}

#[test]
fn a_huge_authored_speed_steps_without_a_panic() {
    let scene = RUN_SCENE.replace("clip: run", "clip: run\n        speed: 1.0e21");
    let (mut engine, _) = built("huge-speed", &scene, None, None);
    let log = log_after(&mut engine, 4);
    assert!(log.len() <= 4 * 65 * 2, "{}", log.len());
}

/// Raw ticks still to hold, set by [`react_and_hold`].
struct HoldFor(u32);

/// [`react_on_end`], plus a five-tick hold that starts on the tick `swing` ends: the end tick and
/// the four after it call `hold()`, so raw ticks `end + 1..=end + 5` are held.
fn react_and_hold(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let ended = world
        .resource::<AnimationEvents>()
        .is_some_and(|events| events.iter().any(|e| e.clip == "swing" && e.name == "end"));
    react_on_end(world, ctx);
    if ended {
        world.insert_resource(HoldFor(5));
    }
    let remaining = world.resource::<HoldFor>().map_or(0, |hold| hold.0);
    if remaining > 0 {
        world.resource_mut::<GameClock>().unwrap().hold();
        world.resource_mut::<HoldFor>().unwrap().0 = remaining - 1;
    }
}

/// A clip started by a late system on the tick before a hold fires its `t = 0` event once, on the
/// first walk after the hold; the held ticks log nothing.
#[test]
fn a_late_start_just_before_a_hold_fires_its_zero_event_once() {
    let (mut engine, _) = built("late-hold", SWING_SCENE, None, Some(react_and_hold));
    let log = log_after(&mut engine, 90);
    let end = fires_of(&log, "swing", "end")[0];
    assert_eq!(
        fires_of(&log, "recover", "vulnerable"),
        [end + 6],
        "{log:?}"
    );
    assert!(
        log.iter()
            .all(|(tick, _)| !(end + 1..=end + 5).contains(tick)),
        "a held tick fires nothing: {log:?}"
    );
}

/// Holds raw ticks 41..=50 (a hold on raw frame N holds tick N + 1) and calls `set_speed(0.5)`
/// mid-hold, at raw frame 45.
fn hold_and_slow(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let raw = ctx.frame.as_u64();
    if (40..=49).contains(&raw) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
    if raw == 45 {
        for (_, (animation,)) in world.query_ids::<(&mut Animation,)>() {
            ludion_engine::set_speed(animation, 0.5, ctx.game_time);
        }
    }
}

/// Clip time of the `fast` character's `run` (overlay 2.0) at raw tick `raw` under [`hold_and_slow`],
/// in 1/120 s: the game clock freezes at 40 for raw 41..=50, and speed halves from game frame 40.
fn held_slow_run_time(raw: u64) -> u64 {
    let game = match raw {
        0..=40 => raw,
        41..=50 => 40,
        _ => raw - 10,
    };
    if game <= 40 {
        4 * game
    } else {
        160 + 2 * (game - 40)
    }
}

#[test]
fn set_speed_during_a_hold_neither_skips_nor_repeats() {
    for (name, early, late) in [
        ("hold-early", Some(hold_and_slow as Driver), None),
        ("hold-late", None, Some(hold_and_slow as Driver)),
    ] {
        let (mut engine, _) = built(name, RUN_SCENE, early, late);
        let log = log_after(&mut engine, 119);
        // The 0.8 s loop is 96 units; `foot_l` at 12 + 96c, `foot_r` at 48 + 96c. The last tick's
        // clip time (296) is 4 units clear of the next event (300).
        let last = held_slow_run_time(118);
        let events: Vec<(&str, u64)> = (0..)
            .flat_map(|cycle| [("foot_l", 96 * cycle + 12), ("foot_r", 96 * cycle + 48)])
            .take_while(|(_, at)| *at <= last + 40)
            .filter(|(_, at)| *at <= last)
            .collect();
        let fired: Vec<(&str, u64)> = log
            .iter()
            .map(|(tick, event)| (event.name.as_str(), *tick))
            .collect();
        assert_eq!(
            fired.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            events.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            "{name}: alternating, none lost or doubled: {log:?}"
        );
        for ((_, at), (_, tick)) in events.iter().zip(&fired) {
            let first = (0..=118).find(|&n| held_slow_run_time(n) >= *at).unwrap();
            let tie = held_slow_run_time(first) == *at;
            assert!(
                *tick == first || (tie && *tick == first + 1),
                "{name}: event at {at} fired at {tick}, due {first}: {log:?}"
            );
        }
        assert!(
            fired.iter().all(|(_, tick)| !(41..=50).contains(tick)),
            "{name}: a held tick fires nothing: {log:?}"
        );
    }
}

/// An `Animation` on a prefab instance's root plays the prefab's `clips:`: the one-shot `open`
/// fires its implicit `end` once, on the root, at its 1 s mark.
#[test]
fn a_prefab_root_fires_end_for_its_one_shot_clip() {
    let dir =
        std::env::temp_dir().join(format!("ludion-anim-events-{}-prefab", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected");
    for file in [
        "model.prefab.yaml",
        "clips/open.anim.yaml",
        "clips/snap.anim.yaml",
        "meshes/base.mesh.yaml",
        "meshes/lid.mesh.yaml",
        "meshes/latch.mesh.yaml",
    ] {
        write(
            &dir,
            &format!("hinge/{file}"),
            &std::fs::read_to_string(source.join(file)).unwrap(),
        );
    }
    let mut engine = game(&dir).build().expect("the game builds");
    engine
        .load_inline_scene(
            "format: 1\nseed: 1\nentities:\n  - name: door\n    prefab: hinge/model.prefab.yaml\n    overrides:\n      base:\n        Animation:\n          clip: open\n",
        )
        .expect("the scene loads");
    let log = log_after(&mut engine, 90);
    let ends = ticks_of(&log, "end");
    assert_eq!(ends.len(), 1, "{log:?}");
    assert!((60..=61).contains(&ends[0]), "{log:?}");
    assert!(
        log.iter()
            .all(|(_, event)| event.entity.index == 0 && event.clip == "open"),
        "the root is the door's first part: {log:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
