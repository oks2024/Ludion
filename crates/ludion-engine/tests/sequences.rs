//! Sequences (`ludion docs sequences`): the `kind: sequence` timeline, `play`, skip and cancel. Every
//! test builds a game from `doc_text` documents on an empty temp root and reads one event list per tick.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::dialogue::DialogueEvent;
use ludion_engine::sequences::{self, SequenceEnd, SequenceEvent, SequenceHandle, SequenceTrack};
use ludion_engine::{
    Animation, CameraRig, Diagnostics, DialogueConfig, DialogueEvents, Engine, EngineBuilder,
    SequenceEvents, UiTween, dialogue_update, sequence_update,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;
use std::path::PathBuf;

const STRINGS: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  t.hello: Copy that.
";

const DIALOGUE: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
lines:
  hello:
    speaker: operator
    text: t.hello
    duration_ticks: 100
";

fn beat(clock: &str) -> String {
    format!(
        "format: 1
kind: sequence
roles:
  - cam
  - actor
  - panel
length_ticks: 40
clock: {clock}
tracks:
  camera:
    - at: 5
      role: cam
      rig:
        focus_offset:
          - 0.0
          - 1.0
          - 0.0
    - at: 30
      role: cam
      restore: true
  animation:
    - at: 10
      role: actor
      clip: run
  ui:
    - at: 15
      role: panel
      tracks:
        - field: opacity
          from:
            - 0.0
          to:
            - 1.0
          duration: 0.1
  audio:
    - at: 20
      play: sounds/ping.wav
  dialogue:
    - at: 25
      line: hello
  event:
    - at: 10
      name: e_fire
    - at: 35
      name: e_drop
      on_skip: drop
"
    )
}

const BEAT: &str = "sequences/beat.yaml";
const BEAT_SIM: &str = "sequences/beat_sim.yaml";
const START_OFFSET: [f32; 3] = [0.0, 0.5, 0.0];

/// A hold window and a slow-motion flag the test sets as resources.
struct HoldRange(u64, u64);
struct Slow;

/// The gameplay frame of every tick, in order.
#[derive(Default)]
struct GameFrames(Vec<u64>);

fn control(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64();
    let hold = world
        .resource::<HoldRange>()
        .is_some_and(|range| range.0 <= frame && frame < range.1);
    let slow = world.resource::<Slow>().is_some();
    let clock = world
        .resource_mut::<ludion_core::time::GameClock>()
        .unwrap();
    if hold {
        clock.hold();
    }
    if slow {
        clock.scale(0.5);
    }
}

fn probe(world: &mut World, ctx: &SystemCtx) {
    if world.resource::<GameFrames>().is_none() {
        world.insert_resource(GameFrames::default());
    }
    world
        .resource_mut::<GameFrames>()
        .unwrap()
        .0
        .push(ctx.game_frame.as_u64());
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-sequences-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bare(name: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .dialogue(DialogueConfig::default())
        .sequences()
        .doc_text("strings/en.yaml", STRINGS)
        .doc_text("dialogue/main.yaml", DIALOGUE)
}

fn game(name: &str) -> EngineBuilder {
    bare(name)
        .system("control", control)
        .system("sequence_update", sequence_update)
        .system("dialogue_update", dialogue_update)
        .system("probe", probe)
        .doc_text(BEAT, &beat("game"))
        .doc_text(BEAT_SIM, &beat("sim"))
}

struct Beat {
    engine: Engine,
    cam: EntityId,
    actor: EntityId,
    panel: EntityId,
}

fn scene(mut engine: Engine) -> Beat {
    engine
        .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
        .unwrap();
    let world = engine.world_mut();
    let cam = world.spawn();
    let mut rig = CameraRig::default();
    rig.rig.focus_offset = START_OFFSET;
    world.insert(cam, rig).unwrap();
    let actor = world.spawn();
    world
        .insert(
            actor,
            Animation {
                clip: "idle".into(),
                ..Animation::default()
            },
        )
        .unwrap();
    let panel = world.spawn();
    Beat {
        engine,
        cam,
        actor,
        panel,
    }
}

fn boot(name: &str) -> Beat {
    scene(game(name).build().expect("the docs are valid"))
}

fn play(beat: &mut Beat, key: &str) -> SequenceHandle {
    let (cam, actor, panel) = (beat.cam, beat.actor, beat.panel);
    sequences::play(
        beat.engine.world_mut(),
        key,
        &[("cam", cam), ("actor", actor), ("panel", panel)],
    )
    .unwrap()
}

fn tick(engine: &mut Engine) -> Vec<SequenceEvent> {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
    engine
        .world()
        .resource::<SequenceEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn ticks(engine: &mut Engine, n: usize) -> Vec<Vec<SequenceEvent>> {
    (0..n).map(|_| tick(engine)).collect()
}

fn dialogue_events(engine: &Engine) -> Vec<DialogueEvent> {
    engine
        .world()
        .resource::<DialogueEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn fired_tick(run: &[Vec<SequenceEvent>], handle: SequenceHandle, name: &str) -> Option<usize> {
    run.iter().position(|events| {
        events.iter().any(|event| {
            matches!(event, SequenceEvent::Fired { handle: h, name: n } if *h == handle && n == name)
        })
    })
}

fn fired_count(run: &[Vec<SequenceEvent>], name: &str) -> usize {
    run.iter()
        .flatten()
        .filter(|event| matches!(event, SequenceEvent::Fired { name: n, .. } if n == name))
        .count()
}

fn ended_at(run: &[Vec<SequenceEvent>]) -> Option<(usize, SequenceEnd)> {
    run.iter().enumerate().find_map(|(i, events)| {
        events.iter().find_map(|event| match event {
            SequenceEvent::Ended { end, .. } => Some((i, *end)),
            _ => None,
        })
    })
}

fn focus(beat: &Beat) -> [f32; 3] {
    beat.engine
        .world()
        .get::<CameraRig>(beat.cam)
        .unwrap()
        .unwrap()
        .rig
        .focus_offset
}

fn clip(beat: &Beat) -> String {
    beat.engine
        .world()
        .get::<Animation>(beat.actor)
        .unwrap()
        .unwrap()
        .clip
        .clone()
}

fn tween(beat: &Beat) -> Option<UiTween> {
    beat.engine
        .world()
        .get::<UiTween>(beat.panel)
        .unwrap()
        .cloned()
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .map(String::from)
        .collect()
}

#[test]
fn the_key_stream_matches_the_expected_list_exactly() {
    let mut beat = boot("stream");
    let handle = play(&mut beat, BEAT);
    let run = ticks(&mut beat.engine, 45);
    let mut stream = Vec::new();
    for (i, events) in run.iter().enumerate() {
        for event in events {
            if let SequenceEvent::KeyApplied { track, at, .. } = event {
                stream.push((i, *track, *at));
            }
        }
    }
    use SequenceTrack::{Animation as Anim, Audio, Camera, Dialogue, Event, Ui};
    assert_eq!(
        stream,
        vec![
            (5, Camera, 5),
            (10, Anim, 10),
            (10, Event, 10),
            (15, Ui, 15),
            (20, Audio, 20),
            (25, Dialogue, 25),
            (30, Camera, 30),
            (35, Event, 35),
        ]
    );
    assert_eq!(ended_at(&run), Some((40, SequenceEnd::Completed)));
    assert!(!sequences::is_playing(beat.engine.world(), handle));
    assert_eq!(fired_tick(&run, handle, "e_fire"), Some(10));
}

#[test]
fn a_hold_shifts_game_clock_keys_and_the_sim_clock_runs_through_it() {
    let mut beat = boot("hold");
    beat.engine.world_mut().insert_resource(HoldRange(5, 15));
    let game_handle = play(&mut beat, BEAT);
    let sim_handle = play(&mut beat, BEAT_SIM);
    let run = ticks(&mut beat.engine, 70);
    let frames = beat
        .engine
        .world()
        .resource::<GameFrames>()
        .unwrap()
        .0
        .clone();
    let expected = frames
        .iter()
        .position(|frame| frame - frames[0] >= 10)
        .unwrap();
    assert!(expected > 10, "the hold must delay the game clock");
    assert_eq!(fired_tick(&run, game_handle, "e_fire"), Some(expected));
    assert_eq!(fired_tick(&run, sim_handle, "e_fire"), Some(10));
}

#[test]
fn a_time_scale_stretches_game_clock_keys() {
    let mut beat = boot("scale");
    beat.engine.world_mut().insert_resource(Slow);
    let handle = play(&mut beat, BEAT);
    let run = ticks(&mut beat.engine, 120);
    let frames = beat
        .engine
        .world()
        .resource::<GameFrames>()
        .unwrap()
        .0
        .clone();
    let expected = frames
        .iter()
        .position(|frame| frame - frames[0] >= 10)
        .unwrap();
    assert!(
        expected >= 18,
        "half speed takes about twice the ticks, got {expected}"
    );
    assert_eq!(fired_tick(&run, handle, "e_fire"), Some(expected));
}

fn natural() -> (Beat, Vec<Vec<SequenceEvent>>) {
    let mut beat = boot("natural");
    play(&mut beat, BEAT);
    let run = ticks(&mut beat.engine, 50);
    (beat, run)
}

#[test]
fn skipping_at_any_tick_ends_in_the_state_a_watched_run_does() {
    let (reference, _) = natural();
    let end_tween = tween(&reference).expect("a watched run writes the tween");
    for t in 0..40 {
        let mut beat = boot(&format!("skip-{t}"));
        let handle = play(&mut beat, BEAT);
        let mut run = ticks(&mut beat.engine, t);
        assert!(sequences::skip(beat.engine.world_mut(), handle));
        run.extend(ticks(&mut beat.engine, 50 - t));
        assert_eq!(
            ended_at(&run),
            Some((t, SequenceEnd::Skipped)),
            "skip at {t}"
        );
        assert_eq!(focus(&beat), START_OFFSET, "rig after skip at {t}");
        assert_eq!(clip(&beat), "run", "clip after skip at {t}");
        let written = tween(&beat).unwrap_or_else(|| panic!("no tween after skip at {t}"));
        assert!(
            written.tracks.iter().all(|track| track.duration == 0.0) || written == end_tween,
            "tween after skip at {t}: {written:?}"
        );
        assert_eq!(
            written
                .tracks
                .iter()
                .map(|track| &track.to)
                .collect::<Vec<_>>(),
            end_tween
                .tracks
                .iter()
                .map(|track| &track.to)
                .collect::<Vec<_>>()
        );
        assert_eq!(fired_count(&run, "e_fire"), 1, "e_fire after skip at {t}");
        assert_eq!(
            fired_count(&run, "e_drop"),
            usize::from(t > 35),
            "e_drop after skip at {t}"
        );
    }
}

#[test]
fn a_watched_run_ends_on_the_pre_sequence_rig() {
    let (beat, _) = natural();
    assert_eq!(focus(&beat), START_OFFSET);
    let mut beat = boot("restore-mid");
    play(&mut beat, BEAT);
    ticks(&mut beat.engine, 12);
    assert_eq!(focus(&beat), [0.0, 1.0, 0.0]);
}

#[test]
fn a_dialogue_key_starts_its_line_the_same_tick_and_skip_interrupts_it() {
    let mut beat = boot("dialogue");
    let handle = play(&mut beat, BEAT);
    for i in 0..25 {
        tick(&mut beat.engine);
        assert!(dialogue_events(&beat.engine).is_empty(), "tick {i}");
    }
    tick(&mut beat.engine);
    assert!(
        dialogue_events(&beat.engine)
            .iter()
            .any(|e| matches!(e, DialogueEvent::Started { line, .. } if line == "hello")),
        "the line starts on the key's tick"
    );
    tick(&mut beat.engine);
    assert!(sequences::skip(beat.engine.world_mut(), handle));
    tick(&mut beat.engine);
    assert!(dialogue_events(&beat.engine).iter().any(|e| matches!(
        e,
        DialogueEvent::Ended {
            reason: ludion_engine::dialogue::EndReason::Interrupted,
            ..
        }
    )));
}

#[test]
fn cancel_leaves_no_end_state() {
    let mut beat = boot("cancel");
    let handle = play(&mut beat, BEAT);
    let mut run = ticks(&mut beat.engine, 16);
    assert!(sequences::cancel(beat.engine.world_mut(), handle));
    run.extend(ticks(&mut beat.engine, 30));
    assert_eq!(ended_at(&run), Some((16, SequenceEnd::Cancelled)));
    assert_eq!(focus(&beat), [0.0, 1.0, 0.0], "no restore on cancel");
    assert_eq!(clip(&beat), "run");
    assert!(run[17..].iter().all(Vec::is_empty), "nothing after the end");
    assert!(!sequences::cancel(beat.engine.world_mut(), handle));
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

#[test]
fn a_restore_mid_span_reproduces_the_event_stream_and_the_hashes() {
    let mut beat = boot("restore");
    play(&mut beat, BEAT);
    ticks(&mut beat.engine, 12);
    let snapshot = snapshot_of(&mut beat.engine);
    let record = |engine: &mut Engine| -> (Vec<Vec<SequenceEvent>>, Vec<u64>) {
        let mut stream = Vec::new();
        let mut hashes = Vec::new();
        for _ in 0..40 {
            stream.push(tick(engine));
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        (stream, hashes)
    };
    let reference = record(&mut beat.engine);
    assert!(reference.0.iter().any(|events| !events.is_empty()));
    restore(&mut beat.engine, snapshot);
    assert_eq!(record(&mut beat.engine), reference);
}

#[test]
fn runs_hash_identically_and_an_unused_opt_in_hashes_like_none() {
    let run = |name: &str| {
        let mut beat = boot(name);
        play(&mut beat, BEAT);
        (0..50)
            .map(|_| {
                tick(&mut beat.engine);
                beat.engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run("hash-a"), run("hash-b"));

    let quiet = |opted_in: bool| {
        let builder = if opted_in {
            game("quiet-in")
        } else {
            Engine::builder().project_root(fresh_dir("quiet-out"))
        };
        let mut engine = builder.build().unwrap();
        engine
            .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
            .unwrap();
        (0..10)
            .map(|_| {
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(quiet(true), quiet(false));
}

fn failing(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(error) => format!("{error:?}"),
    }
}

fn with_doc(name: &str, text: &str) -> EngineBuilder {
    bare(name).doc_text("sequences/bad.yaml", text)
}

#[test]
fn each_document_code_fires_as_specified() {
    let head = "format: 1
kind: sequence
roles:
  - cam
";
    let event = |at: u32, name: &str| {
        format!(
            "tracks:
  event:
    - at: {at}
      name: {name}
"
        )
    };
    let cases = [
        (
            "L1730",
            format!(
                "{head}length_ticks: 0
"
            ),
        ),
        (
            "L1730",
            format!(
                "{head}length_ticks: 5
{}",
                event(9, "a")
            ),
        ),
        (
            "L1730",
            format!(
                "{head}length_ticks: 9
tracks:
  event:
    - at: 5
      name: a
    - at: 2
      name: b
"
            ),
        ),
        (
            "L1730",
            format!(
                "{head}length_ticks: 9
tracks:
  camera:
    - at: 1
      role: cam
"
            ),
        ),
        (
            "L1731",
            format!(
                "{head}length_ticks: 9
tracks:
  animation:
    - at: 1
      role: nobody
      clip: x
"
            ),
        ),
        (
            "L1732",
            format!(
                "{head}length_ticks: 9
{}",
                event(1, "end")
            ),
        ),
        (
            "L1733",
            format!(
                "{head}length_ticks: 9
tracks:
  dialogue:
    - at: 1
      line: missing
"
            ),
        ),
        (
            "L1120",
            format!(
                "{head}length_ticks: 9
tracks:
  particles: []
"
            ),
        ),
        (
            "L1734",
            format!(
                "{head}length_ticks: 9
holds_gameplay: true
clock: game
"
            ),
        ),
        (
            "L1735",
            format!(
                "{head}length_ticks: 9
tracks:
  dialogue:
    - at: 4
      line: hello
      wait: line_end
  event:
    - at: 4
      name: a
      wait: release
"
            ),
        ),
    ];
    for (n, (expected, text)) in cases.iter().enumerate() {
        let message = failing(with_doc(&format!("doc-{n}"), text));
        assert!(
            message.contains(expected),
            "expected {expected} in: {message}"
        );
    }
    let mut ok = with_doc("doc-ok", &format!("{head}length_ticks: 9\n"))
        .build()
        .unwrap();
    assert!(ok.doc_validation_warnings().is_empty());
    let _ = &mut ok;
}

const STRINGS_WAIT: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  t.hello: Copy that.
  t.short: Short.
  t.long: A longer line.
";

const DIALOGUE_WAIT: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
lines:
  hello:
    speaker: operator
    text: t.hello
    duration_ticks: 100
  short:
    speaker: operator
    text: t.short
    duration_ticks: 20
  long:
    speaker: operator
    text: t.long
    duration_ticks: 35
";

fn wait_line(line: &str) -> String {
    format!(
        "format: 1
kind: sequence
length_ticks: 60
tracks:
  dialogue:
    - at: 10
      line: {line}
      wait: line_end
  event:
    - at: 10
      name: with_line
    - at: 20
      name: after_line
"
    )
}

const WAIT_RELEASE: &str = "format: 1
kind: sequence
length_ticks: 30
tracks:
  event:
    - at: 5
      name: door
      wait: release
    - at: 10
      name: after
";

const HOLD: &str = "format: 1
kind: sequence
length_ticks: 10
holds_gameplay: true
tracks:
  event:
    - at: 3
      name: held_event
";

const HOLD_WAIT: &str = "format: 1
kind: sequence
length_ticks: 60
holds_gameplay: true
tracks:
  dialogue:
    - at: 5
      line: long
      wait: line_end
  event:
    - at: 50
      name: late
";

const WAIT_LONG: &str = "sequences/wait_long.yaml";
const WAIT_SHORT: &str = "sequences/wait_short.yaml";
const WAIT_REL: &str = "sequences/wait_release.yaml";
const HOLD_KEY: &str = "sequences/hold.yaml";
const HOLD_WAIT_KEY: &str = "sequences/hold_wait.yaml";

/// The W1 game plus the hold and wait documents (a dialogue doc with a 20- and a 35-tick line).
fn game_waits(name: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .dialogue(DialogueConfig::default())
        .sequences()
        .doc_text("strings/en.yaml", STRINGS_WAIT)
        .doc_text("dialogue/main.yaml", DIALOGUE_WAIT)
        .system("sequence_update", sequence_update)
        .system("dialogue_update", dialogue_update)
        .system("probe", probe)
        .doc_text(BEAT, &beat("game"))
        .doc_text(WAIT_SHORT, &wait_line("short"))
        .doc_text(WAIT_LONG, &wait_line("long"))
        .doc_text(WAIT_REL, WAIT_RELEASE)
        .doc_text(HOLD_KEY, HOLD)
        .doc_text(HOLD_WAIT_KEY, HOLD_WAIT)
}

fn boot_waits(name: &str) -> Beat {
    scene(game_waits(name).build().expect("the docs are valid"))
}

fn play_bare(beat: &mut Beat, key: &str) -> SequenceHandle {
    sequences::play(beat.engine.world_mut(), key, &[]).unwrap()
}

fn game_frames(beat: &Beat) -> Vec<u64> {
    beat.engine
        .world()
        .resource::<GameFrames>()
        .map(|frames| frames.0.clone())
        .unwrap_or_default()
}

#[test]
fn wait_line_end_shifts_later_keys_by_the_line_length() {
    let run = |key: &str, name: &str| {
        let mut beat = boot_waits(name);
        let handle = play_bare(&mut beat, key);
        let run = ticks(&mut beat.engine, 120);
        (
            fired_tick(&run, handle, "with_line"),
            fired_tick(&run, handle, "after_line").unwrap(),
            ended_at(&run).unwrap(),
        )
    };
    let (short_at, short_after, short_end) = run(WAIT_SHORT, "wait-short");
    let (long_at, long_after, long_end) = run(WAIT_LONG, "wait-long");
    assert_eq!(short_at, Some(10));
    assert_eq!(
        long_at,
        Some(10),
        "the key at the wait's own tick is not shifted"
    );
    assert_eq!(long_after - short_after, 15);
    assert_eq!(short_end.1, SequenceEnd::Completed);
    assert_eq!(long_end.1, SequenceEnd::Completed);
    assert_eq!(long_end.0 - short_end.0, 15);
    assert!(short_after > 20, "the wait delays the later key");
}

#[test]
fn wait_release_holds_until_release_and_an_early_release_latches() {
    let mut beat = boot_waits("wait-release");
    let handle = play_bare(&mut beat, WAIT_REL);
    let mut run = ticks(&mut beat.engine, 20);
    assert_eq!(fired_tick(&run, handle, "door"), Some(5));
    assert_eq!(
        fired_tick(&run, handle, "after"),
        None,
        "parked on the wait"
    );
    assert!(sequences::is_playing(beat.engine.world(), handle));
    assert!(sequences::release(beat.engine.world_mut(), handle));
    run.extend(ticks(&mut beat.engine, 40));
    assert_eq!(
        fired_tick(&run, handle, "after"),
        Some(25),
        "resumes on the next clock frame, then runs the 5 ticks to the next key"
    );
    assert_eq!(ended_at(&run), Some((45, SequenceEnd::Completed)));
    assert!(!sequences::release(beat.engine.world_mut(), handle));

    // A release before the key is a latch: the wait never parks.
    let mut early = boot_waits("wait-early");
    let handle = play_bare(&mut early, WAIT_REL);
    assert!(sequences::release(early.engine.world_mut(), handle));
    let run = ticks(&mut early.engine, 40);
    assert_eq!(fired_tick(&run, handle, "door"), Some(5));
    assert_eq!(fired_tick(&run, handle, "after"), Some(10));
    assert_eq!(ended_at(&run), Some((30, SequenceEnd::Completed)));
}

#[test]
fn holds_gameplay_freezes_game_time_for_the_span() {
    let mut beat = boot_waits("holds");
    let hold = play_bare(&mut beat, HOLD_KEY);
    let watched = play(&mut beat, BEAT);
    let mut run = Vec::new();
    let mut holding = Vec::new();
    for _ in 0..30 {
        run.extend(ticks(&mut beat.engine, 1));
        holding.push(sequences::holding(beat.engine.world_mut()));
    }
    let end = ended_at(&run).expect("the holding beat ends").0;
    assert_eq!(end, 10, "the sim clock counts its length");
    assert_eq!(fired_tick(&run, hold, "held_event"), Some(3));
    let frames = game_frames(&beat);
    assert!(
        frames[1..=end].iter().all(|f| *f == frames[1]),
        "game frames stay constant through the span: {frames:?}"
    );
    let resumed = (end..frames.len())
        .find(|&i| frames[i] > frames[i - 1])
        .expect("gameplay resumes");
    assert_eq!(
        resumed,
        end + 1,
        "the last hold is issued on tick end-1; gameplay advances again on end+1: {frames:?}"
    );
    assert_eq!(frames[resumed], frames[1] + 1);
    assert!(
        run[..end]
            .iter()
            .flatten()
            .all(|e| !matches!(e, SequenceEvent::KeyApplied { handle, at, .. } if *handle == watched && *at > 0)),
        "a game-clock beat applies nothing during the span"
    );
    assert!(holding[..end - 1].iter().all(|h| *h), "holding while live");
    assert!(holding[end..].iter().all(|h| !h), "not holding after");
}

#[test]
fn skip_mid_wait_ends_skipped_and_releases_the_hold() {
    let mut beat = boot_waits("skip-wait");
    let handle = play_bare(&mut beat, HOLD_WAIT_KEY);
    let mut run = ticks(&mut beat.engine, 12);
    assert_eq!(fired_tick(&run, handle, "late"), None);
    assert!(sequences::is_playing(beat.engine.world(), handle));
    assert!(sequences::skip(beat.engine.world_mut(), handle));
    run.extend(ticks(&mut beat.engine, 12));
    assert_eq!(ended_at(&run), Some((12, SequenceEnd::Skipped)));
    assert_eq!(
        fired_tick(&run, handle, "late"),
        Some(12),
        "on_skip fires it"
    );
    let frames = game_frames(&beat);
    assert!(
        frames[1..=12].iter().all(|f| *f == frames[1]),
        "held through the wait: {frames:?}"
    );
    assert_eq!(
        frames[13],
        frames[12] + 1,
        "resumes one tick after the ending tick (the two-frame rule): {frames:?}"
    );
}

#[test]
fn skip_mid_wait_interrupts_the_waited_line() {
    let mut beat = boot_waits("skip-line");
    let handle = play_bare(&mut beat, HOLD_WAIT_KEY);
    ticks(&mut beat.engine, 12);
    sequences::skip(beat.engine.world_mut(), handle);
    tick(&mut beat.engine);
    assert!(dialogue_events(&beat.engine).iter().any(|e| matches!(
        e,
        DialogueEvent::Ended {
            reason: ludion_engine::dialogue::EndReason::Interrupted,
            ..
        }
    )));
}

#[test]
fn a_restore_mid_release_wait_and_mid_hold_resumes_identically() {
    let record = |engine: &mut Engine, n: usize| -> (Vec<Vec<SequenceEvent>>, Vec<u64>) {
        let mut stream = Vec::new();
        let mut hashes = Vec::new();
        for _ in 0..n {
            stream.push(tick(engine));
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        (stream, hashes)
    };

    // (a) parked on `wait: release`.
    let mut beat = boot_waits("restore-wait");
    let handle = play_bare(&mut beat, WAIT_REL);
    ticks(&mut beat.engine, 8);
    let snapshot = snapshot_of(&mut beat.engine);
    assert!(sequences::release(beat.engine.world_mut(), handle));
    let reference = record(&mut beat.engine, 40);
    assert!(fired_tick(&reference.0, handle, "after").is_some());
    restore(&mut beat.engine, snapshot);
    assert!(sequences::release(beat.engine.world_mut(), handle));
    assert_eq!(record(&mut beat.engine, 40), reference);

    // (b) mid-span of a holding beat.
    let mut beat = boot_waits("restore-hold");
    play_bare(&mut beat, HOLD_KEY);
    ticks(&mut beat.engine, 5);
    let snapshot = snapshot_of(&mut beat.engine);
    let before = game_frames(&beat).len();
    let reference = record(&mut beat.engine, 20);
    let reference_frames = game_frames(&beat)[before..].to_vec();
    restore(&mut beat.engine, snapshot);
    let before = game_frames(&beat).len();
    assert_eq!(record(&mut beat.engine, 20), reference);
    assert_eq!(game_frames(&beat)[before..], reference_frames[..]);
}

#[test]
fn unused_hold_and_wait_documents_hash_like_no_opt_in() {
    let quiet = |opted_in: bool| {
        let builder = if opted_in {
            game_waits("quiet-waits-in")
        } else {
            Engine::builder().project_root(fresh_dir("quiet-waits-out"))
        };
        let mut engine = builder.build().unwrap();
        engine
            .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
            .unwrap();
        (0..10)
            .map(|_| {
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(quiet(true), quiet(false));
}

#[test]
fn play_refuses_what_it_cannot_start() {
    let mut beat = boot("play-errors");
    let cam = beat.cam;
    let world = beat.engine.world_mut();
    assert_eq!(
        sequences::play(world, "sequences/none.yaml", &[])
            .unwrap_err()
            .code,
        code::SEQUENCE_UNKNOWN
    );
    assert_eq!(
        sequences::play(world, BEAT, &[("nobody", cam)])
            .unwrap_err()
            .code,
        code::SEQUENCE_BIND_INVALID
    );
    assert_eq!(
        sequences::play(world, BEAT, &[("cam", cam), ("cam", cam)])
            .unwrap_err()
            .code,
        code::SEQUENCE_BIND_INVALID
    );
    let mut off = Engine::builder().build().unwrap();
    assert_eq!(
        sequences::play(off.world_mut(), BEAT, &[])
            .unwrap_err()
            .code,
        code::SEQUENCES_NOT_ENABLED
    );
}

#[test]
fn a_missing_role_and_a_degraded_key_each_warn_once() {
    let mut beat = boot("warnings");
    let cam = beat.cam;
    sequences::play(beat.engine.world_mut(), BEAT, &[("cam", cam)]).unwrap();
    ticks(&mut beat.engine, 45);
    // `actor` (animation key) and `panel` (ui key) are unbound.
    assert_eq!(reported(&beat.engine, code::SEQUENCE_ROLE_MISSING).len(), 2);

    let mut beat = boot("degraded");
    let (cam, panel) = (beat.cam, beat.panel);
    // The `actor` role is bound to an entity with no `Animation`.
    sequences::play(
        beat.engine.world_mut(),
        BEAT,
        &[("cam", cam), ("actor", panel), ("panel", panel)],
    )
    .unwrap();
    ticks(&mut beat.engine, 45);
    assert_eq!(reported(&beat.engine, code::SEQUENCE_KEY_DEGRADED).len(), 1);
}

#[test]
fn a_reused_entity_slot_still_warns_for_its_new_sequence() {
    let mut beat = boot("reuse");
    let cam = beat.cam;
    let first = sequences::play(beat.engine.world_mut(), BEAT, &[("cam", cam)]).unwrap();
    ticks(&mut beat.engine, 45);
    assert!(!sequences::is_playing(beat.engine.world(), first));
    let second = sequences::play(beat.engine.world_mut(), BEAT, &[("cam", cam)]).unwrap();
    assert_eq!(
        second.0.index(),
        first.0.index(),
        "the free list reuses the slot"
    );
    assert_ne!(second, first);
    ticks(&mut beat.engine, 45);
    // `actor` and `panel` are unbound for each of the two sequences.
    assert_eq!(reported(&beat.engine, code::SEQUENCE_ROLE_MISSING).len(), 4);
}
