//! Engine-level audio drain tests (Phase 8 Wave 4): the frame-stamped log, the optional sender, and
//! the invariant that audio queueing/draining never perturbs the `WorldHash`. Plus the declarative
//! half — the frame-boundary `AudioEmitter` sync: falloff from the listener, the liveness diff, and
//! the emitter-less world that emits nothing at all.

use ludion_core::audio::{Bus, MixLayer};
use ludion_core::ids::EntityId;
use ludion_engine::{
    AudioCategory, AudioDistance, AudioEvent, AudioLogEntry, AudioMessage, AudioPan, AudioQueue,
    Engine, LoopId,
};

const SCENE: &str = "format: 1\nentities:\n  - components:\n      Transform:\n        translation:\n          - 1.0\n          - 2.0\n          - 3.0\n";

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(SCENE).expect("scene loads");
    engine
}

/// Draining stamps each event with the frame whose systems emitted it, in push order.
#[test]
fn drain_stamps_events_with_the_emitting_frame() {
    let mut engine = engine();
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .expect("AudioQueue installed")
        .play("a.wav", 0.5);
    engine.advance_live(); // frame 0 tick, drains → stamped frame 0

    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .play("b.wav", 1.0);
    engine.advance_live(); // frame 1 tick, drains → stamped frame 1

    let log = engine.audio_log();
    assert_eq!(log.len(), 2);
    assert_eq!(
        (log[0].frame.as_u64(), log[0].sound(), log[0].gain()),
        (0, Some("a.wav"), Some(0.5))
    );
    assert_eq!(
        (log[1].frame.as_u64(), log[1].sound(), log[1].gain()),
        (1, Some("b.wav"), Some(1.0))
    );
}

/// Queueing and draining audio events never changes the `WorldHash`: the queue is a resource and the
/// log is host state, both excluded from the hash. A run that pushes events every frame hashes
/// identically, frame for frame, to one that pushes none.
#[test]
fn audio_never_affects_the_world_hash() {
    fn run(push_audio: bool) -> Vec<u64> {
        let mut engine = engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            if push_audio {
                engine
                    .world_mut()
                    .resource_mut::<AudioQueue>()
                    .unwrap()
                    .play("s.wav", 1.0);
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(
        run(true),
        run(false),
        "audio must not perturb the WorldHash sequence"
    );
}

/// A wired sender receives every drained message (in addition to the log).
#[test]
fn wired_sender_receives_drained_events() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = engine();
    engine.set_audio_sender(tx);
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .play("x.wav", 0.75);
    engine.advance_live();

    let message = rx.try_recv().expect("the sink receives the drained event");
    assert_eq!(
        message,
        AudioMessage::OneShot(ludion_engine::AudioEvent::new("x.wav", 0.75))
    );
    assert!(rx.try_recv().is_err(), "exactly one message");
}

/// The four message shapes reach the log in the order a system queued them, all stamped with the
/// emitting frame — the queue is a queue, not a per-kind bucket.
#[test]
fn loop_messages_drain_in_the_order_they_were_queued() {
    let mut engine = engine();
    {
        let queue = engine
            .world_mut()
            .resource_mut::<AudioQueue>()
            .expect("AudioQueue installed");
        queue.play("shot.wav", 1.0);
        queue.loop_start("bed.wav", 0.4);
        queue.loop_set_gain("bed.wav", 0.7);
        queue.loop_stop("bed.wav");
    }
    engine.advance_live();

    let bed = LoopId::Sound("bed.wav".to_string());
    let sync = |gain| AudioMessage::LoopSync {
        id: bed.clone(),
        sound: "bed.wav".to_string(),
        gain,
        pan: 0.0,
        // A manual loop is an ambient bed by construction.
        category: AudioCategory::Ambient,
    };
    let messages: Vec<&AudioMessage> = engine.audio_log().iter().map(|e| &e.message).collect();
    assert_eq!(
        messages,
        vec![
            &AudioMessage::OneShot(ludion_engine::AudioEvent::new("shot.wav", 1.0)),
            &sync(0.4),
            &sync(0.7),
            &AudioMessage::LoopStop { id: bed.clone() },
        ]
    );
    assert!(engine.audio_log().iter().all(|e| e.frame.as_u64() == 0));
}

/// A world rebuild stops every loop: the blanket stop is emitted to the log (and to a wired sink)
/// on every reinstall, so a bed can never outlive the scene that started it.
#[test]
fn a_world_rebuild_emits_the_blanket_loop_stop() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut engine = engine();
    engine.set_audio_sender(tx);
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .loop_start("bed.wav", 0.4);
    engine.advance_live();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");

    let sent: Vec<AudioMessage> = rx.try_iter().collect();
    assert_eq!(
        sent.last(),
        Some(&AudioMessage::StopAllLoops),
        "the rebuild's blanket stop reaches the sink; sent {sent:?}"
    );
}

