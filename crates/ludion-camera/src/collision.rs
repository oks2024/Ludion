//! The collision stage: a sphere cast from the pivot to the eye that pulls the boom in at once and
//! eases it back out after a hold. The crate stays physics-free; the engine implements the seam.

use ludion_core::math::sqrt;

use crate::rig::{RigCollisionConfig, RigState};
use crate::spring::damp;

/// The sphere cast a rig's collision stage needs; the engine implements it over its physics world.
///
/// # Example
///
/// ```
/// use ludion_camera::RigCollision;
///
/// /// A wall at z = -2 facing +z.
/// #[derive(Debug)]
/// struct Wall;
/// impl RigCollision for Wall {
///     fn cast(&self, from: [f32; 3], dir: [f32; 3], max: f32, radius: f32) -> Option<f32> {
///         if dir[2] >= 0.0 {
///             return None;
///         }
///         let d = (from[2] + 2.0 - radius) / -dir[2];
///         (d <= max).then_some(d.max(0.0))
///     }
/// }
/// assert_eq!(Wall.cast([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], 5.0, 0.5), Some(1.5));
/// ```
pub trait RigCollision: core::fmt::Debug {
    /// Distance along the unit `dir` from `from` to the first hit of a sphere of `radius`, within
    /// `max`; `Some(0.0)` when the sphere starts overlapping; `None` when clear.
    fn cast(&self, from: [f32; 3], dir: [f32; 3], max: f32, radius: f32) -> Option<f32>;
}

/// Below this a boom counts as the full length again and the state returns to default.
const FREE_EPS: f32 = 1e-4;

fn offset(a: [f32; 3], b: [f32; 3]) -> ([f32; 3], f32) {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d, sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]))
}

/// Clears the collided boom so the state is back at default.
pub(crate) fn release(state: &mut RigState) {
    state.boom = None;
    state.boom_hold = 0.0;
}

/// Casts from `origin` toward `eye` and returns the collided eye and whether both `origin` and
/// `target` start inside geometry. Updates `state.boom` and `state.boom_hold`.
pub(crate) fn collide(
    state: &mut RigState,
    cfg: &RigCollisionConfig,
    collision: &dyn RigCollision,
    origin: [f32; 3],
    eye: [f32; 3],
    target: [f32; 3],
    game_dt: f32,
) -> ([f32; 3], bool) {
    let (delta, full) = offset(eye, origin);
    if full < FREE_EPS || !full.is_finite() {
        release(state);
        return (eye, false);
    }
    let dir = delta.map(|v| v / full);
    let mut stuck = false;
    let reach =
        |hit: Option<f32>| hit.map_or(full, |d| (d - cfg.skin).max(cfg.min_distance).min(full));
    let allowed = match collision.cast(origin, dir, full, cfg.radius) {
        Some(d) if d <= 0.0 => {
            let (to_eye, from_target) = offset(eye, target);
            if from_target < FREE_EPS {
                full
            } else {
                let dir = to_eye.map(|v| v / from_target);
                match collision.cast(target, dir, from_target, cfg.radius) {
                    Some(d) if d <= 0.0 => {
                        stuck = true;
                        cfg.min_distance.min(full)
                    }
                    Some(d) => reach(Some(full - (from_target - d))),
                    None => full,
                }
            }
        }
        hit => reach(hit),
    };
    let current = state.boom.unwrap_or(full);
    let boom = if allowed < current {
        state.boom_hold = cfg.recover_delay;
        allowed
    } else if game_dt > 0.0 {
        if state.boom_hold > 0.0 {
            state.boom_hold = (state.boom_hold - game_dt).max(0.0);
            current
        } else {
            damp(current, allowed, cfg.recover_half_life, game_dt)
        }
    } else {
        current
    };
    if boom >= full - FREE_EPS {
        release(state);
        return (eye, stuck);
    }
    state.boom = Some(boom);
    ([0, 1, 2].map(|axis| origin[axis] + dir[axis] * boom), stuck)
}

/// The eye of a boom from `origin` to `eye` held at the tick's collided length, without casting.
pub(crate) fn held_eye(state: &RigState, origin: [f32; 3], eye: [f32; 3]) -> [f32; 3] {
    let Some(boom) = state.boom else {
        return eye;
    };
    let (delta, full) = offset(eye, origin);
    if full < FREE_EPS || boom >= full {
        return eye;
    }
    [0, 1, 2].map(|axis| origin[axis] + delta[axis] / full * boom)
}
