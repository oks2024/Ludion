//! The engine's [`CameraLatch`]: a snapshot of a first-person rig that can re-resolve the frame's
//! camera with fresher look.
//!
//! The presenter runs at display rate on its own thread and never sees the world. A packet therefore
//! carries this copy of the rig, its target position and the camera's projection data; resolving
//! through it reaches the same [`resolved_camera`] seam the tick used, then the same offset and
//! projection composition the extract uses. It is render-only: nothing it computes returns to the
//! sim, so scenes, replays and `WorldHash` never see it.

use std::sync::Arc;

use ludion_camera::{CameraRig, RigMode, look_from_counts, resolved_camera};
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_ecs::{Parent, World};
use ludion_renderer::{CameraBlend, CameraLatch, aspect_ratio};

use crate::components::{Camera, CameraOffset, Projection};
use crate::extract::{compose_camera, rotation_quat, single_camera};

struct RigLatch {
    rig: CameraRig,
    target: [f32; 3],
    camera: Camera,
    offset: Option<CameraOffset>,
    key: EntityId,
    aspect: f32,
}

impl CameraLatch for RigLatch {
    fn resolve(&self, extra_look_deg: [f32; 2]) -> Option<CameraBlend> {
        let view = resolved_camera(&self.rig, self.target, extra_look_deg)?.view;
        let mut camera = self.camera.clone();
        if let (Some(fov), Projection::Perspective { fov_y_deg }) =
            (view.fov_y_deg, &mut camera.projection)
        {
            *fov_y_deg = fov;
        }
        let (near, far) = camera.depth_range();
        let (rotation, translation, projection) = compose_camera(
            rotation_quat(view.rotation_deg),
            Vec3::from(view.translation),
            &camera,
            self.offset.as_ref(),
        );
        Some(CameraBlend {
            key: self.key,
            model: Mat4::from_rotation_translation(rotation, translation),
            fov_y_deg: match projection {
                Projection::Perspective { fov_y_deg } => Some(fov_y_deg),
                Projection::Orthographic { .. } => None,
            },
            near,
            far,
            aspect: self.aspect,
        })
    }

    fn look_from_counts(&self, dx: f32, dy: f32) -> [f32; 2] {
        look_from_counts(&self.rig, dx, dy)
    }
}

/// The latch for the scene's single camera when it carries a posed first-person or chase
/// `CameraRig` with no `Parent`, a perspective projection and a live target; `None` otherwise.
pub(crate) fn collect(world: &World, viewport: (u32, u32)) -> Option<Arc<dyn CameraLatch>> {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let key = single_camera(world, &ids).ok()??;
    if world.has::<Parent>(key) {
        return None;
    }
    let rig = world.get::<CameraRig>(key).ok()??;
    if !matches!(rig.rig.mode, RigMode::FirstPerson(_) | RigMode::Chase(_))
        || rig.state.pose.is_none()
    {
        return None;
    }
    let camera = world.get::<Camera>(key).ok()??.clone();
    if !matches!(camera.projection, Projection::Perspective { .. }) {
        return None;
    }
    let target = crate::hierarchy::world_transform(world, rig.target?)?
        .trs
        .translation;
    Some(Arc::new(RigLatch {
        rig: rig.clone(),
        target,
        camera,
        offset: crate::camera_offset::active(world, &ids),
        key,
        aspect: aspect_ratio(viewport),
    }))
}

#[cfg(test)]
mod tests {
    use crate::Engine;

    const SCENE: &str = "format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            first_person: {}
        trauma: 0.5
        kick_deg:
          - 0.5
          - -2.0
          - 1.0
  - name: hero
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - -2.0
";

    #[test]
    fn a_first_person_rig_resolves_to_the_extracted_camera_bit_for_bit() {
        let mut engine = Engine::builder().build().expect("the game builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        for _ in 0..3 {
            engine.advance_live();
        }
        let latch = engine.present_camera_latch().expect("a first-person rig");
        let camera = engine
            .extract_present_scene()
            .expect("extracts")
            .1
            .camera
            .expect("a camera");
        assert_eq!(latch.resolve([0.0; 2]), Some(camera));
        let turned = latch.resolve([20.0, 5.0]).expect("resolves");
        assert_ne!(turned.model, camera.model);
        assert_eq!(latch.look_from_counts(10.0, 5.0), [-1.0, 0.5]);
    }

