//! Geometry occlusion of positional audio (`EngineBuilder::audio_occlusion`): a wall between the
//! listener and a positional source quiets and muffles it, global sounds and the source's own
//! hierarchy never are, and with no builder call the stream is the one a wall-less world gives.

use ludion_core::audio::LOW_PASS_OPEN_HZ;
use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_engine::{
    AudioCategory, AudioEvent, AudioMessage, AudioOcclusion, AudioQueue, Engine, LoopId,
};
use ludion_physics::QueryFilter;

const CAMERA: EntityId = EntityId::new(0, 0);
const SOURCE: EntityId = EntityId::new(1, 0);

/// The source's authored gain at 8 m of a 20 m range.
const UNOCCLUDED: f32 = 0.6;

fn block3(indent: usize, v: [f32; 3]) -> String {
    v.iter()
        .map(|n| format!("{}- {n:?}\n", " ".repeat(indent)))
        .collect()
}

/// A static box collider entity.
fn wall(name: &str, centre: [f32; 3], half: [f32; 3]) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n",
        block3(10, centre),
        block3(14, half),
    )
}

/// The wall across the way from the origin to the source: x in [3.75, 4.25], far wider than the
/// rays' spread.
fn full_wall() -> String {
    wall("wall", [4.0, 0.0, 0.0], [0.25, 5.0, 10.0])
}

/// A camera at the origin, a bell emitter at `at`, then `rest`.
fn scene(at: [f32; 3], rest: &str) -> String {
    format!(
        "format: 1\nphysics:\n  space: free\nentities:\n  - name: camera\n    components:\n      Transform: {{}}\n      Camera: {{}}\n  - name: bell\n    components:\n      Transform:\n        translation:\n{}      AudioEmitter:\n        sound: sounds/bell.wav\n        range: 20.0\n{rest}",
        block3(10, at),
    )
}

/// A mono PCM16 WAV of `frames` frames at 44100 Hz.
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

/// A scratch project root unique to this process and `name`, holding `long.wav` (4 s).
fn root(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-occlusion-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("long.wav"), wav(176_400)).unwrap();
    dir
}

fn occ_engine(name: &str, occlusion: Option<AudioOcclusion>, scene: &str) -> Engine {
    let mut builder = Engine::builder().project_root(root(name));
    if let Some(occlusion) = occlusion {
        builder = builder.audio_occlusion(occlusion);
    }
    let mut engine = builder.build().expect("engine builds");
    engine.load_inline_scene(scene).expect("scene loads");
    engine
}

fn advance(engine: &mut Engine, ticks: u32) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn move_to(engine: &mut Engine, entity: EntityId, at: [f32; 3]) {
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(entity)
        .expect("the entity is alive")
        .expect("the entity has a Transform")
        .translation = at;
}

fn queue(engine: &mut Engine) -> &mut AudioQueue {
    engine
        .world_mut()
        .resource_mut::<AudioQueue>()
        .expect("AudioQueue installed")
}

/// Every `(gain, low_pass_hz)` the emitter `id` was synced with, in emission order.
fn syncs(engine: &Engine, id: EntityId) -> Vec<(f32, f32)> {
    engine
        .audio_log()
        .iter()
        .filter_map(|entry| match &entry.message {
            AudioMessage::LoopSync {
                id: LoopId::Entity(loop_id),
                gain,
                spatial,
                ..
            } if *loop_id == id => Some((*gain, spatial.filter.low_pass_hz)),
            _ => None,
        })
        .collect()
}

fn last_sync(engine: &Engine, id: EntityId) -> (f32, f32) {
    *syncs(engine, id).last().expect("the emitter synced")
}

fn muffled_gain() -> f32 {
    UNOCCLUDED * ludion_core::math::powf(10.0, AudioOcclusion::default().per_hit_db / 20.0)
}

/// No builder call: an emitter behind a wall syncs exactly as one with no wall.
#[test]
fn without_the_builder_call_a_wall_changes_nothing() {
    let walled = scene([8.0, 0.0, 0.0], &full_wall());
    let open = scene([8.0, 0.0, 0.0], "");
    let mut with_wall = occ_engine("off-wall", None, &walled);
    let mut without = occ_engine("off-open", None, &open);
    advance(&mut with_wall, 20);
    advance(&mut without, 20);
    assert_eq!(with_wall.audio_log(), without.audio_log());
    let (gain, low_pass) = last_sync(&with_wall, SOURCE);
    assert!((gain - UNOCCLUDED).abs() < 1e-5, "{gain}");
    assert_eq!(low_pass, LOW_PASS_OPEN_HZ);
    assert!(with_wall.audio_occlusion().is_none());
}

