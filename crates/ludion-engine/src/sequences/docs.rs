//! The `kind: sequence` document and its validation pass (`L1730`–`L1735`).

use std::collections::BTreeSet;

use ludion_camera::RigConfig;
use ludion_components::path_follow::PathOrient;
use ludion_components::ui::{UiEase, UiTweenTrack};
use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_ecs::{SemanticValidate, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dialogue::DialogueDoc;
use crate::docs::DocStore;

/// The `kind:` of a sequence document.
pub(crate) const SEQUENCE_KIND: &str = "sequence";

/// The reserved event name: the engine's own `Ended`.
const RESERVED_EVENT: &str = "end";

/// Which clock a sequence's `at` ticks count on.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SequenceClock {
    /// The gameplay clock: a `GameClock` hold or time scale stretches the beat (default).
    #[default]
    Game,
    /// The sim clock: runs through a hold, one `at` per tick whatever the scale.
    Sim,
}

/// A linear timeline of keys (`kind: sequence`), played by [`play`](super::play).
///
/// ```yaml
/// format: 1
/// kind: sequence
/// roles:
///   - camera
///   - radar
/// length_ticks: 120
/// tracks:
///   camera:
///     - at: 120
///       role: camera
///       restore: true
///   event:
///     - at: 60
///       name: radar_online
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SequenceDoc {
    /// The roles a game binds entities to, unique.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The beat's length in ticks, at least 1; it ends on the tick the playhead reaches it.
    pub length_ticks: u32,
    /// The clock `at` counts on (default `game`, or `sim` with `holds_gameplay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<SequenceClock>,
    /// Holds the gameplay clock while the beat plays (default `false`); implies `clock: sim`.
    #[serde(default)]
    pub holds_gameplay: bool,
    /// Whether [`skip`](super::skip) may cut the beat to its end state (default `true`).
    #[serde(default = "yes")]
    pub skippable: bool,
    /// The keys, one list per track.
    #[serde(default)]
    pub tracks: SequenceTracks,
}

impl SequenceDoc {
    /// The clock the playhead counts on: the explicit `clock`, else `sim` for a holding beat and
    /// `game` otherwise.
    pub fn effective_clock(&self) -> SequenceClock {
        self.clock.unwrap_or(if self.holds_gameplay {
            SequenceClock::Sim
        } else {
            SequenceClock::Game
        })
    }
}

fn yes() -> bool {
    true
}

/// A sequence's seven track lists; each is in ascending `at` order.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct SequenceTracks {
    /// Writes a role's `CameraRig`.
    #[serde(default)]
    pub camera: Vec<CameraKey>,
    /// Moves a role's `Transform` to a pose or along a `kind: path`.
    #[serde(default)]
    pub transform: Vec<TransformKey>,
    /// Crossfades a role's `Animation`.
    #[serde(default)]
    pub animation: Vec<AnimationKey>,
    /// Writes a role's `UiTween`.
    #[serde(default)]
    pub ui: Vec<UiKey>,
    /// Plays a one-shot sound.
    #[serde(default)]
    pub audio: Vec<AudioKey>,
    /// Says a dialogue line.
    #[serde(default)]
    pub dialogue: Vec<DialogueKey>,
    /// Emits a named event.
    #[serde(default)]
    pub event: Vec<EventKey>,
}

/// One camera key: exactly one of `rig` and `restore`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    /// The tick, counted from the start, the key applies on.
    pub at: u32,
    /// The role whose `CameraRig` it writes.
    #[serde(default)]
    pub role: String,
    /// The rig to write; the first one saves the role's current rig for `restore`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig: Option<RigConfig>,
    /// Writes the saved rig back (default `false`).
    #[serde(default)]
    pub restore: bool,
    /// Cuts to the pose instead of blending (default `false`).
    #[serde(default)]
    pub snap: bool,
}

