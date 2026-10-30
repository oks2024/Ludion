//! Sun shadow cascades: the camera-fitted orthographic boxes a sun with `cascades` renders and
//! samples, as plain `Copy` data.
//!
//! Pure CPU and deterministic: [`fit_sun_cascades`] is a function of the [`RenderScene`] (camera, sun,
//! post), the [`QualityParams`] and whether shafts run this frame. Every transcendental goes through
//! `ludion_core::math`.
//!
//! # Splits
//!
//! With `n` the camera near and `d_max = min(shadow_distance, camera far)`, N cascades split the view
//! depth range `[n, d_max]` at the practical scheme
//! `C_i = λ·n·(d_max/n)^(i/N) + (1−λ)·(n + (d_max − n)·i/N)`, `λ =` [`SUN_CASCADE_LAMBDA`]. The
//! authored `cascade_splits` (fractions of `shadow_distance`) apply only when N equals the authored
//! count; a quality-reduced N re-fits with the practical scheme. On a frame with shafts the last far
//! bound reaches `max(d_max, min(3 / fog_density, camera far))` (the ray-march reach); the inner
//! splits never change.
//!
//! # The fit
//!
//! Each slice's eight frustum corners are unprojected through the camera's inverse projection, so the
//! fit holds for a perspective and an orthographic camera alike. The bounding sphere is measured in
//! **view space** — its radius depends on neither the camera's position nor its yaw — and only its
//! centre goes to world. The box is that sphere plus one texel of margin per side, and its centre is
//! snapped to the texel lattice of a light rotation fixed per sun, so texel edges sit at fixed world
//! positions as the camera moves and shadow edges do not shimmer. The margin keeps the snapped box
//! covering the whole sphere. The depth range is `shadow_near`/`shadow_far` around the cascade's
//! centre, widened to contain the sphere.
//!
//! # Picking
//!
//! A fragment picks the first cascade whose `far` exceeds its view depth (`near`/`far` are view
//! depths in metres).

use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::math::powf;

use crate::quality::{MAX_SUN_CASCADES, QualityParams};
use crate::scene::{RenderScene, SunCascadeConfig};

/// Blend of the logarithmic and uniform split schemes: `1` is fully logarithmic.
pub const SUN_CASCADE_LAMBDA: f32 = 0.75;

/// Smallest shadow map edge the fit accepts: below it the one-texel margin is the whole box.
const MIN_TILE: u32 = 4;

/// One fitted cascade.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct SunCascade {
    /// World→light-clip matrix of the cascade's box, wgpu 0..1 depth.
    pub light_space: Mat4,
    /// View depth, in metres, where the slice starts.
    pub near: f32,
    /// View depth, in metres, where the slice ends.
    pub far: f32,
    /// World metres per shadow texel.
    pub texel: f32,
    /// Depth extent of the box (`depth_far − depth_near`, metres), for converting a world-unit bias.
    pub depth_range: f32,
}

/// The cascades of one frame; slots `count..` are [`Default`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SunCascades {
    /// How many of `cascades` are fitted, `1..=`[`MAX_SUN_CASCADES`].
    pub count: u32,
    /// Nearest slice first.
    pub cascades: [SunCascade; MAX_SUN_CASCADES as usize],
}

