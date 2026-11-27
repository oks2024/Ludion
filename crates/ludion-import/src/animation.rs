//! The animation mapping: each glTF animation onto one `kind: animation` clip whose tracks are
//! named after skin joints. See `ludion devdocs import-animation` for the pipeline; the
//! short form is source frames → per-channel candidate keys (Euler unwrapped) → greedy key
//! reduction → a model-space measurement against the source that refuses a clip over its bound
//! (`L1206`); a clip that reduces to no tracks at all is refused (`L1208`).
//!
//! All arithmetic is `f64`, and every transcendental goes through `ludion_core::math::f64` — the
//! output bytes depend on these decisions, so they must match across machines. glam's `D*` types
//! are used for matrix arithmetic only.

use std::collections::{BTreeMap, BTreeSet};

use gltf::accessor::{DataType, Dimensions};
use gltf::animation::{Interpolation, Property};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::f64::{acos, atan2, cos, quat_to_euler_xyz_degrees, sin, sqrt};
use ludion_core::math::glam::{DMat3, DMat4, DQuat, DVec3};
use ludion_scene::{
    AnimationDocument, AnimationTrack, Ease, JointRest, Keyframe, ROOT_TRACK, sample_channel,
    sample_rotation,
};
use serde_json::Value;

use crate::accessors::check_accessor;
use crate::diagnostics::{
    channel_on_reframed_joint, clip_at_rest, clip_error_bound, clip_extras_invalid,
    cubic_too_curved, malformed, unsupported,
};
use crate::skin::SkinBinding;
use crate::sources::Buffers;
use crate::{CLIP_DIR, ClipArtifact, dedup_file_stem, named, sanitize};

const ROTATION: usize = 0;
const TRANSLATION: usize = 1;
const SCALE: usize = 2;
const CHANNEL_NAMES: [&str; 3] = ["rotation", "translation", "scale"];

/// Reduction and bound tolerances (decision 13 of `Plan/animation-clips.md`): rotation in degrees,
/// translation in mesh units, scale as a per-axis multiplier.
#[derive(Clone, Copy, Debug)]
struct Tolerances {
    reduce: [f64; 3],
    bound: [f64; 3],
}

const DEFAULT_TOLERANCES: Tolerances = Tolerances {
    reduce: [0.1, 0.0005, 0.001],
    bound: [0.5, 0.002, 0.005],
};

/// The `extras` keys that override a tolerance, in `[reduce, bound]` × channel order.
const TOLERANCE_KEYS: [(&str, bool, usize); 6] = [
    ("reduce_rotation_deg", false, ROTATION),
    ("reduce_translation", false, TRANSLATION),
    ("reduce_scale", false, SCALE),
    ("bound_rotation_deg", true, ROTATION),
    ("bound_translation", true, TRANSLATION),
    ("bound_scale", true, SCALE),
];

/// Key counts of one channel before and after reduction; both zero for a channel the clip lacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyCounts {
    /// Candidate keys, one per source frame.
    pub before: usize,
    /// Keys kept by reduction.
    pub after: usize,
}

/// One node of the hierarchy a clip animates: a skin joint or a prefab part. A rig is parent-first.
#[derive(Clone)]
struct RigNode {
    name: String,
    parent: Option<usize>,
    rest: JointRest,
}

/// What an animation's tracks are named after.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The joints of the skin worn by this mesh.
    Skin(usize),
    /// The prefab's parts.
    Parts,
}

/// The prefab parts as an animation rig: parent-first, plus each part's slot by glTF node.
struct PartRig {
    nodes: Vec<RigNode>,
    slot_of_node: BTreeMap<usize, usize>,
}

/// The emitted nodes of `document` parent-first (a depth-first walk from the roots in document
/// order, children in their listed order), each named by `part_names` (`None` for a node that is
/// no part: a skin joint, or a node under one).
fn part_rig(document: &gltf::Document, part_names: &[Option<String>]) -> PartRig {
    let nodes: Vec<gltf::Node> = document.nodes().collect();
    let mut is_child = vec![false; nodes.len()];
    for node in &nodes {
        for child in node.children() {
            is_child[child.index()] = true;
        }
    }
    let mut rig = PartRig {
        nodes: Vec::new(),
        slot_of_node: BTreeMap::new(),
    };
    let mut stack: Vec<(usize, Option<usize>)> = (0..nodes.len())
        .rev()
        .filter(|&index| !is_child[index])
        .map(|index| (index, None))
        .collect();
    while let Some((index, parent)) = stack.pop() {
        let Some(name) = &part_names[index] else {
            continue;
        };
        let (translation, rotation, scale) = nodes[index].transform().decomposed();
        let slot = rig.nodes.len();
        rig.nodes.push(RigNode {
            name: name.clone(),
            parent,
            rest: JointRest {
                translation: translation.map(f64::from),
                rotation: rotation.map(f64::from),
                scale: scale.map(f64::from),
            },
        });
        rig.slot_of_node.insert(index, slot);
        let children: Vec<usize> = nodes[index].children().map(|child| child.index()).collect();
        stack.extend(children.into_iter().rev().map(|child| (child, Some(slot))));
    }
    rig
}

/// One emitted track's key counts, in the order the tracks are emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackReport {
    /// The track's target (a joint or part name).
    pub target: String,
    /// The rotation channel.
    pub rotation: KeyCounts,
    /// The translation channel.
    pub translation: KeyCounts,
    /// The scale channel.
    pub scale: KeyCounts,
}

/// The worst model-space difference between the reduced clip and the source on one joint, and the
/// source frame (an index into [`ClipReport::frame_times`]) where each occurred.
#[derive(Clone, Debug, PartialEq)]
pub struct JointError {
    /// The joint's or part's name.
    pub joint: String,
    /// Worst rotation difference, degrees.
    pub rotation_deg: f64,
    /// Frame of [`Self::rotation_deg`].
    pub rotation_frame: usize,
    /// Worst translation distance, mesh units.
    pub translation: f64,
    /// Frame of [`Self::translation`].
    pub translation_frame: usize,
    /// Worst per-axis scale difference.
    pub scale: f64,
    /// Frame of [`Self::scale`].
    pub scale_frame: usize,
}

/// How far a looping clip's last pose is from its first: the worst model-space difference between
/// the reduced clip at `t = 0` and at `t = duration`, over every joint or part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeamError {
    /// Worst rotation difference, degrees.
    pub rotation_deg: f64,
    /// Worst translation distance, mesh units.
    pub translation: f64,
    /// Worst per-axis scale difference.
    pub scale: f64,
}

/// What reduction did to a clip and how close the result stays to the source.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipReport {
    /// The number of source frames the clip was measured at.
    pub source_frames: usize,
    /// The time of each source frame, seconds.
    pub frame_times: Vec<f64>,
    /// Key counts per emitted track.
    pub tracks: Vec<TrackReport>,
    /// The worst model-space error of every joint of the skin, or part of the prefab, in rig order.
    pub joints: Vec<JointError>,
    /// Present only for a `loop: true` clip.
    pub seam: Option<SeamError>,
}