/// Audio is a pure function of the sim, loops included: re-driving one script — the shape a replay
/// takes — regenerates the identical frame-stamped log, entry for entry.
#[test]
fn a_loop_script_regenerates_an_identical_audio_log() {
    fn run() -> Vec<AudioLogEntry> {
        let mut engine = engine();
        for frame in 0..6 {
            let queue = engine.world_mut().resource_mut::<AudioQueue>().unwrap();
            match frame {
                0 => queue.loop_start("bed.wav", 0.2),
                4 => queue.loop_stop("bed.wav"),
                _ => queue.loop_set_gain("bed.wav", 0.1 * frame as f32),
            }
            engine.advance_live();
        }
        engine.audio_log().iter().cloned().collect()
    }
    let live = run();
    assert_eq!(live.len(), 6, "one message a frame");
    assert_eq!(live, run());
}

// --- The declarative half: `AudioEmitter` ---

/// A listener at the origin and one bell 4 units east of it, in a `range: 10.0` field — so the bell
/// syncs at `1.0 × (1 − 4/10) = 0.6`. Entity ids are document order: camera `e0v0`, bell `e1v0`.
const BELL_SCENE: &str = "\
format: 1
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
  - name: bell
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
      AudioEmitter:
        sound: sounds/bell.wav
        range: 10.0
";

const BELL: EntityId = EntityId::new(1, 0);

fn bell_engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(BELL_SCENE).expect("scene loads");
    engine
}

/// Every loop message about entity `id` in the log, in emission order — the emitter stream a
/// liveness assertion reads, with the world rebuild's blanket stop filtered out.
fn loop_messages(engine: &Engine, id: EntityId) -> Vec<AudioMessage> {
    engine
        .audio_log()
        .iter()
        .map(|entry| entry.message.clone())
        .filter(|message| match message {
            AudioMessage::LoopSync { id: loop_id, .. } | AudioMessage::LoopStop { id: loop_id } => {
                *loop_id == LoopId::Entity(id)
            }
            _ => false,
        })
        .collect()
}

/// The placed emitter syncs itself with no game system registered: one `LoopSync` when it becomes
/// audible, at the falloff its distance from the camera gives it, and nothing while it holds still.
#[test]
fn a_placed_emitter_syncs_at_its_distance_from_the_listener_once_then_holds() {
    let mut engine = bell_engine();
    engine.advance_live();
    engine.advance_live();

    assert_eq!(
        loop_messages(&engine, BELL),
        vec![bell_sync(0.6)],
        "the second tick says nothing new"
    );
}

/// A scene authors `category: voice` and `ui` on an emitter, and the loop syncs on that bus.
#[test]
fn an_emitter_authored_on_the_voice_or_ui_bus_syncs_with_that_category() {
    for (name, category) in [("voice", AudioCategory::Voice), ("ui", AudioCategory::Ui)] {
        let scene = BELL_SCENE.replace(
            "        range: 10.0\n",
            &format!("        range: 10.0\n        category: {name}\n"),
        );
        let mut engine = Engine::new();
        engine.load_inline_scene(&scene).expect("scene loads");
        engine.advance_live();
        let AudioMessage::LoopSync {
            category: synced, ..
        } = &loop_messages(&engine, BELL)[0]
        else {
            panic!("the emitter syncs");
        };
        assert_eq!(*synced, category);
    }
}

fn bell_sync(gain: f32) -> AudioMessage {
    AudioMessage::LoopSync {
        id: LoopId::Entity(BELL),
        sound: "sounds/bell.wav".to_string(),
        gain,
        pan: 0.0,
        category: AudioCategory::Ambient,
    }
}

fn move_bell_to(engine: &mut Engine, x: f32) {
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(BELL)
        .expect("the bell is alive")
        .expect("the bell has a Transform")
        .translation = [x, 0.0, 0.0];
}

/// A gain drift under 1% of the gain last sent is inaudible and sends nothing; past it, one sync at
/// the new gain.
#[test]
fn an_emitter_resyncs_only_when_its_gain_moves_past_one_percent() {
    let mut engine = bell_engine();
    engine.advance_live();
    move_bell_to(&mut engine, 4.05); // 0.595: inside 1% of 0.6
    engine.advance_live();
    assert_eq!(loop_messages(&engine, BELL).len(), 1);

    move_bell_to(&mut engine, 4.1); // 0.59: 1.7% under the 0.6 sent
    engine.advance_live();
    let messages = loop_messages(&engine, BELL);
    assert_eq!(messages.len(), 2, "{messages:?}");
    let AudioMessage::LoopSync { gain, .. } = messages[1] else {
        panic!("a sync, got {messages:?}");
    };
    assert!((gain - 0.59).abs() < 1e-6, "{gain}");
}

/// The sound and the bus are part of what a sync says: changing either sends it again.
#[test]
fn changing_an_emitters_sound_or_bus_resyncs_it() {
    let mut engine = bell_engine();
    engine.advance_live();
    engine
        .world_mut()
        .get_mut::<ludion_engine::AudioEmitter>(BELL)
        .unwrap()
        .unwrap()
        .sound = "sounds/gong.wav".to_string();
    engine.advance_live();
    let messages = loop_messages(&engine, BELL);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(matches!(
        &messages[1],
        AudioMessage::LoopSync { sound, .. } if sound == "sounds/gong.wav"
    ));

    engine
        .world_mut()
        .get_mut::<ludion_engine::AudioEmitter>(BELL)
        .unwrap()
        .unwrap()
        .category = AudioCategory::Sfx;
    engine.advance_live();
    let messages = loop_messages(&engine, BELL);
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(matches!(
        &messages[2],
        AudioMessage::LoopSync {
            category: AudioCategory::Sfx,
            ..
        }
    ));
}