/// One transform key: moves a role's `Transform` over `over` ticks, from the pose it has when the
/// key applies, to a pose (`to`) or along a `kind: path` (`path`); exactly one of the two.
///
/// ```yaml
/// - at: 10
///   role: hero
///   over: 30
///   ease: smooth
///   to: { anchor: door, offset: [0, 0, 1.5], yaw_deg: 180 }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransformKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The role whose `Transform` it moves.
    #[serde(default)]
    pub role: String,
    /// The pose to move to, fixed when the key applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TransformTo>,
    /// A `kind: path` (content-root path) to travel along, in the role's parent frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How a `path` span turns the role (default `none`); only with `path`.
    #[serde(default, skip_serializing_if = "PathOrient::is_none")]
    pub orient: PathOrient,
    /// The span's length in ticks; `0` is a jump (default `0`). `at + over` is at most `length_ticks`.
    #[serde(default)]
    pub over: u32,
    /// The easing of the span's progress (default `linear`).
    #[serde(default)]
    pub ease: UiEase,
}

/// The pose a [`TransformKey`] moves to, in world space.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransformTo {
    /// A declared role the pose is relative to; its world pose when the key applies (default none:
    /// `offset` and `yaw_deg` are absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The position, in the anchor's frame (world space without an anchor; default `[0, 0, 0]`).
    #[serde(default)]
    pub offset: [f32; 3],
    /// The facing, degrees about +Y added to the anchor's rotation; absent leaves the rotation alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw_deg: Option<f32>,
}

/// One animation key.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnimationKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The role whose `Animation` it crossfades.
    #[serde(default)]
    pub role: String,
    /// The clip to fade to.
    pub clip: String,
    /// Crossfade seconds; `0` is a hard cut (default `0`).
    #[serde(default)]
    pub fade: f32,
}

/// One ui key.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct UiKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The role whose `UiTween` it writes.
    #[serde(default)]
    pub role: String,
    /// The tracks; each `start` is a frame offset after the key applies, on the track's own clock.
    pub tracks: Vec<UiTweenTrack>,
}

/// One audio key: a one-shot.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AudioKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The sound, a path relative to the content root.
    pub play: String,
    /// Linear gain (default `1.0`).
    #[serde(default = "one")]
    pub gain: f32,
    /// Plays positionally at this role's entity (default: not positional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The positional falloff range in metres (default `10.0`, the `AudioEmitter` default).
    #[serde(default = "ten")]
    pub range: f32,
}

fn one() -> f32 {
    1.0
}

fn ten() -> f32 {
    10.0
}

/// One dialogue key.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DialogueKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The line id in a `kind: dialogue` doc.
    pub line: String,
    /// The role whose entity speaks it (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// Holds the playhead here until the line ends (default: no wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<DialogueWait>,
}

/// What a dialogue key waits for.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DialogueWait {
    /// The line it says has ended, however it ended.
    LineEnd,
}

/// What an event key waits for.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventWait {
    /// The game calls [`release`](super::release).
    Release,
}

/// What a skip does with an unfired event.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnSkip {
    /// Fires it, in order, at the skip (default).
    #[default]
    Fire,
    /// Drops it.
    Drop,
}

/// One event key.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventKey {
    /// The tick the key applies on.
    pub at: u32,
    /// The event name a game reads from `SequenceEvents`; not empty, not `end`.
    pub name: String,
    /// What a skip does with it (default `fire`).
    #[serde(default)]
    pub on_skip: OnSkip,
    /// Holds the playhead here, after the event fires, until released (default: no wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<EventWait>,
}

fn diagnostic(code: DiagnosticCode, message: String, path: String) -> Diagnostic {
    Diagnostic::new(code, message)
        .with_path(path)
        .with_help("see `ludion docs sequences`")
}

