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
use ludion_core::math::f64::{acos, quat_to_euler_xyz_degrees, sin, sqrt};
use ludion_core::math::glam::{DMat3, DMat4, DQuat, DVec3};
use ludion_scene::{
    AnimationDocument, AnimationTrack, Ease, Keyframe, MeshJoint, sample_channel, sample_rotation,
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

/// One emitted track's key counts, in the order the tracks are emitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackReport {
    /// The track's target (a joint name).
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
    /// The joint's name.
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
/// the reduced clip at `t = 0` and at `t = duration`, over every joint.
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
    /// The worst model-space error of every joint of the skin, in the skin's joint order.
    pub joints: Vec<JointError>,
    /// Present only for a `loop: true` clip.
    pub seam: Option<SeamError>,
}

struct Extras {
    looped: bool,
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

/// One joint's up-to-three source channels, indexed by [`ROTATION`], [`TRANSLATION`], [`SCALE`].
type JointChannels = [Option<SourceChannel>; 3];

/// Per joint and channel, the source sampled at every frame (empty for an absent channel).
type SourceTable = Vec<[Vec<[f64; 4]>; 3]>;

enum Read {
    Channel(SourceChannel),
    /// The channel has no mapping; an `L1202` says why.
    Dropped(Diagnostic),
}

/// Imports every animation of `document` as a clip. `L1202` diagnostics go to `lossy`; the returned
/// errors (`L1201`, `L1206`, `L1207`, `L1208`) are never downgraded.
pub(crate) fn import_animations(
    document: &gltf::Document,
    bindings: &BTreeMap<usize, SkinBinding>,
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

        // The first surviving channel picks the skin; a channel on another skin is dropped.
        let mut skin: Option<(usize, Vec<JointChannels>)> = None;
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
            let Some(&(mesh, joint)) = joint_of_node.get(&node.index()) else {
                lossy.push(unsupported(
                    "an animation channel on a non-joint node",
                    &offender,
                ));
                continue;
            };
            let binding = &bindings[&mesh];
            if binding.reframed[joint] {
                let name = &binding.joints[joint].name;
                let what = if binding.joints[joint].parent.is_some() {
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
            if skin.as_ref().is_some_and(|(chosen, _)| *chosen != mesh) {
                lossy.push(unsupported(
                    "an animation channel on a joint of a second skin",
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
            let (_, joints) = skin.get_or_insert_with(|| {
                (
                    mesh,
                    (0..binding.joints.len())
                        .map(|_| [None, None, None])
                        .collect(),
                )
            });
            if joints[joint][kind].replace(source).is_some() {
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
        let Some((mesh, channels)) = skin else {
            lossy.push(unsupported(
                "an animation with no importable channel",
                &label,
            ));
            continue;
        };
        let joints = &bindings[&mesh].joints;

        let frames = match source_frames(&channels, &extras.tolerances) {
            Ok(frames) => frames,
            Err(miss) => {
                errors.push(cubic_too_curved(format!(
                    "{label}: the CUBICSPLINE {} channel of joint `{}` curves too sharply between \
                     t = {}s and t = {}s for the importer to follow within the reduce tolerance \
                     of {} (it subdivides an interval up to {MAX_CUBIC_STEPS} steps)",
                    CHANNEL_NAMES[miss.kind],
                    joints[miss.joint].name,
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

        let table = sample_source(&channels, &frames);
        let (tracks, counts) =
            reduce_tracks(joints, &channels, &table, &frames, &extras.tolerances);
        let mut doc = AnimationDocument {
            name: None,
            generated_from: None,
            duration,
            looped: extras.looped,
            events: Vec::new(),
            tracks,
        };
        let mut report = measure(joints, &table, &doc, &frames, extras.looped);
        report.tracks = counts;
        if let Some(diagnostic) = check_bound(&label, &report, &extras.tolerances) {
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
        });
    }
    if errors.is_empty() {
        Ok(clips)
    } else {
        Err(errors)
    }
}

/// Reads an animation's `extras`: `loop` and the six tolerance overrides.
fn parse_extras(label: &str, extras: &gltf::json::Extras) -> Result<Extras, Diagnostic> {
    let mut parsed = Extras {
        looped: false,
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
    joint: usize,
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
    for (joint, joint_channels) in channels.iter().enumerate() {
        for (kind, channel) in joint_channels.iter().enumerate() {
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
                            joint,
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
    let fits = |anchor: usize, end: usize| {
        // The validator refuses a rotation segment that does not play as written (`L1142`: half a
        // circle on an axis, or combined), and near gimbal lock the slerp can fit one the Euler
        // keys sweep.
        if kind == ROTATION && !keys[anchor].turns_as_written(&keys[end]) {
            return false;
        }
        let pair = [keys[anchor].clone(), keys[end].clone()];
        (anchor + 1..end).all(|k| {
            channel_error(kind, sample_keys(kind, &pair, keys[k].t), values[k]) <= tolerance
        })
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
fn split_half_turns(keys: Vec<Keyframe>, channel: &SourceChannel) -> Vec<Keyframe> {
    let mut keys = keys.into_iter();
    let mut out: Vec<Keyframe> = keys.next().into_iter().collect();
    let mut split = false;
    for mut key in keys {
        if split {
            key.v = nearest_equivalent_angles(key.v, out.last().expect("non-empty").v);
        }
        let before = out.len();
        push_unwound(&mut out, key, channel, MAX_HALF_TURN_DEPTH);
        split |= out.len() > before + 1;
    }
    out
}

/// Pushes `end`, first splitting the segment from `out`'s last key at the source's midpoint while it
/// does not play as written (`Keyframe::turns_as_written`: half a circle on an axis, or combined).
fn push_unwound(out: &mut Vec<Keyframe>, mut end: Keyframe, channel: &SourceChannel, depth: u32) {
    let start = out.last().expect("non-empty").clone();
    // Snapped like every source time; a midpoint the snap pushes onto a neighbour is not split.
    let t = ((start.t + (end.t - start.t) * 0.5) as f32)
        .to_string()
        .parse::<f64>()
        .expect("a float prints as a float");
    if depth == 0 || start.turns_as_written(&end) || !(start.t < t && t < end.t) {
        out.push(end);
        return;
    }
    let mid = quat_to_euler_xyz_degrees(eval_source(channel, true, t));
    let mid = Keyframe {
        t,
        v: nearest_equivalent_angles(mid, start.v),
        ease: Ease::Linear,
    };
    push_unwound(out, mid, channel, depth - 1);
    end.v = nearest_equivalent_angles(end.v, out.last().expect("non-empty").v);
    push_unwound(out, end, channel, depth - 1);
}

/// Tracks in the joints' parent-first order, with their key counts.
fn reduce_tracks(
    joints: &[MeshJoint],
    channels: &[JointChannels],
    table: &SourceTable,
    frames: &[f64],
    tolerances: &Tolerances,
) -> (Vec<AnimationTrack>, Vec<TrackReport>) {
    let mut tracks = Vec::new();
    let mut counts = Vec::new();
    for (joint, name) in joints.iter().map(|joint| &joint.name).enumerate() {
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
            let rest = &joints[joint].rest;
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
                kept = split_half_turns(kept, channel);
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

/// Every joint's model-space pose from its local matrix, composed down the parent-first list.
fn model_poses(joints: &[MeshJoint], local: impl Fn(usize) -> DMat4) -> Vec<Pose> {
    let mut models: Vec<DMat4> = Vec::with_capacity(joints.len());
    for (index, joint) in joints.iter().enumerate() {
        let matrix = local(index);
        models.push(match joint.parent {
            Some(parent) => models[parent as usize] * matrix,
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

/// Measures `clip` against the source at every source frame, in model space, and (for a looping
/// clip) how far its end pose is from its start. `tracks` in the result is left empty.
fn measure(
    joints: &[MeshJoint],
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
    let clip_poses = |t: f64| {
        model_poses(joints, |j| {
            let rest = &joints[j].rest;
            match track_of.get(joints[j].name.as_str()) {
                None => local_matrix(rest.rotation, rest.translation, rest.scale),
                Some(track) => {
                    let rotation = sample_rotation(&track.rotation, t).unwrap_or(rest.rotation);
                    let translation =
                        sample_channel(&track.translation, t).unwrap_or(rest.translation);
                    let scale = sample_channel(&track.scale, t).unwrap_or(rest.scale);
                    local_matrix(rotation, translation, scale)
                }
            }
        })
    };

    let mut worst: Vec<JointError> = joints
        .iter()
        .map(|joint| JointError {
            joint: joint.name.clone(),
            rotation_deg: 0.0,
            rotation_frame: 0,
            translation: 0.0,
            translation_frame: 0,
            scale: 0.0,
            scale_frame: 0,
        })
        .collect();
    for (frame, &t) in frames.iter().enumerate() {
        let source = model_poses(joints, |j| {
            let rest = &joints[j].rest;
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
        let reduced = clip_poses(t);
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
        let (start, end) = (clip_poses(0.0), clip_poses(duration));
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
fn check_bound(label: &str, report: &ClipReport, tolerances: &Tolerances) -> Option<Diagnostic> {
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
                        "{label}: after key reduction joint `{}` strays from the source by \
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
