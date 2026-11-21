//! The [`AudioListener`] component: where the ear is, when it is not the camera.
//! `audio_listener` in `ludion-engine` reads it; `ludion docs audio-space` is the guide.

use ludion_core::audio::{AUDIO_ZONE_BEDS_MAX, Bus, ReverbParams};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::number::engine_number;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Whose axes place a sound across the stereo field.
///
/// ```
/// use ludion_components::ListenerOrientation;
///
/// let own: ListenerOrientation = serde_json::from_value(serde_json::json!("self")).unwrap();
/// assert_eq!(own, ListenerOrientation::Listener);
/// assert_eq!(serde_json::to_value(ListenerOrientation::Camera).unwrap(), "camera");
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ListenerOrientation {
    /// Left and right follow the lowest-`EntityId` `Camera`, so a sound pans as the screen shows it.
    #[default]
    Camera,
    /// Left and right follow this entity's own world rotation (`+X` right, `+Y` up).
    #[serde(rename = "self")]
    Listener,
}

/// Marks the entity every positional sound is measured from: distance falls off from its world
/// position, and the stereo pan takes its axes from [`ListenerOrientation`]. Without one in the
/// world the camera is the listener. With several, the lowest `EntityId` wins and `L1650` warns.
///
/// ```yaml
/// - name: player-ear
///   components:
///     Parent: { index: 1, generation: 0 }   # the player: the second entity in the scene
///     Transform: { translation: [0.0, 1.6, 0.0] }   # head height, local to the parent
///     AudioListener: {}
/// ```
///
/// ```
/// use ludion_components::{AudioListener, ListenerOrientation};
///
/// let ear: AudioListener = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(ear.orientation, ListenerOrientation::Camera);
/// let own: AudioListener =
///     serde_json::from_value(serde_json::json!({ "orientation": "self" })).unwrap();
/// assert_eq!(own.orientation, ListenerOrientation::Listener);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioListener {
    /// Whose axes pan the sound (default `camera`).
    #[serde(default)]
    pub orientation: ListenerOrientation,
}

/// The volume of an [`AudioZone`], in the zone entity's world pose (scale ignored).
///
/// ```
/// use ludion_components::AudioZoneShape;
///
/// let shape: AudioZoneShape =
///     serde_json::from_value(serde_json::json!({ "sphere": { "radius": 4.0 } })).unwrap();
/// assert_eq!(shape, AudioZoneShape::Sphere { radius: 4.0 });
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioZoneShape {
    /// A box oriented by the entity's rotation.
    Box {
        /// Half the box's size along each local axis, metres (finite and above 0).
        half_extents: [f32; 3],
    },
    /// A sphere about the entity's origin.
    Sphere {
        /// Metres (finite and above 0).
        radius: f32,
    },
}

/// A named reverb starting point; an [`AudioZoneReverb`]'s explicit fields override it.
///
/// ```
/// use ludion_components::ReverbPreset;
///
/// assert_eq!(ReverbPreset::default(), ReverbPreset::RoomSmall);
/// assert_eq!(ReverbPreset::HallMedium.params().decay_s, 1.8);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReverbPreset {
    /// An open street: a short, damped tail.
    OutdoorOpen,
    /// A small room (the default).
    #[default]
    RoomSmall,
    /// A medium hall.
    HallMedium,
    /// A large hangar: a long, wide tail.
    HangarLarge,
}

impl ReverbPreset {
    /// The preset's reverb.
    pub fn params(self) -> ReverbParams {
        let (decay_s, pre_delay_ms, damping, size, wet) = match self {
            ReverbPreset::OutdoorOpen => (0.4, 0.0, 0.7, 0.3, 0.05),
            ReverbPreset::RoomSmall => (0.6, 5.0, 0.5, 0.3, 0.2),
            ReverbPreset::HallMedium => (1.8, 20.0, 0.4, 0.6, 0.25),
            ReverbPreset::HangarLarge => (3.5, 40.0, 0.3, 0.9, 0.3),
        };
        ReverbParams {
            decay_s,
            pre_delay_ms,
            damping,
            size,
            wet,
        }
    }
}

fn is_default_preset(preset: &ReverbPreset) -> bool {
    *preset == ReverbPreset::default()
}