struct Extras {
    looped: bool,
    /// The rig root, by emitted name, whose travel and yaw become the `$root` track.
    root: Option<String>,
    tolerances: Tolerances,
}

/// One glTF sampler read in full: input times (seconds, snapped to the shortest `f32` decimal) and
/// output values (`[x, y, z, w]`, `w` unused for vectors). A CUBICSPLINE sampler holds three values
/// per key: in-tangent, value, out-tangent.
struct SourceChannel {
    interpolation: Interpolation,
    times: Vec<f64>,
    values: Vec<[f64; 4]>,
}

/// One rig node's up-to-three source channels, indexed by [`ROTATION`], [`TRANSLATION`], [`SCALE`].
type JointChannels = [Option<SourceChannel>; 3];

/// Per rig node and channel, the source sampled at every frame (empty for an absent channel).
type SourceTable = Vec<[Vec<[f64; 4]>; 3]>;

enum Read {
    Channel(SourceChannel),
    /// The channel has no mapping; an `L1202` says why.
    Dropped(Diagnostic),
}

/// Imports every animation of `document` as a clip. `part_names` holds each node's prefab part
/// name (`None` for a node that is no part). `L1202` diagnostics go to `lossy`; the returned
/// errors (`L1201`, `L1206`, `L1207`, `L1208`) are never downgraded.
pub(crate) fn import_animations(
    document: &gltf::Document,
    bindings: &BTreeMap<usize, SkinBinding>,
    part_names: &[Option<String>],
    buffers: &Buffers,
    lossy: &mut Vec<Diagnostic>,
) -> Result<Vec<ClipArtifact>, Vec<Diagnostic>> {
    let mut joint_of_node: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (&mesh, binding) in bindings {
        for (slot, &node) in binding.joint_nodes.iter().enumerate() {
            joint_of_node
                .entry(node)
                .or_insert((mesh, binding.gltf_to_joint[slot] as usize));
        }
    }

    let parts = part_rig(document, part_names);

    let mut clips = Vec::new();
    let mut errors = Vec::new();
    let mut used = BTreeSet::new();
    for animation in document.animations() {
        let label = named("animation", animation.index(), animation.name());
        let extras = match parse_extras(&label, animation.extras()) {
            Ok(extras) => extras,
            Err(diagnostic) => {
                errors.push(diagnostic);
                continue;
            }
        };

        // The first surviving channel picks the target set (one skin, or the prefab's parts); a
        // channel of another set is dropped.
        let mut chosen: Option<(Target, Vec<JointChannels>)> = None;
        let mut failed = false;
        for channel in animation.channels() {
            let target = channel.target();
            let node = target.node();
            let offender = format!("{label}, {}", named("node", node.index(), node.name()));
            let kind = match target.property() {
                Property::Rotation => ROTATION,
                Property::Translation => TRANSLATION,
                Property::Scale => SCALE,
                Property::MorphTargetWeights => {
                    lossy.push(unsupported("a morph-weights animation channel", &offender));
                    continue;
                }
            };
            let (set, slot) = if let Some(&(mesh, joint)) = joint_of_node.get(&node.index()) {
                (Target::Skin(mesh), joint)
            } else if let Some(&slot) = parts.slot_of_node.get(&node.index()) {
                (Target::Parts, slot)
            } else {
                lossy.push(unsupported(
                    "an animation channel on a node that has no part (it hangs under a skin joint)",
                    &offender,
                ));
                continue;
            };
            if let Target::Skin(mesh) = set
                && bindings[&mesh].reframed[slot]
            {
                let binding = &bindings[&mesh];
                let name = &binding.joints[slot].name;
                let what = if binding.joints[slot].parent.is_some() {
                    format!(
                        "an animation channel on joint `{name}`, whose rest folds in the \
                         transform of the non-joint node above it"
                    )
                } else {
                    format!(
                        "an animation channel on root joint `{name}`, whose rest is \
                         re-expressed in the mesh frame"
                    )
                };
                lossy.push(channel_on_reframed_joint(&what, &offender));
                continue;
            }
            if let Some((held, _)) = &chosen
                && *held != set
            {
                lossy.push(unsupported(
                    match (held, set) {
                        (Target::Skin(_), Target::Skin(_)) => {
                            "an animation channel on a joint of a second skin"
                        }
                        (Target::Skin(_), Target::Parts) => {
                            "an animation channel on a prefab part in an animation of skin joints"
                        }
                        (Target::Parts, _) => {
                            "an animation channel on a skin joint in an animation of prefab parts"
                        }
                    },
                    &offender,
                ));
                continue;
            }
            let read = match read_channel(&channel, kind, &offender, buffers) {
                Ok(read) => read,
                Err(diagnostic) => {
                    errors.push(diagnostic);
                    failed = true;
                    continue;
                }
            };
            let source = match read {
                Read::Channel(source) => source,
                Read::Dropped(diagnostic) => {
                    lossy.push(diagnostic);
                    continue;
                }
            };
            let (_, nodes) = chosen.get_or_insert_with(|| {
                let count = match set {
                    Target::Skin(mesh) => bindings[&mesh].joints.len(),
                    Target::Parts => parts.nodes.len(),
                };
                (set, (0..count).map(|_| [None, None, None]).collect())
            });
            if nodes[slot][kind].replace(source).is_some() {
                errors.push(malformed(format!(
                    "{offender} is animated on its {} by two channels",
                    CHANNEL_NAMES[kind]
                )));
                failed = true;
            }
        }
        if failed {
            continue;
        }
        let Some((set, channels)) = chosen else {
            lossy.push(unsupported(
                "an animation with no importable channel",
                &label,
            ));
            continue;
        };
        let (rig, noun): (Vec<RigNode>, &str) = match set {
            Target::Skin(mesh) => (
                bindings[&mesh]
                    .joints
                    .iter()
                    .map(|joint| RigNode {
                        name: joint.name.clone(),
                        parent: joint.parent.map(|parent| parent as usize),
                        rest: joint.rest,
                    })
                    .collect(),
                "joint",
            ),
            Target::Parts => (parts.nodes.clone(), "part"),
        };

        let frames = match source_frames(&channels, &extras.tolerances) {
            Ok(frames) => frames,
            Err(miss) => {
                errors.push(cubic_too_curved(format!(
                    "{label}: the CUBICSPLINE {} channel of {noun} `{}` curves too sharply between \
                     t = {}s and t = {}s for the importer to follow within the reduce tolerance \
                     of {} (it subdivides an interval up to {MAX_CUBIC_STEPS} steps)",
                    CHANNEL_NAMES[miss.kind],
                    rig[miss.node].name,
                    miss.start,
                    miss.end,
                    extras.tolerances.reduce[miss.kind],
                )));
                continue;
            }
        };
        let duration = frames.last().copied().unwrap_or(0.0);
        if duration <= 0.0 {
            lossy.push(unsupported("a zero-length animation", &label));
            continue;
        }

        let source = sample_source(&channels, &frames);
        let mut table = source.clone();
        let mut root = None;
        if let Some(name) = &extras.root {
            let reframed = match set {
                Target::Skin(mesh) => Some(bindings[&mesh].reframed.as_slice()),
                Target::Parts => None,
            };
            let slot = match root_slot(&label, &rig, name, reframed) {
                Ok(slot) => slot,
                Err(diagnostic) => {
                    errors.push(diagnostic);
                    continue;
                }
            };
            root = extract_root(
                slot,
                &rig[slot].rest,
                &channels[slot],
                &mut table[slot],
                &frames,
                &extras.tolerances,
            );
        }
        let unyaw = root.as_ref().and_then(|root| root.unyaw.as_ref());
        let (mut tracks, mut counts) =
            reduce_tracks(&rig, &channels, &table, &frames, &extras.tolerances, unyaw);
        if let Some(root) = root {
            tracks.insert(0, root.track);
            counts.insert(0, root.counts);
        }
        let mut doc = AnimationDocument {
            name: None,
            generated_from: None,
            duration,
            looped: extras.looped,
            events: Vec::new(),
            tracks,
        };
        let mut report = measure(&rig, &source, &doc, &frames, extras.looped);
        report.tracks = counts;
        if let Some(diagnostic) = check_bound(&label, noun, &report, &extras.tolerances) {
            errors.push(diagnostic);
            continue;
        }
        // After the bound check: a clip whose real motion reduced away is `L1206`, not at rest.
        if doc.tracks.is_empty() {
            errors.push(clip_at_rest(&label));
            continue;
        }

        let base = match animation.name() {
            Some(name) if !name.is_empty() => sanitize(name),
            _ => format!("animation-{}", animation.index()),
        };
        let stem = dedup_file_stem(&mut used, &base, animation.index());
        doc.name = Some(stem.clone());
        clips.push(ClipArtifact {
            rel_path: format!("{CLIP_DIR}/{stem}.anim.yaml"),
            doc,
            report,
            binds_parts: set == Target::Parts,
        });
    }
    if errors.is_empty() {
        Ok(clips)
    } else {
        Err(errors)
    }
}

