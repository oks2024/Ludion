//! Animation clip documents: `anims/*.anim.yaml` — one keyframed clip per file
//! (`Plan/voxel-animation.md` decision 8).
//!
//! A clip animates named targets — the parts of a `kind: assembly`, or the joints of a skin — by
//! keyframed transforms: `duration` (seconds), `loop` (a one-shot holds its final pose — that
//! semantics is the runtime's concern, not this crate's), and `tracks` — a mapping from target
//! name to `rotation`/`translation`/`scale` keyframe channels. `rotation` values are Euler XYZ
//! degrees, the same convention `Transform` uses; `translation` values are part-local voxel units;
//! `scale` values are per-axis multipliers, strictly positive. The binding from a track's target
//! name to a real part or joint happens later, in whatever plays the clip — this module validates
//! a clip in isolation, the way [`crate::part`] validates a part in isolation from its owning
//! assembly. A clip may also carry `events` (named instants, `{t, name}`, strictly increasing)
//! and `generated_from` (the source path of an imported clip, which then keeps its events in the
//! character overlay instead). Playback does not apply `scale` or deliver `events`.
//!
//! **Block-style keyframes, not flow-style.** `Plan/voxel-animation.md`'s reference sketch writes
//! a keyframe in YAML flow style (`{ t: 0.0, v: [25, 0, 0] }`), but the crate's strict-YAML subset
//! rejects non-empty flow collections (`L1005`, see [`crate::yaml`]'s module docs) — so the
//! canonical, and only accepted, spelling is block style, one field per line:
//!
//! ```yaml
//! format: 1
//! kind: animation
//! name: humanoid-walk
//! duration: 0.6
//! loop: true
//! events:
//!   - t: 0.3
//!     name: footstep
//! tracks:
//!   upper-leg-l:
//!     rotation:
//!       - t: 0.0
//!         v:
//!           - 25.0
//!           - 0.0
//!           - 0.0
//!       - t: 0.3
//!         v:
//!           - -25.0
//!           - 0.0
//!           - 0.0
//!         ease: smooth
//!       - t: 0.6
//!         v:
//!           - 25.0
//!           - 0.0
//!           - 0.0
//! ```
//!
//! This module owns clip *structure* (shape/type checking, `L1141`) and *data consistency*
//! (finite positive duration, non-empty tracks, every track animating at least one channel,
//! finite in-range keyframe times, strictly increasing times within a channel, positive scale;
//! `L1142`) and *event data* (`L1198`) — the
//! `L1113`/`L1114` and `L1139`/`L1140` pattern, extended to the clip family. It reuses the crate's
//! shared strict-YAML layer ([`crate::yaml`]) and the same canonical-writer primitives
//! ([`crate::scalar`]) every other document family uses, so a clip document is byte-idempotent
//! canonical text for the same reasons.
//!
//! An empty channel and an absent one are the same [`Vec`] on a parsed [`AnimationDocument`]
//! (`empty` means `absent`, per the field docs), so "a channel key was authored with zero
//! keyframes" is a fact only the raw YAML tree can see — this module raises that specific
//! diagnostic while the tree is still in hand, during [`parse`], tagged `L1142` (data
//! consistency) even though it runs inside the structure pass — `events: []` is the same fact for
//! `L1198`. [`validate`] still independently catches the coarser "no channel has anything in it"
//! case from the parsed document alone, so a programmatically-built [`AnimationDocument`] (never touched by [`parse`]) is still fully
//! checked.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::math::f64::{acos, euler_xyz_degrees_to_quat};
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::fields::parse_number;
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue, pointer_token};

/// The animation document format version this crate reads and writes.
pub const ANIMATION_FORMAT_VERSION: u64 = 1;

const ANIMATION_KEYS: [&str; 8] = [
    "format",
    "kind",
    "name",
    "generated_from",
    "duration",
    "loop",
    "events",
    "tracks",
];
const TRACK_KEYS: [&str; 3] = ["rotation", "translation", "scale"];
const EVENT_KEYS: [&str; 2] = ["t", "name"];
const KEYFRAME_KEYS: [&str; 3] = ["t", "v", "ease"];
const EASE_VALUES: [&str; 3] = ["linear", "smooth", "step"];

/// Per-axis interpolation between two keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Ease {
    /// Straight linear interpolation. The default — an explicit `ease: linear` in the source
    /// parses to this variant too, and `emit` canonicalizes both spellings to the key's absence
    /// (one spelling per meaning).
    #[default]
    Linear,
    /// Smoothstep (ease-in/ease-out).
    Smooth,
    /// No interpolation: the value holds at the first keyframe until the second is reached.
    Step,
}

