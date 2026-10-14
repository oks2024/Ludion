//! Sampling a `kind: animation` clip — the pure, IO-free half of playback
//! (`Plan/voxel-animation.md` decision 8).
//!
//! [`crate::animation`] owns a clip's *shape*; this module owns its *values over time*: where a
//! clip-relative instant lands inside a looping or one-shot clip ([`clip_phase`]), and what a
//! named part's channels read there ([`part_pose`]). Nothing here knows about assemblies, matrices,
//! or the engine — a caller turns the sampled Euler degrees and voxel-unit offsets into a transform
//! in whatever frame it works in. That split is what lets the engine's extract-time evaluator and
//! the model-view tooling sample one implementation instead of two.
//!
//! [`flipbook_frame`] is the same idea one dimension flatter, for the other thing an `Animation`
//! component can name: a [`Flipbook`] carried on a `kind: atlas` picks *which region* a sprite shows
//! at an instant. It shares the wrap/clamp rule and the degenerate-input rule with [`clip_phase`],
//! and counts in whole clip frames, dividing by the tick rate last, so tick quantization is exact.
//!
//! Everything is plain arithmetic on `f64`: no transcendentals, no accumulation, no state. The same
//! `(clip, time)` always samples to the same bits, which is what makes an animated frame a pure
//! function of sim time (decision 2).
//!
//! # Conventions
//!
//! - A channel's keyframes are time-ascending and strictly increasing (`validate_animation`).
//! - Before the first keyframe the channel **holds** the first value; past the last it holds the
//!   last. A channel is therefore total: it has a value at every time, and an empty channel is the
//!   absence of the channel (`None`), never a zero.
//! - [`Keyframe::ease`] is the easing used **approaching** that keyframe from the previous one, so a
//!   segment's easing is read off its *destination* key and the first key's `ease` is inert.

use crate::animation::{AnimationDocument, AnimationTrack, Ease, Keyframe};
use crate::atlas::Flipbook;
use ludion_core::math::f64::{acos, euler_xyz_degrees_to_quat, sin, sqrt};

/// What one part's (or skin joint's) channels read at an instant: `None` for a channel the clip
/// does not animate, so a caller can leave that rotation (or translation) at rest instead of
/// forcing it to zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PartPose {
    /// Quaternion `[x, y, z, w]` from [`sample_rotation`] — a shortest-path slerp between keys.
    pub rotation: Option<[f64; 4]>,
    /// Part-local voxel units (a joint: parent-relative, in the mesh's units).
    pub translation: Option<[f64; 3]>,
    /// Per-axis multiplier on the part's own axes.
    pub scale: Option<[f64; 3]>,
}

/// Where `elapsed` seconds of playback land inside `clip`: wrapped into `[0, duration)` for a
/// looping clip, clamped to `duration` for a one-shot (which then holds its final pose forever).
///
/// Negative or non-finite input reads as `0.0` rather than producing a NaN pose — a clip whose
/// `started` frame is in the future (a restored snapshot, a game writing a stale value) shows its
/// first frame, and no arithmetic downstream can go non-finite. A degenerate `duration` (which a
/// `validate_animation`-clean document cannot have) is `0.0` for the same reason.
pub fn clip_phase(clip: &AnimationDocument, elapsed: f64) -> f64 {
    if !clip.duration.is_finite() || clip.duration <= 0.0 {
        return 0.0;
    }
    let elapsed = if elapsed.is_finite() {
        elapsed.max(0.0)
    } else {
        0.0
    };
    if clip.looped {
        // `elapsed` is non-negative and `duration` strictly positive, so the remainder is the wrap.
        elapsed % clip.duration
    } else {
        elapsed.min(clip.duration)
    }
}

/// The track `clip` declares for `part`, or `None` when the clip does not animate that part.
///
/// A linear scan: a track list is a handful of limbs, and a linear scan does not assume the sorted
/// order a programmatically built [`AnimationDocument`] need not have.
pub fn track<'a>(clip: &'a AnimationDocument, part: &str) -> Option<&'a AnimationTrack> {
    clip.tracks.iter().find(|track| track.target == part)
}

/// Samples both of `part`'s channels at clip time `t` (seconds, already through [`clip_phase`]).
///
/// A part the clip never mentions poses entirely at rest — the same answer as a clip that mentions
/// it but animates neither channel.
pub fn part_pose(clip: &AnimationDocument, part: &str, t: f64) -> PartPose {
    match track(clip, part) {
        None => PartPose::default(),
        Some(track) => PartPose {
            rotation: sample_rotation(&track.rotation, t),
            translation: sample_channel(&track.translation, t),
            scale: sample_channel(&track.scale, t),
        },
    }
}