/// Fits the sun's cascades to the frame's camera, or `None` when the scene has no sun, the sun
/// authors no `cascades`, the shadow map is too small, or the camera's range is degenerate.
/// `shafts` is whether the frame runs the shafts pass.
pub fn fit_sun_cascades(
    scene: &RenderScene,
    quality: &QualityParams,
    shafts: bool,
) -> Option<SunCascades> {
    let sun = scene.sun.as_ref()?;
    let config = sun.cascades?;
    let tile = quality.shadow_map_size;
    if tile < MIN_TILE {
        return None;
    }
    let count = quality.sun_cascade_count(Some(config.count))?;

    let camera = &scene.camera;
    let inverse_projection = camera.projection.inverse();
    let unproject = |x: f32, y: f32, z: f32| inverse_projection.project_point3(Vec3::new(x, y, z));
    let near_corners =
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| unproject(x, y, 0.0));
    let far_corners =
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| unproject(x, y, 1.0));
    let camera_near = -near_corners[0].z;
    let camera_far = -far_corners[0].z;
    if !(camera_near.is_finite() && camera_far.is_finite() && camera_near < camera_far) {
        return None;
    }

    let fog = scene.post.as_ref().map_or(0.0, |post| post.fog_density);
    let bounds = split_depths(&config, count, camera_near, camera_far, shafts, fog)?;

    let up = if sun.direction.y.abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let rotation = Mat4::look_to_rh(Vec3::ZERO, sun.direction, up);
    let view_to_world = camera.view.inverse();

    let mut out = SunCascades {
        count,
        cascades: [SunCascade::default(); MAX_SUN_CASCADES as usize],
    };
    for index in 0..count as usize {
        let (near, far) = (bounds[index], bounds[index + 1]);
        let corners = slice_corners(
            &near_corners,
            &far_corners,
            camera_near,
            camera_far,
            near,
            far,
        );
        out.cascades[index] =
            fit_cascade(&corners, near, far, &config, rotation, view_to_world, tile);
    }
    Some(out)
}

/// The `count + 1` view-depth bounds `[near, c_1, …, last far]`, strictly ascending; `None` for a
/// degenerate range.
fn split_depths(
    config: &SunCascadeConfig,
    count: u32,
    camera_near: f32,
    camera_far: f32,
    shafts: bool,
    fog_density: f32,
) -> Option<[f32; MAX_SUN_CASCADES as usize + 1]> {
    let distance = if config.shadow_distance.is_finite() && config.shadow_distance > 0.0 {
        config.shadow_distance
    } else {
        100.0
    };
    let d_max = distance.min(camera_far);
    if d_max <= camera_near {
        return None;
    }
    let last = if shafts && fog_density > 0.0 {
        d_max.max((3.0 / fog_density).min(camera_far))
    } else {
        d_max
    };

    let n = count as usize;
    let mut bounds = [0.0; MAX_SUN_CASCADES as usize + 1];
    bounds[0] = camera_near;
    // The authored splits apply only to the authored count, and only while they stay inside the
    // range actually covered (a camera far plane can cut `shadow_distance` short).
    let authored = config
        .splits
        .filter(|_| config.count as usize == n && n > 1)
        .map(|splits| splits.map(|fraction| fraction * distance))
        .filter(|inner| {
            inner[..n - 1]
                .iter()
                .all(|depth| *depth > camera_near && *depth < d_max)
        });
    for i in 1..n {
        bounds[i] = match authored {
            Some(inner) => inner[i - 1],
            None => {
                let t = i as f32 / n as f32;
                SUN_CASCADE_LAMBDA * camera_near * powf(d_max / camera_near, t)
                    + (1.0 - SUN_CASCADE_LAMBDA) * (camera_near + (d_max - camera_near) * t)
            }
        };
    }
    bounds[n] = last;
    Some(bounds)
}

/// The eight view-space corners of the slice between view depths `near` and `far`: the camera's
/// near and far corner rings interpolated per edge, which is exact for a perspective and an
/// orthographic camera.
fn slice_corners(
    near_corners: &[Vec3; 4],
    far_corners: &[Vec3; 4],
    camera_near: f32,
    camera_far: f32,
    near: f32,
    far: f32,
) -> [Vec3; 8] {
    let at = |depth: f32, corner: usize| {
        let t = (depth - camera_near) / (camera_far - camera_near);
        near_corners[corner] + (far_corners[corner] - near_corners[corner]) * t
    };
    std::array::from_fn(|i| at(if i < 4 { near } else { far }, i % 4))
}