/// The reverb an [`AudioZone`] applies: a preset with optional per-field overrides.
///
/// ```
/// use ludion_components::AudioZoneReverb;
///
/// let reverb: AudioZoneReverb =
///     serde_json::from_value(serde_json::json!({ "preset": "hall_medium", "wet": 0.5 })).unwrap();
/// assert_eq!(reverb.params().wet, 0.5);
/// assert_eq!(reverb.params().decay_s, 1.8);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct AudioZoneReverb {
    /// The starting point (default `room_small`).
    #[serde(default, skip_serializing_if = "is_default_preset")]
    pub preset: ReverbPreset,
    /// Seconds for the tail to fall 60 dB, overriding the preset (0.1 to 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay_s: Option<f32>,
    /// Milliseconds before the tail starts, overriding the preset (0 to 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_delay_ms: Option<f32>,
    /// High-frequency absorption, overriding the preset (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f32>,
    /// Room size, overriding the preset (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// The mix of reverb against dry sound, overriding the preset (0 to 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wet: Option<f32>,
}

impl AudioZoneReverb {
    /// The preset with every explicit field applied.
    pub fn params(&self) -> ReverbParams {
        let base = self.preset.params();
        ReverbParams {
            decay_s: self.decay_s.unwrap_or(base.decay_s),
            pre_delay_ms: self.pre_delay_ms.unwrap_or(base.pre_delay_ms),
            damping: self.damping.unwrap_or(base.damping),
            size: self.size.unwrap_or(base.size),
            wet: self.wet.unwrap_or(base.wet),
        }
    }
}

fn default_bed_gain() -> f32 {
    1.0
}

fn is_unit_gain(gain: &f32) -> bool {
    *gain == 1.0
}

/// One looping bed an [`AudioZone`] plays on the ambient bus, non-positional.
///
/// ```
/// use ludion_components::AudioZoneBed;
///
/// let bed: AudioZoneBed =
///     serde_json::from_value(serde_json::json!({ "sound": "amb/hum.wav" })).unwrap();
/// assert_eq!(bed.gain, 1.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AudioZoneBed {
    /// The WAV, as a content-root-relative path.
    pub sound: String,
    /// A linear gain multiplier (default `1.0`; finite, not negative).
    #[serde(default = "default_bed_gain", skip_serializing_if = "is_unit_gain")]
    pub gain: f32,
}

/// The gain a zone sets on each bus while the listener is inside; an absent bus is untouched.
///
/// ```
/// use ludion_components::AudioZoneBusGains;
/// use ludion_core::audio::Bus;
///
/// let gains: AudioZoneBusGains =
///     serde_json::from_value(serde_json::json!({ "ambient": 0.9 })).unwrap();
/// assert_eq!(gains.gain(Bus::Ambient), Some(0.9));
/// assert_eq!(gains.gain(Bus::Music), None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
pub struct AudioZoneBusGains {
    /// The sound-effect bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfx: Option<f32>,
    /// The ambient bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ambient: Option<f32>,
    /// The dialogue bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<f32>,
    /// The interface bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<f32>,
    /// The music bus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<f32>,
}

impl AudioZoneBusGains {
    /// The zone's gain on `bus`; the master bus is never a zone's.
    pub fn gain(&self, bus: Bus) -> Option<f32> {
        match bus {
            Bus::Master => None,
            Bus::Sfx => self.sfx,
            Bus::Ambient => self.ambient,
            Bus::Voice => self.voice,
            Bus::Ui => self.ui,
            Bus::Music => self.music,
        }
    }

    fn is_empty(&self) -> bool {
        *self == AudioZoneBusGains::default()
    }
}

fn is_zero_priority(priority: &i32) -> bool {
    *priority == 0
}

fn is_zero_blend(blend: &f32) -> bool {
    *blend == 0.0
}

/// A volume that sets the sound around the listener: ambient beds, a reverb and per-bus gains,
/// resolved at every frame boundary from the listener point. Among overlapping zones the highest
/// `priority` wins (ties to the lowest entity); inside `blend_m` of the edge it fades with the
/// zone beneath. `ludion docs audio-zones` is the guide.
///
/// ```yaml
/// components:
///   AudioZone:
///     shape:
///       box:
///         half_extents: [30.0, 5.0, 20.0]
///     priority: 10
///     blend_m: 3.0
///     reverb:
///       preset: hall_medium
///       wet: 0.25
///     beds:
///       - sound: amb/machine_hum.wav
///         gain: 0.5
///     bus_gains:
///       ambient: 0.9
/// ```
///
/// ```
/// use ludion_components::AudioZone;
///
/// let zone: AudioZone = serde_json::from_value(serde_json::json!({
///     "shape": { "sphere": { "radius": 8.0 } },
/// }))
/// .unwrap();
/// assert!(zone.beds.is_empty() && zone.reverb.is_none() && zone.priority == 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AudioZone {
    /// The volume.
    pub shape: AudioZoneShape,
    /// The higher one wins where zones overlap; ties go to the lowest entity (default `0`).
    #[serde(default, skip_serializing_if = "is_zero_priority")]
    pub priority: i32,
    /// The edge band, metres: the zone's weight rises from 0 at the edge to 1 this far inside
    /// (default `0`, a hard edge; finite, not negative).
    #[serde(default, skip_serializing_if = "is_zero_blend")]
    pub blend_m: f32,
    /// The reverb while inside; absent means dry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverb: Option<AudioZoneReverb>,
    /// Ambient loops played while inside, at most 8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub beds: Vec<AudioZoneBed>,
    /// Per-bus gains while inside.
    #[serde(default, skip_serializing_if = "AudioZoneBusGains::is_empty")]
    pub bus_gains: AudioZoneBusGains,
}