/// A wall across all three rays: the loss is one hit's, the cutoff the occluded one, and moving the
/// listener around the wall brings both back within the slew.
#[test]
fn a_wall_quiets_and_muffles_and_walking_clear_restores() {
    let config = AudioOcclusion::default();
    let mut engine = occ_engine(
        "wall",
        Some(config.clone()),
        &scene([8.0, 0.0, 0.0], &full_wall()),
    );
    advance(&mut engine, 24);
    let (gain, low_pass) = last_sync(&engine, SOURCE);
    assert!(
        gain <= muffled_gain() * 1.02,
        "{gain} vs {}",
        muffled_gain()
    );
    assert!(gain > muffled_gain() * 0.9, "{gain}");
    assert!(
        (low_pass - config.occluded_low_pass_hz).abs() <= config.occluded_low_pass_hz * 0.06,
        "{low_pass}"
    );

    // Past the wall, on the source's side: nothing between.
    move_to(&mut engine, CAMERA, [12.0, 0.0, 0.0]);
    advance(&mut engine, config.smoothing_ticks + config.interval_ticks);
    let (gain, low_pass) = last_sync(&engine, SOURCE);
    assert_eq!(low_pass, LOW_PASS_OPEN_HZ);
    assert!((gain - 0.8).abs() < 0.8 * 0.02, "{gain}");
}

/// A wall that covers the centre ray and one side but not the other: partial gain and cutoff.
#[test]
fn a_wall_over_two_of_three_rays_is_a_partial_muffle() {
    let config = AudioOcclusion::default();
    // Covers z up to 0.2: the +z side ray (z = 0.3 at the wall) passes.
    let partial = wall("wall", [4.0, 0.0, -4.9], [0.25, 5.0, 5.1]);
    let mut engine = occ_engine(
        "partial",
        Some(config.clone()),
        &scene([8.0, 0.0, 0.0], &partial),
    );
    advance(&mut engine, 24);
    let (gain, low_pass) = last_sync(&engine, SOURCE);
    assert!(low_pass < LOW_PASS_OPEN_HZ * 0.9, "{low_pass}");
    assert!(low_pass > config.occluded_low_pass_hz * 1.2, "{low_pass}");
    assert!(
        gain < UNOCCLUDED * 0.95 && gain > muffled_gain() * 1.05,
        "{gain}"
    );
}

/// 200 emitters under a 96-ray budget at 3 rays: never more than 32 sources cast a tick, every one
/// is muffled in the end, and two runs log the same stream.
#[test]
fn the_ray_budget_caps_casts_and_the_stream_is_deterministic() {
    let emitters: String = (0..200)
        .map(|i| {
            format!(
                "  - name: e{i}\n    components:\n      Transform:\n        translation:\n{}      AudioEmitter:\n        sound: sounds/bell.wav\n        range: 500.0\n",
                block3(10, [10.0 + i as f32 * 0.05, 0.0, 0.0])
            )
        })
        .collect();
    let scene = format!(
        "format: 1\nphysics:\n  space: free\nentities:\n  - name: camera\n    components:\n      Transform: {{}}\n      Camera: {{}}\n{emitters}{}",
        full_wall()
    );
    let run = |name: &str| {
        let mut engine = occ_engine(name, Some(AudioOcclusion::default()), &scene);
        advance(&mut engine, 40);
        engine
    };
    let first = run("budget-a");
    let second = run("budget-b");
    let peak = first.audio_occlusion_peak_rays();
    assert!(peak > 0 && peak <= 96, "{peak}");
    assert_eq!(first.audio_log(), second.audio_log());
    for i in 0..200 {
        let (_, low_pass) = last_sync(&first, EntityId::new(1 + i, 0));
        assert!(low_pass < LOW_PASS_OPEN_HZ, "emitter {i} never muffled");
    }
}

/// A positional one-shot and an attached voice behind the wall are muffled from their first message,
/// and the voice follows the occlusion once the listener walks clear.
#[test]
fn one_shots_and_voices_start_muffled_and_the_voice_follows() {
    let config = AudioOcclusion::default();
    let guard = "  - name: guard\n    components:\n      Transform:\n        translation:\n          - 8.0\n          - 0.0\n          - 0.0\n";
    let scene = scene([30.0, 0.0, 0.0], &format!("{}{guard}", full_wall()));
    let guard_id = EntityId::new(3, 0);
    let mut engine = occ_engine("shots", Some(config.clone()), &scene);
    advance(&mut engine, 3);

    queue(&mut engine).push_at(AudioEvent::new("hit.wav", 1.0), [8.0, 0.0, 0.0], 20.0);
    queue(&mut engine).play_on(guard_id, "long.wav", 1.0, 20.0, AudioCategory::Voice);
    advance(&mut engine, 1);
    let log = engine.audio_log();
    let shot = log
        .iter()
        .find_map(|entry| match &entry.message {
            AudioMessage::OneShot(event) if event.sound == "hit.wav" => Some(event.clone()),
            _ => None,
        })
        .expect("the one-shot played");
    assert!((shot.gain - muffled_gain()).abs() < 1e-3, "{}", shot.gain);
    assert!(
        (shot.spatial.filter.low_pass_hz - config.occluded_low_pass_hz).abs() < 1.0,
        "{}",
        shot.spatial.filter.low_pass_hz
    );
    let started = log
        .iter()
        .find_map(|entry| match &entry.message {
            AudioMessage::VoiceStart { gain, spatial, .. } => {
                Some((*gain, spatial.filter.low_pass_hz))
            }
            _ => None,
        })
        .expect("the voice started");
    assert!((started.0 - muffled_gain()).abs() < 1e-3, "{started:?}");
    assert!(
        (started.1 - config.occluded_low_pass_hz).abs() < 1.0,
        "{started:?}"
    );

    move_to(&mut engine, CAMERA, [12.0, 0.0, 0.0]);
    advance(&mut engine, config.smoothing_ticks + config.interval_ticks);
    let last = engine
        .audio_log()
        .iter()
        .rev()
        .find_map(|entry| match &entry.message {
            AudioMessage::VoiceSync { spatial, .. } => Some(spatial.filter.low_pass_hz),
            _ => None,
        })
        .expect("the voice resynced");
    assert_eq!(last, LOW_PASS_OPEN_HZ);
}

