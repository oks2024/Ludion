//! `audio_mix_at` and `mix_state`: the offline mix probe, over hand-built message lists, and over
//! the gallery's `audio-zones` scene walked from the yard through the dock's doorway.

use std::path::PathBuf;

use ludion_core::audio::{
    AudioCategory, AudioMessage, Bus, LoopId, MixLayer, ReverbParams, VoiceId, VoiceSpatial,
};
use ludion_core::ids::EntityId;
use ludion_engine::{Engine, ReverbPreset, Transform};
use ludion_testkit::audio::{audio_mix_at, mix_state};

fn zone_gain(bus: Bus, gain: f32) -> AudioMessage {
    AudioMessage::ZoneGain {
        bus,
        gain,
        fade_frames: 0,
    }
}

fn loop_sync(id: LoopId, gain: f32) -> AudioMessage {
    AudioMessage::LoopSync {
        id,
        sound: "sounds/bed.wav".to_string(),
        gain,
        pan: 0.0,
        category: AudioCategory::Ambient,
        spatial: VoiceSpatial::default(),
    }
}

const HALL: ReverbParams = ReverbParams {
    decay_s: 1.8,
    pre_delay_ms: 20.0,
    damping: 0.4,
    size: 0.6,
    wet: 0.25,
};

#[test]
fn layers_fold_in_order_and_stop_at_the_asked_frame() {
    let messages = vec![
        (
            1,
            AudioMessage::BusGain {
                bus: Bus::Music,
                layer: MixLayer::User,
                gain: 0.5,
                fade_frames: 0,
            },
        ),
        (
            2,
            AudioMessage::BusGain {
                bus: Bus::Sfx,
                layer: MixLayer::Game,
                gain: 0.25,
                fade_frames: 0,
            },
        ),
        (
            3,
            AudioMessage::Duck {
                bus: Bus::Music,
                gain: 0.4,
                fade_frames: 0,
            },
        ),
        (4, zone_gain(Bus::Ambient, 0.9)),
        (
            5,
            AudioMessage::BusPause {
                buses: vec![Bus::Ui],
                paused: true,
                fade_frames: 0,
            },
        ),
    ];
    let early = mix_state(&messages, 2);
    assert_eq!(early.bus(Bus::Music).user, 0.5);
    assert_eq!(early.bus(Bus::Sfx).game, 0.25);
    assert_eq!(early.bus(Bus::Music).duck, 1.0);
    assert_eq!(early.bus(Bus::Ambient).zone, 1.0);
    let late = mix_state(&messages, 99);
    assert_eq!(late.bus(Bus::Music).duck, 0.4);
    assert_eq!(late.bus(Bus::Ambient).zone, 0.9);
    assert!(late.bus(Bus::Ui).paused);
    assert!(!late.bus(Bus::Sfx).paused);
}

#[test]
fn the_music_verbs_leave_the_mix_untouched() {
    let bed = LoopId::ZoneBed(EntityId::new(4, 0), 1);
    let base = vec![(0, zone_gain(Bus::Music, 0.3)), (0, loop_sync(bed, 0.5))];
    let mut with_music = base.clone();
    with_music.push((
        1,
        AudioMessage::MusicSet {
            id: "a".to_string(),
            stems: vec![("a.ogg".to_string(), 1.0)],
            fade_frames: 0,
        },
    ));
    with_music.push((
        1,
        AudioMessage::MusicCrossfade {
            id: "b".to_string(),
            stems: vec![("b.ogg".to_string(), 1.0)],
            fade_frames: 30,
        },
    ));
    assert_eq!(mix_state(&with_music, 1), mix_state(&base, 1));
}

#[test]
fn a_stop_clears_everything_but_the_users_layer() {
    let bed = LoopId::ZoneBed(EntityId::new(4, 0), 1);
    let messages = vec![
        (
            0,
            AudioMessage::BusGain {
                bus: Bus::Music,
                layer: MixLayer::User,
                gain: 0.5,
                fade_frames: 0,
            },
        ),
        (
            0,
            AudioMessage::BusGain {
                bus: Bus::Music,
                layer: MixLayer::Game,
                gain: 0.5,
                fade_frames: 0,
            },
        ),
        (0, zone_gain(Bus::Music, 0.3)),
        (
            0,
            AudioMessage::ReverbSet {
                params: HALL,
                fade_frames: 0,
            },
        ),
        (0, loop_sync(bed.clone(), 0.5)),
        (1, AudioMessage::StopAllLoops),
    ];
    let before = mix_state(&messages, 0);
    assert!(before.bed(EntityId::new(4, 0), 1).is_some());
    assert_eq!(before.reverb, HALL);
    let after = mix_state(&messages, 1);
    let music = after.bus(Bus::Music);
    assert_eq!(
        (music.user, music.game, music.zone, music.duck),
        (0.5, 1.0, 1.0, 1.0)
    );
    assert_eq!(after.reverb, ReverbParams::DRY);
    assert!(after.loops.is_empty() && after.voices.is_empty());
}