/// Reads an animation's `extras`: `loop`, `root_motion` and the six tolerance overrides.
fn parse_extras(label: &str, extras: &gltf::json::Extras) -> Result<Extras, Diagnostic> {
    let mut parsed = Extras {
        looped: false,
        root: None,
        tolerances: DEFAULT_TOLERANCES,
    };
    let Some(raw) = extras else {
        return Ok(parsed);
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw.get()) else {
        return Err(clip_extras_invalid(format!(
            "{label}'s `extras` is not a JSON object"
        )));
    };
    for (key, value) in &map {
        if key == "loop" {
            let Value::Bool(looped) = value else {
                return Err(clip_extras_invalid(format!(
                    "{label}'s extras key `loop` must be true or false, found {value}"
                )));
            };
            parsed.looped = *looped;
            continue;
        }
        if key == "root_motion" {
            let Some(name) = value.as_str().filter(|name| !name.is_empty()) else {
                return Err(clip_extras_invalid(format!(
                    "{label}'s extras key `root_motion` must name a joint, found {value}"
                )));
            };
            parsed.root = Some(name.to_string());
            continue;
        }
        let Some(&(_, is_bound, channel)) = TOLERANCE_KEYS.iter().find(|(name, ..)| name == key)
        else {
            return Err(clip_extras_invalid(format!(
                "{label}'s extras has the unknown key `{key}`"
            )));
        };
        let number = value.as_f64().filter(|v| v.is_finite() && *v > 0.0);
        let Some(number) = number else {
            return Err(clip_extras_invalid(format!(
                "{label}'s extras key `{key}` must be a finite positive number, found {value}"
            )));
        };
        let slot = if is_bound {
            &mut parsed.tolerances.bound
        } else {
            &mut parsed.tolerances.reduce
        };
        slot[channel] = number;
    }
    Ok(parsed)
}

const VEC3_F32: &[(Dimensions, DataType)] = &[(Dimensions::Vec3, DataType::F32)];
const SCALAR_F32: &[(Dimensions, DataType)] = &[(Dimensions::Scalar, DataType::F32)];
const ROTATION_TYPES: &[(Dimensions, DataType)] = &[
    (Dimensions::Vec4, DataType::F32),
    (Dimensions::Vec4, DataType::I8),
    (Dimensions::Vec4, DataType::U8),
    (Dimensions::Vec4, DataType::I16),
    (Dimensions::Vec4, DataType::U16),
];

/// Reads one channel's sampler. `offender` names the animation and node in every diagnostic.
fn read_channel(
    channel: &gltf::animation::Channel,
    kind: usize,
    offender: &str,
    buffers: &Buffers,
) -> Result<Read, Diagnostic> {
    let sampler = channel.sampler();
    let (input, output) = (sampler.input(), sampler.output());
    if input.sparse().is_some() || output.sparse().is_some() {
        return Ok(Read::Dropped(unsupported(
            "a sparse animation accessor",
            offender,
        )));
    }
    check_accessor(&input, SCALAR_F32, "animation input", offender, buffers)?;
    let output_types = if kind == ROTATION {
        ROTATION_TYPES
    } else {
        VEC3_F32
    };
    check_accessor(&output, output_types, "animation output", offender, buffers)?;

    let reader = channel.reader(|buffer| buffers.get(buffer.index()));
    let unreadable = || malformed(format!("{offender}: an animation buffer could not be read"));
    let mut times = Vec::with_capacity(input.count());
    for time in reader.read_inputs().ok_or_else(unreadable)? {
        if !time.is_finite() || time < 0.0 {
            return Err(malformed(format!(
                "{offender} has the animation input time {time}, which is not a finite value of at \
                 least 0"
            )));
        }
        // The shortest decimal of the f32 keeps the emitted text short and stable.
        let snapped = time
            .to_string()
            .parse::<f64>()
            .expect("a float prints as a float");
        if times.last().is_some_and(|&last| snapped <= last) {
            return Err(malformed(format!(
                "{offender} has animation input times that do not strictly increase (at {time})"
            )));
        }
        times.push(snapped);
    }

    let interpolation = sampler.interpolation();
    let cubic = interpolation == Interpolation::CubicSpline;
    let mut values: Vec<[f64; 4]> = match reader.read_outputs().ok_or_else(unreadable)? {
        gltf::animation::util::ReadOutputs::Translations(iter) if kind == TRANSLATION => iter
            .map(|v| [f64::from(v[0]), f64::from(v[1]), f64::from(v[2]), 0.0])
            .collect(),
        gltf::animation::util::ReadOutputs::Scales(iter) if kind == SCALE => iter
            .map(|v| [f64::from(v[0]), f64::from(v[1]), f64::from(v[2]), 0.0])
            .collect(),
        gltf::animation::util::ReadOutputs::Rotations(rotations) if kind == ROTATION => rotations
            .into_f32()
            .map(|v| {
                [
                    f64::from(v[0]),
                    f64::from(v[1]),
                    f64::from(v[2]),
                    f64::from(v[3]),
                ]
            })
            .collect(),
        _ => return Err(unreadable()),
    };
    let expected = times.len() * if cubic { 3 } else { 1 };
    if values.len() != expected {
        return Err(malformed(format!(
            "{offender} has {} animation input times but {} output values (expected {expected})",
            times.len(),
            values.len()
        )));
    }
    if values.iter().any(|v| v.iter().any(|c| !c.is_finite())) {
        return Err(malformed(format!(
            "{offender} has a non-finite animation output value"
        )));
    }
    if kind == ROTATION && !cubic {
        for value in &mut values {
            let len = norm4(*value);
            if len == 0.0 {
                return Err(malformed(format!(
                    "{offender} has a zero-length rotation quaternion"
                )));
            }
            *value = value.map(|c| c / len);
        }
    }
    Ok(Read::Channel(SourceChannel {
        interpolation,
        times,
        values,
    }))
}