/// `L1730`–`L1733` over every loaded `kind: sequence` doc, in store order. Paths are
/// `sequence/{doc}` plus a JSON Pointer such as `/tracks/camera/2/at`.
pub(crate) fn validate_sequence_docs(world: &World) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(store) = world.resource::<DocStore<SequenceDoc>>() else {
        return out;
    };
    // The config is not installed while the docs validate; the doc store exists iff `.dialogue()` ran.
    let dialogue_on = world.resource::<DocStore<DialogueDoc>>().is_some();
    let lines: BTreeSet<&str> = world
        .resource::<DocStore<DialogueDoc>>()
        .into_iter()
        .flat_map(DocStore::iter)
        .flat_map(|(_, doc)| doc.lines.keys().map(String::as_str))
        .collect();
    for (path, doc) in store.iter() {
        validate_doc(path, doc, dialogue_on, &lines, &mut out);
    }
    out
}

fn validate_doc(
    doc_path: &str,
    doc: &SequenceDoc,
    dialogue_on: bool,
    lines: &BTreeSet<&str>,
    out: &mut Vec<Diagnostic>,
) {
    let at = |pointer: &str| format!("sequence/{doc_path}{pointer}");
    if doc.length_ticks == 0 {
        out.push(diagnostic(
            code::SEQUENCE_INVALID,
            format!("`{doc_path}` has `length_ticks: 0`; a sequence lasts at least one tick"),
            at("/length_ticks"),
        ));
    }
    let mut roles: BTreeSet<&str> = BTreeSet::new();
    for (n, role) in doc.roles.iter().enumerate() {
        if role.is_empty() || !roles.insert(role) {
            out.push(diagnostic(
                code::SEQUENCE_UNKNOWN_ROLE,
                format!("role `{role}` of `{doc_path}` is empty or declared twice"),
                at(&format!("/roles/{n}")),
            ));
        }
    }
    if doc.holds_gameplay && doc.clock == Some(SequenceClock::Game) {
        out.push(diagnostic(
            code::SEQUENCE_HOLD_CLOCK,
            format!(
                "`{doc_path}` holds gameplay with `clock: game`; a held game clock never advances it"
            ),
            at("/clock"),
        ));
    }
    let tracks = &doc.tracks;
    let length = doc.length_ticks;

    // One pending wait per player: no two wait keys at one `at`, across both tracks.
    let mut waits: Vec<(u32, String)> = Vec::new();
    for (n, key) in tracks.dialogue.iter().enumerate() {
        if key.wait.is_some() {
            waits.push((key.at, format!("/tracks/dialogue/{n}/wait")));
        }
    }
    for (n, key) in tracks.event.iter().enumerate() {
        if key.wait.is_some() {
            waits.push((key.at, format!("/tracks/event/{n}/wait")));
        }
    }
    for (n, (wait_at, pointer)) in waits.iter().enumerate() {
        if let Some((_, first)) = waits[..n].iter().find(|(a, _)| a == wait_at) {
            out.push(diagnostic(
                code::SEQUENCE_WAIT_CONFLICT,
                format!("two wait keys share tick {wait_at}: `{first}` and `{pointer}`"),
                at(pointer),
            ));
        }
    }

    // `(track, index, at)` per key, checked for range and order in one place.
    let ats = |track: &str, keys: Vec<u32>, out: &mut Vec<Diagnostic>| {
        let mut previous = 0;
        for (n, key_at) in keys.into_iter().enumerate() {
            if key_at > length {
                out.push(diagnostic(
                    code::SEQUENCE_INVALID,
                    format!("a `{track}` key is at tick {key_at}, past `length_ticks` {length}"),
                    at(&format!("/tracks/{track}/{n}/at")),
                ));
            }
            if key_at < previous {
                out.push(diagnostic(
                    code::SEQUENCE_INVALID,
                    format!(
                        "a `{track}` key is at tick {key_at}, before the key above it at {previous}"
                    ),
                    at(&format!("/tracks/{track}/{n}/at")),
                ));
            }
            previous = previous.max(key_at);
        }
    };
    ats("camera", tracks.camera.iter().map(|k| k.at).collect(), out);
    ats(
        "transform",
        tracks.transform.iter().map(|k| k.at).collect(),
        out,
    );
    ats(
        "animation",
        tracks.animation.iter().map(|k| k.at).collect(),
        out,
    );
    ats("ui", tracks.ui.iter().map(|k| k.at).collect(), out);
    ats("audio", tracks.audio.iter().map(|k| k.at).collect(), out);
    ats(
        "dialogue",
        tracks.dialogue.iter().map(|k| k.at).collect(),
        out,
    );
    ats("event", tracks.event.iter().map(|k| k.at).collect(), out);

    let role_of = |track: &str, n: usize, role: &str, required: bool, out: &mut Vec<Diagnostic>| {
        if role.is_empty() {
            if required {
                out.push(diagnostic(
                    code::SEQUENCE_UNKNOWN_ROLE,
                    format!("a `{track}` key has no `role`"),
                    at(&format!("/tracks/{track}/{n}/role")),
                ));
            }
        } else if !roles.contains(role) {
            out.push(diagnostic(
                code::SEQUENCE_UNKNOWN_ROLE,
                format!("a `{track}` key names role `{role}`, which `roles:` does not declare"),
                at(&format!("/tracks/{track}/{n}/role")),
            ));
        }
    };

    for (n, key) in tracks.camera.iter().enumerate() {
        role_of("camera", n, &key.role, true, out);
        if key.rig.is_some() == key.restore {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                "a camera key needs exactly one of `rig:` and `restore: true`".to_owned(),
                at(&format!("/tracks/camera/{n}")),
            ));
        }
        if let Some(rig) = &key.rig
            && let Ok(value) = serde_json::to_value(rig)
        {
            let prefix = format!("/tracks/camera/{n}/rig");
            out.extend(prefixed(
                RigConfig::validate_semantics(&value),
                doc_path,
                &prefix,
            ));
        }
    }
    for (n, key) in tracks.transform.iter().enumerate() {
        role_of("transform", n, &key.role, true, out);
        let pointer = format!("/tracks/transform/{n}");
        if key.to.is_some() == key.path.is_some() {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                "a transform key needs exactly one of `to:` and `path:`".to_owned(),
                at(&pointer),
            ));
        }
        if let Some(to) = &key.to {
            if let Some(anchor) = &to.anchor
                && !roles.contains(anchor.as_str())
            {
                out.push(diagnostic(
                    code::SEQUENCE_UNKNOWN_ROLE,
                    format!(
                        "a transform key anchors to `{anchor}`, which `roles:` does not declare"
                    ),
                    at(&format!("{pointer}/to/anchor")),
                ));
            }
            let finite =
                to.offset.iter().all(|c| c.is_finite()) && to.yaw_deg.is_none_or(f32::is_finite);
            if !finite {
                out.push(diagnostic(
                    code::SEQUENCE_INVALID,
                    "`offset` and `yaw_deg` must be finite".to_owned(),
                    at(&format!("{pointer}/to")),
                ));
            }
        }
        if key.path.as_ref().is_some_and(|p| p.trim().is_empty()) {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                "`path` is blank; name a `kind: path` document".to_owned(),
                at(&format!("{pointer}/path")),
            ));
        }
        if key.path.is_none() && !key.orient.is_none() {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                "`orient` only applies to a `path` span".to_owned(),
                at(&format!("{pointer}/orient")),
            ));
        }
        if u64::from(key.at) + u64::from(key.over) > u64::from(length) {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                format!(
                    "a transform key at tick {} with `over: {}` ends past `length_ticks` {length}",
                    key.at, key.over
                ),
                at(&format!("{pointer}/over")),
            ));
        }
    }
    for (n, key) in tracks.animation.iter().enumerate() {
        role_of("animation", n, &key.role, true, out);
        if !key.fade.is_finite() || key.fade < 0.0 {
            out.push(diagnostic(
                code::SEQUENCE_INVALID,
                format!("`fade` is {}; it must be finite and not negative", key.fade),
                at(&format!("/tracks/animation/{n}/fade")),
            ));
        }
    }
    for (n, key) in tracks.ui.iter().enumerate() {
        role_of("ui", n, &key.role, true, out);
        if let Ok(tracks) = serde_json::to_value(&key.tracks) {
            let value = serde_json::json!({ "tracks": tracks });
            let prefix = format!("/tracks/ui/{n}");
            out.extend(prefixed(
                ludion_components::ui::UiTween::validate_semantics(&value),
                doc_path,
                &prefix,
            ));
        }
    }
    for (n, key) in tracks.audio.iter().enumerate() {
        if let Some(role) = &key.role {
            role_of("audio", n, role, false, out);
        }
        for (field, value) in [("gain", key.gain), ("range", key.range)] {
            if !value.is_finite() || value < 0.0 {
                out.push(diagnostic(
                    code::SEQUENCE_INVALID,
                    format!("`{field}` is {value}; it must be finite and not negative"),
                    at(&format!("/tracks/audio/{n}/{field}")),
                ));
            }
        }
    }
    for (n, key) in tracks.dialogue.iter().enumerate() {
        if let Some(role) = &key.speaker {
            role_of("dialogue", n, role, false, out);
        }
        let problem = if !dialogue_on {
            Some("the game did not opt in with `.dialogue()`".to_owned())
        } else if !lines.contains(key.line.as_str()) {
            Some("no `kind: dialogue` doc defines it".to_owned())
        } else {
            None
        };
        if let Some(problem) = problem {
            out.push(diagnostic(
                code::SEQUENCE_UNKNOWN_LINE,
                format!("dialogue key line `{}`: {problem}", key.line),
                at(&format!("/tracks/dialogue/{n}/line")),
            ));
        }
    }
    for (n, key) in tracks.event.iter().enumerate() {
        if key.name.trim().is_empty() || key.name == RESERVED_EVENT {
            out.push(diagnostic(
                code::SEQUENCE_EVENT_NAME,
                format!(
                    "event name `{}` is empty or the reserved `{RESERVED_EVENT}`",
                    key.name
                ),
                at(&format!("/tracks/event/{n}/name")),
            ));
        }
    }
}