impl Ease {
    fn as_str(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::Smooth => "smooth",
            Ease::Step => "step",
        }
    }

    fn parse(text: &str) -> Option<Ease> {
        match text {
            "linear" => Some(Ease::Linear),
            "smooth" => Some(Ease::Smooth),
            "step" => Some(Ease::Step),
            _ => None,
        }
    }
}

/// One keyframe of a channel: a time, a value, and the easing used to reach it from the previous
/// keyframe (ignored on the channel's first keyframe).
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    /// Time in seconds, within `[0, duration]` of the owning clip.
    pub t: f64,
    /// The channel's value at `t`: Euler XYZ degrees for a `rotation` channel, part-local voxel
    /// units for a `translation` channel, per-axis multipliers for a `scale` channel.
    pub v: [f64; 3],
    /// The easing used approaching this keyframe from the previous one in its channel.
    pub ease: Ease,
}

impl Keyframe {
    /// Whether the rotation segment from `self` to `next` plays as the Euler values are written.
    /// Playback slerps the shortest way round, so it differs when an axis turns half a circle or
    /// more, or when the two quaternions' raw dot is not positive (the lift of `next` then lies in
    /// the opposite hemisphere and the written path winds the long way). A `step` destination or a
    /// non-finite value is not a turn at all and counts as fine.
    pub fn turns_as_written(&self, next: &Keyframe) -> bool {
        if next.ease == Ease::Step || self.v.iter().chain(&next.v).any(|c| !c.is_finite()) {
            return true;
        }
        let per_axis = (0..3).all(|axis| (next.v[axis] - self.v[axis]).abs() < 180.0);
        per_axis && self.rotation_dot(next) > 0.0
    }

    /// The raw component-wise dot of the two keyframes' rotations as quaternions.
    fn rotation_dot(&self, next: &Keyframe) -> f64 {
        let (a, b) = (
            euler_xyz_degrees_to_quat(self.v),
            euler_xyz_degrees_to_quat(next.v),
        );
        (0..4).map(|k| a[k] * b[k]).sum()
    }
}

/// One target's animated channels within a clip.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTrack {
    /// The target — an assembly part or a skin joint — resolved by whatever plays the clip; this
    /// crate never sees the assembly or the skin.
    pub target: String,
    /// The rotation channel's keyframes, time-ascending as authored. An empty `Vec` means the
    /// channel is absent (a track's raw YAML may still have authored `rotation: []` — see the
    /// module docs on why that distinction cannot survive into this field).
    pub rotation: Vec<Keyframe>,
    /// The translation channel's keyframes, time-ascending as authored. Same absence convention as
    /// [`Self::rotation`].
    pub translation: Vec<Keyframe>,
    /// The scale channel's keyframes: per-axis multipliers, strictly positive. Same absence
    /// convention as [`Self::rotation`].
    pub scale: Vec<Keyframe>,
}

/// A named instant within a clip, delivered by playback when its time passes.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationEvent {
    /// Time in seconds, within `[0, duration]` of the owning clip.
    pub t: f64,
    /// The event name a game reacts to. Non-empty on a `validate`-clean document.
    pub name: String,
}

/// A parsed, structurally valid animation clip document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AnimationDocument {
    /// Optional human-readable clip name.
    pub name: Option<String>,
    /// The source path of an imported clip. A derived clip is never hand-edited, so it carries no
    /// `events` (they live in the character's clip overlay).
    pub generated_from: Option<String>,
    /// The clip length in seconds. Finite and strictly positive on a `validate`-clean document.
    pub duration: f64,
    /// Whether the clip loops. No default — canonical text always states it explicitly. A
    /// one-shot (`false`) holding its final pose past `duration` is runtime behavior this crate
    /// does not implement.
    pub looped: bool,
    /// Named instants, time-strictly-ascending as authored. Empty means absent.
    pub events: Vec<AnimationEvent>,
    /// Tracks, sorted by target name (the canonical order `emit` writes). Duplicate target names in
    /// the source are already rejected by the shared YAML layer's duplicate-key check (`L1007`)
    /// before this module ever sees them.
    pub tracks: Vec<AnimationTrack>,
}