/// One cascade's box: the slice's view-space bounding sphere, snapped to the texel lattice of
/// `rotation`.
fn fit_cascade(
    corners: &[Vec3; 8],
    near: f32,
    far: f32,
    config: &SunCascadeConfig,
    rotation: Mat4,
    view_to_world: Mat4,
    tile: u32,
) -> SunCascade {
    let centre = corners.iter().copied().sum::<Vec3>() / 8.0;
    let radius = corners
        .iter()
        .map(|corner| (*corner - centre).length())
        .fold(0.0, f32::max);

    // One texel of margin per side, so a snap of less than a texel never uncovers the sphere.
    let texel = 2.0 * radius / (tile - 2) as f32;
    let half = texel * tile as f32 * 0.5;

    let light = rotation.transform_point3(view_to_world.transform_point3(centre));
    let snapped_x = (light.x / texel).floor() * texel;
    let snapped_y = (light.y / texel).floor() * texel;

    let depth = -light.z;
    let depth_near = depth + config.shadow_near.min(-radius);
    let depth_far = depth + config.shadow_far.max(radius);
    let projection = Mat4::orthographic_rh(
        snapped_x - half,
        snapped_x + half,
        snapped_y - half,
        snapped_y + half,
        depth_near,
        depth_far,
    );
    SunCascade {
        light_space: projection * rotation,
        near,
        far,
        texel,
        depth_range: depth_far - depth_near,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{PostData, RenderCamera, SunData, aspect_ratio};

    const EXTENT: (u32, u32) = (1920, 1080);

    fn config(count: u32, distance: f32, splits: Option<[f32; 3]>) -> SunCascadeConfig {
        SunCascadeConfig {
            count,
            shadow_distance: distance,
            splits,
            shadow_near: -60.0,
            shadow_far: 60.0,
        }
    }

    fn scene_with(cascades: Option<SunCascadeConfig>) -> RenderScene {
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
        scene.sun = Some(SunData {
            direction: Vec3::new(0.3, -0.75, 0.55).normalize(),
            color: Vec3::ONE,
            light_space: Mat4::IDENTITY,
            bias: 0.0,
            angular_radius: 0.0,
            cascades,
        });
        scene
    }

    fn move_camera(scene: &mut RenderScene, view: Mat4) {
        scene.camera.view = view;
        scene.camera.position = view.inverse().transform_point3(Vec3::ZERO);
    }

    fn fit(scene: &RenderScene, quality: &QualityParams, shafts: bool) -> SunCascades {
        fit_sun_cascades(scene, quality, shafts).expect("a fit")
    }

    fn fars(fit: &SunCascades) -> Vec<f32> {
        fit.cascades[..fit.count as usize]
            .iter()
            .map(|c| c.far)
            .collect()
    }

    #[test]
    fn no_sun_or_no_cascades_is_none() {
        let mut scene = scene_with(None);
        assert!(fit_sun_cascades(&scene, &QualityParams::FULL, false).is_none());
        scene.sun = None;
        assert!(fit_sun_cascades(&scene, &QualityParams::FULL, false).is_none());
        let scene = scene_with(Some(config(4, 100.0, None)));
        let tiny = QualityParams {
            shadow_map_size: 0,
            ..QualityParams::FULL
        };
        assert!(fit_sun_cascades(&scene, &tiny, false).is_none());
    }

    #[test]
    fn four_cascades_tile_the_range() {
        let scene = scene_with(Some(config(4, 100.0, None)));
        let fit = fit(&scene, &QualityParams::FULL, false);
        assert_eq!(fit.count, 4);
        assert!((fit.cascades[0].near - 0.1).abs() < 1e-5);
        for pair in fit.cascades[..4].windows(2) {
            assert_eq!(pair[0].far, pair[1].near);
            assert!(pair[0].near < pair[0].far);
        }
        assert!(fit.cascades[3].near < fit.cascades[3].far);
        assert_eq!(fit.cascades[3].far, 100.0);
    }

    #[test]
    fn authored_splits_are_fractions_of_the_distance() {
        let scene = scene_with(Some(config(4, 60.0, Some([0.06, 0.15, 0.4]))));
        let fit = fit(&scene, &QualityParams::FULL, false);
        for (got, want) in fars(&fit).iter().zip([3.6, 9.0, 24.0, 60.0]) {
            assert!((got - want).abs() < 1e-5, "{got} vs {want}");
        }
    }

    #[test]
    fn the_quality_cap_refits_with_practical_splits() {
        let scene = scene_with(Some(config(4, 60.0, Some([0.06, 0.15, 0.4]))));
        let medium = fit(&scene, &QualityParams::MEDIUM, false);
        assert_eq!(medium.count, 2);
        let n = 0.1_f32;
        let want = SUN_CASCADE_LAMBDA * n * powf(60.0 / n, 0.5)
            + (1.0 - SUN_CASCADE_LAMBDA) * (n + (60.0 - n) * 0.5);
        let got = fars(&medium);
        assert!((got[0] - want).abs() < 1e-4, "{} vs {want}", got[0]);
        assert_eq!(got[1], 60.0);
        assert_eq!(fit(&scene, &QualityParams::HIGH, false).count, 3);
    }

    #[test]
    fn one_cascade_spans_the_whole_range() {
        let scene = scene_with(Some(config(1, 100.0, None)));
        let fit = fit(&scene, &QualityParams::FULL, false);
        assert_eq!(fit.count, 1);
        assert!((fit.cascades[0].near - 0.1).abs() < 1e-5);
        assert_eq!(fit.cascades[0].far, 100.0);
    }

    #[test]
    fn shafts_extend_only_the_last_far() {
        let mut scene = scene_with(Some(config(4, 60.0, None)));
        let plain = fars(&fit(&scene, &QualityParams::FULL, true));
        assert_eq!(plain[3], 60.0, "no post, no extension");

        scene.post = Some(PostData {
            fog_density: 0.02,
            ..PostData::default()
        });
        let shafted = fars(&fit(&scene, &QualityParams::FULL, true));
        assert_eq!(shafted[..3], plain[..3]);
        assert!((shafted[3] - 150.0).abs() < 1e-3, "{}", shafted[3]);
        assert_eq!(fars(&fit(&scene, &QualityParams::FULL, false))[3], 60.0);

        scene.camera.projection =
            Mat4::perspective_rh(60.0_f32.to_radians(), aspect_ratio(EXTENT), 0.1, 120.0);
        let clamped = fars(&fit(&scene, &QualityParams::FULL, true));
        assert!((clamped[3] - 120.0).abs() < 0.12, "{}", clamped[3]);
    }

    #[test]
    fn the_distance_is_clipped_to_the_camera_far() {
        let scene = scene_with(Some(config(4, 2000.0, None)));
        let fit = fit(&scene, &QualityParams::FULL, false);
        let got = fars(&fit);
        assert!((got[3] - 1000.0).abs() < 1.0, "{}", got[3]);
        assert!(got.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn every_slice_corner_lands_inside_its_cascade() {
        let mut scene = scene_with(Some(config(4, 100.0, None)));
        move_camera(
            &mut scene,
            Mat4::look_at_rh(
                Vec3::new(7.3, 2.1, -4.9),
                Vec3::new(1.0, 0.0, -20.0),
                Vec3::Y,
            ),
        );
        let fit = fit(&scene, &QualityParams::FULL, false);
        let inverse = scene.camera.projection.inverse();
        let view_to_world = scene.camera.view.inverse();
        let ring = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        let near_ring = ring.map(|(x, y)| inverse.project_point3(Vec3::new(x, y, 0.0)));
        let far_ring = ring.map(|(x, y)| inverse.project_point3(Vec3::new(x, y, 1.0)));
        for cascade in &fit.cascades[..fit.count as usize] {
            let corners = slice_corners(
                &near_ring,
                &far_ring,
                0.1,
                1000.0,
                cascade.near,
                cascade.far,
            );
            for corner in corners {
                let ndc = cascade
                    .light_space
                    .project_point3(view_to_world.transform_point3(corner));
                let eps = 1e-3;
                assert!(
                    ndc.x.abs() <= 1.0 + eps && ndc.y.abs() <= 1.0 + eps,
                    "{ndc:?} outside cascade {cascade:?}"
                );
                assert!(ndc.z >= -eps && ndc.z <= 1.0 + eps, "{ndc:?}");
            }
        }
    }

    /// The fractional texel coordinate of a fixed world point under `cascade`.
    fn texel_phase(cascade: &SunCascade, point: Vec3, tile: u32) -> (f32, f32) {
        let ndc = cascade.light_space.project_point3(point);
        let coord = |v: f32| (v + 1.0) * 0.5 * tile as f32;
        (coord(ndc.x).fract(), coord(ndc.y).fract())
    }

    #[test]
    fn a_sub_texel_camera_move_does_not_slide_the_lattice() {
        let tile = QualityParams::FULL.shadow_map_size;
        let base_eye = Vec3::new(2.0, 1.0, 3.0);
        let make = |eye: Vec3| {
            let mut scene = scene_with(Some(config(4, 100.0, None)));
            move_camera(&mut scene, Mat4::look_at_rh(eye, eye - Vec3::Z, Vec3::Y));
            scene
        };
        let base = fit(&make(base_eye), &QualityParams::FULL, false);
        let direction = make(base_eye).sun.unwrap().direction;
        let lateral = direction.cross(Vec3::Y).normalize();
        let probe = Vec3::new(1.234, 0.5, -3.21);
        for axis in [Vec3::X, lateral] {
            let moved_scene = make(base_eye + axis * 0.3 * base.cascades[0].texel);
            let moved = fit(&moved_scene, &QualityParams::FULL, false);
            for i in 0..4 {
                assert_eq!(base.cascades[i].texel, moved.cascades[i].texel);
                let a = texel_phase(&base.cascades[i], probe, tile);
                let b = texel_phase(&moved.cascades[i], probe, tile);
                let wrap = |d: f32| (d - d.round()).abs();
                assert!(
                    wrap(a.0 - b.0) < 1e-3 && wrap(a.1 - b.1) < 1e-3,
                    "cascade {i}: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn a_yaw_keeps_every_radius() {
        let eye = Vec3::new(0.0, 1.0, 0.0);
        let mut a = scene_with(Some(config(4, 100.0, None)));
        move_camera(&mut a, Mat4::look_at_rh(eye, eye - Vec3::Z, Vec3::Y));
        let mut b = a.clone();
        let yawed = Mat4::from_rotation_y(37.0_f32.to_radians());
        move_camera(
            &mut b,
            Mat4::look_to_rh(eye, yawed.transform_vector3(-Vec3::Z), Vec3::Y),
        );
        let (fa, fb) = (
            fit(&a, &QualityParams::FULL, false),
            fit(&b, &QualityParams::FULL, false),
        );
        for i in 0..4 {
            assert_eq!(fa.cascades[i].texel, fb.cascades[i].texel, "cascade {i}");
        }
    }

    #[test]
    fn an_orthographic_camera_fits() {
        let mut scene = scene_with(Some(config(4, 100.0, None)));
        scene.camera.projection = Mat4::orthographic_rh(
            -20.0 * aspect_ratio(EXTENT),
            20.0 * aspect_ratio(EXTENT),
            -20.0,
            20.0,
            0.1,
            1000.0,
        );
        let fit = fit(&scene, &QualityParams::FULL, false);
        let got = fars(&fit);
        assert!(got.windows(2).all(|pair| pair[0] < pair[1]), "{got:?}");
        for cascade in &fit.cascades[..4] {
            assert!(
                cascade
                    .light_space
                    .to_cols_array()
                    .iter()
                    .all(|v| v.is_finite())
            );
            assert!(cascade.texel > 0.0);
        }
    }

    #[test]
    fn the_fit_is_deterministic_and_clamps_a_rust_built_count() {
        let scene = scene_with(Some(config(9, 100.0, None)));
        let a = fit(&scene, &QualityParams::FULL, false);
        assert_eq!(a, fit(&scene, &QualityParams::FULL, false));
        assert_eq!(a.count, MAX_SUN_CASCADES);
    }
}
