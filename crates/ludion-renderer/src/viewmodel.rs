//! The first-person viewmodel packet: the lens projection, the depth slice and the camera-local
//! pose of each flagged mesh. [`compose_viewmodel`] is the one place a local pose is composed onto
//! the frame's drawn camera, shared by extract, [`blend`](crate::blend) and
//! [`latch_camera`](crate::latch_camera).

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4, Vec3, Vec4};

use crate::scene::RenderScene;

/// Clip-space depth remap of the world into `[s, 1]`: `z' = (1-s)·z + s·w`.
pub(crate) fn world_depth_remap(s: f32) -> Mat4 {
    Mat4::from_cols(
        Vec4::X,
        Vec4::Y,
        Vec4::new(0.0, 0.0, 1.0 - s, 0.0),
        Vec4::new(0.0, 0.0, s, 1.0),
    )
}

/// Clip-space depth remap of the viewmodel into `[0, s]`: `z' = s·z`.
pub(crate) fn viewmodel_depth_remap(s: f32) -> Mat4 {
    Mat4::from_cols(Vec4::X, Vec4::Y, Vec4::new(0.0, 0.0, s, 0.0), Vec4::W)
}

impl RenderScene {
    /// The frame's world view-projection with the depth remap folded in: `projection * view`
    /// unchanged when no viewmodel is drawn, else the world occupies depth `[s, 1]` of
    /// [`ViewmodelData::depth_slice`]. Every world depth-writing draw and every depth decode
    /// (through its inverse) reads this or `RenderScene::viewmodel_view_proj`.
    pub fn depth_view_proj(&self) -> Mat4 {
        let view_proj = self.camera.projection * self.camera.view;
        match &self.viewmodel {
            None => view_proj,
            Some(data) => world_depth_remap(data.depth_slice) * view_proj,
        }
    }

    /// The viewmodel's view-projection with its depth in `[0, s]`; `None` without a viewmodel.
    pub(crate) fn viewmodel_view_proj(&self) -> Option<Mat4> {
        let data = self.viewmodel.as_ref()?;
        Some(viewmodel_depth_remap(data.depth_slice) * data.projection * self.camera.view)
    }
}

/// The viewmodel lens and the camera-local pose of every composed item in one frame.
#[derive(Clone, PartialEq, Debug)]
pub struct ViewmodelData {
    /// Vertical field of view of the viewmodel's own projection, in degrees.
    pub fov_y_deg: f32,
    /// Near plane of the viewmodel's own projection.
    pub near: f32,
    /// The reserved depth slice the viewmodel draws into, as a fraction of the depth range.
    pub depth_slice: f32,
    /// `perspective_rh(fov, aspect, near, world camera far)`.
    pub projection: Mat4,
    /// One entry per composed item, ascending by [`ViewmodelLocal::mesh`].
    pub locals: Vec<ViewmodelLocal>,
    /// One entry per flagged particle, ascending by [`ViewmodelParticle::particle`].
    pub particles: Vec<ViewmodelParticle>,
}

/// A flagged particle's centre and billboard axes in the drawn camera's frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewmodelParticle {
    /// Index into [`RenderScene::particles`].
    pub particle: u32,
    /// Camera-local centre.
    pub center: Vec3,
    /// Camera-local billboard right.
    pub right: Vec3,
    /// Camera-local billboard up.
    pub up: Vec3,
}

/// A composed mesh's pose in the drawn camera's frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewmodelLocal {
    /// Index into [`RenderScene::meshes`].
    pub mesh: u32,
    /// The entity that owns the mesh; blending pairs locals across ticks by it.
    pub owner: EntityId,
    /// Camera-local model matrix.
    pub model: Mat4,
    /// Camera-local normal matrix.
    pub normal_matrix: Mat3,
}