/// Parses strict-subset YAML into an [`AnimationDocument`], checking structure/shape (`L1141`).
///
/// Also raises the diagnostics that only the raw YAML tree can answer — a channel key authored
/// with zero keyframes (`L1142`), `events: []` (`L1198`; see the module docs) — since by the time
/// this returns, those facts are no longer representable in the parsed document.
pub(crate) fn parse(source: &str) -> Result<AnimationDocument, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                "the animation document is empty; a clip needs at least `format: 1`, \
                 `kind: animation`, `duration`, `loop`, and `tracks`",
            )
            .with_path("")
            .with_help(
                "a minimal clip is:\n  format: 1\n  kind: animation\n  duration: 1.0\n  \
                 loop: false\n  tracks:\n    target-name:\n      rotation:\n        - t: 0.0\n     \
                 \x20    v:\n            - 0.0\n            - 0.0\n            - 0.0",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "the animation document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: animation`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = AnimationDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut duration_seen = false;
    let mut loop_seen = false;
    let mut tracks_seen = false;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    ANIMATION_FORMAT_VERSION,
                    "the animation document",
                    "clip",
                    "animation",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    "animation",
                    "an animation document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the clip name",
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "`generated_from`",
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "events" => {
                doc.events = parse_events(
                    &entry.node,
                    "/events",
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
                if doc.events.is_empty() && matches!(entry.node.value, NodeValue::Sequence(_)) {
                    diagnostics.push(
                        Diagnostic::new(
                            code::ANIMATION_EVENT_INVALID,
                            "`events` is declared but has zero entries",
                        )
                        .with_path("/events")
                        .with_help(
                            "add at least one `{t, name}` event, or remove the `events` key",
                        ),
                    );
                }
            }
            "duration" => {
                duration_seen = true;
                if let Some(duration) = parse_duration(entry, &mut diagnostics) {
                    doc.duration = duration;
                }
            }
            "loop" => {
                loop_seen = true;
                if let Some(looped) = parse_loop(entry, &mut diagnostics) {
                    doc.looped = looped;
                }
            }
            "tracks" => {
                tracks_seen = true;
                doc.tracks = parse_tracks(entry, &mut diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "an animation document",
                ANIMATION_KEYS,
                code::ANIMATION_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: animation`"));
    }
    if !duration_seen {
        diagnostics.push(missing_key(
            "duration",
            "add `duration`: the clip length in seconds",
        ));
    }
    if !loop_seen {
        diagnostics.push(missing_key(
            "loop",
            "add `loop: true` (a looping clip) or `loop: false` (a one-shot)",
        ));
    }
    if !tracks_seen {
        diagnostics.push(missing_key(
            "tracks",
            "add `tracks`: a mapping of target name to animated channels",
        ));
    }

    if diagnostics.is_empty() {
        Ok(doc)
    } else {
        Err(diagnostics)
    }
}

/// Parses `duration`: any number. Finiteness and positivity are `L1142` data-consistency concerns,
/// checked later by [`validate`].
fn parse_duration(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<f64> {
    match &entry.node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    format!(
                        "`duration` must be a number, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path("/duration")
                .with_help("write `duration` as the clip length in seconds, e.g. `duration: 0.6`"),
            );
            None
        }
    }
}

/// Parses `loop`: a boolean, required with no default (canonical text is always explicit).
fn parse_loop(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Option<bool> {
    match &entry.node.value {
        NodeValue::Scalar(Value::Bool(v)) => Some(*v),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    format!(
                        "`loop` must be a boolean, found {} (line {})",
                        other.type_phrase(),
                        entry.node.line
                    ),
                )
                .with_path("/loop")
                .with_help("write `loop: true` for a looping clip or `loop: false` for a one-shot"),
            );
            None
        }
    }
}

/// Parses `tracks`: a mapping of target name to a track. Returns the tracks in sorted-target-name
/// order (the canonical order [`emit`] writes); a duplicate target name is already rejected as
/// `L1007` by the shared YAML layer before this function runs.
fn parse_tracks(entry: &MapEntry, diagnostics: &mut Vec<Diagnostic>) -> Vec<AnimationTrack> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "`tracks` must be a mapping of target name to animated channels, found {} \
                     (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/tracks")
            .with_help(
                "write `tracks` as target names (parts or joints) mapping to \
                 `rotation`/`translation`/`scale` keyframe lists:\n  tracks:\n    torso:\n      rotation:\n        - t: 0.0\n          \
                 v:\n            - 0.0\n            - 0.0\n            - 0.0",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/tracks/{}", pointer_token(&sub.key));
        out.push(parse_track(
            &sub.node,
            &pointer,
            sub.key.clone(),
            diagnostics,
        ));
    }
    out.sort_by(|a, b| a.target.cmp(&b.target));
    out
}

/// Parses one track: a mapping with optional `rotation`/`translation`/`scale` keyframe-channel
/// keys.
///
/// Raises `L1142` directly (see the module docs) for the two facts only visible here, before the
/// channel keys collapse into the returned [`AnimationTrack`]'s plain `Vec`s: a channel key
/// authored with zero keyframes, and no channel key present at all.
fn parse_track(
    node: &Node,
    pointer: &str,
    target: String,
    diagnostics: &mut Vec<Diagnostic>,
) -> AnimationTrack {
    let mut track = AnimationTrack {
        target: target.clone(),
        rotation: Vec::new(),
        translation: Vec::new(),
        scale: Vec::new(),
    };
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "a track must be a mapping with `rotation`, `translation` and/or `scale`, \
                     found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("give the target a `rotation`, `translation` and/or `scale` keyframe list"),
        );
        return track;
    };
    let mut rotation_seen = false;
    let mut translation_seen = false;
    let mut scale_seen = false;
    for sub in entries {
        match sub.key.as_str() {
            "rotation" => {
                rotation_seen = true;
                let channel_pointer = format!("{pointer}/rotation");
                track.rotation = parse_channel(&sub.node, &channel_pointer, diagnostics);
                if track.rotation.is_empty() && matches!(sub.node.value, NodeValue::Sequence(_)) {
                    diagnostics.push(empty_channel(&channel_pointer, "rotation"));
                }
            }
            "translation" => {
                translation_seen = true;
                let channel_pointer = format!("{pointer}/translation");
                track.translation = parse_channel(&sub.node, &channel_pointer, diagnostics);
                if track.translation.is_empty() && matches!(sub.node.value, NodeValue::Sequence(_))
                {
                    diagnostics.push(empty_channel(&channel_pointer, "translation"));
                }
            }
            "scale" => {
                scale_seen = true;
                let channel_pointer = format!("{pointer}/scale");
                track.scale = parse_channel(&sub.node, &channel_pointer, diagnostics);
                if track.scale.is_empty() && matches!(sub.node.value, NodeValue::Sequence(_)) {
                    diagnostics.push(empty_channel(&channel_pointer, "scale"));
                }
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a track",
                TRACK_KEYS,
                code::ANIMATION_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if !rotation_seen && !translation_seen && !scale_seen {
        diagnostics.push(no_channel(&target, pointer));
    }
    track
}

fn no_channel(target: &str, pointer: &str) -> Diagnostic {
    Diagnostic::new(
        code::ANIMATION_DATA_CONSISTENCY,
        format!(
            "the track for target `{target}` has none of the `rotation`, `translation` or \
             `scale` channels"
        ),
    )
    .with_path(pointer.to_string())
    .with_help("add at least one of `rotation`/`translation`/`scale`, or remove the track")
}

fn empty_channel(pointer: &str, channel: &str) -> Diagnostic {
    Diagnostic::new(
        code::ANIMATION_DATA_CONSISTENCY,
        format!("the `{channel}` channel is declared but has zero keyframes"),
    )
    .with_path(pointer.to_string())
    .with_help(format!(
        "add at least one keyframe to `{channel}`, or remove the empty key"
    ))
}

/// Parses a channel: a sequence of keyframes.
fn parse_channel(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Vec<Keyframe> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "a channel must be a sequence of keyframes, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write one keyframe per list item:\n  - t: 0.0\n    v:\n      - 0.0\n      - \
                 0.0\n      - 0.0",
            ),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_pointer = format!("{pointer}/{index}");
        if let Some(keyframe) = parse_keyframe(item, &item_pointer, diagnostics) {
            out.push(keyframe);
        }
    }
    out
}

/// Parses `events`: a sequence of `{t, name}` mappings at `pointer`, with structure problems raised
/// under `structure` (a clip's `L1141`, a character overlay's `L1199`).
pub(crate) fn parse_events(
    node: &Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<AnimationEvent> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "`events` must be a sequence of `{{t, name}}` events, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write one event per list item:\n  events:\n    - t: 0.3\n      name: hit"),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if let Some(event) =
            parse_event(item, &format!("{pointer}/{index}"), structure, diagnostics)
        {
            out.push(event);
        }
    }
    out
}

/// Parses one event: a mapping with required `t` and `name`.
fn parse_event(
    node: &Node,
    pointer: &str,
    structure: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AnimationEvent> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                structure,
                format!(
                    "an event must be a mapping with `t` and `name`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write an event as `t` and `name`:\n  - t: 0.3\n    name: hit"),
        );
        return None;
    };
    let mut t = None;
    let mut t_seen = false;
    let mut name = None;
    let mut name_seen = false;
    for sub in entries {
        match sub.key.as_str() {
            "t" => {
                t_seen = true;
                t = parse_number(
                    &sub.node,
                    &format!("{pointer}/t"),
                    "t",
                    structure,
                    diagnostics,
                );
            }
            "name" => {
                name_seen = true;
                name = envelope::expect_string(
                    &sub.node,
                    &format!("{pointer}/name"),
                    "the event name",
                    structure,
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown, sub, pointer, "an event", EVENT_KEYS, structure,
            )),
        }
    }
    if !t_seen {
        diagnostics.push(
            Diagnostic::new(structure, "an event is missing its required `t`")
                .with_path(pointer.to_string())
                .with_help("add `t`: the event's time in seconds"),
        );
    }
    if !name_seen {
        diagnostics.push(
            Diagnostic::new(structure, "an event is missing its required `name`")
                .with_path(pointer.to_string())
                .with_help("add `name`: the event name a game reacts to"),
        );
    }
    match (t, name) {
        (Some(t), Some(name)) => Some(AnimationEvent { t, name }),
        _ => None,
    }
}

/// Parses one keyframe: a mapping with required `t`/`v` and optional `ease`.
fn parse_keyframe(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Keyframe> {
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "a keyframe must be a mapping with `t` and `v`, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help(
                "write a keyframe as `t`, `v`, and an optional `ease`:\n  - t: 0.0\n    v:\n      \
                 - 0.0\n      - 0.0\n      - 0.0",
            ),
        );
        return None;
    };
    let mut t = None;
    let mut t_seen = false;
    let mut v = None;
    let mut v_seen = false;
    let mut ease = Ease::Linear;
    for sub in entries {
        match sub.key.as_str() {
            "t" => {
                t_seen = true;
                t = parse_number(
                    &sub.node,
                    &format!("{pointer}/t"),
                    "t",
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    diagnostics,
                );
            }
            "v" => {
                v_seen = true;
                v = parse_v(&sub.node, &format!("{pointer}/v"), diagnostics);
            }
            "ease" => {
                ease = parse_ease(&sub.node, &format!("{pointer}/ease"), diagnostics)
                    .unwrap_or(Ease::Linear);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                sub,
                pointer,
                "a keyframe",
                KEYFRAME_KEYS,
                code::ANIMATION_DOCUMENT_STRUCTURE,
            )),
        }
    }
    if !t_seen {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                "a keyframe is missing its required `t`",
            )
            .with_path(pointer.to_string())
            .with_help("add `t`: the keyframe's time in seconds"),
        );
    }
    if !v_seen {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                "a keyframe is missing its required `v`",
            )
            .with_path(pointer.to_string())
            .with_help("add `v`: three numbers, [x, y, z], one per line"),
        );
    }
    match (t, v) {
        (Some(t), Some(v)) => Some(Keyframe { t, v, ease }),
        _ => None,
    }
}

/// Parses `v`: a sequence of exactly three numbers.
fn parse_v(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<[f64; 3]> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "`v` must be a sequence of three numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer.to_string())
            .with_help("write three numbers (x, y, z), one per line"),
        );
        return None;
    };
    if items.len() != 3 {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!(
                    "`v` must have exactly 3 components (x, y, z), found {}",
                    items.len()
                ),
            )
            .with_path(pointer.to_string())
            .with_help("supply exactly three numbers"),
        );
        return None;
    }
    let mut out = [0.0f64; 3];
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out[index] = crate::fields::number(n),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_DOCUMENT_STRUCTURE,
                        format!(
                            "`v[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}"))
                    .with_help("each component of `v` must be a number"),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses `ease`: a string, one of [`EASE_VALUES`].
fn parse_ease(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<Ease> {
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => match Ease::parse(s) {
            Some(ease) => Some(ease),
            None => {
                let mut diagnostic = Diagnostic::new(
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    format!("`ease` value `{s}` is not recognized (line {})", node.line),
                )
                .with_path(pointer.to_string());
                diagnostic = match suggest::nearest(s, EASE_VALUES) {
                    Some(candidate) => {
                        diagnostic.with_help(format!("nearest match is `{candidate}`"))
                    }
                    None => diagnostic
                        .with_help(format!("valid values are: {}", EASE_VALUES.join(", "))),
                };
                diagnostics.push(diagnostic);
                None
            }
        },
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_DOCUMENT_STRUCTURE,
                    format!(
                        "`ease` must be a string, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!("write one of: {}", EASE_VALUES.join(", "))),
            );
            None
        }
    }
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(
        key,
        help,
        code::ANIMATION_DOCUMENT_STRUCTURE,
        "animation document",
    )
}

/// Validates data-consistency invariants of an already-parsed [`AnimationDocument`] (`L1142`):
/// `duration` is finite and positive, `tracks` is non-empty, every track animates at least one
/// channel, every keyframe's `t`/`v` are finite with `t` inside `[0, duration]`, and each
/// channel's keyframe times strictly increase, `scale` components are positive (`L1142`); events
/// are valid instants with a name, strictly increasing, on a non-derived clip (`L1198`).
///
/// Independent of how the document was built — a hand-authored one via [`parse`], or one assembled
/// programmatically — so this is the one place those invariants are enforced (the "declared
/// channel with zero keyframes" case is the one exception: see the module docs for why only
/// [`parse`] can see it).
pub(crate) fn validate(doc: &AnimationDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if !doc.duration.is_finite() || doc.duration <= 0.0 {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DATA_CONSISTENCY,
                format!(
                    "`duration` must be a finite number greater than zero, found {}",
                    doc.duration
                ),
            )
            .with_path("/duration")
            .with_help("set `duration` to the clip's length in seconds, e.g. `duration: 0.6`"),
        );
    }
    if doc.tracks.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DATA_CONSISTENCY,
                "a clip must animate at least one target",
            )
            .with_path("/tracks")
            .with_help("add at least one entry under `tracks`, or delete this clip"),
        );
    }
    for track in &doc.tracks {
        let pointer = format!("/tracks/{}", pointer_token(&track.target));
        if track.rotation.is_empty() && track.translation.is_empty() && track.scale.is_empty() {
            diagnostics.push(no_channel(&track.target, &pointer));
        }
        validate_channel(
            &track.rotation,
            &format!("{pointer}/rotation"),
            doc.duration,
            &mut diagnostics,
        );
        validate_rotation_turns(
            &track.rotation,
            &format!("{pointer}/rotation"),
            &mut diagnostics,
        );
        validate_channel(
            &track.translation,
            &format!("{pointer}/translation"),
            doc.duration,
            &mut diagnostics,
        );
        validate_channel(
            &track.scale,
            &format!("{pointer}/scale"),
            doc.duration,
            &mut diagnostics,
        );
        for (index, keyframe) in track.scale.iter().enumerate() {
            for (axis, component) in keyframe.v.iter().enumerate() {
                if component.is_finite() && *component <= 0.0 {
                    diagnostics.push(
                        Diagnostic::new(
                            code::ANIMATION_DATA_CONSISTENCY,
                            format!(
                                "keyframe `v[{axis}]` of a `scale` channel must be greater than \
                                 zero, found {}",
                                format_float(*component)
                            ),
                        )
                        .with_path(format!("{pointer}/scale/{index}/v/{axis}"))
                        .with_help("scale is a per-axis multiplier; use a positive number"),
                    );
                }
            }
        }
    }
    validate_events(doc, &mut diagnostics);
    diagnostics
}

