//! Sequence audio (`ludion docs sequences`): `play` keys are voices stopped on skip and cancel, a
//! `duck` key is released at every end, a skip applies the last pending `music_crossfade`.

use ludion_core::audio::{AudioMessage, Bus};
use ludion_core::ids::EntityId;
use ludion_engine::sequences::{self, SequenceEvent, SequenceHandle, SequenceTrack};
use ludion_engine::{Engine, SequenceEvents, Transform, sequence_update};
use std::path::PathBuf;

const AUDIO_STEP: u32 = 6;

fn wav(frames: usize) -> Vec<u8> {
    let data: Vec<u8> = (0..frames).flat_map(|_| 1000i16.to_le_bytes()).collect();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&44100u32.to_le_bytes());
    wav.extend_from_slice(&88200u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    wav
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-seq-audio-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("long.wav"), wav(176_400)).unwrap();
    dir
}

const KEY: &str = "sequences/a.yaml";

fn doc(length: u32, tracks: &str) -> String {
    format!(
        "format: 1\nkind: sequence\nroles:\n  - actor\nlength_ticks: {length}\ntracks:\n  audio:\n{tracks}"
    )
}

fn engine(name: &str, text: &str) -> (Engine, EntityId) {
    let mut engine = Engine::builder()
        .project_root(fresh_dir(name))
        .sequences()
        .system("sequence_update", sequence_update)
        .doc_text(KEY, text)
        .build()
        .expect("the docs are valid");
    engine
        .load_inline_scene(
            "format: 1\nseed: 1\nentities:\n  - name: camera\n    components:\n      Transform: {}\n      Camera: {}\n",
        )
        .unwrap();
    let actor = engine.world_mut().spawn();
    engine
        .world_mut()
        .insert(actor, Transform::default())
        .unwrap();
    (engine, actor)
}

fn play(engine: &mut Engine, actor: EntityId) -> SequenceHandle {
    sequences::play(engine.world_mut(), KEY, &[("actor", actor)]).unwrap()
}

fn run(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

/// `(frame, id)` of every `VoiceStart`.
fn starts(engine: &Engine) -> Vec<(u64, u64)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|e| match &e.message {
            AudioMessage::VoiceStart { id, .. } => Some((e.frame.as_u64(), id.0)),
            _ => None,
        })
        .collect()
}

/// `(id, fade_frames)` of every `VoiceStop`, in order.
fn stops(engine: &Engine) -> Vec<(u64, u32)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|e| match &e.message {
            AudioMessage::VoiceStop { id, fade_frames } => Some((id.0, *fade_frames)),
            _ => None,
        })
        .collect()
}

/// `(frame, bus, gain, fade)` of every `Duck`.
fn ducks(engine: &Engine) -> Vec<(u64, Bus, f32, u32)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|e| match &e.message {
            AudioMessage::Duck {
                bus,
                gain,
                fade_frames,
            } => Some((e.frame.as_u64(), *bus, *gain, *fade_frames)),
            _ => None,
        })
        .collect()
}

