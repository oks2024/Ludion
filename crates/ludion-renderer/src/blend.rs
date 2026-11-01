//! Display-time pose blending: a pure, GPU-free function that moves the pose-driven items of the
//! newest extracted [`RenderScene`] back toward the previous tick's poses.
//!
//! The previous *scene* is never kept. Extract records, per tick, a [`BlendData`]: the world model of
//! every entity that drives a render item, plus a parallel key list per `RenderScene` list. [`blend`]
//! turns two ticks' `BlendData` and the newer scene into the scene to draw at `alpha` between them.

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Quat};

use crate::scene::{RenderScene, SpotLightData};

/// The poses and per-item keys one extracted tick records for [`blend`].
///
/// Each `Vec<Option<EntityId>>` is parallel to the [`RenderScene`] list of the same name: the same
/// length, or empty (every item unkeyed). `Some(key)` names the entity whose world affine moved the
/// item; `None` takes the newer tick unchanged. `poses` and `snaps` are sorted ascending.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BlendData {
    /// Bumped when the world is replaced (restore, scene load): blending across epochs is a cut.
    pub epoch: u64,
    /// The `render_pose` model of every keyed entity, ascending by id.
    pub poses: Vec<(EntityId, Mat4)>,
    /// Keys cut this tick (the entity itself or an ancestor): they take the newer pose, ascending.
    pub snaps: Vec<EntityId>,
    /// The camera's pose and projection inputs, when the scene's camera comes from an entity.
    pub camera: Option<CameraBlend>,
    /// Keys parallel to [`RenderScene::meshes`].
    pub meshes: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::sprites`].
    pub sprites: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::lights`].
    pub lights: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::spot_lights`].
    pub spot_lights: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::texts`].
    pub texts: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::ground_markers`].
    pub ground_markers: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::drop_shadows`].
    pub drop_shadows: Vec<Option<EntityId>>,
    /// Keys parallel to [`RenderScene::decals`].
    pub decals: Vec<Option<EntityId>>,
}

/// What [`blend`] needs to re-derive the camera between two ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraBlend {
    /// The camera entity.
    pub key: EntityId,
    /// The camera's world model (camera space to world).
    pub model: Mat4,
    /// Vertical field of view in degrees; `None` for a projection that is not blended.
    pub fov_y_deg: Option<f32>,
    /// Near plane distance.
    pub near: f32,
    /// Far plane distance.
    pub far: f32,
    /// Viewport aspect ratio.
    pub aspect: f32,
}