/// A rebuild's blanket stop silences the sink, so the engine must forget what it sent: the same bell
/// in the rebuilt world syncs again.
#[test]
fn a_world_rebuild_resyncs_every_audible_emitter() {
    let mut engine = bell_engine();
    engine.advance_live();
    engine.load_inline_scene(BELL_SCENE).expect("scene loads");
    engine.advance_live();
    assert_eq!(loop_messages(&engine, BELL), vec![bell_sync(0.6)]);
}

/// Despawning an audible emitter mid-scene — no scene switch, no world rebuild, nothing to blanket-
/// stop against — stops its loop on the very next tick, off the liveness diff alone.
#[test]
fn despawning_an_emitter_stops_its_loop_the_next_tick() {
    let mut engine = bell_engine();
    engine.advance_live();
    engine.world_mut().despawn(BELL).expect("the bell is alive");
    engine.advance_live();

    let messages = loop_messages(&engine, BELL);
    assert_eq!(
        messages.last(),
        Some(&AudioMessage::LoopStop {
            id: LoopId::Entity(BELL)
        }),
        "a despawned emitter is stopped, not left sounding; stream was {messages:?}"
    );
    // And the stop is emitted once, not every tick after.
    engine.advance_live();
    assert_eq!(loop_messages(&engine, BELL).len(), messages.len());
}

/// An emitter that walks out of `range` is inaudible, and inaudible is a stop: below the audibility
/// epsilon nothing is sent, so the sink is not fed a stream of silent syncs.
#[test]
fn an_emitter_out_of_range_is_stopped_and_sends_nothing_further() {
    let mut engine = bell_engine();
    engine.advance_live();
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(BELL)
        .expect("the bell is alive")
        .expect("the bell has a Transform")
        .translation = [400.0, 0.0, 0.0];
    engine.advance_live();
    engine.advance_live();

    assert_eq!(
        loop_messages(&engine, BELL),
        vec![
            AudioMessage::LoopSync {
                id: LoopId::Entity(BELL),
                sound: "sounds/bell.wav".to_string(),
                gain: 0.6,
                pan: 0.0,
                category: AudioCategory::Ambient,
            },
            AudioMessage::LoopStop {
                id: LoopId::Entity(BELL)
            },
        ]
    );
}

/// Two cameras tie-break on ascending `EntityId`: the ear is the lower one, so the gain is measured
/// from it and not from the far camera (which would put the bell out of range entirely).
#[test]
fn the_lowest_entity_id_camera_is_the_listener() {
    let scene = BELL_SCENE.replace(
        "  - name: bell\n",
        "  - name: far-camera\n    components:\n      Transform:\n        translation:\n          \
         - 400.0\n          - 0.0\n          - 0.0\n      Camera: {}\n  - name: bell\n",
    );
    let mut engine = Engine::new();
    engine.load_inline_scene(&scene).expect("scene loads");
    engine.advance_live();

    let bell = EntityId::new(2, 0);
    assert_eq!(
        loop_messages(&engine, bell)
            .iter()
            .filter_map(|message| match message {
                AudioMessage::LoopSync { gain, .. } => Some(*gain),
                _ => None,
            })
            .collect::<Vec<f32>>(),
        vec![0.6],
        "the ear is the camera at the origin, not the one 400 units away"
    );
}

/// A world with no `Camera` has no ear, so the step is a no-op for the tick: nothing syncs, and what
/// was already sounding is not stopped either (a world between cameras did not go quiet).
#[test]
fn a_world_without_a_camera_syncs_nothing() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene(&BELL_SCENE.replace("      Camera: {}\n", ""))
        .expect("scene loads");
    engine.advance_live();
    engine.advance_live();

    assert!(
        loop_messages(&engine, BELL).is_empty(),
        "no listener, no sync: {:?}",
        engine.audio_log()
    );
}

/// Invisibility: a world that places no `AudioEmitter` is byte-identical in both observable streams —
/// the audio log carries nothing but the rebuild's blanket stop, and the `WorldHash` sequence matches
/// the same run made with an emitter placed (the component is read, never written).
#[test]
fn an_emitterless_world_emits_nothing_and_hashes_identically() {
    fn run(scene: &str) -> (Vec<AudioLogEntry>, Vec<u64>) {
        let mut engine = Engine::new();
        engine.load_inline_scene(scene).expect("scene loads");
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        (engine.audio_log().iter().cloned().collect(), hashes)
    }

    let bare = BELL_SCENE
        .split("  - name: bell\n")
        .next()
        .expect("the camera half")
        .to_string();
    let (log, bare_hashes) = run(&bare);
    assert!(
        log.is_empty(),
        "an emitter-less world emits nothing at all: {log:?}"
    );

    let (_, emitter_hashes) = run(BELL_SCENE);
    assert_eq!(bare_hashes, run(&bare).1, "the sequence is reproducible");
    for hashes in [&bare_hashes, &emitter_hashes] {
        assert!(
            hashes.windows(2).all(|pair| pair[0] == pair[1]),
            "nothing in the frame boundary writes the world, so a static scene's hash never moves: \
             {hashes:?}"
        );
    }
}