fn norm4(v: [f64; 4]) -> f64 {
    sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3])
}

fn normalize4(v: [f64; 4]) -> [f64; 4] {
    let len = norm4(v);
    if len > 0.0 && len.is_finite() {
        v.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// The finest subdivision of a CUBICSPLINE interval: 4, 8, 16, 32 or 64 equal steps.
const MAX_CUBIC_STEPS: u32 = 64;

/// A CUBICSPLINE interval that 64 steps cannot follow within the reduce tolerance.
struct CubicMiss {
    node: usize,
    kind: usize,
    start: f64,
    end: f64,
}

/// The sorted, deduplicated union of every channel's input times. Reduction and the measurement
/// see only frames while the runtime interpolates between them, so each CUBICSPLINE interval gets
/// the fewest of 4, 8, ... 64 equal steps at which every step's chord midpoint is within the
/// channel's reduce tolerance of the spline; the clip then stays within that tolerance of what was
/// measured between frames too.
fn source_frames(
    channels: &[JointChannels],
    tolerances: &Tolerances,
) -> Result<Vec<f64>, CubicMiss> {
    let mut frames = Vec::new();
    for (node, node_channels) in channels.iter().enumerate() {
        for (kind, channel) in node_channels.iter().enumerate() {
            let Some(channel) = channel else { continue };
            let cubic = channel.interpolation == Interpolation::CubicSpline;
            for pair in channel.times.windows(2) {
                frames.push(pair[0]);
                if !cubic {
                    continue;
                }
                let tolerance = tolerances.reduce[kind];
                let at = |step: u32, steps: u32| {
                    pair[0] + (pair[1] - pair[0]) * f64::from(step) / f64::from(steps)
                };
                let mut steps = 4;
                while !(0..steps).all(|step| {
                    cubic_step_fits(
                        channel,
                        kind,
                        at(step, steps),
                        at(step + 1, steps),
                        tolerance,
                    )
                }) {
                    if steps == MAX_CUBIC_STEPS {
                        return Err(CubicMiss {
                            node,
                            kind,
                            start: pair[0],
                            end: pair[1],
                        });
                    }
                    steps *= 2;
                }
                frames.extend((1..steps).map(|step| at(step, steps)));
            }
            frames.push(
                *channel
                    .times
                    .last()
                    .expect("a channel has at least one key"),
            );
        }
    }
    frames.sort_by(f64::total_cmp);
    frames.dedup();
    Ok(frames)
}

/// Whether the chord midpoint of the source between `a` and `b` is within `tolerance` of the source
/// at the midpoint — the way the runtime will interpolate the two frames.
fn cubic_step_fits(channel: &SourceChannel, kind: usize, a: f64, b: f64, tolerance: f64) -> bool {
    let rotation = kind == ROTATION;
    let (start, end) = (
        eval_source(channel, rotation, a),
        eval_source(channel, rotation, b),
    );
    let chord = if rotation {
        normalize4(slerp(start, end, 0.5))
    } else {
        std::array::from_fn(|c| (start[c] + end[c]) * 0.5)
    };
    channel_error(kind, chord, eval_source(channel, rotation, (a + b) * 0.5)) <= tolerance
}

/// The source value of `channel` at `t` (per the glTF spec: clamped outside the keys).
fn eval_source(channel: &SourceChannel, rotation: bool, t: f64) -> [f64; 4] {
    let count = channel.times.len();
    let cubic = channel.interpolation == Interpolation::CubicSpline;
    let value = |i: usize| channel.values[if cubic { 3 * i + 1 } else { i }];
    let result = if t.is_nan() || t <= channel.times[0] {
        value(0)
    } else if t >= channel.times[count - 1] {
        value(count - 1)
    } else {
        let i = channel.times.partition_point(|&time| time <= t) - 1;
        let (t0, t1) = (channel.times[i], channel.times[i + 1]);
        let dt = t1 - t0;
        let u = (t - t0) / dt;
        match channel.interpolation {
            Interpolation::Step => value(i),
            Interpolation::Linear if rotation => slerp(value(i), value(i + 1), u),
            Interpolation::Linear => {
                let (a, b) = (value(i), value(i + 1));
                std::array::from_fn(|c| a[c] + (b[c] - a[c]) * u)
            }
            Interpolation::CubicSpline => {
                let (p0, p1) = (value(i), value(i + 1));
                let (m0, m1) = (channel.values[3 * i + 2], channel.values[3 * (i + 1)]);
                let (u2, u3) = (u * u, u * u * u);
                let (h00, h10) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u);
                let (h01, h11) = (-2.0 * u3 + 3.0 * u2, u3 - u2);
                std::array::from_fn(|c| {
                    h00 * p0[c] + h10 * dt * m0[c] + h01 * p1[c] + h11 * dt * m1[c]
                })
            }
        }
    };
    if rotation { normalize4(result) } else { result }
}

/// Shortest-path slerp of two unit quaternions (a normalized lerp when nearly parallel). Unlike
/// `ludion_scene::slerp` it does not normalize: the caller normalizes once, after interpolation, and
/// a second normalize moves last-ulp bits of the imported text.
fn slerp(a: [f64; 4], mut b: [f64; 4], u: f64) -> [f64; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        b = b.map(|c| -c);
        dot = -dot;
    }
    let (wa, wb) = if dot > 1.0 - 1e-9 {
        (1.0 - u, u)
    } else {
        let theta = acos(dot.min(1.0));
        let inv = 1.0 / sin(theta);
        (sin((1.0 - u) * theta) * inv, sin(u * theta) * inv)
    };
    std::array::from_fn(|c| wa * a[c] + wb * b[c])
}

fn sample_source(channels: &[JointChannels], frames: &[f64]) -> SourceTable {
    channels
        .iter()
        .map(|joint| {
            std::array::from_fn(|kind| match &joint[kind] {
                Some(channel) => frames
                    .iter()
                    .map(|&t| eval_source(channel, kind == ROTATION, t))
                    .collect(),
                None => Vec::new(),
            })
        })
        .collect()
}

