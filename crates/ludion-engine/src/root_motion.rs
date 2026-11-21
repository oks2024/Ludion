//! Root motion on voxel assemblies: the displacement a clip's reserved `$root` track authors
//! (`Plan/animation-layers.md` decisions 55-60, request 0007 section 5).
//!
//! A `kind: animation` clip may carry a `$root` track: horizontal translation (`v[0]`, `v[2]`) and
//! yaw (rotation `v[1]`) only, keyed in the entity's frame at the clip's start, in the assembly's
//! units, forward being `-Z`. The track is never drawn and nothing applies it for you:
//! [`root_motion_delta`] is a pure sim read a game feeds to its motor.

use ludion_core::ids::EntityId;
use ludion_core::math::f64::{cos, euler_xyz_degrees_to_quat, sin};
use ludion_core::time::GameTime;
use ludion_ecs::{SystemCtx, World};
use ludion_scene::sample_channel;

use crate::animation_events::{AnimationLibrary, RootClip};
use crate::components::{Animation, Mesh, MeshSource, Transform};
use crate::extract::{clock_elapsed, fade_weight};

/// The most loop cycles one window visits, the latest ones: an absurd speed never stalls the sum.
const MAX_CYCLES: u64 = 64;

/// One tick of root motion from [`root_motion_delta`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RootMotion {
    /// The world-frame displacement in metres for [`move_character`](crate::move_character); `y` is
    /// always 0 (vertical travel belongs to the motor).
    pub translation: [f32; 3],
    /// The yaw change in radians; apply it as `turn_character(world, id, current + yaw)`.
    pub yaw: f32,
    /// The summed weight of the sides that carry a `$root` track (1 for a plain clip, the fade
    /// shares during a crossfade): scale code-driven movement by `1 - weight`.
    pub weight: f32,
}

/// This tick's root motion of the assembly `entity` is playing, or `None` when no side of its
/// base clip carries a `$root` track, or it has no `Animation`, no assembly `Mesh` or no library.
///
/// **Window.** The clip time is the exact gameplay clock (speed, anchor and `game_scale` included,
/// not the held pose clock) over `(previous instant, this instant]`. A one-shot clamps to its
/// duration, a loop wraps (the latest 64 cycles), a held frame or a clip started this tick opens
/// at 0, so windows tile and a scaled clock neither drops nor repeats travel. A reversed clock
/// yields nothing.
///
/// **Crossfade.** While `Animation.fade` runs, the incoming clip counts at the fade weight and the
/// outgoing `from` clip at its complement; a side that is a blend, unknown, lacks `$root` or is a
/// `PoseSnapshot` contributes nothing. [`RootMotion::weight`] is the summed weight. Layers never
/// contribute: root motion is the base clip's.
///
/// **Frames.** Travel accumulates per key segment in the frame the entity faces at the segment
/// start, then is scaled by the entity's own `Transform.scale` (x, z) and rotated by its own
/// `Transform.rotation`: the frame `move_character` moves a root motor in. The turn is absolute in
/// `turn_character`, so add [`RootMotion::yaw`] to the current yaw.
///
/// ```
/// use ludion_engine::{
///     Animation, Engine, Mesh, Transform, move_character, root_motion_delta, turn_character,
/// };
/// use ludion_protocol::{Command, command};
///
/// let mut engine = Engine::builder()
///     .system("root_motion", |world, ctx| {
///         let ids: Vec<_> = world.query_ids::<(&Animation, &Mesh, &Transform)>().map(|(id, _)| id).collect();
///         for id in ids {
///             if let Some(rm) = root_motion_delta(&*world, id, ctx) {
///                 let _ = move_character(world, id, rm.translation);
///                 let yaw = world.get::<Transform>(id).ok().flatten().map_or(0.0, |t| t.rotation[1].to_radians());
///                 let _ = turn_character(world, id, yaw + rm.yaw);
///             }
///         }
///     })
///     .query::<(&Animation, &Mesh, &Transform)>()
///     .build()
///     .unwrap();
/// engine
///     .execute(Command::SimStep(command::SimStepParams { ticks: 1, hash: false }))
///     .unwrap();
/// ```
pub fn root_motion_delta(world: &World, entity: EntityId, ctx: &SystemCtx) -> Option<RootMotion> {
    let animation = world.get::<Animation>(entity).ok().flatten()?;
    let mesh = world.get::<Mesh>(entity).ok().flatten()?;
    let MeshSource::Assembly(rel) = &mesh.shape else {
        return None;
    };
    let transform = world.get::<Transform>(entity).ok().flatten()?;
    let roots = world.resource::<AnimationLibrary>()?.root_clips(rel);

    let now = ctx.game_instant;
    let prev = if ctx.game_advanced() {
        let step_q16 = (ctx.game_scale() * 65536.0) as u64;
        GameTime::from_q16(now.as_q16().saturating_sub(step_q16), now.rate())
    } else {
        now
    };
    let (incoming, outgoing) = match fade_weight(animation, now) {
        Some(w) => (w, 1.0 - w),
        None => (1.0, 0.0),
    };
    let sides = [
        (
            &animation.clip,
            animation.started,
            animation.speed,
            animation.anchor.as_ref(),
            incoming,
        ),
        (
            &animation.from,
            animation.from_started,
            animation.from_speed,
            animation.from_anchor.as_ref(),
            outgoing,
        ),
    ];
    let (mut travel, mut yaw, mut weight) = ([0.0f64; 2], 0.0f64, 0.0f64);
    for (name, started, speed, anchor, share) in sides {
        let Some(clip) = roots.get(name.as_str()).filter(|_| share > 0.0) else {
            continue;
        };
        let tp = clock_elapsed(prev, started, speed, anchor);
        let tn = clock_elapsed(now, started, speed, anchor);
        let (side_travel, side_yaw) = window_delta(clip, tp, tn);
        travel[0] += share * side_travel[0];
        travel[1] += share * side_travel[1];
        yaw += share * side_yaw;
        weight += share;
    }
    if weight <= 0.0 {
        return None;
    }
    let scaled = [
        travel[0] * f64::from(transform.scale[0]),
        0.0,
        travel[1] * f64::from(transform.scale[2]),
    ];
    let q = euler_xyz_degrees_to_quat(transform.rotation.map(f64::from));
    let world_frame = rotate(q, scaled);
    Some(RootMotion {
        translation: [world_frame[0] as f32, 0.0, world_frame[2] as f32],
        yaw: yaw as f32,
        weight: weight as f32,
    })
}