/// Validates `events` (`L1198`): finite `t` inside `[0, duration]` (below it on a looping clip), strictly increasing, a
/// non-blank `name`, and none on a derived (`generated_from`) clip.
fn validate_events(doc: &AnimationDocument, diagnostics: &mut Vec<Diagnostic>) {
    if doc.generated_from.is_some() && !doc.events.is_empty() {
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_EVENT_INVALID,
                "a clip with `generated_from` is derived and must not carry `events`",
            )
            .with_path("/events")
            .with_help(
                "a derived clip's events live in the character's clip overlay; add them there",
            ),
        );
    }
    validate_event_list(
        &doc.events,
        "/events",
        Some(doc.duration),
        doc.looped,
        diagnostics,
    );
}

/// Validates an event list at `base` (`L1198`): finite `t`, strictly increasing, a non-blank `name`,
/// and `t` inside `[0, duration]` when a `duration` bound is given (a character overlay has none
/// until it is checked against its clip). On a `looped` clip an event at `t == duration` is refused:
/// that instant is the loop's 0, so it would fire twice at the seam.
pub(crate) fn validate_event_list(
    events: &[AnimationEvent],
    base: &str,
    duration: Option<f64>,
    looped: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut previous: Option<f64> = None;
    for (index, event) in events.iter().enumerate() {
        let pointer = format!("{base}/{index}");
        if !event.t.is_finite() {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_EVENT_INVALID,
                    format!("event `t` must be finite, found {}", event.t),
                )
                .with_path(format!("{pointer}/t"))
                .with_help("use a finite number of seconds"),
            );
        } else {
            match duration {
                Some(duration) if event.t < 0.0 || event.t > duration => diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!(
                            "event `t` = {} is outside the clip's [0, {}] duration",
                            format_float(event.t),
                            format_float(duration)
                        ),
                    )
                    .with_path(format!("{pointer}/t"))
                    .with_help("keep every event's `t` within [0, duration]"),
                ),
                Some(duration) if looped && event.t == duration => diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!(
                            "event `t` = {} is at the end of a looping clip, which is the same \
                             instant as its start",
                            format_float(event.t)
                        ),
                    )
                    .with_path(format!("{pointer}/t"))
                    .with_help("a loop's duration is its 0: move the event to `t: 0`"),
                ),
                None if event.t < 0.0 => diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!("event `t` = {} is negative", format_float(event.t)),
                    )
                    .with_path(format!("{pointer}/t"))
                    .with_help("an event's `t` is seconds from the start of the clip"),
                ),
                _ => {}
            }
            if let Some(prev) = previous
                && event.t <= prev
            {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_EVENT_INVALID,
                        format!(
                            "event times must be strictly increasing; `t` = {} does not exceed \
                             the previous event's {}",
                            format_float(event.t),
                            format_float(prev)
                        ),
                    )
                    .with_path(format!("{pointer}/t"))
                    .with_help("reorder or renumber the events so `t` strictly increases"),
                );
            }
            previous = Some(event.t);
        }
        if event.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_EVENT_INVALID,
                    "an event `name` must not be empty",
                )
                .with_path(format!("{pointer}/name"))
                .with_help("name the event, e.g. `name: footstep`"),
            );
        }
        if event.name == "end" {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_EVENT_INVALID,
                    "the event name `end` is reserved for the implicit end of a one-shot clip",
                )
                .with_path(format!("{pointer}/name"))
                .with_help(
                    "`end` is emitted by the engine when a one-shot finishes; rename this event",
                ),
            );
        }
    }
}

