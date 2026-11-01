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
//! covering the whole sphere. The depth range is centred on the cascade, half
//! of `shadow_far − shadow_near` to each side (the anchored box's convention), widened to contain the
//! sphere.
//!
//! # Picking
//!
//! A fragment picks the first cascade whose `far` exceeds its view depth (`near`/`far` are view
//! depths in metres).
//!
//! # Atlas
//!
//! The cascades share one depth texture of `shadow_map_size`-edge tiles, laid out row-major by
//! [`sun_atlas_grid`]: 1×1, 2×1 or 2×2. A three-cascade atlas leaves its last tile cleared.

use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::math::powf;

use crate::cull::{Aabb, CullVolume, Frustum, cull, instance_bounds};
use crate::graph::PostChain;
use crate::quality::{MAX_SUN_CASCADES, Quality, QualityParams};
use crate::scene::{RenderCamera, RenderScene, SunCascadeConfig};

/// Blend of the logarithmic and uniform split schemes: `1` is fully logarithmic.
pub const SUN_CASCADE_LAMBDA: f32 = 0.75;

/// World metres per shadow texel of the default 40 m anchored box at 2048². The authored bias holds
/// the same number of texels in every cascade, so a cascade's bias scales by its texel over this.
pub const SUN_CASCADE_BIAS_TEXEL: f32 = 80.0 / 2048.0;

/// Smallest shadow map edge the fit accepts: below it the one-texel margin is the whole box.
pub const SUN_CASCADE_MIN_TILE: u32 = 4;

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

/// The atlas grid of `count` cascades as `(columns, rows)`; `count` clamps into
/// `1..=`[`MAX_SUN_CASCADES`].
pub fn sun_atlas_grid(count: u32) -> (u32, u32) {
    match count.clamp(1, MAX_SUN_CASCADES) {
        1 => (1, 1),
        2 => (2, 1),
        _ => (2, 2),
    }
}

/// The `(column, row)` of cascade `index` in the atlas of `count` cascades.
pub fn sun_atlas_tile(count: u32, index: u32) -> (u32, u32) {
    let (columns, _) = sun_atlas_grid(count);
    (index % columns, index / columns)
}

/// A camera's unprojected near and far corner rings and its view-depth range.
pub(crate) struct CascadeCamera {
    near_corners: [Vec3; 4],
    far_corners: [Vec3; 4],
    near: f32,
    far: f32,
}

/// Unprojects `camera`'s near and far rings, or `None` unless its view-depth range is finite with
/// `0 < near < far`: the one test that decides whether cascades can be fitted to a camera.
pub(crate) fn cascade_camera(camera: &RenderCamera) -> Option<CascadeCamera> {
    let inverse_projection = camera.projection.inverse();
    let unproject = |x: f32, y: f32, z: f32| inverse_projection.project_point3(Vec3::new(x, y, z));
    let near_corners =
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| unproject(x, y, 0.0));
    let far_corners =
        [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(x, y)| unproject(x, y, 1.0));
    let near = -near_corners[0].z;
    let far = -far_corners[0].z;
    (near.is_finite() && far.is_finite() && 0.0 < near && near < far).then_some(CascadeCamera {
        near_corners,
        far_corners,
        near,
        far,
    })
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
    if tile < SUN_CASCADE_MIN_TILE {
        return None;
    }
    let count = quality.sun_cascade_count(Some(config.count))?;

    let camera = &scene.camera;
    let CascadeCamera {
        near_corners,
        far_corners,
        near: camera_near,
        far: camera_far,
    } = cascade_camera(camera)?;

    let fog = scene.post.as_ref().map_or(0.0, |post| post.fog_density);
    let bounds = split_depths(&config, count, camera_near, camera_far, shafts, fog);

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

/// The caster indices each fitted cascade's box keeps, nearest cascade first, by culling
/// `caster_bounds` (per instance, as `cull::instance_bounds` returns them) against the box.
///
/// The frame's shadow pass and [`sun_cascade_stats`] both call this, so the profile panel and the
/// draw cannot disagree about which casters a cascade keeps.
pub(crate) fn sun_cascade_casters(
    fit: &SunCascades,
    caster_bounds: &[Option<Aabb>],
) -> Vec<Vec<u32>> {
    fit.cascades[..fit.count as usize]
        .iter()
        .map(|cascade| {
            let volume = CullVolume::Frustum(Frustum::from_view_proj(cascade.light_space));
            cull(caster_bounds, &volume)
        })
        .collect()
}

/// What the frame's sun cascades decided, for the profile report.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SunCascadeStats {
    /// The cascade count the sun authors, 0 for a sun without `cascades` or a sunless scene.
    pub authored: u32,
    /// The cascades fitted this frame: `min(authored, the quality cap)`, 0 when none ran.
    pub count: u32,
    /// The edge of one cascade's shadow tile in texels, 0 when none ran.
    pub tile: u32,
    /// One entry per fitted cascade, nearest first.
    pub cascades: Vec<SunCascadeStat>,
}

