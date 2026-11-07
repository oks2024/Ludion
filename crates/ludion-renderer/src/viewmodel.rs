//! The first-person viewmodel packet: the lens projection, the depth slice and the camera-local
//! pose of each flagged mesh. [`compose_viewmodel`] is the one place a local pose is composed onto
//! the frame's drawn camera, shared by extract, [`blend`](crate::blend) and
//! [`latch_camera`](crate::latch_camera).

use ludion_core::math::glam::{Mat3, Mat4};

use crate::scene::RenderScene;

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
}

/// A composed mesh's pose in the drawn camera's frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewmodelLocal {
    /// Index into [`RenderScene::meshes`].
    pub mesh: u32,
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
}

#[cfg(test)]
mod tests {
    use ludion_core::ids::EntityId;
    use ludion_core::math::glam::{Quat, Vec3};

    use super::*;
    use crate::blend::{BlendData, CameraBlend, blend, latch_camera};
    use crate::scene::{MeshInstance, MeshShape};

    const CAMERA: EntityId = EntityId::new(1, 0);

    fn camera_model(x: f32, yaw: f32) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(x, 1.0, 2.0))
    }

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
                model: local,
                normal_matrix: Mat3::from_mat4(local),
            }],
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
