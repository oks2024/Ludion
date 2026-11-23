//! Blend spaces resolved against a rig's bound clips (`Plan/animation-layers.md` decisions 13-19 and
//! 31), shared by the skinned and voxel `kind: assembly` bodies: the resolver is neutral over the rig,
//! taking each bound clip's `(name, duration, speed, events)`.

use ludion_scene::{AnimationEvent, BlendAt, BlendSpace};

/// One blend point resolved against its clip binding (`Plan/animation-layers.md` decisions 13-16).
#[derive(Clone, Debug, PartialEq)]
pub struct BlendPointData {
    /// The bound clip's short name.
    pub name: String,
    /// The bound clip's index in the rig's clip list.
    pub clip: usize,
    /// The point's position: `[x, 0]` for a 1D blend.
    pub at: [f64; 2],
    /// The clip's length in seconds (positive).
    pub duration: f64,
    /// The binding's playback speed multiplier.
    pub speed: f64,
    /// The sync offset `s = t_sync / duration`; `0` without a `sync` event.
    pub sync: f64,
    /// The clip's effective events (its own, else the binding's overlay), time-ascending.
    pub events: Vec<AnimationEvent>,
}

/// A blend space resolved against the bound clips: its points, and for a 2D blend the
/// triangulation its weights interpolate over.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendSpaceData {
    /// The short name `Animation.clip` selects by.
    pub name: String,
    /// The points, in authored order.
    pub points: Vec<BlendPointData>,
    /// Whether the positions are 2D (`params[0..2]`) rather than 1D (`params[0]`).
    pub two_d: bool,
    /// The 2D triangulation of the points (empty for a 1D blend).
    pub triangles: Vec<[u16; 3]>,
}

impl BlendSpaceData {
    /// The point weights at `params`.
    pub fn weights(&self, params: [f32; 2]) -> ludion_scene::BlendWeights {
        let params = params.map(|p| if p.is_finite() { p } else { 0.0 });
        if self.two_d {
            let at: Vec<[f64; 2]> = self.points.iter().map(|p| p.at).collect();
            ludion_scene::blend_weights_2d(
                &at,
                &self.triangles,
                [f64::from(params[0]), f64::from(params[1])],
            )
        } else {
            let at: Vec<f64> = self.points.iter().map(|p| p.at[0]).collect();
            ludion_scene::blend_weights_1d(&at, f64::from(params[0]))
        }
    }

    /// The shared phase rate in cycles per second, before `Animation.speed`:
    /// `sum(weight * binding_speed / duration)`.
    pub fn rate(&self, weights: &ludion_scene::BlendWeights) -> f64 {
        weights
            .iter()
            .map(|(i, w)| {
                let point = &self.points[usize::from(i)];
                w * point.speed / point.duration
            })
            .sum()
    }

    /// The clip time point `index` plays at on shared phase `phase` (unwrapped cycles).
    pub fn point_time(&self, index: u16, phase: f64) -> f64 {
        let point = &self.points[usize::from(index)];
        let x = phase + point.sync;
        (x - x.floor()) * point.duration
    }
}

/// One bound clip as the resolver sees it: a character passes its effective events (own, else the
/// overlay) and binding speed, an assembly a clip's own events at speed 1.
pub(crate) struct BoundClip<'a> {
    pub name: &'a str,
    pub duration: f64,
    pub speed: f64,
    pub events: &'a [AnimationEvent],
}

/// Resolves each blend against the bound clips; the rig's cross-file check has already proved every
/// point names a bound, looping clip.
pub(crate) fn resolve_blends(
    blends: &[BlendSpace],
    clips: &[BoundClip<'_>],
) -> Vec<BlendSpaceData> {
    blends
        .iter()
        .map(|blend| {
            let points: Vec<BlendPointData> = blend
                .points
                .iter()
                .map(|point| {
                    let clip = clips
                        .iter()
                        .position(|c| c.name == point.clip)
                        .expect("the cross-file check proved the point's clip is bound");
                    let bound = &clips[clip];
                    let sync = blend
                        .sync
                        .as_deref()
                        .and_then(|name| bound.events.iter().find(|event| event.name == name))
                        .map_or(0.0, |event| event.t / bound.duration);
                    BlendPointData {
                        name: point.clip.clone(),
                        clip,
                        at: match point.at {
                            BlendAt::One(x) => [x, 0.0],
                            BlendAt::Two(at) => at,
                        },
                        duration: bound.duration,
                        speed: bound.speed,
                        sync,
                        events: bound.events.to_vec(),
                    }
                })
                .collect();
            let two_d = matches!(blend.points[0].at, BlendAt::Two(_));
            let triangles = if two_d {
                let at: Vec<[f64; 2]> = points.iter().map(|p| p.at).collect();
                ludion_scene::triangulate(&at)
            } else {
                Vec::new()
            };
            BlendSpaceData {
                name: blend.name.clone(),
                points,
                two_d,
                triangles,
            }
        })
        .collect()
}