/// `v` rotated by the quaternion `q = [x, y, z, w]`, in plain arithmetic.
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    [
        v[0] + q[3] * t[0] + ut[0],
        v[1] + q[3] * t[1] + ut[1],
        v[2] + q[3] * t[2] + ut[2],
    ]
}

/// `[x, z]` turned about the vertical axis by `theta` radians (glam's `from_rotation_y`).
fn rot_y(theta: f64, v: [f64; 2]) -> [f64; 2] {
    let (s, c) = (sin(theta), cos(theta));
    [v[0] * c + v[1] * s, -v[0] * s + v[1] * c]
}

/// The travel `[x, z]` (in the frame at `tp`) and yaw (radians) a clip authors over the clip times
/// `(tp, tn]`, both unwrapped seconds.
fn window_delta(clip: &RootClip, tp: f64, tn: f64) -> ([f64; 2], f64) {
    if tn.is_nan() || tp.is_nan() || tn <= tp {
        return ([0.0; 2], 0.0);
    }
    let d = clip.duration;
    let position = |t: f64| sample_channel(&clip.translation, t).map_or([0.0; 2], |v| [v[0], v[2]]);
    let heading = |t: f64| sample_channel(&clip.rotation, t).map_or(0.0, |v| v[1]);
    let segments = if clip.looped {
        let (cycle_p, cycle_n) = ((tp / d).floor(), (tn / d).floor());
        let (r_p, r_n) = (
            (tp - cycle_p * d).clamp(0.0, d),
            (tn - cycle_n * d).clamp(0.0, d),
        );
        if cycle_p == cycle_n {
            vec![(r_p, r_n)]
        } else {
            let whole = ((cycle_n - cycle_p - 1.0).max(0.0) as u64).min(MAX_CYCLES);
            let mut segments = vec![(r_p, d)];
            segments.extend((0..whole).map(|_| (0.0, d)));
            segments.push((0.0, r_n));
            segments
        }
    } else {
        vec![(tp.clamp(0.0, d), tn.clamp(0.0, d))]
    };
    let (mut pos, mut yaw) = ([0.0f64; 2], 0.0f64);
    for (a, b) in segments {
        let (pa, pb) = (position(a), position(b));
        let local = rot_y(-heading(a).to_radians(), [pb[0] - pa[0], pb[1] - pa[1]]);
        let moved = rot_y(yaw, local);
        pos = [pos[0] + moved[0], pos[1] + moved[1]];
        yaw += (heading(b) - heading(a)).to_radians();
    }
    (pos, yaw)
}

