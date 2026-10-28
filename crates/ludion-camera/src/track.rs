//! The `track` pose: the target projected onto a path, the eye at an offset in its frame.

use ludion_scene::{PathMetric, PathSampler};

use crate::rig::{RigConfig, RigMode};
use crate::spring::look_angles;

/// Where a rig wants the camera this tick, before springs.
pub(crate) struct Desired {
    pub param: f32,
    pub eye: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
}

/// The desired pose for `target` with the anchored height and smoothed look-ahead. `hint` is last
/// tick's projection (a windowed search); `None` searches the whole path. `last` is the
/// `(yaw, pitch)` kept when the look is degenerate.
pub(crate) fn desired(
    cfg: &RigConfig,
    sampler: &PathSampler,
    target: [f32; 3],
    anchor_y: f32,
    look_ahead: [f32; 2],
    hint: Option<f32>,
    last: (f32, f32),
) -> Desired {
    let RigMode::Track(track) = &cfg.mode;
    let p = [
        f64::from(target[0]),
        f64::from(anchor_y),
        f64::from(target[2]),
    ];
    let s = match hint {
        Some(hint) => sampler.nearest_s(
            p,
            f64::from(hint),
            f64::from(track.search_window),
            PathMetric::Full,
        ),
        None => sampler.nearest_s_global(p, PathMetric::Full),
    };
    let frame = sampler.frame_at(s);
    let [ox, oy, oz] = track.offset.map(f64::from);
    let eye = [0, 1, 2].map(|i| {
        (frame.position[i] + ox * frame.right[i] + oy * frame.up[i] - oz * frame.tangent[i]) as f32
    });
    let focus = [
        target[0] + cfg.focus_offset[0] + look_ahead[0],
        anchor_y + cfg.focus_offset[1],
        target[2] + cfg.focus_offset[2] + look_ahead[1],
    ];
    let (yaw_deg, pitch_deg) = look_angles(eye, focus, last.0).unwrap_or(last);
    Desired {
        param: s as f32,
        eye,
        yaw_deg,
        pitch_deg,
    }
}