    #[test]
    fn a_chase_rig_latches_and_turns_about_the_pivot() {
        let scene = SCENE.replace("first_person: {}", "chase: {}");
        let mut engine = Engine::builder().build().expect("the game builds");
        engine.load_inline_scene(&scene).expect("the scene loads");
        for _ in 0..3 {
            engine.advance_live();
        }
        let latch = engine.present_camera_latch().expect("a chase rig");
        let camera = engine
            .extract_present_scene()
            .expect("extracts")
            .1
            .camera
            .expect("a camera");
        assert_eq!(latch.resolve([0.0; 2]), Some(camera));
        let turned = latch.resolve([20.0, 5.0]).expect("resolves");
        assert_ne!(turned.model, camera.model);
    }

    #[test]
    fn a_follow_rig_has_no_latch() {
        let scene = SCENE.replace("first_person: {}", "follow: {}");
        let mut engine = Engine::builder().build().expect("the game builds");
        engine.load_inline_scene(&scene).expect("the scene loads");
        engine.advance_live();
        assert!(engine.present_camera_latch().is_none());
    }

    #[test]
    fn a_track_rig_has_no_latch() {
        let scene = SCENE.replace("first_person: {}", "track: {}");
        let mut engine = Engine::builder().build().expect("the game builds");
        engine.load_inline_scene(&scene).expect("the scene loads");
        engine.advance_live();
        assert!(engine.present_camera_latch().is_none());
    }

    const PLAIN_SCENE: &str = "format: 1
seed: 1
entities:
  - name: camera
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 1
          generation: 0
        rig:
          mode:
            first_person: {}
  - name: hero
    components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - -2.0
";

    fn look(world: &mut ludion_ecs::World, _: &ludion_ecs::SystemCtx) {
        let [dx, dy] = world
            .resource::<ludion_sim::InputState>()
            .unwrap()
            .mouse()
            .delta();
        let cam = world
            .entities()
            .find(|&id| world.has::<ludion_camera::CameraRig>(id))
            .unwrap();
        let rig = world
            .get_mut::<ludion_camera::CameraRig>(cam)
            .unwrap()
            .unwrap();
        ludion_camera::add_look_counts(rig, dx, dy);
    }

    /// The latched view and the view the sim draws once it consumes the same counts are the same
    /// camera, so reconciling never pops.
    #[test]
    fn the_sim_lands_exactly_where_the_latch_showed() {
        use ludion_camera::{CameraRig, resolved_camera};
        use ludion_protocol::Command;
        use ludion_protocol::command::InputInjectParams;

        let mut engine = Engine::builder()
            .system("look", look)
            .build()
            .expect("the game builds");
        engine.load_inline_scene(PLAIN_SCENE).expect("loads");
        for _ in 0..3 {
            engine.advance_live();
        }
        let latch = engine.present_camera_latch().expect("a first-person rig");
        let rig = |engine: &Engine| {
            let world = engine.world();
            let id = world
                .entities()
                .find(|&id| world.has::<CameraRig>(id))
                .unwrap();
            world.get::<CameraRig>(id).unwrap().unwrap().clone()
        };
        let before = rig(&engine);
        let (counts, extra) = ([10.0_f32, 5.0], latch.look_from_counts(10.0, 5.0));
        let latched = latch.resolve(extra).expect("resolves");
        let expected = resolved_camera(&before, [1.0, 0.0, -2.0], extra).expect("resolves");

        let event = serde_json::json!({
            "kind": "mouse_motion",
            "delta_bits": [counts[0].to_bits(), counts[1].to_bits()],
        });
        crate::dispatch(
            &mut engine,
            Command::InputInject(InputInjectParams { frame: None, event }),
        )
        .expect("injects");
        engine.advance_live();

        let drawn = engine
            .extract_present_scene()
            .expect("extracts")
            .1
            .camera
            .expect("a camera");
        assert_eq!(drawn, latched);
        let after = rig(&engine).aim().expect("posed");
        assert_eq!(after.yaw_deg, expected.aim.yaw_deg);
        assert_eq!(after.pitch_deg, expected.aim.pitch_deg);
    }
}