/// Refuses a rotation segment that turns half a circle or more on an Euler axis (`L1142`): the
/// sampler slerps the shortest way round, so such a segment plays as a different turn than written.
/// A `step` destination jumps instead of turning; a non-finite value is `validate_channel`'s.
fn validate_rotation_turns(
    keyframes: &[Keyframe],
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, pair) in keyframes.windows(2).enumerate() {
        let (from, to) = (&pair[0], &pair[1]);
        if to.ease == Ease::Step || from.v.iter().chain(&to.v).any(|c| !c.is_finite()) {
            continue;
        }
        let Some((axis, turn)) = (0..3)
            .map(|axis| (axis, (to.v[axis] - from.v[axis]).abs()))
            .find(|&(_, turn)| turn >= 180.0)
        else {
            let dot = from.rotation_dot(to);
            if dot <= 0.0 {
                diagnostics.push(combined_turn_diagnostic(from, to, dot, pointer, index));
            }
            continue;
        };
        let segments = (turn / 180.0).floor() as u32 + 1;
        diagnostics.push(
            Diagnostic::new(
                code::ANIMATION_DATA_CONSISTENCY,
                format!(
                    "rotation turns {} degrees on axis {} between two keyframes, but a \
                     segment plays the short way round (a full turn plays as none)",
                    format_float(turn),
                    ["x", "y", "z"][axis]
                ),
            )
            .with_path(format!("{pointer}/{}/v/{axis}", index + 1))
            .with_help(format!(
                "split the segment; rotation takes the shortest path, so use at least {segments} \
                 segments, each under 180 degrees per axis"
            )),
        );
    }
}