/// Loading a scene starts a fresh audio stream (the log resets on world reset).
#[test]
fn scene_load_resets_the_audio_log() {
    let mut engine = engine();
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .unwrap()
        .play("x.wav", 1.0);
    engine.advance_live();
    assert!(!engine.audio_log().is_empty());

    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    assert!(
        engine.audio_log().is_empty(),
        "a new scene starts a fresh audio stream"
    );
}

// --- Positional one-shots: play_at ---

fn queue(engine: &mut Engine) -> &mut AudioQueue {
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .expect("AudioQueue installed")
}

/// Every one-shot in the log as `(frame, sound, gain, category)`, in emission order — the bell
/// emitter's loop syncs filtered out.
fn one_shots(engine: &Engine) -> Vec<(u64, String, f32, AudioCategory)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|entry| match &entry.message {
            AudioMessage::OneShot(event) => Some((
                entry.frame.as_u64(),
                event.sound.clone(),
                event.gain,
                event.category,
            )),
            _ => None,
        })
        .collect()
}

/// A one-shot placed 4 units from the camera in a `range: 10.0` field logs at the emitter's falloff,
/// `0.6`, on the `Sfx` bus, stamped with the frame it was pushed on.
#[test]
fn play_at_logs_the_attenuated_gain() {
    let mut engine = bell_engine();
    engine.advance_live();
    queue(&mut engine).play_at("hit.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
    engine.advance_live();

    assert_eq!(
        one_shots(&engine),
        vec![(1, "hit.wav".to_string(), 0.6, AudioCategory::Sfx)]
    );
}

/// Out of range, and exactly at range (the hard cutoff), send nothing.
#[test]
fn play_at_out_of_range_sends_nothing() {
    let mut engine = bell_engine();
    queue(&mut engine).play_at("far.wav", 1.0, [25.0, 0.0, 0.0], 10.0);
    queue(&mut engine).play_at("edge.wav", 1.0, [10.0, 0.0, 0.0], 10.0);
    engine.advance_live();

    assert!(one_shots(&engine).is_empty(), "{:?}", engine.audio_log());
}

/// Height does not attenuate: the same gain straight above as on the ground.
#[test]
fn play_at_ignores_height() {
    let mut engine = bell_engine();
    queue(&mut engine).play_at("up.wav", 1.0, [0.0, 40.0, 4.0], 10.0);
    queue(&mut engine).play_at("flat.wav", 1.0, [0.0, 0.0, 4.0], 10.0);
    engine.advance_live();

    let gains: Vec<f32> = one_shots(&engine).iter().map(|shot| shot.2).collect();
    assert_eq!(gains, vec![0.6, 0.6]);
}

/// `push_at` passes the event's category through unchanged.
#[test]
fn push_at_keeps_the_events_category() {
    let mut engine = bell_engine();
    let mut event = AudioEvent::new("wind.wav", 1.0);
    event.category = AudioCategory::Ambient;
    queue(&mut engine).push_at(event, [4.0, 0.0, 0.0], 10.0);
    engine.advance_live();

    assert_eq!(
        one_shots(&engine),
        vec![(0, "wind.wav".to_string(), 0.6, AudioCategory::Ambient)]
    );
}

/// With no camera there is no listener, so a positional one-shot is dropped — and not retried when a
/// camera appears later.
#[test]
fn play_at_without_a_camera_logs_nothing() {
    let mut engine = engine();
    queue(&mut engine).play_at("hit.wav", 1.0, [1.0, 2.0, 3.0], 10.0);
    engine.advance_live();
    engine.advance_live();

    assert!(one_shots(&engine).is_empty(), "{:?}", engine.audio_log());
}

/// Plain and positional one-shots log in push order.
#[test]
fn play_at_keeps_push_order() {
    let mut engine = bell_engine();
    queue(&mut engine).play("a.wav", 1.0);
    queue(&mut engine).play_at("b.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
    queue(&mut engine).play("c.wav", 1.0);
    engine.advance_live();

    let sounds: Vec<String> = one_shots(&engine).into_iter().map(|shot| shot.1).collect();
    assert_eq!(sounds, ["a.wav", "b.wav", "c.wav"]);
}

/// The listener is the camera's pose when the frame ends: move it onto the sound and the gain is full.
#[test]
fn play_at_follows_the_listener() {
    let mut engine = bell_engine();
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(EntityId::new(0, 0))
        .expect("the camera is alive")
        .expect("the camera has a Transform")
        .translation = [4.0, 0.0, 0.0];
    queue(&mut engine).play_at("hit.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
    engine.advance_live();

    assert_eq!(
        one_shots(&engine),
        vec![(0, "hit.wav".to_string(), 1.0, AudioCategory::Sfx)]
    );
}

/// A positional one-shot never perturbs the `WorldHash`.
#[test]
fn play_at_never_affects_the_world_hash() {
    fn run(push_audio: bool) -> Vec<u64> {
        let mut engine = bell_engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            if push_audio {
                queue(&mut engine).play_at("s.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(run(true), run(false));
}

// --- Music ---

/// Every music message in the log as `(frame, message)`, in emission order.
fn music_messages(engine: &Engine) -> Vec<(u64, AudioMessage)> {
    engine
        .audio_log()
        .iter()
        .filter(|entry| {
            matches!(
                entry.message,
                AudioMessage::MusicPlay { .. }
                    | AudioMessage::MusicStop { .. }
                    | AudioMessage::MusicSet { .. }
                    | AudioMessage::MusicStemGain { .. }
            )
        })
        .map(|entry| (entry.frame.as_u64(), entry.message.clone()))
        .collect()
}

/// `music_play` logs a looping `MusicPlay` with its fields, stamped with the frame it was pushed on.
#[test]
fn music_play_logs_a_looping_play_with_its_fields() {
    let mut engine = engine();
    engine.advance_live();
    queue(&mut engine).music_play("music/theme.ogg", 0.8, 60);
    engine.advance_live();
    assert_eq!(
        music_messages(&engine),
        vec![(
            1,
            AudioMessage::MusicPlay {
                track: "music/theme.ogg".to_string(),
                gain: 0.8,
                looping: true,
                fade_frames: 60,
            }
        )]
    );
}

#[test]
fn music_play_once_is_not_looping() {
    let mut engine = engine();
    queue(&mut engine).music_play_once("music/sting.ogg", 1.0, 0);
    engine.advance_live();
    assert!(matches!(
        music_messages(&engine).as_slice(),
        [(
            0,
            AudioMessage::MusicPlay {
                looping: false,
                fade_frames: 0,
                ..
            }
        )]
    ));
}

#[test]
fn music_stop_logs_its_fade() {
    let mut engine = engine();
    queue(&mut engine).music_stop(30);
    engine.advance_live();
    assert_eq!(
        music_messages(&engine),
        vec![(0, AudioMessage::MusicStop { fade_frames: 30 })]
    );
}

/// The log entry names the track and carries its gain, which is what the testkit's `audio_stream`
/// flattens; a stop names nothing.
#[test]
fn a_music_play_names_its_track_in_the_stream() {
    let mut engine = engine();
    queue(&mut engine).music_play("music/theme.ogg", 0.5, 0);
    queue(&mut engine).music_stop(0);
    engine.advance_live();
    let log = engine.audio_log();
    assert_eq!(
        (log[0].sound(), log[0].gain()),
        (Some("music/theme.ogg"), Some(0.5))
    );
    assert_eq!((log[1].sound(), log[1].gain()), (None, None));
}

/// Music never perturbs the `WorldHash`.
#[test]
fn music_never_affects_the_world_hash() {
    fn run(push_audio: bool) -> Vec<u64> {
        let mut engine = engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            if push_audio {
                queue(&mut engine).music_play("m.ogg", 1.0, 10);
                queue(&mut engine).music_stop(5);
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(run(true), run(false));
}

/// A game system that resends the set every tick and raises the perc stem at frame 3.
fn adaptive_music(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let queue = world.resource_mut::<AudioQueue>().expect("AudioQueue");
    queue.music_set(
        "dungeon",
        &[("music/base.ogg", 1.0), ("music/perc.ogg", 0.0)],
        60,
    );
    if ctx.frame.as_u64() >= 3 {
        queue.music_stem_gain("music/perc.ogg", 1.0, 30);
    }
}

/// `music_set` resent every tick, with a stem raised later, logs frame-stamped `MusicSet` and
/// `MusicStemGain` messages — the sink (not the log) makes the resend a no-op.
#[test]
fn an_adaptive_music_system_logs_the_set_and_the_stem_moves_by_frame() {
    let mut engine = Engine::builder()
        .system("adaptive_music", adaptive_music)
        .build()
        .expect("engine builds");
    engine.load_inline_scene(SCENE).expect("scene loads");
    for _ in 0..5 {
        engine.advance_live();
    }
    let set = |frame| {
        (
            frame,
            AudioMessage::MusicSet {
                id: "dungeon".to_string(),
                stems: vec![
                    ("music/base.ogg".to_string(), 1.0),
                    ("music/perc.ogg".to_string(), 0.0),
                ],
                fade_frames: 60,
            },
        )
    };
    let raise = |frame| {
        (
            frame,
            AudioMessage::MusicStemGain {
                stem: "music/perc.ogg".to_string(),
                gain: 1.0,
                fade_frames: 30,
            },
        )
    };
    assert_eq!(
        music_messages(&engine),
        vec![set(0), set(1), set(2), set(3), raise(3), set(4), raise(4)]
    );
    let log = engine.audio_log();
    assert_eq!((log[0].sound(), log[0].gain()), (Some("dungeon"), None));
    assert_eq!(
        (log[4].sound(), log[4].gain()),
        (Some("music/perc.ogg"), Some(1.0))
    );
}

/// The set verbs never perturb the `WorldHash` either.
#[test]
fn music_sets_never_affect_the_world_hash() {
    fn run(push_audio: bool) -> Vec<u64> {
        let mut engine = engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            if push_audio {
                queue(&mut engine).music_set("s", &[("a.ogg", 1.0), ("b.ogg", 0.0)], 10);
                queue(&mut engine).music_stem_gain("b.ogg", 1.0, 5);
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(run(true), run(false));
}

// --- Buses: runtime gain and pause ---

/// A game system that sets the music slider, ducks sfx and pauses the world's buses on fixed frames.
fn mix_script(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let queue = world.resource_mut::<AudioQueue>().expect("AudioQueue");
    match ctx.frame.as_u64() {
        1 => queue.bus_gain(Bus::Music, MixLayer::User, 0.4, 0),
        2 => queue.bus_gain(Bus::Sfx, MixLayer::Game, 0.25, 12),
        3 => queue.bus_pause(&[Bus::Sfx, Bus::Ambient], 6),
        4 => queue.bus_resume(&[Bus::Sfx, Bus::Ambient], 6),
        _ => {}
    }
}

fn bus_messages(engine: &Engine) -> Vec<(u64, AudioMessage)> {
    engine
        .audio_log()
        .iter()
        .filter(|entry| {
            matches!(
                entry.message,
                AudioMessage::BusGain { .. } | AudioMessage::BusPause { .. }
            )
        })
        .map(|entry| (entry.frame.as_u64(), entry.message.clone()))
        .collect()
}

fn mixed_engine() -> Engine {
    let mut engine = Engine::builder()
        .system("mix_script", mix_script)
        .build()
        .expect("engine builds");
    engine.load_inline_scene(SCENE).expect("scene loads");
    for _ in 0..6 {
        engine.advance_live();
    }
    engine
}

/// The bus verbs drain in push order, stamped with the frame that emitted them.
#[test]
fn bus_messages_drain_in_push_order_with_the_emitting_frame() {
    let engine = mixed_engine();
    assert_eq!(
        bus_messages(&engine),
        vec![
            (
                1,
                AudioMessage::BusGain {
                    bus: Bus::Music,
                    layer: MixLayer::User,
                    gain: 0.4,
                    fade_frames: 0,
                }
            ),
            (
                2,
                AudioMessage::BusGain {
                    bus: Bus::Sfx,
                    layer: MixLayer::Game,
                    gain: 0.25,
                    fade_frames: 12,
                }
            ),
            (
                3,
                AudioMessage::BusPause {
                    buses: vec![Bus::Sfx, Bus::Ambient],
                    paused: true,
                    fade_frames: 6,
                }
            ),
            (
                4,
                AudioMessage::BusPause {
                    buses: vec![Bus::Sfx, Bus::Ambient],
                    paused: false,
                    fade_frames: 6,
                }
            ),
        ]
    );
    let log = engine.audio_log();
    let gains: Vec<_> = log.iter().map(|entry| entry.gain()).collect();
    assert_eq!(gains, [Some(0.4), Some(0.25), None, None]);
    assert!(log.iter().all(|entry| entry.sound().is_none()));
}

/// A scripted mix regenerates an identical log.
#[test]
fn a_bus_script_regenerates_an_identical_audio_log() {
    assert_eq!(mixed_engine().audio_log(), mixed_engine().audio_log());
}

/// Bus gain and pause never perturb the `WorldHash`.
#[test]
fn bus_calls_never_affect_the_world_hash() {
    fn run(push_audio: bool) -> Vec<u64> {
        let mut engine = engine();
        let mut hashes = vec![engine.world_hash().unwrap().as_u64()];
        for _ in 0..5 {
            if push_audio {
                queue(&mut engine).bus_gain(Bus::Master, MixLayer::User, 0.5, 3);
                queue(&mut engine).bus_pause(&[Bus::Music], 2);
            }
            engine.advance_live();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        hashes
    }
    assert_eq!(run(true), run(false));
}

// --- Stereo pan and spatial distance ---

/// The bell scene on an engine whose game authored `pan` and `distance`.
fn placed_engine(pan: AudioPan, distance: AudioDistance) -> Engine {
    let mut engine = Engine::builder()
        .audio_pan(pan)
        .audio_distance(distance)
        .build()
        .expect("engine builds");
    engine.load_inline_scene(BELL_SCENE).expect("scene loads");
    engine
}

fn set_camera_yaw(engine: &mut Engine, degrees: f32) {
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(EntityId::new(0, 0))
        .expect("the camera is alive")
        .expect("the camera has a Transform")
        .rotation = [0.0, degrees, 0.0];
}

/// The pan of every bell sync in the log, in emission order.
fn bell_pans(engine: &Engine) -> Vec<f32> {
    loop_messages(engine, BELL)
        .iter()
        .filter_map(|message| match message {
            AudioMessage::LoopSync { pan, .. } => Some(*pan),
            _ => None,
        })
        .collect()
}

/// The pan of every one-shot in the log, in emission order.
fn shot_pans(engine: &Engine) -> Vec<f32> {
    engine
        .audio_log()
        .iter()
        .filter_map(|entry| match &entry.message {
            AudioMessage::OneShot(event) => Some(event.pan),
            _ => None,
        })
        .collect()
}

/// Under `Stereo` a bell to the listener's right pans right, one to the left pans left, and a half
/// turn of the camera flips the sign.
#[test]
fn a_stereo_emitter_pans_to_its_side_of_the_camera() {
    let mut engine = placed_engine(AudioPan::Stereo, AudioDistance::Planar);
    engine.advance_live();
    assert_eq!(bell_pans(&engine), vec![1.0]);
    move_bell_to(&mut engine, -4.0);
    engine.advance_live();
    assert_eq!(bell_pans(&engine), vec![1.0, -1.0]);

    let mut turned = placed_engine(AudioPan::Stereo, AudioDistance::Planar);
    set_camera_yaw(&mut turned, 180.0);
    turned.advance_live();
    let pans = bell_pans(&turned);
    assert!(pans.len() == 1 && (pans[0] + 1.0).abs() < 1e-5, "{pans:?}");
}

/// Dead ahead, behind and straight overhead are exactly centre, whatever the distance.
#[test]
fn ahead_behind_and_overhead_are_exactly_centre() {
    let mut engine = placed_engine(AudioPan::Stereo, AudioDistance::Planar);
    for at in [[0.0, 0.0, -5.0], [0.0, 0.0, 5.0], [0.0, 7.0, 0.0]] {
        queue(&mut engine).play_at("hit.wav", 1.0, at, 100.0);
    }
    engine.advance_live();
    assert_eq!(shot_pans(&engine), vec![0.0; 3]);
}

/// `Spatial` counts height; `Planar` (the default, kept by `play_at_ignores_height`) does not.
#[test]
fn spatial_distance_attenuates_a_sound_overhead() {
    let gain_overhead = |distance| {
        let mut engine = placed_engine(AudioPan::Centred, distance);
        queue(&mut engine).play_at("up.wav", 1.0, [0.0, 7.0, 0.0], 10.0);
        engine.advance_live();
        one_shots(&engine)[0].2
    };
    assert_eq!(gain_overhead(AudioDistance::Planar), 1.0);
    assert!((gain_overhead(AudioDistance::Spatial) - 0.3).abs() < 1e-6);
}

/// A camera yawing every tick under a static bell: `Centred` logs exactly one sync (pan `0.0`, no
/// pan resync at all), `Stereo` follows the sweep with more.
#[test]
fn a_centred_game_never_resyncs_for_pan_and_a_stereo_one_does() {
    let drive = |pan| {
        let mut engine = placed_engine(pan, AudioDistance::Planar);
        for tick in 0..60 {
            set_camera_yaw(&mut engine, tick as f32 * 5.0);
            engine.advance_live();
        }
        bell_pans(&engine)
    };
    assert_eq!(drive(AudioPan::Centred), vec![0.0]);
    assert!(drive(AudioPan::Stereo).len() > 1);
}

/// A pan move inside the resync step says nothing new.
#[test]
fn a_small_yaw_inside_the_pan_step_sends_no_extra_sync() {
    let mut engine = placed_engine(AudioPan::Stereo, AudioDistance::Planar);
    engine.advance_live();
    set_camera_yaw(&mut engine, 1.0); // pan 1.0 -> cos(1 deg), 1.5e-4 away
    engine.advance_live();
    assert_eq!(bell_pans(&engine).len(), 1);
}

/// `play_at` logs a centred pan by default and its side under `Stereo`.
#[test]
fn play_at_carries_the_pan_only_under_stereo() {
    for (pan, right, left) in [(AudioPan::Centred, 0.0, 0.0), (AudioPan::Stereo, 1.0, -1.0)] {
        let mut engine = placed_engine(pan, AudioDistance::Planar);
        queue(&mut engine).play_at("r.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
        queue(&mut engine).play_at("l.wav", 1.0, [-4.0, 0.0, 0.0], 10.0);
        engine.advance_live();
        assert_eq!(shot_pans(&engine), vec![right, left], "{pan:?}");
    }
}

/// A world rebuild keeps both authored settings, and the rebuilt emitter pans again.
#[test]
fn a_world_rebuild_keeps_the_audio_placement() {
    let mut engine = placed_engine(AudioPan::Stereo, AudioDistance::Spatial);
    engine.advance_live();
    engine.load_inline_scene(BELL_SCENE).expect("scene loads");
    engine.advance_live();
    assert_eq!(engine.audio_pan(), AudioPan::Stereo);
    assert_eq!(engine.audio_distance(), AudioDistance::Spatial);
    assert_eq!(bell_pans(&engine), vec![1.0]);
}

// --- The listener component: `AudioListener` ---

/// A camera at the origin (yawed `camera_yaw` degrees) and the bell 4 units east of it, then
/// `extra` entities appended — entity ids are document order: camera `e0v0`, bell `e1v0`.
fn listener_scene(camera_yaw: f32, extra: &str) -> String {
    let bell = BELL_SCENE.replace(
        "      Transform: {}\n      Camera: {}\n",
        &format!(
            "      Transform:\n        rotation:\n          - 0.0\n          - {camera_yaw}\n          - 0.0\n      Camera: {{}}\n"
        ),
    );
    format!("{bell}{extra}")
}

fn listener_engine(pan: AudioPan, scene: &str) -> Engine {
    let mut engine = Engine::builder()
        .audio_pan(pan)
        .build()
        .expect("engine builds");
    engine.load_inline_scene(scene).expect("scene loads");
    engine
}

fn bell_gains(engine: &Engine) -> Vec<f32> {
    loop_messages(engine, BELL)
        .iter()
        .filter_map(|message| match message {
            AudioMessage::LoopSync { gain, .. } => Some(*gain),
            _ => None,
        })
        .collect()
}

const EAR_AT_3: &str = "  - name: ear
    components:
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      AudioListener: {}
";

fn near(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() < 1e-5
}

/// A listener far from the camera: the bell is 1 unit from the ear (gain `0.9`), not 4 from the
/// camera (`0.6`).
#[test]
fn an_emitter_is_measured_from_the_listener_not_the_camera() {
    let mut engine = listener_engine(AudioPan::Centred, &listener_scene(0.0, EAR_AT_3));
    engine.advance_live();
    let gains = bell_gains(&engine);
    assert!(gains.len() == 1 && near(gains[0], 0.9), "{gains:?}");
}

/// A listener parented to a mover rides it: moving the parent changes the measured distance.
#[test]
fn a_listener_under_a_moving_parent_follows_it() {
    let ear = "  - name: mover
    components:
      Transform: {}
  - name: ear
    components:
      Parent:
        index: 2
        generation: 0
      Transform:
        translation:
          - 3.0
          - 0.0
          - 0.0
      AudioListener: {}
";
    let mut engine = listener_engine(AudioPan::Centred, &listener_scene(0.0, ear));
    engine.advance_live();
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(EntityId::new(2, 0))
        .expect("the mover is alive")
        .expect("the mover has a Transform")
        .translation = [4.0, 0.0, 0.0]; // the ear is now at x = 7, three from the bell
    engine.advance_live();
    let gains = bell_gains(&engine);
    assert!(
        gains.len() == 2 && near(gains[0], 0.9) && near(gains[1], 0.7),
        "{gains:?}"
    );
}

/// Under `orientation: camera` the pan follows the camera's right vector (a camera yawed 90 degrees
/// has the bell dead ahead: centre); under `self` it follows the listener's own (right: full right).
#[test]
fn listener_orientation_picks_whose_axes_pan() {
    let ear = |orientation: &str| {
        format!(
            "  - name: ear
    components:
      Transform: {{}}
      AudioListener:
        orientation: {orientation}
"
        )
    };
    let mut by_camera = listener_engine(AudioPan::Stereo, &listener_scene(90.0, &ear("camera")));
    by_camera.advance_live();
    let pans = bell_pans(&by_camera);
    assert!(pans.len() == 1 && pans[0].abs() < 1e-5, "{pans:?}");

    let mut by_self = listener_engine(AudioPan::Stereo, &listener_scene(90.0, &ear("self")));
    by_self.advance_live();
    assert_eq!(bell_pans(&by_self), vec![1.0]);
}

/// A listener with no camera in the world: the emitter syncs (a camera-less world otherwise syncs
/// nothing), at its distance from the listener.
#[test]
fn a_listener_alone_listens_without_a_camera() {
    let scene = "format: 1
entities:
  - name: ear
    components:
      Transform: {}
      AudioListener: {}
  - name: bell
    components:
      Transform:
        translation:
          - 4.0
          - 0.0
          - 0.0
      AudioEmitter:
        sound: sounds/bell.wav
        range: 10.0
";
    let mut engine = listener_engine(AudioPan::Centred, scene);
    engine.advance_live();
    let gains = bell_gains(&engine);
    assert!(gains.len() == 1 && near(gains[0], 0.6), "{gains:?}");
}

/// A positional one-shot resolves against the listener.
#[test]
fn play_at_resolves_against_the_listener() {
    let mut engine = listener_engine(AudioPan::Centred, &listener_scene(0.0, EAR_AT_3));
    queue(&mut engine).play_at("hit.wav", 1.0, [4.0, 0.0, 0.0], 10.0);
    engine.advance_live();
    let shots = one_shots(&engine);
    assert!(shots.len() == 1 && near(shots[0].2, 0.9), "{shots:?}");
}

/// Two listeners: the lowest id is heard from, and `L1650` is reported once however many ticks run.
#[test]
fn two_listeners_hear_from_the_lowest_id_and_report_l1650_once() {
    let second = "  - name: ear-two
    components:
      Transform:
        translation:
          - -6.0
          - 0.0
          - 0.0
      AudioListener: {}
";
    let scene = listener_scene(0.0, &format!("{EAR_AT_3}{second}"));
    let mut engine = listener_engine(AudioPan::Centred, &scene);
    for _ in 0..4 {
        engine.advance_live();
    }
    let gains = bell_gains(&engine);
    assert!(gains.len() == 1 && near(gains[0], 0.9), "{gains:?}");
    let diagnostics = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .expect("Diagnostics installed");
    assert_eq!(
        diagnostics
            .reported(ludion_core::codes::code::AUDIO_LISTENER_DUPLICATE)
            .count(),
        1
    );
}