/// The rig slot `root_motion` names. The `$root` track moves the whole entity, so only the rig's
/// one root can feed it, and only when its channels survive the import. `reframed` is the skin's
/// [`SkinBinding::reframed`], `None` for a rig of prefab parts.
fn root_slot(
    label: &str,
    rig: &[RigNode],
    name: &str,
    reframed: Option<&[bool]>,
) -> Result<usize, Diagnostic> {
    let refused = |why: String| {
        clip_extras_invalid(format!(
            "{label}'s extras key `root_motion` names `{name}`: {why}"
        ))
    };
    let roots: Vec<String> = rig
        .iter()
        .filter(|node| node.parent.is_none())
        .map(|node| format!("`{}`", node.name))
        .collect();
    let listed = roots.join(", ");
    let Some(slot) = rig.iter().position(|node| node.name == name) else {
        return Err(refused(format!(
            "the rig has no joint or part of that name; the rig's root is {listed}"
        )));
    };
    if let Some(parent) = rig[slot].parent {
        return Err(refused(format!(
            "`{name}` hangs under `{}`; root motion is read from the rig's root",
            rig[parent].name
        )));
    }
    if roots.len() > 1 {
        return Err(refused(format!(
            "the rig has several roots ({listed}), and `$root` moves the whole entity, so it \
             needs a rig with one root"
        )));
    }
    if reframed.is_some_and(|reframed| reframed[slot]) {
        return Err(refused(format!(
            "the rest of `{name}` is re-expressed in the mesh frame, so its channels are dropped \
             and its motion cannot be read; apply the armature transform in the DCC"
        )));
    }
    Ok(slot)
}

/// The yaw in degrees of the unit quaternion `q`'s twist about +Y (swing-twist), as the equivalent
/// nearest `previous`. A half-turn swing has no defined twist and holds `previous`.
fn twist_yaw_deg(q: [f64; 4], previous: f64) -> f64 {
    let [_, y, _, w] = q;
    if y * y + w * w < 1e-12 {
        return previous;
    }
    let raw = (2.0 * atan2(y, w)).to_degrees();
    raw + 360.0 * ((previous - raw) / 360.0).round()
}

/// A turn of `degrees` about +Y as `[x, y, z, w]`.
fn yaw_quat(degrees: f64) -> [f64; 4] {
    let half = degrees.to_radians() * 0.5;
    [0.0, sin(half), 0.0, cos(half)]
}

/// `q` with a turn of `degrees` about +Y taken off its left (the parent's side).
fn without_yaw(q: [f64; 4], degrees: f64) -> [f64; 4] {
    (DQuat::from_array(yaw_quat(-degrees)) * DQuat::from_array(q)).to_array()
}

/// `[x, z]` turned about +Y by `degrees`, the runtime's convention (glam's `from_rotation_y`).
fn rot_y(degrees: f64, v: [f64; 2]) -> [f64; 2] {
    let theta = degrees.to_radians();
    let (s, c) = (sin(theta), cos(theta));
    [v[0] * c + v[1] * s, -v[0] * s + v[1] * c]
}

/// The yaw that was taken off one joint's rotation, for sampling its stripped source between
/// frames.
struct Unyaw {
    slot: usize,
    /// The source's unwrapped twist yaw at every frame, degrees; the first is the clip's origin.
    yaws: Vec<f64>,
    frames: Vec<f64>,
}

impl Unyaw {
    /// The yaw the source has turned by at `t`, unwrapped near the last frame at or before it.
    fn turn(&self, channel: &SourceChannel, t: f64) -> f64 {
        let after = self.frames.partition_point(|&frame| frame <= t);
        let near = self.yaws[after.saturating_sub(1)];
        twist_yaw_deg(eval_source(channel, true, t), near) - self.yaws[0]
    }

    /// The stripped rotation at `t`.
    fn stripped(&self, channel: &SourceChannel, t: f64) -> [f64; 4] {
        without_yaw(eval_source(channel, true, t), self.turn(channel, t))
    }
}

/// What the rig root's motion became.
struct RootMotion {
    track: AnimationTrack,
    counts: TrackReport,
    /// Present when `$root` turns, so the joint's rotation lost its yaw.
    unyaw: Option<Unyaw>,
}

/// Moves the horizontal travel and yaw of the rig root (`rows` is its sampled source), relative to
/// `t = 0`, into a `$root` track, rewriting `rows` to what is left for the joint: the entity's
/// `T(d)·RY(r)` times the stripped local is the source local. `None`, with `rows` untouched, for a
/// root that never travels or turns.
fn extract_root(
    slot: usize,
    rest: &JointRest,
    channels: &JointChannels,
    rows: &mut [Vec<[f64; 4]>; 3],
    frames: &[f64],
    tolerances: &Tolerances,
) -> Option<RootMotion> {
    let mut yaws: Vec<f64> = Vec::with_capacity(frames.len());
    for frame in 0..frames.len() {
        let q = rows[ROTATION].get(frame).copied().unwrap_or(rest.rotation);
        let previous = yaws.last().copied().unwrap_or(0.0);
        yaws.push(twist_yaw_deg(q, previous));
    }
    let mut turns: Vec<f64> = yaws.iter().map(|yaw| yaw - yaws[0]).collect();
    let turning = turns
        .iter()
        .any(|turn| turn.abs() > tolerances.reduce[ROTATION]);
    if !turning {
        turns.fill(0.0);
    }

    let [x, y, z] = rest.translation;
    let point = |frame: usize| {
        rows[TRANSLATION]
            .get(frame)
            .copied()
            .unwrap_or([x, y, z, 0.0])
    };
    let start = point(0);
    let travel: Vec<[f64; 4]> = turns
        .iter()
        .enumerate()
        .map(|(frame, &turn)| {
            let t = point(frame);
            // The pivot term keeps the joint's start offset fixed while the entity turns.
            let pivot = rot_y(turn, [start[0], start[2]]);
            [t[0] - pivot[0], 0.0, t[2] - pivot[1], 0.0]
        })
        .collect();
    let travelling = travel
        .iter()
        .any(|d| distance([d[0], d[1], d[2]], [0.0; 3]) > tolerances.reduce[TRANSLATION]);
    if !turning && !travelling {
        return None;
    }

    // Either channel moves the entity, so the joint stays at its start in x and z.
    for t in &mut rows[TRANSLATION] {
        *t = [start[0], t[1], start[2], 0.0];
    }
    if turning {
        for (q, &turn) in rows[ROTATION].iter_mut().zip(&turns) {
            *q = without_yaw(*q, turn);
        }
    }
    let unyaw = turning.then(|| Unyaw {
        slot,
        yaws,
        frames: frames.to_vec(),
    });

    let ease = |kind: usize| match &channels[kind] {
        Some(channel) if channel.interpolation == Interpolation::Step => Ease::Step,
        _ => Ease::Linear,
    };
    let mut track = AnimationTrack {
        target: ROOT_TRACK.to_string(),
        rotation: Vec::new(),
        translation: Vec::new(),
        scale: Vec::new(),
    };
    let mut key_counts = [KeyCounts::default(); 3];
    if travelling {
        let keys = candidates(TRANSLATION, ease(TRANSLATION), frames, &travel);
        track.translation = reduce(TRANSLATION, &keys, &travel, tolerances.reduce[TRANSLATION]);
        key_counts[TRANSLATION] = KeyCounts {
            before: keys.len(),
            after: track.translation.len(),
        };
    }
    if let (Some(unyaw), Some(channel)) = (&unyaw, &channels[ROTATION]) {
        // The runtime lerps `$root` yaw in degrees, so it is a scalar here, not a rotation.
        let values: Vec<[f64; 4]> = turns.iter().map(|&turn| [0.0, turn, 0.0, 0.0]).collect();
        let keys = candidates(TRANSLATION, ease(ROTATION), frames, &values);
        let kept = reduce_with(
            &keys,
            &values,
            tolerances.reduce[ROTATION],
            true,
            |pair, t, source| (sample_keys(TRANSLATION, pair, t)[1] - source[1]).abs(),
        );
        track.rotation = split_yaw_half_turns(kept, unyaw, channel);
        key_counts[ROTATION] = KeyCounts {
            before: keys.len(),
            after: track.rotation.len(),
        };
    }
    Some(RootMotion {
        track,
        counts: TrackReport {
            target: ROOT_TRACK.to_string(),
            rotation: key_counts[ROTATION],
            translation: key_counts[TRANSLATION],
            scale: key_counts[SCALE],
        },
        unyaw,
    })
}