/// The scene to draw at `alpha` between the previous tick (`prev`) and the newer one (`next`,
/// described by `next_data`).
///
/// Returns `next.clone()` bit-exactly when `alpha >= 1`, when the epochs differ, or when `prev` holds
/// no poses. Each keyed item moves by the delta that takes its entity's newer pose to the pose
/// interpolated (scale and translation lerped, rotation slerped) between the two ticks; a snapped,
/// newly spawned, unmoved or degenerate key gets no delta. Everything not pose-driven comes from
/// `next`. A keyed list whose length differs from its scene list is treated as unkeyed.
///
/// ```
/// use ludion_renderer::{BlendData, RenderScene, blend};
/// let scene = RenderScene::empty((64, 64));
/// let out = blend(&BlendData::default(), &scene, &BlendData::default(), 0.5);
/// assert_eq!(out, scene);
/// ```
pub fn blend(
    prev: &BlendData,
    next: &RenderScene,
    next_data: &BlendData,
    alpha: f32,
) -> RenderScene {
    let alpha = if alpha.is_nan() {
        1.0
    } else {
        alpha.clamp(0.0, 1.0)
    };
    if alpha >= 1.0 || prev.epoch != next_data.epoch || prev.poses.is_empty() {
        return next.clone();
    }
    let delta = |key: Option<EntityId>| -> Option<Mat4> { delta_for(prev, next_data, key?, alpha) };
    let mut out = next.clone();

    for (inst, key) in out
        .meshes
        .iter_mut()
        .zip(keys(&next_data.meshes, next.meshes.len()))
    {
        if let Some(d) = delta(key) {
            inst.model = d * inst.model;
            let d3 = Mat3::from_mat4(d);
            if d3.determinant().abs() > DET_EPS {
                inst.normal_matrix = d3.inverse().transpose() * inst.normal_matrix;
            }
        }
    }
    for (inst, key) in out
        .sprites
        .iter_mut()
        .zip(keys(&next_data.sprites, next.sprites.len()))
    {
        if let Some(d) = delta(key) {
            inst.center = d.transform_point3(inst.center);
            inst.right = d.transform_vector3(inst.right);
            inst.up = d.transform_vector3(inst.up);
        }
    }
    for (inst, key) in out
        .lights
        .iter_mut()
        .zip(keys(&next_data.lights, next.lights.len()))
    {
        if let Some(d) = delta(key) {
            inst.position = d.transform_point3(inst.position);
        }
    }
    for (inst, key) in out
        .spot_lights
        .iter_mut()
        .zip(keys(&next_data.spot_lights, next.spot_lights.len()))
    {
        if let Some(d) = delta(key) {
            move_spot(inst, d);
        }
    }
    for (inst, key) in out
        .texts
        .iter_mut()
        .zip(keys(&next_data.texts, next.texts.len()))
    {
        if let Some(d) = delta(key) {
            inst.position = d.transform_point3(inst.position);
        }
    }
    for (inst, key) in out
        .ground_markers
        .iter_mut()
        .zip(keys(&next_data.ground_markers, next.ground_markers.len()))
    {
        if let Some(d) = delta(key) {
            // The shape is already scaled to world units: only the translation follows the entity.
            let t = d.transform_point3(inst.model.w_axis.truncate());
            inst.model.w_axis = t.extend(inst.model.w_axis.w);
        }
    }
    for (inst, key) in out
        .drop_shadows
        .iter_mut()
        .zip(keys(&next_data.drop_shadows, next.drop_shadows.len()))
    {
        if let Some(d) = delta(key) {
            inst.anchor = d.transform_point3(inst.anchor);
        }
    }
    for (inst, key) in out
        .decals
        .iter_mut()
        .zip(keys(&next_data.decals, next.decals.len()))
    {
        if let Some(d) = delta(key) {
            inst.box_to_world = d * inst.box_to_world;
            inst.world_to_box = inst.box_to_world.inverse();
        }
    }

    if let (Some(p), Some(n)) = (&prev.camera, &next_data.camera)
        && p.key == n.key
        && next_data.snaps.binary_search(&n.key).is_err()
        && (p.model != n.model || p.fov_y_deg != n.fov_y_deg)
    {
        let model = trs_lerp(p.model, n.model, alpha);
        if model.is_finite() && model.determinant().abs() > DET_EPS {
            out.camera.view = model.inverse();
            out.camera.position = model.w_axis.truncate();
            if let (Some(a), Some(b)) = (p.fov_y_deg, n.fov_y_deg) {
                let fov = a + (b - a) * alpha;
                out.camera.projection =
                    Mat4::perspective_rh(fov.to_radians(), n.aspect, n.near, n.far);
            }
        }
    }
    out
}

const DET_EPS: f32 = 1e-12;

/// The keys for a list of `len` items: the parallel list when it matches, otherwise all unkeyed.
fn keys(list: &[Option<EntityId>], len: usize) -> impl Iterator<Item = Option<EntityId>> + '_ {
    let keyed = list.len() == len;
    (0..len).map(move |i| if keyed { list[i] } else { None })
}

fn pose_of(poses: &[(EntityId, Mat4)], key: EntityId) -> Option<Mat4> {
    poses
        .binary_search_by_key(&key, |(id, _)| *id)
        .ok()
        .map(|i| poses[i].1)
}

