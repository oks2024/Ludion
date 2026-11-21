//! Audio zones (Neon Shroud 0023 A5a): the frame-boundary resolution of `AudioZone`s from the
//! listener point into bed loops, zone gains and a reverb, all in the frame-stamped audio log.

use ludion_core::audio::{Bus, ReverbParams};
use ludion_core::ids::EntityId;
use ludion_engine::{AudioMessage, Engine, LoopId, ReverbPreset, Transform};

const EAR: EntityId = EntityId::new(0, 0);
const YARD: EntityId = EntityId::new(1, 0);
const DOCK: EntityId = EntityId::new(2, 0);

/// A listener at the origin, a yard (priority 0, rain bed, `outdoor_open`) spanning x -20..20 and a
/// dock (priority 10, 3 m blend, hum bed, `hall_medium`, ambient bus 0.9) spanning x 10..50.
const WALK: &str = "\
format: 1
entities:
  - name: ear
    components:
      Transform: {}
      AudioListener: {}
  - name: yard
    components:
      Transform: {}
      AudioZone:
        shape:
          box:
            half_extents:
              - 20.0
              - 5.0
              - 10.0
        reverb:
          preset: outdoor_open
        beds:
          - sound: amb/rain.wav
            gain: 0.6
  - name: dock
    components:
      Transform:
        translation:
          - 30.0
          - 0.0
          - 0.0
      AudioZone:
        shape:
          box:
            half_extents:
              - 20.0
              - 5.0
              - 10.0
        priority: 10
        blend_m: 3.0
        reverb:
          preset: hall_medium
        beds:
          - sound: amb/hum.wav
            gain: 0.5
        bus_gains:
          ambient: 0.9
";

fn engine(scene: &str) -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(scene).expect("scene loads");
    engine
}

fn walk_to(engine: &mut Engine, x: f32) {
    engine
        .world_mut()
        .get_mut::<Transform>(EAR)
        .expect("the ear is alive")
        .expect("the ear has a Transform")
        .translation = [x, 0.0, 0.0];
    engine.advance_live();
}

fn messages(engine: &Engine) -> Vec<AudioMessage> {
    engine
        .audio_log()
        .iter()
        .map(|entry| entry.message.clone())
        .collect()
}

fn bed_gains(engine: &Engine, zone: EntityId) -> Vec<f32> {
    messages(engine)
        .iter()
        .filter_map(|message| match message {
            AudioMessage::LoopSync {
                id: LoopId::ZoneBed(z, 0),
                gain,
                ..
            } if *z == zone => Some(*gain),
            _ => None,
        })
        .collect()
}

fn is_stop(message: &AudioMessage, zone: EntityId) -> bool {
    matches!(message, AudioMessage::LoopStop { id: LoopId::ZoneBed(z, _) } if *z == zone)
}

fn last_reverb(engine: &Engine) -> Option<ReverbParams> {
    messages(engine)
        .iter()
        .rev()
        .find_map(|message| match message {
            AudioMessage::ReverbSet { params, .. } => Some(*params),
            _ => None,
        })
}

fn last_ambient_gain(engine: &Engine) -> Option<f32> {
    messages(engine)
        .iter()
        .rev()
        .find_map(|message| match message {
            AudioMessage::ZoneGain {
                bus: Bus::Ambient,
                gain,
                ..
            } => Some(*gain),
            _ => None,
        })
}

/// A world with an emitter and no zone sends no zone message of any kind.
#[test]
fn a_scene_without_a_zone_sends_nothing_new() {
    let scene = "format: 1\nentities:\n  - name: camera\n    components:\n      Transform: {}\n      Camera: {}\n  - name: bell\n    components:\n      Transform:\n        translation:\n          - 4.0\n          - 0.0\n          - 0.0\n      AudioEmitter:\n        sound: sounds/bell.wav\n        range: 10.0\n";
    let mut engine = engine(scene);
    for _ in 0..5 {
        engine.advance_live();
    }
    let sent = messages(&engine);
    assert!(
        sent.iter()
            .any(|m| matches!(m, AudioMessage::LoopSync { .. }))
    );
    assert!(sent.iter().all(|m| !matches!(
        m,
        AudioMessage::ReverbSet { .. }
            | AudioMessage::ZoneGain { .. }
            | AudioMessage::LoopSync {
                id: LoopId::ZoneBed(..),
                ..
            }
    )));
}