/// [`split_half_turns`] for the `$root` yaw: two adjacent frames half a turn apart get a key at the
/// source's midpoint, which pins the direction; the keys after it move by whole turns to follow.
fn split_yaw_half_turns(
    keys: Vec<Keyframe>,
    unyaw: &Unyaw,
    channel: &SourceChannel,
) -> Vec<Keyframe> {
    let near = |degrees: f64, to: f64| degrees + 360.0 * ((to - degrees) / 360.0).round();
    let mut out: Vec<Keyframe> = Vec::with_capacity(keys.len());
    let mut shift = 0.0;
    for mut key in keys {
        key.v[1] += shift;
        if let Some(start) = out.last().cloned()
            && !start.turns_as_written(&key)
            && let Some(t) = snapped_midpoint(start.t, key.t)
        {
            let middle = near(unyaw.turn(channel, t), start.v[1]);
            let end = near(key.v[1], middle);
            shift += end - key.v[1];
            key.v[1] = end;
            out.push(Keyframe {
                t,
                v: [0.0, middle, 0.0],
                ease: Ease::Linear,
            });
        }
        out.push(key);
    }
    out
}

/// The Euler triple equivalent to `base` that is nearest `previous`: `base` or its alternate
/// solution `(x + 180, 180 − y, z + 180)`, each axis shifted by the multiple of 360 nearest
/// `previous`. The base wins a tie.
fn nearest_equivalent_angles(base: [f64; 3], previous: [f64; 3]) -> [f64; 3] {
    let shift = |value: f64, near: f64| value + 360.0 * ((near - value) / 360.0).round();
    let nearest = |candidate: [f64; 3]| -> ([f64; 3], f64) {
        let shifted: [f64; 3] = std::array::from_fn(|a| shift(candidate[a], previous[a]));
        let cost: f64 = (0..3)
            .map(|a| {
                let d = shifted[a] - previous[a];
                d * d
            })
            .sum();
        (shifted, cost)
    };
    let (direct, direct_cost) = nearest(base);
    let (alternate, alternate_cost) = nearest([base[0] + 180.0, 180.0 - base[1], base[2] + 180.0]);
    if alternate_cost < direct_cost {
        alternate
    } else {
        direct
    }
}

/// One candidate key per source frame for a channel: rotation as unwrapped Euler degrees, the
/// others as the value itself. `Step` easing for a STEP sampler, `Linear` otherwise (and on the
/// first key, whose ease is inert).
fn candidates(kind: usize, ease: Ease, frames: &[f64], values: &[[f64; 4]]) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = Vec::with_capacity(frames.len());
    for (index, (&t, &value)) in frames.iter().zip(values).enumerate() {
        let v = if kind == ROTATION {
            let base = quat_to_euler_xyz_degrees(value);
            match keys.last() {
                Some(previous) => nearest_equivalent_angles(base, previous.v),
                None => base,
            }
        } else {
            [value[0], value[1], value[2]]
        };
        keys.push(Keyframe {
            t,
            v,
            ease: if index == 0 { Ease::Linear } else { ease },
        });
    }
    keys
}

/// `keys` sampled at `t` as `[x, y, z, w]` (a quaternion for rotation, `w = 0` otherwise).
fn sample_keys(kind: usize, keys: &[Keyframe], t: f64) -> [f64; 4] {
    if kind == ROTATION {
        sample_rotation(keys, t).expect("a reduced channel has keys")
    } else {
        let v = sample_channel(keys, t).expect("a reduced channel has keys");
        [v[0], v[1], v[2], 0.0]
    }
}

fn quat_angle_deg(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs();
    (2.0 * acos(dot.min(1.0))).to_degrees()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2])
}

fn max_abs_difference(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max)
}

/// The error of `sampled` against `source` in the channel's own metric.
fn channel_error(kind: usize, sampled: [f64; 4], source: [f64; 4]) -> f64 {
    let (a, b) = (
        [sampled[0], sampled[1], sampled[2]],
        [source[0], source[1], source[2]],
    );
    match kind {
        ROTATION => quat_angle_deg(sampled, source),
        TRANSLATION => distance(a, b),
        _ => max_abs_difference(a, b),
    }
}

/// Greedy key reduction: keep the first key, extend each segment from its anchor to the farthest
/// key whose two-key clip still reproduces every source frame in between within `tolerance`, and
/// always keep the last key.
fn reduce(kind: usize, keys: &[Keyframe], values: &[[f64; 4]], tolerance: f64) -> Vec<Keyframe> {
    reduce_with(
        keys,
        values,
        tolerance,
        kind == ROTATION,
        |pair, t, source| channel_error(kind, sample_keys(kind, pair, t), source),
    )
}

/// [`reduce`] over any metric: `error(pair, t, source)` is how far the two-key clip `pair` strays
/// at `t` from the `source` frame there. `guard_turns` refuses a segment that does not play as
/// written.
fn reduce_with(
    keys: &[Keyframe],
    values: &[[f64; 4]],
    tolerance: f64,
    guard_turns: bool,
    error: impl Fn(&[Keyframe], f64, [f64; 4]) -> f64,
) -> Vec<Keyframe> {
    let fits = |anchor: usize, end: usize| {
        // The validator refuses a rotation segment that does not play as written (`L1142`: half a
        // circle on an axis, or combined), and near gimbal lock the slerp can fit one the Euler
        // keys sweep.
        if guard_turns && !keys[anchor].turns_as_written(&keys[end]) {
            return false;
        }
        let pair = [keys[anchor].clone(), keys[end].clone()];
        (anchor + 1..end).all(|k| error(&pair, keys[k].t, values[k]) <= tolerance)
    };
    let mut kept = vec![keys[0].clone()];
    let mut anchor = 0;
    while anchor + 1 < keys.len() {
        let mut end = anchor + 1;
        while end + 1 < keys.len() && fits(anchor, end + 1) {
            end += 1;
        }
        kept.push(keys[end].clone());
        anchor = end;
    }
    kept
}

/// Depth of halving per emitted segment; one split resolves the unwrap's tie.
const MAX_HALF_TURN_DEPTH: u32 = 8;