fn number(value: &Value) -> Option<f32> {
    value.as_f64().map(|n| engine_number(n) as f32)
}

/// Collects the `L1651` diagnostics of one validation.
#[derive(Default)]
struct Found {
    out: Vec<Diagnostic>,
}

impl Found {
    fn reject(&mut self, pointer: String, problem: String) {
        self.out.push(
            Diagnostic::new(code::AUDIO_ZONE_INVALID, format!("`AudioZone` {problem}"))
                .with_path(pointer)
                .with_help("see `ludion docs audio-zones`; `ludion explain L1651`".to_string()),
        );
    }

    /// Rejects `container[field]` when it is a number and `bad` it.
    fn check(
        &mut self,
        base: &str,
        container: &Value,
        field: &str,
        bad: &dyn Fn(f32) -> bool,
        needs: &str,
    ) {
        if let Some(n) = container.get(field).and_then(number)
            && bad(n)
        {
            self.reject(format!("{base}/{field}"), format!("`{field}` {n} {needs}"));
        }
    }
}

impl SemanticValidate for AudioZone {
    /// Rejects what a schema cannot (`L1651`, pointer into the field): a box half extent or sphere
    /// radius that is not finite and above 0, a negative or non-finite `blend_m`, more than
    /// [`AUDIO_ZONE_BEDS_MAX`] beds, a bed `gain` or `bus_gains` entry that is negative or not
    /// finite, and an explicit `reverb` field outside its range.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut found = Found::default();
        if let Some(shape) = value.get("shape") {
            if let Some(extents) = shape
                .get("box")
                .and_then(|b| b.get("half_extents"))
                .and_then(Value::as_array)
            {
                for (i, c) in extents.iter().enumerate() {
                    if number(c).is_some_and(|n| !(n.is_finite() && n > 0.0)) {
                        found.reject(
                            format!("/shape/box/half_extents/{i}"),
                            "`half_extents` must be finite and above 0".into(),
                        );
                    }
                }
            }
            if let Some(sphere) = shape.get("sphere") {
                found.check(
                    "/shape/sphere",
                    sphere,
                    "radius",
                    &|n| !(n.is_finite() && n > 0.0),
                    "must be finite and above 0",
                );
            }
        }
        found.check(
            "",
            value,
            "blend_m",
            &|n| !(n.is_finite() && n >= 0.0),
            "must be finite and not negative",
        );
        if let Some(beds) = value.get("beds").and_then(Value::as_array) {
            if beds.len() > AUDIO_ZONE_BEDS_MAX {
                found.reject(
                    format!("/beds/{AUDIO_ZONE_BEDS_MAX}"),
                    format!(
                        "has {} beds; at most {AUDIO_ZONE_BEDS_MAX} play",
                        beds.len()
                    ),
                );
            }
            for (i, bed) in beds.iter().enumerate() {
                found.check(
                    &format!("/beds/{i}"),
                    bed,
                    "gain",
                    &|n| !(n.is_finite() && n >= 0.0),
                    "must be finite and not negative",
                );
            }
        }
        if let Some(reverb) = value.get("reverb") {
            for (field, lo, hi) in [
                ("decay_s", 0.1, 20.0),
                ("pre_delay_ms", 0.0, 200.0),
                ("damping", 0.0, 1.0),
                ("size", 0.0, 1.0),
                ("wet", 0.0, 1.0),
            ] {
                found.check(
                    "/reverb",
                    reverb,
                    field,
                    &|n| !(n.is_finite() && (lo..=hi).contains(&n)),
                    &format!("must be within {lo} to {hi}"),
                );
            }
        }
        if let Some(gains) = value.get("bus_gains") {
            for field in ["sfx", "ambient", "voice", "ui", "music"] {
                found.check(
                    "/bus_gains",
                    gains,
                    field,
                    &|n| !(n.is_finite() && n >= 0.0),
                    "must be finite and not negative",
                );
            }
        }
        found.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_round_trips_as_camera_and_self() {
        for (variant, text) in [
            (ListenerOrientation::Camera, "camera"),
            (ListenerOrientation::Listener, "self"),
        ] {
            let value = serde_json::to_value(variant).unwrap();
            assert_eq!(value, text);
            assert_eq!(
                serde_json::from_value::<ListenerOrientation>(value).unwrap(),
                variant
            );
        }
    }