/// Walking from the yard through the dock's blend band: the rain fades out and stops, the hum fades
/// in, the reverb ends on the hall and the ambient bus on the dock's gain.
#[test]
fn walking_through_the_band_crossfades_beds_reverb_and_gain() {
    let mut engine = engine(WALK);
    walk_to(&mut engine, 0.0);
    let mut x = 0.0;
    while x < 30.0 {
        x += 0.25;
        walk_to(&mut engine, x);
    }

    let rain = bed_gains(&engine, YARD);
    assert!(rain.len() > 3, "{rain:?}");
    assert!(rain.windows(2).all(|w| w[1] <= w[0]), "{rain:?}");
    assert!((rain[0] - 0.6).abs() < 1e-5, "starts at its authored gain");
    assert!(
        messages(&engine).iter().any(|m| is_stop(m, YARD)),
        "the rain stops"
    );

    let hum = bed_gains(&engine, DOCK);
    assert!(hum.len() > 3, "{hum:?}");
    assert!(hum.windows(2).all(|w| w[1] >= w[0]), "{hum:?}");
    assert!(*hum.last().unwrap() >= 0.495, "{hum:?}");

    let reverb = last_reverb(&engine).expect("a reverb was sent");
    assert!(
        reverb.near(&ReverbPreset::HallMedium.params()),
        "{reverb:?}"
    );

    let ambient = last_ambient_gain(&engine).expect("a zone gain was sent");
    assert!((ambient - 0.9).abs() <= 0.9 * 0.011, "{ambient}");
}

/// Leaving every zone lands the reverb exactly dry and the zone gain exactly on unity.
#[test]
fn leaving_every_zone_releases_the_reverb_and_the_gain_exactly() {
    let mut engine = engine(WALK);
    for x in [0.0, 12.0, 30.0, 45.0, 60.0, 100.0] {
        walk_to(&mut engine, x);
    }
    assert_eq!(last_reverb(&engine), Some(ReverbParams::DRY));
    assert_eq!(last_ambient_gain(&engine), Some(1.0));
    assert!(messages(&engine).iter().any(|m| is_stop(m, DOCK)));
}

fn two_zones(low: &str, high: &str) -> String {
    format!(
        "format: 1
entities:
  - name: ear
    components:
      Transform: {{}}
      AudioListener: {{}}
  - name: low
    components:
      Transform: {{}}
      AudioZone:
        shape:
          sphere:
            radius: 50.0
{low}
        beds:
          - sound: amb/low.wav
  - name: high
    components:
      Transform: {{}}
      AudioZone:
        shape:
          sphere:
            radius: 50.0
{high}
        beds:
          - sound: amb/high.wav
"
    )
}

fn bed_sounds(engine: &Engine) -> Vec<String> {
    messages(engine)
        .iter()
        .filter_map(|message| match message {
            AudioMessage::LoopSync {
                id: LoopId::ZoneBed(..),
                sound,
                ..
            } => Some(sound.clone()),
            _ => None,
        })
        .collect()
}

/// A full-weight higher-priority zone masks the one beneath; equal priority goes to the lowest id.
#[test]
fn a_full_weight_winner_masks_the_zone_beneath_and_ties_go_to_the_lowest_id() {
    let mut masked = engine(&two_zones("        priority: 1", "        priority: 5"));
    masked.advance_live();
    assert_eq!(bed_sounds(&masked), ["amb/high.wav"]);

    let mut tied = engine(&two_zones("        priority: 3", "        priority: 3"));
    tied.advance_live();
    assert_eq!(bed_sounds(&tied), ["amb/low.wav"]);
}

/// Despawning a zone stops its beds on the next tick; a scene rebuild re-derives from scratch.
#[test]
fn despawning_a_zone_stops_its_beds_and_a_rebuild_rederives() {
    let mut engine = engine(WALK);
    walk_to(&mut engine, 30.0);
    assert!(!bed_gains(&engine, DOCK).is_empty());
    engine.world_mut().despawn(DOCK).expect("the dock is alive");
    engine.advance_live();
    assert!(messages(&engine).iter().any(|m| is_stop(m, DOCK)));

    engine.load_inline_scene(WALK).expect("scene loads");
    engine.advance_live();
    let sent = messages(&engine);
    // The rebuild's blanket stop clears the sink; the session reset empties the log, so the fresh
    // log holds only what the new world derives.
    assert!(
        sent.iter().any(|m| matches!(
            m,
            AudioMessage::LoopSync {
                id: LoopId::ZoneBed(YARD, 0),
                ..
            }
        )),
        "the yard rain starts again"
    );
}

/// The zone stream is a pure function of the world: two runs agree message for message.
#[test]
fn two_runs_give_identical_messages() {
    let run = || {
        let mut engine = engine(WALK);
        for step in 0..60 {
            walk_to(&mut engine, step as f32 * 0.75);
        }
        format!("{:?}", messages(&engine))
    };
    assert_eq!(run(), run());
}