/// Splits every emitted rotation segment that turns half a circle or more on an axis, which the
/// validator refuses (`L1142`). The unwrap leaves adjacent keys at most 180° apart, so this is its
/// tie: two frames an exact half turn apart. Playback slerps the short way between the keys'
/// quaternions, so the key at the source's midpoint is what pins the direction; the keys after a
/// split are re-expressed near it, the same rotations, so their playback is unchanged.
fn split_half_turns(keys: Vec<Keyframe>, source: &dyn Fn(f64) -> [f64; 4]) -> Vec<Keyframe> {
    let mut keys = keys.into_iter();
    let mut out: Vec<Keyframe> = keys.next().into_iter().collect();
    let mut split = false;
    for mut key in keys {
        if split {
            key.v = nearest_equivalent_angles(key.v, out.last().expect("non-empty").v);
        }
        let before = out.len();
        push_unwound(&mut out, key, source, MAX_HALF_TURN_DEPTH);
        split |= out.len() > before + 1;
    }
    out
}

/// Pushes `end`, first splitting the segment from `out`'s last key at the source's midpoint while it
/// does not play as written (`Keyframe::turns_as_written`: half a circle on an axis, or combined).
fn push_unwound(
    out: &mut Vec<Keyframe>,
    mut end: Keyframe,
    source: &dyn Fn(f64) -> [f64; 4],
    depth: u32,
) {
    let start = out.last().expect("non-empty").clone();
    let midpoint =
        snapped_midpoint(start.t, end.t).filter(|_| depth > 0 && !start.turns_as_written(&end));
    let Some(t) = midpoint else {
        out.push(end);
        return;
    };
    let mid = quat_to_euler_xyz_degrees(source(t));
    let mid = Keyframe {
        t,
        v: nearest_equivalent_angles(mid, start.v),
        ease: Ease::Linear,
    };
    push_unwound(out, mid, source, depth - 1);
    end.v = nearest_equivalent_angles(end.v, out.last().expect("non-empty").v);
    push_unwound(out, end, source, depth - 1);
}

/// The middle of `start..end`, snapped like every source time; `None` when the snap pushes it onto
/// a neighbour, which is then not split.
fn snapped_midpoint(start: f64, end: f64) -> Option<f64> {
    let t = ((start + (end - start) * 0.5) as f32)
        .to_string()
        .parse::<f64>()
        .expect("a float prints as a float");
    (start < t && t < end).then_some(t)
}

/// Tracks in the rig's parent-first order, with their key counts.
fn reduce_tracks(
    rig: &[RigNode],
    channels: &[JointChannels],
    table: &SourceTable,
    frames: &[f64],
    tolerances: &Tolerances,
    unyaw: Option<&Unyaw>,
) -> (Vec<AnimationTrack>, Vec<TrackReport>) {
    let mut tracks = Vec::new();
    let mut counts = Vec::new();
    for (joint, name) in rig.iter().map(|node| &node.name).enumerate() {
        let mut reduced: [Vec<Keyframe>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut key_counts = [KeyCounts::default(); 3];
        for kind in [ROTATION, TRANSLATION, SCALE] {
            let Some(channel) = &channels[joint][kind] else {
                continue;
            };
            let ease = if channel.interpolation == Interpolation::Step {
                Ease::Step
            } else {
                Ease::Linear
            };
            let rest = &rig[joint].rest;
            let rest_value = match kind {
                ROTATION => rest.rotation,
                TRANSLATION => [
                    rest.translation[0],
                    rest.translation[1],
                    rest.translation[2],
                    0.0,
                ],
                _ => [rest.scale[0], rest.scale[1], rest.scale[2], 0.0],
            };
            // A channel that never leaves rest is dropped; the measurement samples it as rest.
            if table[joint][kind]
                .iter()
                .all(|&v| channel_error(kind, v, rest_value) <= tolerances.reduce[kind])
            {
                continue;
            }
            let keys = candidates(kind, ease, frames, &table[joint][kind]);
            let mut kept = reduce(kind, &keys, &table[joint][kind], tolerances.reduce[kind]);
            if kind == ROTATION {
                kept = split_half_turns(kept, &|t| match unyaw {
                    Some(unyaw) if unyaw.slot == joint => unyaw.stripped(channel, t),
                    _ => eval_source(channel, true, t),
                });
            }
            key_counts[kind] = KeyCounts {
                before: keys.len(),
                after: kept.len(),
            };
            reduced[kind] = kept;
        }
        if reduced.iter().all(Vec::is_empty) {
            continue;
        }
        let [rotation, translation, scale] = reduced;
        tracks.push(AnimationTrack {
            target: name.clone(),
            rotation,
            translation,
            scale,
        });
        counts.push(TrackReport {
            target: name.clone(),
            rotation: key_counts[ROTATION],
            translation: key_counts[TRANSLATION],
            scale: key_counts[SCALE],
        });
    }
    (tracks, counts)
}

/// A joint's decomposed model-space pose.
struct Pose {
    scale: DVec3,
    rotation: DQuat,
    translation: DVec3,
}

fn local_matrix(rotation: [f64; 4], translation: [f64; 3], scale: [f64; 3]) -> DMat4 {
    DMat4::from_scale_rotation_translation(
        DVec3::from_array(scale),
        DQuat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]),
        DVec3::from_array(translation),
    )
}

/// Every node's model-space pose from its local matrix, composed down the parent-first list.
fn model_poses(rig: &[RigNode], local: impl Fn(usize) -> DMat4) -> Vec<Pose> {
    let mut models: Vec<DMat4> = Vec::with_capacity(rig.len());
    for (index, node) in rig.iter().enumerate() {
        let matrix = local(index);
        models.push(match node.parent {
            Some(parent) => models[parent] * matrix,
            None => matrix,
        });
    }
    models
        .iter()
        .map(|model| {
            let (scale, _, translation) = model.to_scale_rotation_translation();
            Pose {
                scale,
                rotation: orthonormal_rotation(model),
                translation,
            }
        })
        .collect()
}

/// The rotation of a model matrix, Gram-Schmidt orthonormalized. A non-uniform parent scale shears
/// the matrix, and glam's decomposition then returns a non-unit, branch-dependent quaternion.
fn orthonormal_rotation(model: &DMat4) -> DQuat {
    let (mut x, c1, c2) = (
        model.x_axis.truncate(),
        model.y_axis.truncate(),
        model.z_axis.truncate(),
    );
    if x.cross(c1).dot(c2) < 0.0 {
        x = -x;
    }
    let x = x / sqrt(x.dot(x));
    let y = c1 - x * x.dot(c1);
    let y = y / sqrt(y.dot(y));
    DQuat::from_mat3(&DMat3::from_cols(x, y, x.cross(y)))
}

/// The rotation, translation and scale differences between two poses.
fn pose_difference(a: &Pose, b: &Pose) -> [f64; 3] {
    let quat = |q: DQuat| [q.x, q.y, q.z, q.w];
    [
        quat_angle_deg(quat(a.rotation), quat(b.rotation)),
        distance(a.translation.to_array(), b.translation.to_array()),
        max_abs_difference(a.scale.to_array(), b.scale.to_array()),
    ]
}