/// Re-roots a nested validator's diagnostics: its pointer (relative to the embedded value) goes
/// under `prefix` inside the document at `doc_path`.
fn prefixed(found: Vec<Diagnostic>, doc_path: &str, prefix: &str) -> Vec<Diagnostic> {
    found
        .into_iter()
        .map(|mut d| {
            let inner = d.path.take().unwrap_or_default();
            d.path = Some(format!("sequence/{doc_path}{prefix}{inner}"));
            d
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(doc: serde_json::Value, dialogue_on: bool) -> Vec<Diagnostic> {
        let doc: SequenceDoc = serde_json::from_value(doc).unwrap();
        let lines: BTreeSet<&str> = ["hello"].into_iter().collect();
        let mut out = Vec::new();
        validate_doc("sequences/t.yaml", &doc, dialogue_on, &lines, &mut out);
        out
    }

    fn codes(out: &[Diagnostic]) -> Vec<&str> {
        out.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn a_key_at_the_length_is_valid_and_one_past_it_is_not() {
        let doc = |at: u32| {
            json!({ "roles": [], "length_ticks": 4,
                "tracks": { "event": [{ "at": at, "name": "x" }] } })
        };
        assert!(run(doc(4), true).is_empty());
        let out = run(doc(5), true);
        assert_eq!(codes(&out), ["L1730"]);
        assert_eq!(
            out[0].path.as_deref(),
            Some("sequence/sequences/t.yaml/tracks/event/0/at")
        );
    }

    #[test]
    fn equal_ats_are_in_order_and_a_decrease_is_not() {
        let doc = |second: u32| {
            json!({ "length_ticks": 9, "tracks": { "event": [
                { "at": 3, "name": "a" }, { "at": second, "name": "b" } ] } })
        };
        assert!(run(doc(3), true).is_empty());
        assert_eq!(codes(&run(doc(2), true)), ["L1730"]);
    }

    #[test]
    fn a_camera_key_needs_exactly_one_of_rig_and_restore() {
        let doc = |key: serde_json::Value| json!({ "roles": ["cam"], "length_ticks": 9, "tracks": { "camera": [key] } });
        assert!(
            run(
                doc(json!({ "at": 0, "role": "cam", "restore": true })),
                true
            )
            .is_empty()
        );
        assert!(run(doc(json!({ "at": 0, "role": "cam", "rig": {} })), true).is_empty());
        assert_eq!(
            codes(&run(doc(json!({ "at": 0, "role": "cam" })), true)),
            ["L1730"]
        );
        assert_eq!(
            codes(&run(
                doc(json!({ "at": 0, "role": "cam", "rig": {}, "restore": true })),
                true
            )),
            ["L1730"]
        );
    }

    #[test]
    fn a_transform_key_is_checked_for_shape_anchor_and_span() {
        let doc = |key: serde_json::Value| json!({ "roles": ["hero", "door"], "length_ticks": 10, "tracks": { "transform": [key] } });
        let to = json!({ "anchor": "door", "offset": [0, 0, 1], "yaw_deg": 90 });
        assert!(
            run(
                doc(json!({ "at": 2, "role": "hero", "over": 8, "to": to })),
                true
            )
            .is_empty()
        );
        assert!(
            run(
                doc(json!({ "at": 0, "role": "hero", "path": "paths/a.yaml", "orient": "yaw" })),
                true
            )
            .is_empty()
        );
        let neither = run(doc(json!({ "at": 0, "role": "hero" })), true);
        assert_eq!(codes(&neither), ["L1730"]);
        let both = run(
            doc(json!({ "at": 0, "role": "hero", "to": {}, "path": "p.yaml" })),
            true,
        );
        assert_eq!(codes(&both), ["L1730"]);
        let ghost = run(
            doc(json!({ "at": 0, "role": "hero", "to": { "anchor": "ghost" } })),
            true,
        );
        assert_eq!(codes(&ghost), ["L1731"]);
        assert_eq!(
            ghost[0].path.as_deref(),
            Some("sequence/sequences/t.yaml/tracks/transform/0/to/anchor")
        );
        let long = run(
            doc(json!({ "at": 5, "role": "hero", "over": 6, "to": {} })),
            true,
        );
        assert_eq!(codes(&long), ["L1730"]);
        assert!(long[0].message.contains('5') && long[0].message.contains('6'));
        let orient = run(
            doc(json!({ "at": 0, "role": "hero", "to": {}, "orient": "yaw" })),
            true,
        );
        assert_eq!(codes(&orient), ["L1730"]);
        let blank = run(doc(json!({ "at": 0, "role": "hero", "path": " " })), true);
        assert_eq!(codes(&blank), ["L1730"]);
        let nan = run(
            doc(json!({ "at": 0, "role": "hero", "to": { "offset": [0, 1e39, 0] } })),
            true,
        );
        assert_eq!(codes(&nan), ["L1730"]);
    }

    #[test]
    fn roles_must_be_declared_once_and_present_where_required() {
        let out = run(
            json!({ "roles": ["a", "a"], "length_ticks": 9, "tracks": {
                "animation": [{ "at": 0, "clip": "x" }],
                "audio": [{ "at": 0, "play": "s.wav", "role": "ghost" }] } }),
            true,
        );
        assert_eq!(codes(&out), ["L1731", "L1731", "L1731"]);
    }

    #[test]
    fn a_dialogue_line_must_exist_and_dialogue_must_be_on() {
        let doc = |line: &str| json!({ "length_ticks": 9, "tracks": { "dialogue": [{ "at": 0, "line": line }] } });
        assert!(run(doc("hello"), true).is_empty());
        assert_eq!(codes(&run(doc("nope"), true)), ["L1733"]);
        assert_eq!(codes(&run(doc("hello"), false)), ["L1733"]);
    }

    #[test]
    fn non_finite_numbers_and_reserved_event_names_are_refused() {
        let doc = |gain: f64, name: &str| {
            json!({ "length_ticks": 9, "tracks": {
                "audio": [{ "at": 0, "play": "s.wav", "gain": gain }],
                "event": [{ "at": 0, "name": name }] } })
        };
        assert!(run(doc(0.5, "ok"), true).is_empty());
        assert_eq!(codes(&run(doc(-1.0, "ok"), true)), ["L1730"]);
        assert_eq!(codes(&run(doc(0.5, "end"), true)), ["L1732"]);
        assert_eq!(codes(&run(doc(0.5, ""), true)), ["L1732"]);
    }

    #[test]
    fn holding_with_an_explicit_game_clock_is_refused() {
        let doc = |clock: Option<&str>, holds: bool| {
            let mut v = json!({ "length_ticks": 9, "holds_gameplay": holds });
            if let Some(c) = clock {
                v["clock"] = json!(c);
            }
            v
        };
        assert!(run(doc(None, true), true).is_empty());
        assert!(run(doc(Some("sim"), true), true).is_empty());
        assert!(run(doc(Some("game"), false), true).is_empty());
        let out = run(doc(Some("game"), true), true);
        assert_eq!(codes(&out), ["L1734"]);
        assert_eq!(
            out[0].path.as_deref(),
            Some("sequence/sequences/t.yaml/clock")
        );
    }

    #[test]
    fn the_effective_clock_resolves_all_four_combinations() {
        let doc = |clock: Option<&str>, holds: bool| {
            let mut v = json!({ "length_ticks": 9, "holds_gameplay": holds });
            if let Some(c) = clock {
                v["clock"] = json!(c);
            }
            serde_json::from_value::<SequenceDoc>(v)
                .unwrap()
                .effective_clock()
        };
        assert_eq!(doc(None, false), SequenceClock::Game);
        assert_eq!(doc(None, true), SequenceClock::Sim);
        assert_eq!(doc(Some("sim"), false), SequenceClock::Sim);
        assert_eq!(doc(Some("game"), false), SequenceClock::Game);
    }

    #[test]
    fn two_waits_at_one_tick_conflict_within_and_across_tracks() {
        let doc = |tracks: serde_json::Value| json!({ "length_ticks": 9, "tracks": tracks });
        let line = |at: u32| json!({ "at": at, "line": "hello", "wait": "line_end" });
        let event = |at: u32| json!({ "at": at, "name": "e", "wait": "release" });
        assert!(
            run(
                doc(json!({ "dialogue": [line(2)], "event": [event(3)] })),
                true
            )
            .is_empty()
        );
        let within = run(doc(json!({ "event": [event(2), event(2)] })), true);
        assert_eq!(codes(&within), ["L1735"]);
        assert_eq!(
            within[0].path.as_deref(),
            Some("sequence/sequences/t.yaml/tracks/event/1/wait")
        );
        let across = run(
            doc(json!({ "dialogue": [line(2)], "event": [event(2)] })),
            true,
        );
        assert_eq!(codes(&across), ["L1735"]);
        assert!(across[0].message.contains("/tracks/dialogue/0/wait"));
        assert!(across[0].message.contains("/tracks/event/0/wait"));
    }

    #[test]
    fn a_wait_on_the_wrong_track_fails_to_deserialize() {
        let wrong = |track: &str, key: serde_json::Value| {
            serde_json::from_value::<SequenceDoc>(
                json!({ "length_ticks": 9, "tracks": { track: [key] } }),
            )
        };
        assert!(wrong("event", json!({ "at": 0, "name": "e", "wait": "line_end" })).is_err());
        assert!(
            wrong(
                "dialogue",
                json!({ "at": 0, "line": "l", "wait": "release" })
            )
            .is_err()
        );
    }

    #[test]
    fn nested_validators_report_under_the_key_pointer() {
        let out = run(
            json!({ "roles": ["p"], "length_ticks": 9, "tracks": { "ui": [{ "at": 0, "role": "p",
                "tracks": [{ "field": "opacity", "from": [0.0, 1.0], "to": [1.0] }] }] } }),
            true,
        );
        assert_eq!(codes(&out), ["L1610"]);
        assert_eq!(
            out[0].path.as_deref(),
            Some("sequence/sequences/t.yaml/tracks/ui/0/tracks/0/from")
        );
    }
}