/// The `L1142` for a segment whose axes each turn under half a circle but whose combined rotation
/// does not (`dot ≤ 0`): it plays as a `2·acos|dot|` turn, not the one written.
fn combined_turn_diagnostic(
    from: &Keyframe,
    to: &Keyframe,
    dot: f64,
    pointer: &str,
    index: usize,
) -> Diagnostic {
    let played = 2.0 * acos(dot.abs().min(1.0)).to_degrees();
    let triple = |v: [f64; 3]| {
        format!(
            "({}, {}, {})",
            format_float(v[0]),
            format_float(v[1]),
            format_float(v[2])
        )
    };
    let segments = (2..=64u32).find(|&n| {
        let at = |k: u32| Keyframe {
            t: 0.0,
            v: std::array::from_fn(|a| {
                from.v[a] + (to.v[a] - from.v[a]) * f64::from(k) / f64::from(n)
            }),
            ease: Ease::Linear,
        };
        (0..n).all(|k| at(k).turns_as_written(&at(k + 1)))
    });
    let count = segments.map_or_else(String::new, |n| {
        format!(", at least {n} equal segments play as written")
    });
    Diagnostic::new(
        code::ANIMATION_DATA_CONSISTENCY,
        format!(
            "rotation from {} to {} degrees plays as a {played:.1} degree turn: its axes combine past \
             half a circle, and a segment plays the short way round",
            triple(from.v),
            triple(to.v),
        ),
    )
    .with_path(format!("{pointer}/{}/v", index + 1))
    .with_help(format!(
        "split the segment; rotation takes the shortest path, so key the Euler values in \
         between{count}"
    ))
}