    #[test]
    fn empty_listener_orients_by_camera() {
        let ear: AudioListener = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(ear, AudioListener::default());
        assert_eq!(ear.orientation, ListenerOrientation::Camera);
    }

    fn zone(value: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({ "shape": { "sphere": { "radius": 5.0 } } });
        for (k, v) in value.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    }

    fn pointers(value: serde_json::Value) -> Vec<String> {
        AudioZone::validate_semantics(&value)
            .into_iter()
            .map(|d| {
                assert_eq!(d.code, code::AUDIO_ZONE_INVALID);
                d.path.clone().unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn a_zone_round_trips_and_omits_defaults() {
        let minimal =
            serde_json::json!({ "shape": { "box": { "half_extents": [1.0, 2.0, 3.0] } } });
        let parsed: AudioZone = serde_json::from_value(minimal.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), minimal);

        let full = serde_json::json!({
            "shape": { "sphere": { "radius": 4.0 } },
            "priority": 10,
            "blend_m": 3.0,
            "reverb": { "preset": "hall_medium", "wet": 0.25 },
            "beds": [{ "sound": "amb/hum.wav", "gain": 0.5 }, { "sound": "amb/rain.wav" }],
            "bus_gains": { "ambient": 0.75 },
        });
        let parsed: AudioZone = serde_json::from_value(full.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), full);
        assert!(pointers(full).is_empty());
    }

    #[test]
    fn a_default_preset_reverb_is_written_as_an_empty_map() {
        let reverb: AudioZoneReverb = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(serde_json::to_value(reverb).unwrap(), serde_json::json!({}));
        assert_eq!(reverb.params(), ReverbPreset::RoomSmall.params());
    }

    #[test]
    fn an_unknown_preset_is_a_parse_error() {
        assert!(
            serde_json::from_value::<AudioZoneReverb>(serde_json::json!({ "preset": "cave" }))
                .is_err()
        );
    }

    #[test]
    fn bus_gains_name_the_five_user_buses() {
        let gains = AudioZoneBusGains {
            sfx: Some(0.1),
            ambient: Some(0.2),
            voice: Some(0.3),
            ui: Some(0.4),
            music: Some(0.5),
        };
        let got: Vec<_> = Bus::ALL.iter().map(|b| gains.gain(*b)).collect();
        assert_eq!(
            got,
            [None, Some(0.1), Some(0.2), Some(0.3), Some(0.4), Some(0.5)]
        );
    }

    #[test]
    fn every_l1651_pointer_is_reported() {
        let bad = |value: serde_json::Value| pointers(zone(value));
        assert_eq!(
            pointers(
                serde_json::json!({ "shape": { "box": { "half_extents": [1.0, 0.0, -2.0] } } })
            ),
            ["/shape/box/half_extents/1", "/shape/box/half_extents/2"]
        );
        assert_eq!(
            pointers(serde_json::json!({ "shape": { "sphere": { "radius": 0.0 } } })),
            ["/shape/sphere/radius"]
        );
        assert_eq!(bad(serde_json::json!({ "blend_m": -1.0 })), ["/blend_m"]);
        let nine: Vec<_> = (0..9)
            .map(|_| serde_json::json!({ "sound": "a.wav" }))
            .collect();
        assert_eq!(bad(serde_json::json!({ "beds": nine })), ["/beds/8"]);
        assert_eq!(
            bad(serde_json::json!({ "beds": [{ "sound": "a.wav", "gain": -0.5 }] })),
            ["/beds/0/gain"]
        );
        assert_eq!(
            bad(serde_json::json!({ "reverb": { "decay_s": 30.0, "wet": 2.0 } })),
            ["/reverb/decay_s", "/reverb/wet"]
        );
        assert_eq!(
            bad(
                serde_json::json!({ "reverb": { "pre_delay_ms": -1.0, "damping": 1.5, "size": -0.1 } })
            ),
            ["/reverb/pre_delay_ms", "/reverb/damping", "/reverb/size"]
        );
        assert_eq!(
            bad(serde_json::json!({ "bus_gains": { "music": -1.0 } })),
            ["/bus_gains/music"]
        );
    }
}