/// Samples the channels `clip` declares for the skin joint `target` at clip time `t`, or `None`
/// when the clip has no track for it — the caller then keeps the joint's exact rest local.
pub fn joint_pose(clip: &AnimationDocument, target: &str, t: f64) -> Option<PartPose> {
    track(clip, target)?;
    Some(part_pose(clip, target, t))
}

/// Samples one channel at time `t`, per-axis, or `None` when the channel has no keyframes.
///
/// Holds the endpoints outside `[first.t, last.t]`; inside, interpolates each axis independently
/// between the bracketing keys under the destination key's easing.
pub fn sample_channel(keys: &[Keyframe], t: f64) -> Option<[f64; 3]> {
    let first = keys.first()?;
    // A NaN time holds the first key rather than falling through to the interpolation, where it
    // would poison every axis — hence the explicit `is_nan` arm rather than a bare `t <= first.t`.
    if t.is_nan() || t <= first.t {
        return Some(first.v);
    }
    let last = keys.last().expect("a non-empty slice has a last element");
    if t >= last.t {
        return Some(last.v);
    }
    // `t` is strictly inside the channel's span, so there is a key at or before it and another
    // after: the index is in `0..keys.len() - 1` and `index + 1` is in range.
    let index = locate(keys, t, |key| key.t);
    let (from, to) = (&keys[index], &keys[index + 1]);
    let u = segment_factor(from.t, to.t, to.ease, t);
    Some([
        lerp(from.v[0], to.v[0], u),
        lerp(from.v[1], to.v[1], u),
        lerp(from.v[2], to.v[2], u),
    ])
}

/// Samples a rotation channel at time `t` as a quaternion `[x, y, z, w]`, or `None` when the
/// channel has no keyframes.
///
/// Endpoints hold exactly like [`sample_channel`]; between keys the blend is a shortest-path slerp
/// under the destination key's easing (a normalized lerp when the keys are nearly parallel), so a
/// rotation never takes the long way round and never gimbal-flips the way per-axis Euler lerp does.
pub fn sample_rotation(keys: &[Keyframe], t: f64) -> Option<[f64; 4]> {
    let first = keys.first()?;
    if t.is_nan() || t <= first.t {
        return Some(euler_xyz_degrees_to_quat(first.v));
    }
    let last = keys.last().expect("a non-empty slice has a last element");
    if t >= last.t {
        return Some(euler_xyz_degrees_to_quat(last.v));
    }
    let index = locate(keys, t, |key| key.t);
    let (from, to) = (&keys[index], &keys[index + 1]);
    let u = segment_factor(from.t, to.t, to.ease, t);
    Some(slerp(
        euler_xyz_degrees_to_quat(from.v),
        euler_xyz_degrees_to_quat(to.v),
        u,
    ))
}

/// The index of the segment's start key for a `t` strictly inside the channel's span.
fn locate<K>(keys: &[K], t: f64, key_t: impl Fn(&K) -> f64) -> usize {
    // On strictly increasing times this count is in `1..len`; the clamp keeps an unvalidated
    // channel in range instead of panicking.
    keys.partition_point(|key| key_t(key) <= t)
        .clamp(1, keys.len() - 1)
        - 1
}

/// The eased position of `t` inside the segment `from_t..to_t`, under the destination's easing.
fn segment_factor(from_t: f64, to_t: f64, ease: Ease, t: f64) -> f64 {
    let span = to_t - from_t;
    // A zero span cannot occur on a validated clip (times strictly increase); reading the
    // destination is the honest degrade if one ever does.
    let u = if span > 0.0 { (t - from_t) / span } else { 1.0 };
    ease_factor(ease, u)
}

/// A rotation key with its Euler value already converted to a quaternion.
#[derive(Clone, Debug, PartialEq)]
struct QuatKey {
    t: f64,
    q: [f64; 4],
    ease: Ease,
}

/// One target's channels as a [`ClipSampler`] holds them.
#[derive(Clone, Debug, PartialEq)]
struct SamplerTrack {
    rotation: Vec<QuatKey>,
    translation: Vec<Keyframe>,
    scale: Vec<Keyframe>,
}

/// A clip resolved against an ordered target list (a skin's joints): each target's track is found
/// once and its Euler keys converted to quaternions once, so [`Self::pose`] is a binary search and
/// a slerp. It samples bit-for-bit like [`joint_pose`].
#[derive(Clone, Debug, PartialEq)]
pub struct ClipSampler {
    tracks: Vec<Option<SamplerTrack>>,
}