/// Validates one channel's keyframes: finite `t`/`v`, `t` inside `[0, duration]`, and strictly
/// increasing `t` between consecutive keyframes.
fn validate_channel(
    keyframes: &[Keyframe],
    pointer: &str,
    duration: f64,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut previous: Option<f64> = None;
    for (index, keyframe) in keyframes.iter().enumerate() {
        let kf_pointer = format!("{pointer}/{index}");
        let t_finite = keyframe.t.is_finite();
        if !t_finite {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_DATA_CONSISTENCY,
                    format!("keyframe `t` must be finite, found {}", keyframe.t),
                )
                .with_path(format!("{kf_pointer}/t"))
                .with_help("use a finite number of seconds"),
            );
        } else if keyframe.t < 0.0 || keyframe.t > duration {
            diagnostics.push(
                Diagnostic::new(
                    code::ANIMATION_DATA_CONSISTENCY,
                    format!(
                        "keyframe `t` = {} is outside the clip's [0, {}] duration",
                        format_float(keyframe.t),
                        format_float(duration)
                    ),
                )
                .with_path(format!("{kf_pointer}/t"))
                .with_help("keep every keyframe's `t` within [0, duration]"),
            );
        }
        for (axis, component) in keyframe.v.iter().enumerate() {
            if !component.is_finite() {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_DATA_CONSISTENCY,
                        format!("keyframe `v[{axis}]` must be finite, found {component}"),
                    )
                    .with_path(format!("{kf_pointer}/v/{axis}"))
                    .with_help("use a finite number"),
                );
            }
        }
        if t_finite {
            if let Some(prev) = previous
                && keyframe.t <= prev
            {
                diagnostics.push(
                    Diagnostic::new(
                        code::ANIMATION_DATA_CONSISTENCY,
                        format!(
                            "keyframe times must be strictly increasing; `t` = {} does not \
                             exceed the previous keyframe's {}",
                            format_float(keyframe.t),
                            format_float(prev)
                        ),
                    )
                    .with_path(format!("{kf_pointer}/t"))
                    .with_help("reorder or renumber the keyframes so `t` strictly increases"),
                );
            }
            previous = Some(keyframe.t);
        }
    }
}