/// One fitted cascade, as the profile reports it.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct SunCascadeStat {
    /// View depth, in metres, where the slice starts.
    pub near: f32,
    /// View depth, in metres, where the slice ends.
    pub far: f32,
    /// World metres per shadow texel.
    pub texel: f32,
    /// Instances the cascade's box keeps for the shadow pass.
    pub casters: u32,
}

/// Fits the frame's cascades and returns only what was decided — the profile panel, with the boxes
/// themselves dropped.
///
/// This is the profile driver's entry point (`ludion_engine::profile`): it does the fit and the
/// per-cascade cull the renderer's per-frame prologue does, over bounds computed on the CPU from the
/// scene's mesh assets, and needs no GPU adapter. Deliberately a separate call rather than a hook
/// inside [`Renderer::render`](crate::Renderer::render): a profiler must never make the path it
/// measures pay for being measured.
pub fn sun_cascade_stats(
    scene: &RenderScene,
    quality: &Quality,
    params: &QualityParams,
) -> SunCascadeStats {
    let authored = scene
        .sun
        .as_ref()
        .and_then(|sun| sun.cascades)
        .map_or(0, |config| config.count);
    let chain = PostChain::for_scene(scene).masked(quality, params);
    let fit = chain
        .sun_cascades
        .and_then(|_| fit_sun_cascades(scene, params, chain.shafts));
    let Some(fit) = fit else {
        return SunCascadeStats {
            authored,
            ..SunCascadeStats::default()
        };
    };
    let asset_bounds: Vec<Option<Aabb>> = scene
        .mesh_assets
        .iter()
        .map(|asset| Aabb::of_positions(&asset.positions))
        .collect();
    let caster_bounds = instance_bounds(&scene.meshes, &asset_bounds);
    let casters = sun_cascade_casters(&fit, &caster_bounds);
    SunCascadeStats {
        authored,
        count: fit.count,
        tile: params.shadow_map_size,
        cascades: fit.cascades[..fit.count as usize]
            .iter()
            .zip(&casters)
            .map(|(cascade, kept)| SunCascadeStat {
                near: cascade.near,
                far: cascade.far,
                texel: cascade.texel,
                casters: kept.len() as u32,
            })
            .collect(),
    }
}