/// Measures `clip` against the source at every source frame, in model space with the clip's
/// `$root` applied, and (for a looping clip) how far its end pose is from its start in place, the
/// `$root` travel left out. `tracks` in the result is left empty.
fn measure(
    rig: &[RigNode],
    table: &SourceTable,
    clip: &AnimationDocument,
    frames: &[f64],
    looped: bool,
) -> ClipReport {
    let track_of: BTreeMap<&str, &AnimationTrack> = clip
        .tracks
        .iter()
        .map(|track| (track.target.as_str(), track))
        .collect();
    let root = track_of.get(ROOT_TRACK).copied();
    let clip_poses = |t: f64, with_root: bool| {
        model_poses(rig, |j| {
            let rest = &rig[j].rest;
            let local = match track_of.get(rig[j].name.as_str()) {
                None => local_matrix(rest.rotation, rest.translation, rest.scale),
                Some(track) => {
                    let rotation = sample_rotation(&track.rotation, t).unwrap_or(rest.rotation);
                    let translation =
                        sample_channel(&track.translation, t).unwrap_or(rest.translation);
                    let scale = sample_channel(&track.scale, t).unwrap_or(rest.scale);
                    local_matrix(rotation, translation, scale)
                }
            };
            match root {
                // A clip with a `$root` has a one-root rig; this is the runtime's sampling.
                Some(root) if with_root && rig[j].parent.is_none() => {
                    let p = sample_channel(&root.translation, t).unwrap_or([0.0; 3]);
                    let yaw = sample_channel(&root.rotation, t).map_or(0.0, |v| v[1]);
                    local_matrix(yaw_quat(yaw), [p[0], 0.0, p[2]], [1.0; 3]) * local
                }
                _ => local,
            }
        })
    };

    let mut worst: Vec<JointError> = rig
        .iter()
        .map(|node| JointError {
            joint: node.name.clone(),
            rotation_deg: 0.0,
            rotation_frame: 0,
            translation: 0.0,
            translation_frame: 0,
            scale: 0.0,
            scale_frame: 0,
        })
        .collect();
    for (frame, &t) in frames.iter().enumerate() {
        let source = model_poses(rig, |j| {
            let rest = &rig[j].rest;
            let row = &table[j];
            let vec3 = |kind: usize, default: [f64; 3]| {
                row[kind].get(frame).map_or(default, |v| [v[0], v[1], v[2]])
            };
            let rotation = row[ROTATION].get(frame).copied().unwrap_or(rest.rotation);
            local_matrix(
                rotation,
                vec3(TRANSLATION, rest.translation),
                vec3(SCALE, rest.scale),
            )
        });
        let reduced = clip_poses(t, true);
        for (j, error) in worst.iter_mut().enumerate() {
            let [rotation, translation, scale] = pose_difference(&source[j], &reduced[j]);
            if rotation > error.rotation_deg {
                error.rotation_deg = rotation;
                error.rotation_frame = frame;
            }
            if translation > error.translation {
                error.translation = translation;
                error.translation_frame = frame;
            }
            if scale > error.scale {
                error.scale = scale;
                error.scale_frame = frame;
            }
        }
    }

    let seam = looped.then(|| {
        let duration = frames.last().copied().unwrap_or(0.0);
        let (start, end) = (clip_poses(0.0, false), clip_poses(duration, false));
        let mut seam = SeamError {
            rotation_deg: 0.0,
            translation: 0.0,
            scale: 0.0,
        };
        for (a, b) in start.iter().zip(&end) {
            let [rotation, translation, scale] = pose_difference(a, b);
            seam.rotation_deg = seam.rotation_deg.max(rotation);
            seam.translation = seam.translation.max(translation);
            seam.scale = seam.scale.max(scale);
        }
        seam
    });

    ClipReport {
        source_frames: frames.len(),
        frame_times: frames.to_vec(),
        tracks: Vec::new(),
        joints: worst,
        seam,
    }
}

/// `L1206` for the worst offence (the largest error-to-bound ratio) of a clip over its bound.
fn check_bound(
    label: &str,
    noun: &str,
    report: &ClipReport,
    tolerances: &Tolerances,
) -> Option<Diagnostic> {
    let mut offence: Option<(f64, String)> = None;
    for joint in &report.joints {
        let measured = [
            (ROTATION, joint.rotation_deg, joint.rotation_frame, "°"),
            (
                TRANSLATION,
                joint.translation,
                joint.translation_frame,
                " units",
            ),
            (SCALE, joint.scale, joint.scale_frame, ""),
        ];
        for (kind, error, frame, unit) in measured {
            let bound = tolerances.bound[kind];
            if error <= bound {
                continue;
            }
            let ratio = error / bound;
            if offence.as_ref().is_none_or(|(worst, _)| ratio > *worst) {
                offence = Some((
                    ratio,
                    format!(
                        "{label}: after key reduction {noun} `{}` strays from the source by \
                         {error:.6}{unit} in model-space {} at frame {frame} (t = {}s), over \
                         the bound of {bound}{unit}",
                        joint.joint, CHANNEL_NAMES[kind], report.frame_times[frame]
                    ),
                ));
            }
        }
    }
    offence.map(|(_, message)| clip_error_bound(message))
}

#[cfg(test)]
mod tests {
    use ludion_core::math::f64::euler_xyz_degrees_to_quat;
    use ludion_scene::validate_animation;

    use super::*;

    /// Gimbal lock: at `Ry(90)` the rotation depends on one combination of x and z, so Euler keys
    /// that sweep both hundreds of degrees describe a rotation that stays put.
    #[test]
    fn reduction_never_merges_a_rotation_turning_half_a_circle_on_an_axis() {
        let euler: Vec<[f64; 3]> = (0..=8)
            .map(|k| [40.0 * f64::from(k), 90.0, -40.0 * f64::from(k)])
            .collect();
        let values: Vec<[f64; 4]> = euler
            .iter()
            .map(|&v| euler_xyz_degrees_to_quat(v))
            .collect();
        for q in &values {
            let agreement = (0..4).map(|i| q[i] * values[0][i]).sum::<f64>().abs();
            assert!(
                (agreement - 1.0).abs() < 1e-12,
                "fixture is not degenerate: {agreement}"
            );
        }
        let keys: Vec<Keyframe> = euler
            .iter()
            .enumerate()
            .map(|(k, &v)| Keyframe {
                t: 0.1 * k as f64,
                v,
                ease: Ease::Linear,
            })
            .collect();

        let kept = reduce(ROTATION, &keys, &values, 0.01);

        let clip = AnimationDocument {
            name: None,
            generated_from: None,
            duration: 0.8,
            looped: false,
            events: Vec::new(),
            tracks: vec![AnimationTrack {
                target: "joint".to_string(),
                rotation: kept.clone(),
                translation: Vec::new(),
                scale: Vec::new(),
            }],
        };
        let diagnostics = validate_animation(&clip);
        assert!(
            diagnostics.is_empty(),
            "{:?}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        for pair in kept.windows(2) {
            for axis in 0..3 {
                assert!(
                    (pair[1].v[axis] - pair[0].v[axis]).abs() < 180.0,
                    "{kept:?}"
                );
            }
        }
    }
}