#[cfg(test)]
mod tests {
    use ludion_scene::{Ease, Keyframe};

    use super::*;

    fn key(t: f64, v: [f64; 3]) -> Keyframe {
        Keyframe {
            t,
            v,
            ease: Ease::Linear,
        }
    }

    fn clip(looped: bool, translation: &[(f64, [f64; 3])], rotation: &[(f64, f64)]) -> RootClip {
        RootClip {
            duration: 1.0,
            looped,
            translation: translation.iter().map(|&(t, v)| key(t, v)).collect(),
            rotation: rotation
                .iter()
                .map(|&(t, y)| key(t, [0.0, y, 0.0]))
                .collect(),
        }
    }

    fn walk(looped: bool) -> RootClip {
        clip(looped, &[(0.0, [0.0; 3]), (1.0, [0.0, 0.0, -8.0])], &[])
    }

    #[test]
    fn a_one_shot_clamps_to_its_duration() {
        let c = walk(false);
        assert_eq!(window_delta(&c, 0.5, 5.0).0, [0.0, -4.0]);
        assert_eq!(window_delta(&c, 2.0, 3.0), ([0.0; 2], 0.0));
        assert_eq!(window_delta(&c, -1.0, 0.25).0, [0.0, -2.0]);
    }

    #[test]
    fn a_reversed_or_empty_window_is_zero() {
        let c = walk(false);
        assert_eq!(window_delta(&c, 0.5, 0.5), ([0.0; 2], 0.0));
        assert_eq!(window_delta(&c, 0.5, 0.2), ([0.0; 2], 0.0));
    }

    #[test]
    fn a_loop_wrap_tiles() {
        let c = walk(true);
        let spanning = window_delta(&c, 0.75, 2.5).0;
        let (a, b) = (
            window_delta(&c, 0.75, 1.25).0,
            window_delta(&c, 1.25, 2.5).0,
        );
        assert!((spanning[1] - (a[1] + b[1])).abs() < 1e-12, "{spanning:?}");
        assert!((spanning[1] - -14.0).abs() < 1e-12);
    }

    #[test]
    fn a_yaw_free_clip_reduces_to_a_plain_difference() {
        let c = clip(
            false,
            &[(0.0, [1.0, 0.0, 2.0]), (1.0, [4.0, 0.0, -6.0])],
            &[],
        );
        assert_eq!(window_delta(&c, 0.0, 1.0), ([3.0, -8.0], 0.0));
    }

    #[test]
    fn a_ninety_degree_segment_rotates_travel() {
        // Turn 90 degrees while the track walks 8 forward in the start frame: the second half is
        // travelled in the turned frame.
        let c = clip(
            false,
            &[(0.0, [0.0; 3]), (1.0, [0.0, 0.0, -8.0])],
            &[(0.0, 0.0), (1.0, 90.0)],
        );
        let (pos, yaw) = window_delta(&c, 0.0, 1.0);
        assert!((yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        // Heading at the segment start is 0, so the key delta is taken in the start frame.
        assert!(
            (pos[0]).abs() < 1e-12 && (pos[1] - -8.0).abs() < 1e-12,
            "{pos:?}"
        );
        let split = window_delta(&c, 0.0, 0.5);
        let rest = window_delta(&c, 0.5, 1.0);
        assert!((split.1 + rest.1 - yaw).abs() < 1e-12);
    }

    #[test]
    fn a_turned_frame_rotates_the_following_travel() {
        let c = clip(
            false,
            &[(0.0, [0.0; 3]), (1.0, [0.0, 0.0, -8.0])],
            &[(0.0, 90.0), (1.0, 90.0)],
        );
        // Facing +90 about Y the clip-frame -Z key delta is expressed in the faced frame.
        let (pos, _) = window_delta(&c, 0.0, 1.0);
        let expected = rot_y(-std::f64::consts::FRAC_PI_2, [0.0, -8.0]);
        assert!((pos[0] - expected[0]).abs() < 1e-12 && (pos[1] - expected[1]).abs() < 1e-12);
    }
}
