//! Which local lights scatter into the `PostProcess` medium this frame.
//!
//! Pure CPU and deterministic. At most [`MAX_SCATTER_LIGHTS`] lights are chosen from the lights
//! that opted in (`scatter > 0`): the result is empty unless the scene's medium has
//! `fog_density > 0`, and a candidate's footprint (the cluster binner's own sphere) must touch the
//! camera frustum and lie inside the march's reach.
//!
//! # Rank and ties
//!
//! A candidate's weight is `scatter * intensity * max(color) * range * exp(-σ d)`, `σ` the fog
//! density and `d` the camera distance to the footprint surface (0 inside it). Heaviest first; an
//! exact tie resolves to the lower light-list index, which is [`RenderScene::lights`] followed by
//! [`RenderScene::spot_lights`] (the order `cluster.rs` packs them in).
//!
//! The survivors are returned in ascending light-list index, so the GPU's sum order is list order
//! and the result does not depend on the weights' order.

use ludion_core::math::glam::Vec3;

use crate::cluster::light_sphere;
use crate::cull::Frustum;
use crate::scene::RenderScene;

/// The most lights one frame scatters.
pub(crate) const MAX_SCATTER_LIGHTS: usize = 64;

/// Optical depth `σ d` past which a light adds nothing to the medium. Mirrors `shafts.wgsl`'s
/// `SHAFT_OPTICAL_CUTOFF`.
const SCATTER_OPTICAL_CUTOFF: f32 = 3.0;