/// The source's own collider and the listener's parent body never occlude.
#[test]
fn a_sources_own_collider_and_the_listeners_parent_never_occlude() {
    let config = AudioOcclusion::default();
    // The emitter sits inside its own box; the ray reaches the box's near face first.
    let own = scene_with_own_box();
    let mut engine = occ_engine("own", Some(config.clone()), &own);
    advance(&mut engine, 12);
    let (gain, low_pass) = last_sync(&engine, SOURCE);
    assert_eq!(low_pass, LOW_PASS_OPEN_HZ);
    assert!((gain - UNOCCLUDED).abs() < 1e-5, "{gain}");

    // The camera is a child of a boxed player at the origin, 1.5 m behind it: the ray crosses the
    // player's box.
    let parented = "format: 1
physics:
  space: free
entities:
  - name: player
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 1.0
              - 1.0
              - 1.0
        body: static
  - name: bell
    components:
      Transform:
        translation:
          - 8.0
          - 0.0
          - 0.0
      AudioEmitter:
        sound: sounds/bell.wav
        range: 20.0
  - name: camera
    components:
      Transform:
        translation:
          - -1.5
          - 0.0
          - 0.0
      Parent:
        index: 0
        generation: 0
      Camera: {}
";
    let mut engine = occ_engine("parent", Some(config), parented);
    advance(&mut engine, 12);
    let (_, low_pass) = last_sync(&engine, EntityId::new(1, 0));
    assert_eq!(low_pass, LOW_PASS_OPEN_HZ);
}

fn scene_with_own_box() -> String {
    format!(
        "format: 1\nphysics:\n  space: free\nentities:\n  - name: camera\n    components:\n      Transform: {{}}\n      Camera: {{}}\n  - name: bell\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n      AudioEmitter:\n        sound: sounds/bell.wav\n        range: 20.0\n",
        block3(10, [8.0, 0.0, 0.0]),
        block3(14, [0.5, 0.5, 0.5]),
    )
}

/// A filter naming a layer the game never declared reports `L5102` once and every source reads
/// clear; a config that cannot run fails the build with `L5101`.
#[test]
fn a_refused_filter_reports_once_and_a_bad_config_fails_the_build() {
    let config = AudioOcclusion {
        filter: QueryFilter::new().only_fixed().include_layer("nope"),
        ..AudioOcclusion::default()
    };
    let mut engine = occ_engine(
        "refused",
        Some(config),
        &scene([8.0, 0.0, 0.0], &full_wall()),
    );
    advance(&mut engine, 12);
    let (gain, low_pass) = last_sync(&engine, SOURCE);
    assert_eq!(low_pass, LOW_PASS_OPEN_HZ);
    assert!((gain - UNOCCLUDED).abs() < 1e-5, "{gain}");
    let diagnostics = engine
        .world()
        .resource::<ludion_engine::Diagnostics>()
        .expect("Diagnostics installed");
    assert_eq!(
        diagnostics
            .reported(code::AUDIO_OCCLUSION_QUERY_REJECTED)
            .count(),
        1
    );

    let error = Engine::builder()
        .audio_occlusion(AudioOcclusion {
            rays: 2,
            ..AudioOcclusion::default()
        })
        .build()
        .err()
        .expect("a bad config does not build");
    assert_eq!(error.code, code::AUDIO_OCCLUSION_INVALID);
}

/// Occlusion is host state over an out-of-sim stream: the world hashes the same with it on or off.
#[test]
fn occlusion_never_moves_the_world_hash() {
    let scene = scene([8.0, 0.0, 0.0], &full_wall());
    let run = |name: &str, occlusion: Option<AudioOcclusion>| {
        let mut engine = occ_engine(name, occlusion, &scene);
        (0..10)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run("hash-off", None),
        run("hash-on", Some(AudioOcclusion::default()))
    );
}