fn crossfades(engine: &Engine) -> Vec<String> {
    engine
        .audio_log()
        .iter()
        .filter_map(|e| match &e.message {
            AudioMessage::MusicCrossfade { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

const VOICES: &str =
    "    - at: 2\n      play: long.wav\n    - at: 4\n      play: long.wav\n      role: actor\n";

#[test]
fn a_play_key_is_a_voice_and_a_role_attaches_it_to_the_entity() {
    let (mut engine, actor) = engine("voices", &doc(40, VOICES));
    play(&mut engine, actor);
    run(&mut engine, 6);
    let started = starts(&engine);
    assert_eq!(started.len(), 2, "{started:?}");
    assert_eq!(started[1].0 - started[0].0, 2, "the keys are 2 ticks apart");
    let attached = started[1].1;
    engine
        .world_mut()
        .get_mut::<Transform>(actor)
        .unwrap()
        .unwrap()
        .translation = [8.0, 0.0, 0.0];
    run(&mut engine, 2);
    let synced = engine
        .audio_log()
        .iter()
        .any(|e| matches!(&e.message, AudioMessage::VoiceSync { id, .. } if id.0 == attached));
    assert!(synced, "the role's voice follows its entity");
}

#[test]
fn skip_and_cancel_stop_every_started_voice_in_order_and_completion_does_not() {
    for end in ["skip", "cancel", "complete"] {
        let (mut engine, actor) = engine(end, &doc(12, VOICES));
        let handle = play(&mut engine, actor);
        run(&mut engine, 6);
        let started = starts(&engine);
        assert_eq!(started.len(), 2);
        match end {
            "skip" => assert!(sequences::skip(engine.world_mut(), handle)),
            "cancel" => assert!(sequences::cancel(engine.world_mut(), handle)),
            _ => {}
        }
        run(&mut engine, 12);
        let stopped = stops(&engine);
        if end == "complete" {
            assert!(stopped.is_empty(), "a completed beat lets voices play out");
        } else {
            let expected: Vec<(u64, u32)> =
                started.iter().map(|(_, id)| (*id, AUDIO_STEP)).collect();
            assert_eq!(stopped, expected, "{end}");
        }
    }
}

fn duck_doc(tracks: &str) -> String {
    doc(10, tracks)
}

const DUCK: &str = "    - at: 3\n      duck:\n        buses:\n          - music\n          - ambient\n        depth_db: -12.0\n        attack: 2\n        release: 9\n";

#[test]
fn a_duck_key_ducks_at_its_tick_and_releases_at_every_end() {
    for end in ["complete", "skip", "cancel"] {
        let (mut engine, actor) = engine(&format!("duck-{end}"), &duck_doc(DUCK));
        let handle = play(&mut engine, actor);
        run(&mut engine, 5);
        let down = ducks(&engine);
        assert_eq!(down.len(), 2, "{end}: {down:?}");
        assert!(down.iter().all(|d| d.2 < 1.0 && d.3 == 2));
        match end {
            "skip" => assert!(sequences::skip(engine.world_mut(), handle)),
            "cancel" => assert!(sequences::cancel(engine.world_mut(), handle)),
            _ => {}
        }
        run(&mut engine, 10);
        let all = ducks(&engine);
        let released: Vec<_> = all.iter().filter(|d| d.2 == 1.0).collect();
        assert_eq!(released.len(), 2, "{end}: {all:?}");
        assert!(released.iter().all(|d| d.3 == 9), "{end}: {all:?}");
    }
}

#[test]
fn a_second_duck_key_replaces_the_first() {
    let two = format!(
        "{DUCK}    - at: 5\n      duck:\n        buses:\n          - music\n        depth_db: -6.0\n        release: 4\n"
    );
    let (mut engine, actor) = engine("duck-two", &duck_doc(&two));
    play(&mut engine, actor);
    run(&mut engine, 14);
    let all = ducks(&engine);
    let end = all.last().unwrap().0;
    let released: Vec<_> = all.iter().filter(|d| d.0 == end).collect();
    assert_eq!(released.len(), 1, "one live duck, one release: {all:?}");
    assert_eq!(
        (released[0].1, released[0].2, released[0].3),
        (Bus::Music, 1.0, 4)
    );
    assert!(
        all.iter().filter(|d| d.0 < end).all(|d| d.0 <= 5),
        "nothing between the replacement and the end: {all:?}"
    );
}

const FADES: &str = "    - at: 20\n      music_crossfade:\n        id: first\n        stems:\n          - - a.ogg\n            - 1.0\n    - at: 30\n      music_crossfade:\n        id: second\n        stems:\n          - - b.ogg\n            - 0.5\n        ticks: 12\n";

#[test]
fn a_skip_applies_only_the_last_pending_crossfade() {
    let (mut engine, actor) = engine("fades", &doc(40, FADES));
    let handle = play(&mut engine, actor);
    run(&mut engine, 3);
    assert!(crossfades(&engine).is_empty());
    assert!(sequences::skip(engine.world_mut(), handle));
    run(&mut engine, 1);
    assert_eq!(crossfades(&engine), ["second"]);
    let events: Vec<SequenceEvent> = engine
        .world()
        .resource::<SequenceEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    let applied: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            SequenceEvent::KeyApplied {
                track: SequenceTrack::Audio,
                at,
                ..
            } => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(applied, [30]);
}

#[test]
fn a_despawned_player_leaves_no_voices_behind() {
    let (mut engine, actor) = engine("despawn", &doc(40, "    - at: 2\n      play: long.wav\n"));
    let first = play(&mut engine, actor);
    run(&mut engine, 4);
    engine.world_mut().despawn(first.0).unwrap();
    run(&mut engine, 1);
    let second = play(&mut engine, actor);
    run(&mut engine, 4);
    assert_eq!(starts(&engine).len(), 2);
    assert!(sequences::skip(engine.world_mut(), second));
    run(&mut engine, 2);
    let started = starts(&engine);
    assert_eq!(stops(&engine), [(started[1].1, AUDIO_STEP)]);
}

#[test]
fn audio_keys_do_not_move_the_world_hash() {
    let with = format!(
        "    - at: 2\n      play: long.wav\n{DUCK}    - at: 4\n      play: long.wav\n      role: actor\n{FADES}"
    );
    let hashes = |tracks: &str, name: &str| {
        let (mut engine, actor) = engine(name, &doc(40, tracks));
        play(&mut engine, actor);
        (0..45)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    let loud = hashes(&with, "hash-with");
    let quiet = hashes("    []\n", "hash-without");
    assert_eq!(loud, quiet);
}