/// Rewrites each composed mesh's world `model` and `normal_matrix` from its camera-local pose and
/// the drawn camera's world `camera_model` (a rotation plus a translation). Does nothing when the
/// scene carries no viewmodel data.
pub fn compose_viewmodel(scene: &mut RenderScene, camera_model: Mat4) {
    let Some(data) = &scene.viewmodel else {
        return;
    };
    let rotation = Mat3::from_mat4(camera_model);
    for local in &data.locals {
        let mesh = &mut scene.meshes[local.mesh as usize];
        mesh.model = camera_model * local.model;
        mesh.normal_matrix = rotation * local.normal_matrix;
    }
    for local in &data.particles {
        let particle = &mut scene.particles[local.particle as usize];
        particle.center = camera_model.transform_point3(local.center);
        particle.right = rotation * local.right;
        particle.up = rotation * local.up;
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::ids::EntityId;
    use ludion_core::math::glam::{Quat, Vec3};

    use super::*;
    use crate::blend::{BlendData, CameraBlend, InstanceKey, blend, latch_camera};
    use crate::scene::{MeshInstance, MeshShape, ParticleInstance};

    const CAMERA: EntityId = EntityId::new(1, 0);
    const GUN: EntityId = EntityId::new(2, 0);

    fn camera_model(x: f32, yaw: f32) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(x, 1.0, 2.0))
    }

    const PARTICLE_LOCAL: Vec3 = Vec3::new(0.0, 0.1, -0.6);

    fn local() -> Mat4 {
        Mat4::from_translation(Vec3::new(0.3, -0.2, -0.5))
    }

    /// One viewmodel item composed onto `camera_model`.
    fn scene(camera_model: Mat4) -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        let local = local();
        scene.meshes.push(MeshInstance {
            shape: MeshShape::Cube,
            model: Mat4::IDENTITY,
            normal_matrix: Mat3::IDENTITY,
            color: [1.0; 4],
            material: [0.5, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: false,
            viewmodel: true,
        });
        scene.viewmodel = Some(ViewmodelData {
            fov_y_deg: 55.0,
            near: 0.01,
            depth_slice: 0.01,
            projection: Mat4::IDENTITY,
            locals: vec![ViewmodelLocal {
                mesh: 0,
                owner: GUN,
                model: local,
                normal_matrix: Mat3::from_mat4(local),
            }],
            particles: vec![ViewmodelParticle {
                particle: 0,
                center: PARTICLE_LOCAL,
                right: Vec3::X,
                up: Vec3::Y,
            }],
        });
        scene.particles.push(ParticleInstance {
            center: Vec3::ZERO,
            right: Vec3::ZERO,
            up: Vec3::ZERO,
            size: 0.1,
            color: [1.0; 4],
            softness: 0.0,
            additive: true,
            lit: false,
            viewmodel: true,
        });
        scene.camera.view = camera_model.inverse();
        scene.camera.position = camera_model.w_axis.truncate();
        compose_viewmodel(&mut scene, camera_model);
        scene
    }

    fn data(model: Mat4) -> BlendData {
        BlendData {
            epoch: 1,
            poses: vec![(CAMERA, model)],
            camera: Some(CameraBlend {
                key: CAMERA,
                model,
                fov_y_deg: None,
                near: 0.1,
                far: 100.0,
                aspect: 1.0,
            }),
            meshes: vec![None],
            ..BlendData::default()
        }
    }

    /// NDC depth of a view-space point under `view_proj` (camera at origin, view = identity).
    fn depth(view_proj: Mat4, z: f32) -> f32 {
        let clip = view_proj * Vec4::new(0.0, 0.0, -z, 1.0);
        clip.z / clip.w
    }

    fn lens_scene(s: f32) -> RenderScene {
        let mut scene = RenderScene::empty((64, 64));
        scene.camera.projection = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0);
        scene.camera.view = Mat4::IDENTITY;
        scene.viewmodel = Some(ViewmodelData {
            fov_y_deg: 55.0,
            near: 0.01,
            depth_slice: s,
            projection: Mat4::perspective_rh(0.9, 1.0, 0.01, 100.0),
            locals: Vec::new(),
            particles: Vec::new(),
        });
        scene
    }

    #[test]
    fn without_a_viewmodel_the_view_proj_is_untouched() {
        let mut s = lens_scene(0.01);
        s.viewmodel = None;
        assert_eq!(s.depth_view_proj(), s.camera.projection * s.camera.view);
        assert!(s.viewmodel_view_proj().is_none());
    }

    #[test]
    fn the_world_maps_into_the_upper_slice() {
        let s = lens_scene(0.2);
        let vp = s.depth_view_proj();
        assert!((depth(vp, 0.1) - 0.2).abs() < 1e-5);
        assert!((depth(vp, 100.0) - 1.0).abs() < 1e-5);
        assert!(depth(vp, 5.0) > 0.2);
    }

    #[test]
    fn the_lens_maps_into_the_lower_slice() {
        let s = lens_scene(0.2);
        let vp = s.viewmodel_view_proj().unwrap();
        assert!(depth(vp, 0.01).abs() < 1e-5);
        assert!((depth(vp, 100.0) - 0.2).abs() < 1e-5);
    }

    #[test]
    fn a_gun_point_is_below_every_world_depth() {
        let s = lens_scene(0.2);
        let (gun, world) = (s.viewmodel_view_proj().unwrap(), s.depth_view_proj());
        let nearest_world = depth(world, 0.1);
        for z in [0.01, 0.2, 1.0, 50.0, 100.0] {
            assert!(depth(gun, z) <= nearest_world + 1e-5);
        }
    }

    /// Every gun depth `d` in `[0, s)` decodes through the world inverse to a view distance just
    /// under the world near plane, so fog and shafts see the gun at about `near`.
    #[test]
    fn gun_depths_decode_to_just_under_the_world_near() {
        let s = lens_scene(0.01);
        let inverse = s.depth_view_proj().inverse();
        let near = 0.1;
        for step in 0..=100 {
            let d = 0.01 * (step as f32 / 100.0) * 0.999;
            let p = inverse * Vec4::new(0.0, 0.0, d, 1.0);
            let distance = -p.z / p.w;
            assert!(
                (0.98 * near - 1e-5..=near + 1e-5).contains(&distance),
                "d = {d}: decoded to {distance}"
            );
        }
    }

    #[test]
    fn an_identity_local_composes_to_the_camera() {
        let model = camera_model(3.0, 0.7);
        let mut s = scene(model);
        s.viewmodel.as_mut().unwrap().locals[0].model = Mat4::IDENTITY;
        compose_viewmodel(&mut s, model);
        assert_eq!(s.meshes[0].model, model);
        assert!(s.meshes[0].model.inverse().abs_diff_eq(s.camera.view, 1e-6));
    }

    #[test]
    fn latching_recomposes_onto_the_patched_camera() {
        let tick_model = camera_model(0.0, 0.0);
        let latched_model = camera_model(0.0, 0.3);
        let mut s = scene(tick_model);
        let blend_of = |model| CameraBlend {
            key: CAMERA,
            model,
            fov_y_deg: None,
            near: 0.1,
            far: 100.0,
            aspect: 1.0,
        };
        latch_camera(&mut s, &blend_of(tick_model), &blend_of(latched_model));
        assert_eq!(s.meshes[0].model, latched_model * local());
    }

    #[test]
    fn a_blended_camera_carries_the_item() {
        let (p, n) = (camera_model(0.0, 0.0), camera_model(2.0, 0.4));
        let prev = scene(p);
        let next = scene(n);
        let out = blend(&prev, &data(p), &next, &data(n), 0.5);
        let blended = out.camera.view.inverse();
        assert!(out.meshes[0].model.abs_diff_eq(blended * local(), 1e-5));
        assert_ne!(out.meshes[0].model, next.meshes[0].model);
    }

    /// `scene(model)` with the gun's camera-local translation set to `x` along X, composed.
    fn scene_with_local_x(model: Mat4, x: f32) -> RenderScene {
        let mut s = scene(model);
        let local = Mat4::from_translation(Vec3::new(x, -0.2, -0.5));
        let locals = &mut s.viewmodel.as_mut().unwrap().locals;
        locals[0].model = local;
        locals[0].normal_matrix = Mat3::from_mat4(local);
        compose_viewmodel(&mut s, model);
        s
    }

    fn local_at(x: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, -0.2, -0.5))
    }

    #[test]
    fn a_moving_local_lands_halfway_at_half_alpha() {
        let cam = camera_model(1.0, 0.3);
        let (prev, next) = (scene_with_local_x(cam, 0.0), scene_with_local_x(cam, 0.2));
        let out = blend(&prev, &data(cam), &next, &data(cam), 0.5);
        assert!(out.meshes[0].model.abs_diff_eq(cam * local_at(0.1), 1e-5));
        assert_ne!(out.meshes[0].model, next.meshes[0].model);
    }

    #[test]
    fn a_moving_local_and_camera_blend_together() {
        let (p, n) = (camera_model(0.0, 0.0), camera_model(2.0, 0.4));
        let (prev, next) = (scene_with_local_x(p, 0.0), scene_with_local_x(n, 0.2));
        let out = blend(&prev, &data(p), &next, &data(n), 0.5);
        let blended = out.camera.view.inverse();
        assert!(
            out.meshes[0]
                .model
                .abs_diff_eq(blended * local_at(0.1), 1e-5)
        );
    }

    #[test]
    fn a_snapped_owner_takes_the_newer_local() {
        let cam = camera_model(1.0, 0.3);
        let (prev, next) = (scene_with_local_x(cam, 0.0), scene_with_local_x(cam, 0.2));
        let mut next_data = data(cam);
        next_data.snaps = vec![GUN];
        let out = blend(&prev, &data(cam), &next, &next_data, 0.5);
        assert_eq!(out.meshes[0].model, next.meshes[0].model);
        assert_eq!(out.viewmodel, next.viewmodel);
    }

    #[test]
    fn a_moving_flash_centre_lands_halfway() {
        let cam = camera_model(1.0, 0.3);
        let key = InstanceKey {
            owner: GUN,
            started: 0,
            burst: true,
            index: 0,
        };
        let moved = |z: f32| {
            let mut s = scene(cam);
            s.viewmodel.as_mut().unwrap().particles[0].center = Vec3::new(0.0, 0.1, z);
            compose_viewmodel(&mut s, cam);
            s
        };
        let (prev, next) = (moved(-0.6), moved(-0.8));
        let keyed = |cam| BlendData {
            particles: vec![key],
            ..data(cam)
        };
        let out = blend(&prev, &keyed(cam), &next, &keyed(cam), 0.5);
        assert!(
            out.particles[0]
                .center
                .abs_diff_eq(cam.transform_point3(Vec3::new(0.0, 0.1, -0.7)), 1e-5)
        );
    }

    #[test]
    fn a_listed_particle_composes_onto_the_camera() {
        let model = camera_model(3.0, 0.7);
        let s = scene(model);
        let p = &s.particles[0];
        assert!(
            p.center
                .abs_diff_eq(model.transform_point3(PARTICLE_LOCAL), 1e-6)
        );
        assert!(p.right.abs_diff_eq(model.transform_vector3(Vec3::X), 1e-6));
        assert!(p.up.abs_diff_eq(model.transform_vector3(Vec3::Y), 1e-6));
    }

    #[test]
    fn latching_moves_a_listed_particle_with_the_camera() {
        let (tick, latched) = (camera_model(0.0, 0.0), camera_model(0.0, 0.3));
        let mut s = scene(tick);
        let blend_of = |model| CameraBlend {
            key: CAMERA,
            model,
            fov_y_deg: None,
            near: 0.1,
            far: 100.0,
            aspect: 1.0,
        };
        latch_camera(&mut s, &blend_of(tick), &blend_of(latched));
        assert!(
            s.particles[0]
                .center
                .abs_diff_eq(latched.transform_point3(PARTICLE_LOCAL), 1e-5)
        );
    }

    #[test]
    fn a_blended_camera_carries_a_listed_particle() {
        let (p, n) = (camera_model(0.0, 0.0), camera_model(2.0, 0.4));
        let (prev, next) = (scene(p), scene(n));
        let out = blend(&prev, &data(p), &next, &data(n), 0.5);
        let blended = out.camera.view.inverse();
        assert!(
            out.particles[0]
                .center
                .abs_diff_eq(blended.transform_point3(PARTICLE_LOCAL), 1e-5)
        );
        assert_ne!(out.particles[0].center, next.particles[0].center);
    }

    #[test]
    fn without_viewmodel_data_meshes_are_untouched() {
        let p = camera_model(0.0, 0.0);
        let n = camera_model(2.0, 0.4);
        let mut s = scene(p);
        s.viewmodel = None;
        let before = s.meshes.clone();
        let blend_of = |model| CameraBlend {
            key: CAMERA,
            model,
            fov_y_deg: None,
            near: 0.1,
            far: 100.0,
            aspect: 1.0,
        };
        latch_camera(&mut s, &blend_of(p), &blend_of(n));
        assert_eq!(s.meshes, before);
        let out = blend(&s, &data(p), &s, &data(n), 0.5);
        assert_eq!(out.meshes, before);
    }
}