/// Writes `doc` in canonical form: `format`, `kind`, `name` and `generated_from` (optional),
/// `duration`, `loop`, `events` (optional), `tracks` — events in stored order, tracks sorted by
/// target name, `rotation`, `translation`, `scale` within a track, keyframes in stored order, `ease: linear` canonicalized to the key's absence.
pub(crate) fn emit(doc: &AnimationDocument) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: animation\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if let Some(source) = &doc.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(source));
        out.push('\n');
    }
    out.push_str("duration: ");
    out.push_str(&format_float(doc.duration));
    out.push('\n');
    out.push_str("loop: ");
    out.push_str(if doc.looped { "true" } else { "false" });
    out.push('\n');
    emit_events(&mut out, &doc.events, "");

    if doc.tracks.is_empty() {
        out.push_str("tracks: {}\n");
        return out;
    }
    out.push_str("tracks:\n");
    let mut tracks: Vec<&AnimationTrack> = doc.tracks.iter().collect();
    tracks.sort_by(|a, b| a.target.cmp(&b.target));
    for track in tracks {
        out.push_str("  ");
        out.push_str(&scalar_string(&track.target));
        out.push(':');
        if track.rotation.is_empty() && track.translation.is_empty() && track.scale.is_empty() {
            // Structurally legal (an empty mapping round-trips) even though `validate` rejects
            // it — the same reason `PartDocument`'s empty `slices` still emits `[]`.
            out.push_str(" {}\n");
            continue;
        }
        out.push('\n');
        if !track.rotation.is_empty() {
            out.push_str("    rotation:\n");
            emit_channel(&mut out, &track.rotation);
        }
        if !track.translation.is_empty() {
            out.push_str("    translation:\n");
            emit_channel(&mut out, &track.translation);
        }
        if !track.scale.is_empty() {
            out.push_str("    scale:\n");
            emit_channel(&mut out, &track.scale);
        }
    }
    out
}

/// Writes an `events:` block indented by `indent`; nothing for an empty list.
pub(crate) fn emit_events(out: &mut String, events: &[AnimationEvent], indent: &str) {
    if events.is_empty() {
        return;
    }
    out.push_str(indent);
    out.push_str("events:\n");
    for event in events {
        out.push_str(indent);
        out.push_str("  - t: ");
        out.push_str(&format_float(event.t));
        out.push('\n');
        out.push_str(indent);
        out.push_str("    name: ");
        out.push_str(&scalar_string(&event.name));
        out.push('\n');
    }
}

fn emit_channel(out: &mut String, keyframes: &[Keyframe]) {
    for keyframe in keyframes {
        out.push_str("      - t: ");
        out.push_str(&format_float(keyframe.t));
        out.push('\n');
        out.push_str("        v:\n");
        for component in keyframe.v {
            out.push_str("          - ");
            out.push_str(&format_float(component));
            out.push('\n');
        }
        if keyframe.ease != Ease::Linear {
            out.push_str("        ease: ");
            out.push_str(keyframe.ease.as_str());
            out.push('\n');
        }
    }
}