#[test]
fn beds_and_voices_follow_their_syncs_and_stops() {
    let zone = EntityId::new(2, 0);
    let bed = LoopId::ZoneBed(zone, 0);
    let voice = VoiceId(7);
    let messages = vec![
        (1, loop_sync(bed.clone(), 0.2)),
        (2, loop_sync(bed.clone(), 0.4)),
        (
            2,
            AudioMessage::VoiceStart {
                id: voice,
                sound: "sounds/v.wav".to_string(),
                gain: 0.8,
                pan: -0.2,
                category: AudioCategory::Voice,
                spatial: VoiceSpatial::default(),
                offset_ticks: 0,
            },
        ),
        (
            3,
            AudioMessage::VoiceSync {
                id: voice,
                gain: 0.6,
                pan: 0.1,
                spatial: VoiceSpatial::default(),
            },
        ),
        (4, AudioMessage::LoopStop { id: bed }),
        (
            5,
            AudioMessage::VoiceStop {
                id: voice,
                fade_frames: 0,
            },
        ),
    ];
    assert_eq!(mix_state(&messages, 1).bed(zone, 0).unwrap().gain, 0.2);
    let mid = mix_state(&messages, 3);
    assert_eq!(mid.bed(zone, 0).unwrap().gain, 0.4);
    assert_eq!(mid.voices[&voice].gain, 0.6);
    assert_eq!(mid.voices[&voice].pan, 0.1);
    let end = mix_state(&messages, 5);
    assert!(end.bed(zone, 0).is_none() && end.voices.is_empty());
}

#[test]
fn the_reverb_is_the_last_one_set() {
    let messages = vec![
        (
            1,
            AudioMessage::ReverbSet {
                params: HALL,
                fade_frames: 0,
            },
        ),
        (
            3,
            AudioMessage::ReverbSet {
                params: ReverbParams::DRY,
                fade_frames: 0,
            },
        ),
    ];
    assert_eq!(mix_state(&messages, 2).reverb, HALL);
    assert_eq!(mix_state(&messages, 3).reverb, ReverbParams::DRY);
}

// Entity ids in `examples/gallery/audio-zones.yaml`, by document order.
const LISTENER: EntityId = EntityId::new(11, 0);
const YARD: EntityId = EntityId::new(13, 0);
const DOCK: EntityId = EntityId::new(14, 0);

fn last_frame(engine: &Engine) -> u64 {
    engine
        .audio_log()
        .iter()
        .map(|entry| entry.frame.as_u64())
        .max()
        .expect("the zones sent messages")
}

/// The listener walks from the yard, 0.25 m a tick, through the doorway to deep inside the dock.
#[test]
fn walking_the_gallery_scene_from_the_yard_into_the_dock() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/audio-zones.yaml");
    let text = std::fs::read_to_string(path).expect("the gallery scene reads");
    let mut engine = Engine::new();
    engine.load_inline_scene(&text).expect("scene loads");

    let walk_to = |engine: &mut Engine, x: f32| {
        engine
            .world_mut()
            .get_mut::<Transform>(LISTENER)
            .expect("the listener is alive")
            .expect("the listener has a Transform")
            .translation = [x, 1.0, 0.0];
        engine.advance_live();
    };

    walk_to(&mut engine, -2.0);
    walk_to(&mut engine, -2.0);
    let yard_frame = last_frame(&engine);
    let yard = audio_mix_at(&engine, yard_frame);
    assert!((yard.bed(YARD, 0).expect("rain plays").gain - 0.6).abs() < 1e-5);
    assert!(yard.bed(DOCK, 0).is_none());
    assert!(yard.reverb.near(&ReverbPreset::OutdoorOpen.params()));

    let mut x = -2.0;
    while x < 20.0 {
        x += 0.25;
        walk_to(&mut engine, x);
    }
    let last = audio_mix_at(&engine, last_frame(&engine));
    assert!(last.bed(YARD, 0).is_none(), "the rain has stopped");
    assert!(last.bed(DOCK, 0).expect("hum plays").gain >= 0.495);
    assert!(last.reverb.near(&ReverbPreset::HallMedium.params()));
    assert!((last.reverb.wet - 0.25).abs() < 1e-3);
    assert!((last.bus(Bus::Ambient).zone - 0.9).abs() <= 0.9 * 0.01);
}