impl ClipSampler {
    /// Binds `clip` to `targets`; target `i` takes the first track naming it, as [`track`] does.
    pub fn new<'a>(clip: &AnimationDocument, targets: impl IntoIterator<Item = &'a str>) -> Self {
        let tracks = targets
            .into_iter()
            .map(|target| {
                track(clip, target).map(|track| SamplerTrack {
                    rotation: track
                        .rotation
                        .iter()
                        .map(|key| QuatKey {
                            t: key.t,
                            q: euler_xyz_degrees_to_quat(key.v),
                            ease: key.ease,
                        })
                        .collect(),
                    translation: track.translation.clone(),
                    scale: track.scale.clone(),
                })
            })
            .collect();
        Self { tracks }
    }

    /// How many targets the sampler was bound to; a consumer posing a different list must not read it.
    pub fn targets(&self) -> usize {
        self.tracks.len()
    }

    /// What target `index` reads at clip time `t`, or `None` when it has no track (or `index` is
    /// past the target list) — [`joint_pose`]'s answer.
    pub fn pose(&self, index: usize, t: f64) -> Option<PartPose> {
        let track = self.tracks.get(index)?.as_ref()?;
        Some(PartPose {
            rotation: sample_quat_keys(&track.rotation, t),
            translation: sample_channel(&track.translation, t),
            scale: sample_channel(&track.scale, t),
        })
    }
}

/// [`sample_rotation`] over keys already converted to quaternions.
fn sample_quat_keys(keys: &[QuatKey], t: f64) -> Option<[f64; 4]> {
    let first = keys.first()?;
    if t.is_nan() || t <= first.t {
        return Some(first.q);
    }
    let last = keys.last().expect("a non-empty slice has a last element");
    if t >= last.t {
        return Some(last.q);
    }
    let index = locate(keys, t, |key| key.t);
    let (from, to) = (&keys[index], &keys[index + 1]);
    Some(slerp(
        from.q,
        to.q,
        segment_factor(from.t, to.t, to.ease, t),
    ))
}

/// Shortest-path slerp between unit quaternions `[x, y, z, w]` at `u` in `[0, 1]` (a normalized
/// lerp when they are nearly parallel). Libm-only, so the render path and the sim agree bit-for-bit.
pub fn slerp(a: [f64; 4], b: [f64; 4], u: f64) -> [f64; 4] {
    let mut b = b;
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    let (wa, wb) = if dot > 1.0 - 1e-9 {
        (1.0 - u, u)
    } else {
        let theta = acos(dot.min(1.0));
        let inv = 1.0 / sin(theta);
        (sin((1.0 - u) * theta) * inv, sin(u * theta) * inv)
    };
    let q = [
        wa * a[0] + wb * b[0],
        wa * a[1] + wb * b[1],
        wa * a[2] + wb * b[2],
        wa * a[3] + wb * b[3],
    ];
    let len = sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]);
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

/// The eased blend factor for a normalized segment position `u` in `[0, 1]`.
fn ease_factor(ease: Ease, u: f64) -> f64 {
    match ease {
        Ease::Linear => u,
        // Smoothstep — the cubic with zero derivative at both ends.
        Ease::Smooth => u * u * (3.0 - 2.0 * u),
        // No interpolation: the value holds at the segment's start until its end is reached.
        Ease::Step => 0.0,
    }
}

/// Linear blend that is **exact at both ends**: `u == 0.0` reproduces `from` bit-for-bit, which is
/// what keeps a keyframe's authored value the value that actually renders at its own time.
fn lerp(from: f64, to: f64, u: f64) -> f64 {
    from + (to - from) * u
}

impl Flipbook {
    /// The clip's whole length in **clip frames**: the sum of [`Self::hold`], or one per entry of
    /// [`Self::frames`] when no holds are authored.
    pub fn total_holds(&self) -> u64 {
        match &self.hold {
            Some(hold) => hold.iter().map(|&count| u64::from(count)).sum(),
            None => self.frames.len() as u64,
        }
    }

    /// The clip's length in seconds: its clip frames at its own [`Self::fps`].
    pub fn duration(&self) -> f64 {
        self.total_holds() as f64 / self.fps
    }
}