/// Light-list indices (points `0..P`, then spots `P..`), ascending, at most [`MAX_SCATTER_LIGHTS`].
pub(crate) fn select_scatter(scene: &RenderScene) -> Vec<u32> {
    let Some(sigma) = scene
        .post
        .as_ref()
        .map(|post| post.fog_density)
        .filter(|sigma| sigma.is_finite() && *sigma > 0.0)
    else {
        return Vec::new();
    };
    let reach = SCATTER_OPTICAL_CUTOFF / sigma;
    let frustum = Frustum::from_view_proj(scene.camera.projection * scene.camera.view);
    let camera = scene.camera.position;
    let forward = scene.camera.view_forward_lane();
    let point_count = scene.lights.len();

    let mut candidates: Vec<(f32, u32)> = Vec::new();
    let mut consider =
        |index: usize, apex: Vec3, axis: Vec3, light: (f32, f32, f32, f32), cos_outer: f32| {
            let (scatter, intensity, peak, range) = light;
            // Each test is false for NaN, so a non-finite light is never a candidate.
            if scatter <= 0.0 || scatter.is_nan() {
                return;
            }
            let (center, radius) = light_sphere(apex, axis, range, cos_outer);
            let touches = center.is_finite()
                && radius.is_finite()
                && frustum.intersects_sphere(center, radius);
            // Perspective rays start at the eye; orthographic ones on the camera plane, so
            // distance runs along the view forward.
            let to_center = center - camera;
            let along = if forward[3] == 1.0 {
                to_center.dot(Vec3::new(forward[0], forward[1], forward[2]))
            } else {
                to_center.length()
            };
            let d = (along - radius).max(0.0);
            let w = scatter * intensity * peak * range * ludion_core::math::exp(-sigma * d);
            if touches && d < reach && w.is_finite() && w > 0.0 {
                candidates.push((w, index as u32));
            }
        };
    for (i, l) in scene.lights.iter().enumerate() {
        consider(
            i,
            l.position,
            Vec3::ZERO,
            (l.scatter, l.intensity, l.color.max_element(), l.range),
            -2.0,
        );
    }
    for (i, l) in scene.spot_lights.iter().enumerate() {
        consider(
            point_count + i,
            l.position,
            l.direction,
            (l.scatter, l.intensity, l.color.max_element(), l.range),
            l.cos_outer,
        );
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    candidates.truncate(MAX_SCATTER_LIGHTS);
    let mut out: Vec<u32> = candidates.into_iter().map(|(_, i)| i).collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{
        PointLightData, PostData, RenderCamera, RenderScene, SpotLightData, aspect_ratio,
    };
    use ludion_core::math::glam::Mat4;

    const EXTENT: (u32, u32) = (1920, 1080);

    /// A camera at the origin looking down -Z, in a medium of density `fog`.
    fn scene_at_origin(fog: f32) -> RenderScene {
        let mut scene = RenderScene::empty(EXTENT);
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                aspect_ratio(EXTENT),
                0.1,
                1000.0,
            ),
            position: Vec3::ZERO,
        };
        scene.post = Some(PostData {
            fog_density: fog,
            ..PostData::default()
        });
        scene
    }

    fn point(position: Vec3, range: f32, intensity: f32, scatter: f32) -> PointLightData {
        PointLightData {
            position,
            color: Vec3::ONE,
            intensity,
            range,
            dynamic: false,
            shadow: None,
            scatter,
        }
    }

    fn spot(position: Vec3, scatter: f32) -> SpotLightData {
        SpotLightData {
            position,
            direction: -Vec3::Z,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 3.0,
            cos_inner: 0.95,
            cos_outer: 0.9,
            shadow: None,
            scatter,
        }
    }

    #[test]
    fn no_medium_selects_nothing() {
        let mut scene = scene_at_origin(0.0);
        scene.lights = vec![point(Vec3::new(0.0, 0.0, -5.0), 3.0, 1.0, 1.0)];
        assert!(select_scatter(&scene).is_empty());
        scene.post = None;
        assert!(select_scatter(&scene).is_empty());
        scene.post = Some(PostData {
            fog_density: 0.05,
            ..PostData::default()
        });
        assert_eq!(select_scatter(&scene), vec![0]);
    }

    #[test]
    fn zero_scatter_is_not_a_candidate() {
        let mut scene = scene_at_origin(0.05);
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, -5.0), 3.0, 1.0, 0.0),
            point(Vec3::new(0.0, 0.0, -5.0), 3.0, 1.0, 1.0),
        ];
        assert_eq!(select_scatter(&scene), vec![1]);
    }

    #[test]
    fn a_light_behind_the_camera_is_dropped() {
        let mut scene = scene_at_origin(0.05);
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, 10.0), 2.0, 1.0, 1.0),
            point(Vec3::new(0.0, 0.0, -10.0), 2.0, 1.0, 1.0),
        ];
        assert_eq!(select_scatter(&scene), vec![1]);
    }

    #[test]
    fn a_light_past_the_march_reach_is_dropped() {
        // σ = 0.1 reaches 30 m to the footprint surface; a 2 m sphere at 32 m has d = 30.
        let mut scene = scene_at_origin(0.1);
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, -32.0), 2.0, 1.0, 1.0),
            point(Vec3::new(0.0, 0.0, -31.5), 2.0, 1.0, 1.0),
        ];
        assert_eq!(select_scatter(&scene), vec![1]);
    }

    #[test]
    fn ortho_reach_is_measured_along_the_view() {
        let mut scene = scene_at_origin(0.1);
        scene.camera.projection = Mat4::orthographic_rh(-50.0, 50.0, -50.0, 50.0, 0.1, 1000.0);
        // Radial distance 41 m exceeds the 30 m reach, the along-view distance (10 m) does not.
        scene.lights = vec![
            point(Vec3::new(40.0, 0.0, -10.0), 2.0, 1.0, 1.0),
            point(Vec3::new(0.0, 0.0, -40.0), 2.0, 1.0, 1.0),
        ];
        assert_eq!(select_scatter(&scene), vec![0]);
    }

    #[test]
    fn the_cap_keeps_the_64_heaviest_and_returns_them_ascending() {
        let mut scene = scene_at_origin(0.01);
        // Intensity 1 + ((i * 37) % 100) is distinct per light and scrambled in index order.
        let intensity = |i: usize| 1.0 + ((i * 37) % 100) as f32;
        scene.lights = (0..100)
            .map(|i| point(Vec3::new(0.0, 0.0, -5.0), 1.0, intensity(i), 1.0))
            .collect();
        let picked = select_scatter(&scene);
        assert_eq!(picked.len(), MAX_SCATTER_LIGHTS);
        assert!(picked.windows(2).all(|w| w[0] < w[1]));
        let mut by_weight: Vec<usize> = (0..100).collect();
        by_weight.sort_by(|a, b| intensity(*b).total_cmp(&intensity(*a)));
        let mut want: Vec<u32> = by_weight[..64].iter().map(|i| *i as u32).collect();
        want.sort_unstable();
        assert_eq!(picked, want);
    }

    #[test]
    fn equal_weights_tie_to_the_lower_index() {
        let mut scene = scene_at_origin(0.05);
        scene.lights = (0..70)
            .map(|_| point(Vec3::new(0.0, 0.0, -5.0), 1.0, 1.0, 1.0))
            .collect();
        assert_eq!(select_scatter(&scene), (0..64).collect::<Vec<u32>>());
    }

    #[test]
    fn spots_index_after_points() {
        let mut scene = scene_at_origin(0.05);
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, -5.0), 1.0, 1.0, 1.0),
            point(Vec3::new(0.0, 0.0, -6.0), 1.0, 1.0, 0.0),
            point(Vec3::new(0.0, 0.0, -7.0), 1.0, 1.0, 1.0),
        ];
        scene.spot_lights = vec![
            spot(Vec3::new(0.0, 0.0, -5.0), 0.0),
            spot(Vec3::new(0.0, 0.0, -5.0), 1.0),
        ];
        assert_eq!(select_scatter(&scene), vec![0, 2, 4]);
    }
}