/// The `count + 1` view-depth bounds `[near, c_1, …, last far]`, strictly ascending.
fn split_depths(
    config: &SunCascadeConfig,
    count: u32,
    camera_near: f32,
    camera_far: f32,
    shafts: bool,
    fog_density: f32,
) -> [f32; MAX_SUN_CASCADES as usize + 1] {
    let distance = if config.shadow_distance.is_finite() && config.shadow_distance > 0.0 {
        config.shadow_distance
    } else {
        100.0
    };
    // The chain already said cascades, so a distance short of the camera's near yields a sliver that
    // fades to lit rather than a fit failure that swaps in the anchored box.
    let d_max = distance.min(camera_far).max(camera_near * (1.0 + 1e-3));
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
    bounds
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
    let reach = (0.5 * (config.shadow_far - config.shadow_near)).max(radius);
    let depth_near = depth - reach;
    let depth_far = depth + reach;
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

    #[test]
    fn the_atlas_grid_is_row_major_and_clamps() {
        let grids: Vec<_> = (0..=5).map(sun_atlas_grid).collect();
        assert_eq!(grids, [(1, 1), (1, 1), (2, 1), (2, 2), (2, 2), (2, 2)]);
    }

    #[test]
    fn four_cascade_tiles_fill_the_grid_row_major() {
        let tiles: Vec<_> = (0..4).map(|i| sun_atlas_tile(4, i)).collect();
        assert_eq!(tiles, [(0, 0), (1, 0), (0, 1), (1, 1)]);
        assert_eq!(sun_atlas_tile(2, 1), (1, 0));
    }

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

    fn cube(model: Mat4) -> crate::scene::MeshInstance {
        crate::scene::MeshInstance {
            shape: crate::scene::MeshShape::Cube,
            model,
            normal_matrix: ludion_core::math::glam::Mat3::IDENTITY,
            color: [1.0; 4],
            material: [1.0, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
        }
    }

    #[test]
    fn the_stats_mirror_the_fit_and_the_shared_caster_cull() {
        let mut scene = scene_with(Some(config(4, 100.0, None)));
        scene.meshes = [-2.0, -10.0, -30.0, -80.0]
            .map(|z| cube(Mat4::from_translation(Vec3::new(0.0, 0.0, z))))
            .to_vec();
        let stats = sun_cascade_stats(&scene, &Quality::FULL, &QualityParams::FULL);
        let fit = fit(&scene, &QualityParams::FULL, false);
        assert_eq!((stats.authored, stats.count), (4, fit.count));
        assert_eq!(stats.tile, QualityParams::FULL.shadow_map_size);
        assert_eq!(stats.cascades.len(), 4);
        for (i, stat) in stats.cascades.iter().enumerate() {
            assert_eq!(
                (stat.near, stat.far),
                (fit.cascades[i].near, fit.cascades[i].far)
            );
            assert_eq!(stat.texel, fit.cascades[i].texel);
            if i > 0 {
                assert_eq!(stat.near, stats.cascades[i - 1].far);
                assert!(stat.far > stat.near);
            }
        }
        let bounds = instance_bounds(&scene.meshes, &[]);
        let want = sun_cascade_casters(&fit, &bounds);
        let got: Vec<u32> = stats.cascades.iter().map(|s| s.casters).collect();
        assert_eq!(got, want.iter().map(|k| k.len() as u32).collect::<Vec<_>>());
        assert!(got.iter().all(|&n| n >= 1), "{got:?}");
    }

    #[test]
    fn a_sun_side_caster_50m_away_stays_in_cascade_0() {
        let mut scene = scene_with(Some(SunCascadeConfig {
            shadow_near: 1.0,
            shadow_far: 200.0,
            ..config(4, 100.0, None)
        }));
        move_camera(
            &mut scene,
            Mat4::look_at_rh(
                Vec3::new(3.0, 2.0, 1.0),
                Vec3::new(0.0, 0.0, -20.0),
                Vec3::Y,
            ),
        );
        let fit = fit(&scene, &QualityParams::FULL, false);
        let camera = cascade_camera(&scene.camera).expect("a camera");
        let view_to_world = scene.camera.view.inverse();
        let direction = scene.sun.as_ref().unwrap().direction;
        let cascade = &fit.cascades[0];
        let corners = slice_corners(
            &camera.near_corners,
            &camera.far_corners,
            camera.near,
            camera.far,
            cascade.near,
            cascade.far,
        );
        for corner in corners {
            let caster = view_to_world.transform_point3(corner) - direction * 50.0;
            let ndc = cascade.light_space.project_point3(caster);
            assert!((0.0..=1.0).contains(&ndc.z), "{ndc:?}");
        }
    }

    #[test]
    fn a_symmetric_range_keeps_its_box() {
        let scene = scene_with(Some(config(4, 100.0, None)));
        let tile = QualityParams::FULL.shadow_map_size;
        let fit = fit(&scene, &QualityParams::FULL, false);
        for cascade in &fit.cascades[..4] {
            let radius = cascade.texel * (tile - 2) as f32 * 0.5;
            let want = 2.0 * 60.0_f32.max(radius);
            assert!(
                (cascade.depth_range - want).abs() <= want * 1e-5,
                "{} vs {want}",
                cascade.depth_range
            );
        }
    }

    #[test]
    fn the_chain_and_the_fit_agree_on_every_trigger() {
        let normal = scene_with(Some(config(4, 100.0, None)));
        let sliver = scene_with(Some(config(4, 0.05, None)));
        let mut ortho = normal.clone();
        ortho.camera.projection = Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.0, 100.0);
        let mut nan = normal.clone();
        nan.camera.projection = Mat4::from_cols_array(&[f32::NAN; 16]);
        let tiny_map = QualityParams {
            shadow_map_size: 3,
            ..QualityParams::FULL
        };
        let rows = [
            ("a sliver distance", &sliver, QualityParams::FULL, Some(4)),
            ("a 3-texel map", &normal, tiny_map, None),
            ("an ortho near of 0", &ortho, QualityParams::FULL, None),
            ("a NaN projection", &nan, QualityParams::FULL, None),
            ("the normal scene", &normal, QualityParams::FULL, Some(4)),
        ];
        for (name, scene, params, want) in rows {
            let chain = PostChain::for_scene(scene).masked(&Quality::FULL, &params);
            let fit = fit_sun_cascades(scene, &params, chain.shafts);
            assert_eq!(chain.sun_cascades, want, "{name}: chain");
            assert_eq!(fit.map(|f| f.count), want, "{name}: fit");
        }
        let fit = fit(&sliver, &QualityParams::FULL, false);
        let bounds: Vec<f32> = std::iter::once(fit.cascades[0].near)
            .chain(fars(&fit))
            .collect();
        assert!(bounds.iter().all(|b| b.is_finite()), "{bounds:?}");
        assert!(
            bounds.windows(2).all(|pair| pair[0] < pair[1]),
            "{bounds:?}"
        );
    }

    #[test]
    fn the_stats_are_empty_without_a_fit() {
        let scene = scene_with(Some(config(4, 100.0, None)));
        let low = sun_cascade_stats(&scene, &Quality::LOW, &QualityParams::LOW);
        assert_eq!(
            low,
            SunCascadeStats {
                authored: 4,
                ..SunCascadeStats::default()
            }
        );
        let plain = sun_cascade_stats(&scene_with(None), &Quality::FULL, &QualityParams::FULL);
        assert_eq!(plain, SunCascadeStats::default());
    }
}