/// Which entry of `flipbook.frames` is showing `ticks / rate` seconds into playback.
///
/// The clip frame is `floor(ticks × fps / rate)` — counted, never accumulated, so two hosts that
/// reach the same tick by different routes show the same frame. The division comes last: `ticks × fps`
/// is exact whenever its operands are (whole tick counts at power-of-two speeds, whole-number fps),
/// so a frame boundary that lands on a tick is exact, where converting to seconds first
/// (`ticks / rate`) rounds it a hair below the boundary at 15, 30 and 60 fps. A caller already
/// holding seconds passes `rate = 1.0`. The clip frame is then wrapped for a looping clip or clamped
/// to the last for a one-shot (which holds its final frame forever, [`clip_phase`]'s rule exactly).
/// Walking [`Flipbook::hold`] turns that clip frame into a frame index; with no holds the two are
/// the same number.
///
/// Degenerate input reads as the first frame rather than producing a nonsense index: a negative or
/// non-finite `ticks` (a `started` in the future), a non-positive or non-finite `fps` or `rate`, and an
/// empty `frames` list — none of which a validated document can carry — all answer `0`.
pub fn flipbook_frame(flipbook: &Flipbook, ticks: f64, rate: f64) -> usize {
    let total = flipbook.total_holds();
    if flipbook.frames.is_empty() || total == 0 {
        return 0;
    }
    if !(flipbook.fps.is_finite() && flipbook.fps > 0.0 && rate.is_finite() && rate > 0.0) {
        return 0;
    }
    let ticks = if ticks.is_finite() {
        ticks.max(0.0)
    } else {
        0.0
    };
    // `as u64` saturates at both ends, so an absurd `ticks` clamps instead of wrapping.
    let clip_frame = (ticks * flipbook.fps / rate).floor() as u64;
    let clip_frame = if flipbook.looped {
        clip_frame % total
    } else {
        clip_frame.min(total - 1)
    };
    let last = flipbook.frames.len() - 1;
    let Some(hold) = &flipbook.hold else {
        // One clip frame per entry, so the count *is* the index — and it is below `frames.len()`.
        return (clip_frame as usize).min(last);
    };
    let mut consumed = 0u64;
    for (index, &count) in hold.iter().enumerate() {
        consumed += u64::from(count);
        if clip_frame < consumed {
            return index.min(last);
        }
    }
    // Unreachable on a validated document (`hold` is one count per frame), and the honest degrade
    // for one assembled some other way: hold the last frame.
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: f64, v: f64, ease: Ease) -> Keyframe {
        Keyframe {
            t,
            v: [v, v * 2.0, v * 3.0],
            ease,
        }
    }

    fn clip(duration: f64, looped: bool, tracks: Vec<AnimationTrack>) -> AnimationDocument {
        AnimationDocument {
            name: Some("fixture".to_string()),
            duration,
            looped,
            tracks,
            ..Default::default()
        }
    }

    fn rotation_track(part: &str, keys: Vec<Keyframe>) -> AnimationTrack {
        AnimationTrack {
            target: part.to_string(),
            rotation: keys,
            translation: Vec::new(),
            scale: Vec::new(),
        }
    }

    /// An empty channel has no value at all — the absence a caller leaves at rest.
    #[test]
    fn an_empty_channel_samples_to_nothing() {
        assert_eq!(sample_channel(&[], 0.5), None);
    }

    /// Before the first key (and at it) the channel holds the first value, bit-exactly.
    #[test]
    fn before_the_first_key_holds_it() {
        let keys = [key(1.0, 10.0, Ease::Linear), key(2.0, 20.0, Ease::Linear)];
        assert_eq!(sample_channel(&keys, 0.0), Some([10.0, 20.0, 30.0]));
        assert_eq!(sample_channel(&keys, 1.0), Some([10.0, 20.0, 30.0]));
    }

    /// Past the last key the channel holds the last value.
    #[test]
    fn after_the_last_key_holds_it() {
        let keys = [key(1.0, 10.0, Ease::Linear), key(2.0, 20.0, Ease::Linear)];
        assert_eq!(sample_channel(&keys, 2.0), Some([20.0, 40.0, 60.0]));
        assert_eq!(sample_channel(&keys, 99.0), Some([20.0, 40.0, 60.0]));
    }

    /// Linear easing is the plain per-axis blend, and the midpoint is the average.
    #[test]
    fn linear_interpolates_per_axis() {
        let keys = [key(0.0, 0.0, Ease::Linear), key(2.0, 10.0, Ease::Linear)];
        assert_eq!(sample_channel(&keys, 1.0), Some([5.0, 10.0, 15.0]));
        assert_eq!(sample_channel(&keys, 0.5), Some([2.5, 5.0, 7.5]));
    }

    /// Smoothstep eases in and out: the midpoint still reads the average, but the quarter point sits
    /// below the linear answer (`u²(3 − 2u)` at `u = 0.25` is `0.15625`).
    #[test]
    fn smooth_is_smoothstep() {
        let keys = [key(0.0, 0.0, Ease::Linear), key(4.0, 100.0, Ease::Smooth)];
        assert_eq!(sample_channel(&keys, 2.0).unwrap()[0], 50.0);
        assert_eq!(sample_channel(&keys, 1.0).unwrap()[0], 15.625);
    }

    /// Step holds the segment's start value until the destination key's own time.
    #[test]
    fn step_holds_until_the_destination() {
        let keys = [key(0.0, 1.0, Ease::Linear), key(2.0, 9.0, Ease::Step)];
        assert_eq!(sample_channel(&keys, 0.5).unwrap()[0], 1.0);
        assert_eq!(sample_channel(&keys, 1.999).unwrap()[0], 1.0);
        assert_eq!(sample_channel(&keys, 2.0).unwrap()[0], 9.0);
    }

    /// Easing is read off the *destination* key, so two adjacent segments can ease differently.
    #[test]
    fn easing_belongs_to_the_destination_key() {
        let keys = [
            key(0.0, 0.0, Ease::Smooth),
            key(2.0, 100.0, Ease::Step),
            key(4.0, 200.0, Ease::Linear),
        ];
        // First segment: `Step` (from its destination), so it holds 0 until t = 2.
        assert_eq!(sample_channel(&keys, 1.0).unwrap()[0], 0.0);
        // Second segment: `Linear`, so its midpoint is the average.
        assert_eq!(sample_channel(&keys, 3.0).unwrap()[0], 150.0);
    }

    /// A looping clip wraps: one duration past the start is the start again. The fixture's times
    /// are binary-exact (halves and quarters), so the wrap is asserted to the bit — `%` is exact
    /// arithmetic, but a decimal like `1.5 % 0.6` carries its operands' representation error.
    #[test]
    fn a_loop_wraps_at_the_duration() {
        let looping = clip(0.5, true, Vec::new());
        assert_eq!(clip_phase(&looping, 0.0), 0.0);
        assert_eq!(clip_phase(&looping, 0.25), 0.25);
        assert_eq!(clip_phase(&looping, 0.5), 0.0);
        assert_eq!(clip_phase(&looping, 1.25), 0.25);
        assert_eq!(clip_phase(&looping, 10.0), 0.0);
    }

    /// A one-shot clamps at the duration and holds there — the final pose, forever.
    #[test]
    fn a_one_shot_holds_the_final_pose() {
        let once = clip(0.6, false, Vec::new());
        assert_eq!(clip_phase(&once, 0.3), 0.3);
        assert_eq!(clip_phase(&once, 0.6), 0.6);
        assert_eq!(clip_phase(&once, 99.0), 0.6);
    }

    /// Degenerate inputs read as the clip's first instant instead of a NaN pose.
    #[test]
    fn degenerate_times_and_durations_read_as_zero() {
        let looping = clip(0.6, true, Vec::new());
        assert_eq!(clip_phase(&looping, -1.0), 0.0);
        assert_eq!(clip_phase(&looping, f64::NAN), 0.0);
        assert_eq!(clip_phase(&looping, f64::INFINITY), 0.0);
        assert_eq!(clip_phase(&clip(0.0, true, Vec::new()), 1.0), 0.0);
    }

    /// A part the clip does not mention poses entirely at rest, and one it does poses only on the
    /// channels the track actually declares.
    #[test]
    fn part_pose_reads_only_the_declared_channels() {
        let keys = vec![key(0.0, 0.0, Ease::Linear), key(2.0, 10.0, Ease::Linear)];
        let doc = clip(2.0, false, vec![rotation_track("arm", keys.clone())]);
        assert_eq!(part_pose(&doc, "torso", 1.0), PartPose::default());
        let arm = part_pose(&doc, "arm", 1.0);
        assert_eq!(arm.rotation, sample_rotation(&keys, 1.0));
        assert_eq!(arm.translation, None, "the track declares no translation");
        assert_eq!(arm.scale, None, "the track declares no scale");
    }

    /// The scale channel samples like translation: per-axis, under the destination's easing.
    #[test]
    fn scale_samples_like_translation() {
        let mut track = rotation_track("body", Vec::new());
        track.scale = vec![key(0.0, 1.0, Ease::Linear), key(2.0, 3.0, Ease::Linear)];
        let doc = clip(2.0, false, vec![track]);
        assert_eq!(part_pose(&doc, "body", 1.0).scale, Some([2.0, 4.0, 6.0]));
    }

    #[test]
    fn joint_pose_is_none_for_an_untracked_target() {
        let doc = clip(
            1.0,
            false,
            vec![rotation_track("a", vec![key(0.0, 0.0, Ease::Linear)])],
        );
        assert_eq!(joint_pose(&doc, "b", 0.5), None);
    }

    #[test]
    fn joint_pose_rotation_is_the_quaternion_sample() {
        let keys = vec![
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [0.0, 170.0, 0.0], Ease::Linear),
        ];
        let doc = clip(1.0, false, vec![rotation_track("a", keys.clone())]);
        let pose = joint_pose(&doc, "a", 0.5).unwrap();
        assert_eq!(pose.rotation, sample_rotation(&keys, 0.5));
        assert_eq!(pose.translation, None);
        assert_eq!(pose.scale, None);
    }

    /// A part's rotation between keys is the shortest-path slerp, not the per-axis Euler lerp: on a
    /// two-axis segment the two disagree.
    #[test]
    fn part_rotation_slerps_rather_than_lerping_euler_axes() {
        let keys = vec![
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [90.0, 90.0, 0.0], Ease::Linear),
        ];
        let doc = clip(1.0, false, vec![rotation_track("arm", keys.clone())]);
        let rotation = part_pose(&doc, "arm", 0.5).rotation.unwrap();
        assert_eq!(Some(rotation), sample_rotation(&keys, 0.5));
        let euler_lerp = euler_xyz_degrees_to_quat(sample_channel(&keys, 0.5).unwrap());
        assert!(
            angle_between(rotation, euler_lerp) > 1.0,
            "slerp and Euler lerp must differ on a two-axis segment"
        );
    }

    fn angle_between(a: [f64; 4], b: [f64; 4]) -> f64 {
        let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs();
        (2.0 * acos(dot.min(1.0))).to_degrees()
    }

    fn rot_key(t: f64, e: [f64; 3], ease: Ease) -> Keyframe {
        Keyframe { t, v: e, ease }
    }

    #[test]
    fn rotation_holds_its_endpoints() {
        let keys = [
            rot_key(1.0, [10.0, 0.0, 0.0], Ease::Linear),
            rot_key(2.0, [50.0, 0.0, 0.0], Ease::Linear),
        ];
        assert_eq!(sample_rotation(&[], 0.0), None);
        assert_eq!(
            sample_rotation(&keys, 0.0),
            Some(euler_xyz_degrees_to_quat([10.0, 0.0, 0.0]))
        );
        assert_eq!(
            sample_rotation(&keys, 9.0),
            Some(euler_xyz_degrees_to_quat([50.0, 0.0, 0.0]))
        );
    }

    #[test]
    fn slerp_lands_on_its_ends_and_takes_the_short_path() {
        let a = euler_xyz_degrees_to_quat([0.0, 0.0, 0.0]);
        let b = euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]);
        for (got, want) in slerp(a, b, 0.0).iter().zip(a) {
            assert!((got - want).abs() < 1e-12);
        }
        for (got, want) in slerp(a, b, 1.0).iter().zip(b) {
            assert!((got - want).abs() < 1e-12);
        }
        let mid = slerp(a, b, 0.5);
        assert!(angle_between(mid, euler_xyz_degrees_to_quat([0.0, 45.0, 0.0])) < 1e-9);
        let flipped = [-b[0], -b[1], -b[2], -b[3]];
        let mid = slerp(a, flipped, 0.5);
        assert!(angle_between(mid, euler_xyz_degrees_to_quat([0.0, 45.0, 0.0])) < 1e-9);
    }

    /// A 170 degree turn about Y slerps to exactly 85 degrees at its midpoint.
    #[test]
    fn rotation_slerps_a_wide_arc() {
        let keys = [
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [0.0, 170.0, 0.0], Ease::Linear),
        ];
        let mid = sample_rotation(&keys, 0.5).unwrap();
        let want = euler_xyz_degrees_to_quat([0.0, 85.0, 0.0]);
        assert!(angle_between(mid, want) < 1e-6);
    }

    /// Two keys 350 degrees apart on paper are 10 degrees apart in rotation: the short way wins.
    #[test]
    fn rotation_takes_the_short_path() {
        let keys = [
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [350.0, 0.0, 0.0], Ease::Linear),
        ];
        let mid = sample_rotation(&keys, 0.5).unwrap();
        let want = euler_xyz_degrees_to_quat([-5.0, 0.0, 0.0]);
        assert!(angle_between(mid, want) < 1e-6);
    }

    #[test]
    fn rotation_step_ease_holds_the_start() {
        let keys = [
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [0.0, 90.0, 0.0], Ease::Step),
        ];
        assert_eq!(
            sample_rotation(&keys, 0.99),
            Some(euler_xyz_degrees_to_quat([0.0, 0.0, 0.0]))
        );
    }

    /// Today's linear-scan samplers, kept as the oracle for the binary search and `ClipSampler`.
    fn reference_segment(keys: &[Keyframe], t: f64) -> (&Keyframe, &Keyframe, f64) {
        let index = keys.iter().rposition(|key| key.t <= t).unwrap();
        let (from, to) = (&keys[index], &keys[index + 1]);
        let span = to.t - from.t;
        let u = if span > 0.0 { (t - from.t) / span } else { 1.0 };
        (from, to, ease_factor(to.ease, u))
    }

    fn reference_channel(keys: &[Keyframe], t: f64) -> Option<[f64; 3]> {
        let first = keys.first()?;
        if t.is_nan() || t <= first.t {
            return Some(first.v);
        }
        let last = keys.last().unwrap();
        if t >= last.t {
            return Some(last.v);
        }
        let (from, to, u) = reference_segment(keys, t);
        Some(std::array::from_fn(|a| lerp(from.v[a], to.v[a], u)))
    }

    fn reference_rotation(keys: &[Keyframe], t: f64) -> Option<[f64; 4]> {
        let first = keys.first()?;
        if t.is_nan() || t <= first.t {
            return Some(euler_xyz_degrees_to_quat(first.v));
        }
        let last = keys.last().unwrap();
        if t >= last.t {
            return Some(euler_xyz_degrees_to_quat(last.v));
        }
        let (from, to, u) = reference_segment(keys, t);
        Some(slerp(
            euler_xyz_degrees_to_quat(from.v),
            euler_xyz_degrees_to_quat(to.v),
            u,
        ))
    }

    fn bits3(v: Option<[f64; 3]>) -> Option<[u64; 3]> {
        v.map(|v| v.map(f64::to_bits))
    }

    fn bits4(v: Option<[f64; 4]>) -> Option<[u64; 4]> {
        v.map(|v| v.map(f64::to_bits))
    }

    type PoseBits = (Option<[u64; 4]>, Option<[u64; 3]>, Option<[u64; 3]>);

    fn pose_bits(pose: Option<PartPose>) -> Option<PoseBits> {
        pose.map(|p| (bits4(p.rotation), bits3(p.translation), bits3(p.scale)))
    }

    fn vector_track(
        target: &str,
        translation: Vec<Keyframe>,
        scale: Vec<Keyframe>,
    ) -> AnimationTrack {
        AnimationTrack {
            target: target.to_string(),
            rotation: Vec::new(),
            translation,
            scale,
        }
    }

    /// A clip that fires every branch of the samplers, and the targets it is bound to.
    fn branchy_clip() -> (AnimationDocument, Vec<&'static str>) {
        let wide = vec![
            rot_key(0.0, [0.0, 0.0, 0.0], Ease::Linear),
            rot_key(0.25, [90.0, 120.0, 0.0], Ease::Smooth),
            rot_key(0.5, [90.0, 120.0, 0.0], Ease::Linear),
            rot_key(0.75, [350.0, 0.0, 0.0], Ease::Step),
            rot_key(1.0, [0.0, 0.0, 0.0], Ease::Linear),
        ];
        let nearly_parallel = vec![
            rot_key(0.0, [10.0, 0.0, 0.0], Ease::Linear),
            rot_key(0.5, [10.000001, 0.0, 0.0], Ease::Linear),
            rot_key(1.0, [10.000002, 0.0, 0.0], Ease::Smooth),
        ];
        let tracks = vec![
            rotation_track("late", wide),
            vector_track(
                "slide",
                vec![
                    key(0.0, 1.0, Ease::Linear),
                    key(0.3, 4.0, Ease::Smooth),
                    key(1.0, 2.0, Ease::Step),
                ],
                vec![key(0.1, 1.0, Ease::Linear), key(0.9, 2.0, Ease::Smooth)],
            ),
            rotation_track("tiny", nearly_parallel),
            vector_track("scaled", Vec::new(), vec![key(0.0, 1.0, Ease::Linear)]),
            // A second track for `late`: the first must win.
            rotation_track(
                "late",
                vec![
                    rot_key(0.0, [1.0, 2.0, 3.0], Ease::Linear),
                    rot_key(1.0, [4.0, 5.0, 6.0], Ease::Linear),
                ],
            ),
        ];
        (
            clip(1.0, false, tracks),
            vec!["scaled", "none", "late", "tiny", "slide", "late"],
        )
    }

    #[test]
    fn binary_search_and_the_sampler_are_bit_identical_to_the_linear_scan() {
        let (doc, targets) = branchy_clip();
        let sampler = ClipSampler::new(&doc, targets.iter().copied());
        let mut times = vec![-1.0, f64::NAN, 2.0, 1.0, 0.0, f64::INFINITY];
        for track in &doc.tracks {
            for channel in [&track.rotation, &track.translation, &track.scale] {
                for pair in channel.windows(2) {
                    times.extend([pair[0].t, pair[1].t, (pair[0].t + pair[1].t) / 2.0]);
                }
            }
        }
        times.extend((0..=1100).map(|ms| f64::from(ms) / 1000.0));
        for t in times {
            for track in &doc.tracks {
                assert_eq!(
                    bits3(sample_channel(&track.translation, t)),
                    bits3(reference_channel(&track.translation, t)),
                    "translation at {t}"
                );
                assert_eq!(
                    bits3(sample_channel(&track.scale, t)),
                    bits3(reference_channel(&track.scale, t)),
                    "scale at {t}"
                );
                assert_eq!(
                    bits4(sample_rotation(&track.rotation, t)),
                    bits4(reference_rotation(&track.rotation, t)),
                    "rotation at {t}"
                );
            }
            for (index, target) in targets.iter().enumerate() {
                assert_eq!(
                    pose_bits(sampler.pose(index, t)),
                    pose_bits(joint_pose(&doc, target, t)),
                    "{target} at {t}"
                );
            }
            assert_eq!(sampler.pose(targets.len(), t), None);
        }
        assert_eq!(sampler.pose(1, 0.5), None, "a target with no track");
        let late = sampler.pose(2, 0.5).unwrap().rotation;
        assert_eq!(
            late,
            sample_rotation(&track(&doc, "late").unwrap().rotation, 0.5),
            "the first track for a target wins"
        );
    }

    /// A three-frame flipbook at `fps`, holding each frame once unless `hold` says otherwise.
    fn flipbook(fps: f64, looped: bool, hold: Option<Vec<u32>>) -> Flipbook {
        Flipbook {
            name: "fly".to_string(),
            frames: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            fps,
            looped,
            hold,
        }
    }

    /// Length is counted in whole clip frames and divided by `fps` exactly once.
    #[test]
    fn a_flipbooks_length_counts_its_holds() {
        assert_eq!(flipbook(4.0, true, None).total_holds(), 3);
        assert_eq!(flipbook(4.0, true, None).duration(), 0.75);
        let held = flipbook(2.0, true, Some(vec![2, 1, 3]));
        assert_eq!(held.total_holds(), 6);
        assert_eq!(held.duration(), 3.0);
    }

    /// A looping clip advances one frame per `1 / fps` and wraps at its own length.
    #[test]
    fn a_looping_flipbook_wraps() {
        let fb = flipbook(4.0, true, None);
        assert_eq!(flipbook_frame(&fb, 0.0, 1.0), 0);
        // The tick boundary belongs to the frame it opens, not the one it closes.
        assert_eq!(flipbook_frame(&fb, 0.24, 1.0), 0);
        assert_eq!(flipbook_frame(&fb, 0.25, 1.0), 1);
        assert_eq!(flipbook_frame(&fb, 0.5, 1.0), 2);
        assert_eq!(flipbook_frame(&fb, 0.75, 1.0), 0);
        // A second wrap: clip frame 4 of a three-frame loop.
        assert_eq!(flipbook_frame(&fb, 1.0, 1.0), 1);
    }

    /// A one-shot clamps to its last frame and holds it forever.
    #[test]
    fn a_one_shot_flipbook_holds_its_last_frame() {
        let fb = flipbook(4.0, false, None);
        assert_eq!(flipbook_frame(&fb, 0.5, 1.0), 2);
        assert_eq!(flipbook_frame(&fb, 0.75, 1.0), 2);
        assert_eq!(flipbook_frame(&fb, 900.0, 1.0), 2);
    }

    /// `hold` stretches a frame over the clip frames it covers, and the wrap is over the *summed*
    /// length rather than the frame count.
    #[test]
    fn holds_stretch_their_frames() {
        let fb = flipbook(1.0, true, Some(vec![2, 1, 3]));
        let shown: Vec<usize> = (0..7)
            .map(|t| flipbook_frame(&fb, f64::from(t), 1.0))
            .collect();
        assert_eq!(shown, vec![0, 0, 1, 2, 2, 2, 0]);
    }

    /// At 15, 30 and 60 fps every tick that lands on a frame boundary opens that frame, at speed 1
    /// and at speed 0.5 (a half-speed tick count), with no drift across a wrap: `ticks × fps` is
    /// exact, so dividing by the rate last cannot round a boundary below itself the way
    /// `tick / 60 × fps` does.
    #[test]
    fn flipbook_frame_is_exact_on_tick_boundaries() {
        for fps in [15u64, 30, 60] {
            let fb = Flipbook {
                frames: (0..7).map(|i| format!("f{i}")).collect(),
                ..flipbook(fps as f64, true, None)
            };
            for tick in 0..=600u64 {
                assert_eq!(
                    flipbook_frame(&fb, tick as f64, 60.0),
                    (tick * fps / 60 % 7) as usize,
                    "{fps} fps, tick {tick}, speed 1"
                );
                assert_eq!(
                    flipbook_frame(&fb, tick as f64 * 0.5, 60.0),
                    (tick * fps / 120 % 7) as usize,
                    "{fps} fps, tick {tick}, speed 0.5"
                );
            }
        }
    }

    /// Degenerate input shows the first frame instead of an invented index.
    #[test]
    fn degenerate_flipbook_input_shows_the_first_frame() {
        let fb = flipbook(4.0, true, None);
        assert_eq!(flipbook_frame(&fb, -1.0, 1.0), 0);
        assert_eq!(flipbook_frame(&fb, f64::NAN, 1.0), 0);
        assert_eq!(flipbook_frame(&fb, f64::INFINITY, 1.0), 0);
        assert_eq!(flipbook_frame(&fb, 5.0, 0.0), 0);
        assert_eq!(flipbook_frame(&fb, 5.0, f64::NAN), 0);
        assert_eq!(flipbook_frame(&flipbook(0.0, true, None), 5.0, 1.0), 0);
        let empty = Flipbook {
            frames: Vec::new(),
            ..flipbook(4.0, true, None)
        };
        assert_eq!(flipbook_frame(&empty, 5.0, 1.0), 0);
    }
}