/// The world-space delta taking `key`'s newer pose to its pose at `alpha`; `None` when the item
/// should stay exactly where the newer tick put it.
fn delta_for(prev: &BlendData, next: &BlendData, key: EntityId, alpha: f32) -> Option<Mat4> {
    if next.snaps.binary_search(&key).is_ok() {
        return None;
    }
    let w_prev = pose_of(&prev.poses, key)?;
    let w_next = pose_of(&next.poses, key)?;
    if w_prev == w_next {
        return None;
    }
    let det = w_next.determinant();
    if !det.is_finite() || det.abs() < DET_EPS {
        return None;
    }
    let d = trs_lerp(w_prev, w_next, alpha) * w_next.inverse();
    d.is_finite().then_some(d)
}

/// Scale and translation lerped, rotation slerped (shortest arc), recomposed.
fn trs_lerp(a: Mat4, b: Mat4, t: f32) -> Mat4 {
    let (sa, ra, ta) = a.to_scale_rotation_translation();
    let (sb, rb, tb) = b.to_scale_rotation_translation();
    let r: Quat = ra.slerp(rb, t);
    Mat4::from_scale_rotation_translation(sa.lerp(sb, t), r, ta.lerp(tb, t))
}

fn move_spot(inst: &mut SpotLightData, d: Mat4) {
    inst.position = d.transform_point3(inst.position);
    let dir = d.transform_vector3(inst.direction);
    inst.direction = if dir.length_squared() > 0.0 {
        dir.normalize()
    } else {
        inst.direction
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::Vec3;

    use crate::scene::{
        DecalBlend, DecalInstance, DecalMaps, DecalReceivers, DropShadowInstance,
        GroundMarkerInstance, GroundMarkerShape, MeshInstance, MeshShape, PointLightData,
        SpriteInstance, TextRun,
    };

    const E1: EntityId = EntityId::new(1, 0);
    const E2: EntityId = EntityId::new(2, 0);

    fn at(x: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, 0.0, 0.0))
    }

    fn mesh(model: Mat4) -> MeshInstance {
        MeshInstance {
            shape: MeshShape::Cube,
            model,
            normal_matrix: Mat3::from_mat4(model),
            color: [1.0, 0.0, 0.0, 1.0],
            material: [0.5, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            bone_offset: 0,
        }
    }

    fn sprite(center: Vec3) -> SpriteInstance {
        SpriteInstance {
            center,
            right: Vec3::X,
            up: Vec3::Y,
            color: [1.0; 4],
            texture: None,
            uv: SpriteInstance::FULL_UV,
            lit: false,
            layer: 0,
            additive: false,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
        }
    }

    fn point(position: Vec3) -> PointLightData {
        PointLightData {
            position,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 5.0,
            dynamic: false,
            shadow: None,
        }
    }

    fn spot(position: Vec3) -> SpotLightData {
        SpotLightData {
            position,
            direction: Vec3::X,
            color: Vec3::ONE,
            intensity: 1.0,
            range: 5.0,
            cos_inner: 0.9,
            cos_outer: 0.8,
            shadow: None,
        }
    }

    fn text(position: Vec3) -> TextRun {
        TextRun {
            position,
            size: 1.0,
            color: [1.0; 4],
            text: String::new(),
            quads: Vec::new(),
            extent: [0.0, 0.0],
            font: None,
        }
    }

    fn marker(model: Mat4) -> GroundMarkerInstance {
        GroundMarkerInstance {
            shape: GroundMarkerShape::Disc { radius: 2.0 },
            model,
            color: [1.0; 4],
        }
    }

    fn shadow(anchor: Vec3) -> DropShadowInstance {
        DropShadowInstance {
            shape: GroundMarkerShape::Disc { radius: 1.0 },
            anchor,
            yaw: 0.0,
            opacity: 0.5,
            softness: 0.5,
            max_distance: 10.0,
            fade_distance: 2.0,
            exclude: Vec::new(),
        }
    }

    fn decal(box_to_world: Mat4) -> DecalInstance {
        DecalInstance {
            box_to_world,
            world_to_box: box_to_world.inverse(),
            blend: DecalBlend::Multiply,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: DecalMaps::default(),
            receivers: DecalReceivers::Exclude(Vec::new()),
        }
    }

    /// One item of every kind keyed to `E1`, at x = `x`.
    fn world(x: f32) -> (RenderScene, BlendData) {
        let mut s = RenderScene::empty((64, 64));
        s.meshes.push(mesh(at(x)));
        s.sprites.push(sprite(Vec3::new(x, 0.0, 0.0)));
        s.lights.push(point(Vec3::new(x, 0.0, 0.0)));
        s.spot_lights.push(spot(Vec3::new(x, 0.0, 0.0)));
        s.texts.push(text(Vec3::new(x, 0.0, 0.0)));
        s.ground_markers.push(marker(at(x)));
        s.drop_shadows.push(shadow(Vec3::new(x, 0.0, 0.0)));
        s.decals.push(decal(at(x)));
        let k = vec![Some(E1)];
        let d = BlendData {
            epoch: 1,
            poses: vec![(E1, at(x))],
            meshes: k.clone(),
            sprites: k.clone(),
            lights: k.clone(),
            spot_lights: k.clone(),
            texts: k.clone(),
            ground_markers: k.clone(),
            drop_shadows: k.clone(),
            decals: k,
            ..BlendData::default()
        };
        (s, d)
    }

    fn assert_x(s: &RenderScene, x: f32) {
        assert_eq!(s.meshes[0].model.w_axis.x, x);
        assert_eq!(s.sprites[0].center.x, x);
        assert_eq!(s.lights[0].position.x, x);
        assert_eq!(s.spot_lights[0].position.x, x);
        assert_eq!(s.texts[0].position.x, x);
        assert_eq!(s.ground_markers[0].model.w_axis.x, x);
        assert_eq!(s.drop_shadows[0].anchor.x, x);
        assert_eq!(s.decals[0].box_to_world.w_axis.x, x);
    }

    #[test]
    fn midpoint_lands_halfway() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        assert_x(&blend(&p, &s, &n, 0.5), 0.5);
    }

    #[test]
    fn alpha_one_is_next_and_zero_is_previous() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        assert_eq!(blend(&p, &s, &n, 1.0), s);
        assert_eq!(blend(&p, &s, &n, 3.0), s);
        assert_x(&blend(&p, &s, &n, 0.0), 0.0);
        assert_x(&blend(&p, &s, &n, -1.0), 0.0);
    }

    #[test]
    fn snap_spawn_and_epoch_take_next() {
        let (_, p) = world(0.0);
        let (s, mut n) = world(1.0);
        n.snaps = vec![E1];
        assert_eq!(blend(&p, &s, &n, 0.5), s);

        let (_, mut p2) = world(0.0);
        p2.poses = vec![(E2, at(0.0))];
        let (_, n2) = world(1.0);
        assert_eq!(blend(&p2, &s, &n2, 0.5), s);

        let (_, mut p3) = world(0.0);
        p3.epoch = 0;
        let (_, n3) = world(1.0);
        assert_eq!(blend(&p3, &s, &n3, 0.5), s);
    }

    #[test]
    fn unkeyed_and_non_pose_fields_come_from_next() {
        let (_, p) = world(0.0);
        let (mut s, mut n) = world(1.0);
        s.meshes.push(mesh(at(7.0)));
        n.meshes.push(None);
        s.meshes[0].color = [0.0, 1.0, 0.0, 1.0];
        s.meshes[0].fade = 0.25;
        let out = blend(&p, &s, &n, 0.5);
        assert_eq!(out.meshes[1], s.meshes[1]);
        assert_eq!(out.meshes[0].color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(out.meshes[0].fade, 0.25);
        assert_eq!(out.meshes[0].model.w_axis.x, 0.5);
    }

    #[test]
    fn yaw_slerps_to_forty_five_degrees() {
        let prev = Mat4::IDENTITY;
        let next = Mat4::from_rotation_y(90f32.to_radians());
        let mut s = RenderScene::empty((64, 64));
        s.meshes.push(mesh(next));
        let p = BlendData {
            epoch: 1,
            poses: vec![(E1, prev)],
            ..BlendData::default()
        };
        let n = BlendData {
            epoch: 1,
            poses: vec![(E1, next)],
            meshes: vec![Some(E1)],
            ..BlendData::default()
        };
        let out = blend(&p, &s, &n, 0.5);
        let want = Mat4::from_rotation_y(45f32.to_radians());
        assert!(out.meshes[0].model.abs_diff_eq(want, 1e-5));
        assert!(
            out.meshes[0]
                .normal_matrix
                .abs_diff_eq(Mat3::from_mat4(want), 1e-5)
        );
    }

    #[test]
    fn ground_marker_moves_by_translation_only() {
        let prev = Mat4::from_scale(Vec3::splat(1.0));
        let next = Mat4::from_scale_rotation_translation(Vec3::splat(3.0), Quat::IDENTITY, Vec3::X);
        let m = marker(at(1.0));
        let mut s = RenderScene::empty((64, 64));
        s.ground_markers.push(m);
        let p = BlendData {
            epoch: 1,
            poses: vec![(E1, prev)],
            ..BlendData::default()
        };
        let n = BlendData {
            epoch: 1,
            poses: vec![(E1, next)],
            ground_markers: vec![Some(E1)],
            ..BlendData::default()
        };
        let out = blend(&p, &s, &n, 0.5);
        assert_eq!(out.ground_markers[0].model.x_axis, m.model.x_axis);
        assert_eq!(out.ground_markers[0].model.w_axis.x, 0.5);
    }

    #[test]
    fn decal_inverse_follows_the_blended_box() {
        let (_, p) = world(0.0);
        let (s, n) = world(1.0);
        let out = blend(&p, &s, &n, 0.5);
        let d = &out.decals[0];
        assert!((d.world_to_box * d.box_to_world).abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    fn camera_data(x: f32, fov: f32, key: EntityId) -> BlendData {
        BlendData {
            epoch: 1,
            poses: vec![(E1, at(x))],
            camera: Some(CameraBlend {
                key,
                model: at(x),
                fov_y_deg: Some(fov),
                near: 0.1,
                far: 100.0,
                aspect: 1.0,
            }),
            ..BlendData::default()
        }
    }

    #[test]
    fn camera_midpoint_position_and_fov() {
        let s = RenderScene::empty((64, 64));
        let out = blend(
            &camera_data(0.0, 60.0, E1),
            &s,
            &camera_data(2.0, 90.0, E1),
            0.5,
        );
        assert_eq!(out.camera.position, Vec3::new(1.0, 0.0, 0.0));
        assert!(out.camera.view.abs_diff_eq(at(1.0).inverse(), 1e-6));
        let want = Mat4::perspective_rh(75f32.to_radians(), 1.0, 0.1, 100.0);
        assert!(out.camera.projection.abs_diff_eq(want, 1e-5));
    }

    #[test]
    fn a_different_camera_key_takes_next() {
        let s = RenderScene::empty((64, 64));
        let out = blend(
            &camera_data(0.0, 60.0, E1),
            &s,
            &camera_data(2.0, 90.0, E2),
            0.5,
        );
        assert_eq!(out.camera, s.camera);
    }

    #[test]
    fn a_mismatched_key_list_is_untouched() {
        let (_, p) = world(0.0);
        let (mut s, mut n) = world(1.0);
        s.meshes.push(mesh(at(1.0)));
        assert_eq!(n.meshes.len(), 1);
        let out = blend(&p, &s, &n, 0.5);
        assert_eq!(out.meshes, s.meshes);
        n.meshes.clear();
        assert_eq!(blend(&p, &s, &n, 0.5).meshes, s.meshes);
    }
}
