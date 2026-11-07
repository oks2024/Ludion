//! First-person viewmodel extraction: which meshes are flagged, whether the frame has a camera to
//! compose them onto, and the packet data the renderer carries.
//!
//! A flagged mesh keeps its pose relative to the first-person camera entity; the renderer composes
//! that local pose onto the frame's drawn camera (`ludion_renderer::compose_viewmodel`), so the gun
//! follows the late-latched, offset camera exactly.

use std::collections::BTreeSet;

use ludion_camera::{CameraRig, RigMode};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat3, Mat4};
use ludion_ecs::{SemanticValidate, World};
use ludion_renderer::{MeshInstance, ViewmodelData, ViewmodelLocal, aspect_ratio};

use crate::components::{Camera, Projection, Viewmodel, ViewmodelLens};

/// The entities that are viewmodels this frame: flagged, or under a flagged `Parent` ancestor.
/// Empty, with no walk, when the world has no `Viewmodel`.
pub(crate) struct FrameViewmodel {
    flagged: BTreeSet<EntityId>,
}

impl FrameViewmodel {
    pub(crate) fn of(world: &World, ids: &[EntityId]) -> Self {
        let mut flagged = BTreeSet::new();
        if world.component_count::<Viewmodel>() != 0 {
            for &id in ids {
                if crate::hierarchy::inherits(world, id, |e| world.has::<Viewmodel>(e)) {
                    flagged.insert(id);
                }
            }
        }
        Self { flagged }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.flagged.is_empty()
    }

    fn contains(&self, id: EntityId) -> bool {
        self.flagged.contains(&id)
    }
}

/// What the frame's camera means for flagged meshes.
pub(crate) enum Site {
    /// A single perspective first-person rig camera, and no `render.view` override.
    Camera(EntityId),
    /// A `render.view` override is in force: flagged meshes draw as world meshes, unreported.
    Overridden,
    /// No such camera: flagged meshes draw as world meshes and the miss is `L1336`.
    Missing,
}

pub(crate) fn site(world: &World, ids: &[EntityId], view_override: bool) -> Site {
    if view_override {
        return Site::Overridden;
    }
    let Ok(Some(cam)) = crate::extract::single_camera(world, ids) else {
        return Site::Missing;
    };
    let first_person = world
        .get::<CameraRig>(cam)
        .ok()
        .flatten()
        .is_some_and(|rig| matches!(rig.rig.mode, RigMode::FirstPerson(_)));
    let perspective = world
        .get::<Camera>(cam)
        .ok()
        .flatten()
        .is_some_and(|camera| matches!(camera.projection, Projection::Perspective { .. }));
    if first_person && perspective {
        Site::Camera(cam)
    } else {
        Site::Missing
    }
}

/// The camera's lens, and whether it was authored out of range (the defaults are used then).
fn lens(world: &World, cam: EntityId) -> (ViewmodelLens, bool) {
    let Some(lens) = world.get::<ViewmodelLens>(cam).ok().flatten().copied() else {
        return (ViewmodelLens::default(), true);
    };
    let value = serde_json::to_value(lens).unwrap_or_default();
    if ViewmodelLens::validate_semantics(&value).is_empty() {
        (lens, true)
    } else {
        (ViewmodelLens::default(), false)
    }
}

/// Latches the misuses: flagged meshes with no first-person camera (`L1336`, keyed by the lowest
/// flagged entity), a lens off a camera (`L1337`, per entity) and an out-of-range lens the defaults
/// replaced (`L1338`, keyed by the camera).
pub(crate) fn report(
    world: &World,
    ids: &[EntityId],
    flagged: &FrameViewmodel,
    view_override: bool,
    degraded: &mut Vec<(String, Diagnostic)>,
) {
    if world.component_count::<ViewmodelLens>() != 0 {
        for &id in ids {
            if world.has::<ViewmodelLens>(id) && !world.has::<Camera>(id) {
                let key = crate::diagnostics::offender_key(world, id);
                degraded.push((
                    format!("{key}/viewmodel-lens-not-camera"),
                    Diagnostic::new(
                        code::VIEWMODEL_LENS_NOT_ON_CAMERA,
                        format!(
                            "{id} carries a `ViewmodelLens` but has no `Camera`; it is ignored"
                        ),
                    )
                    .with_help("move the `ViewmodelLens` onto the first-person camera entity"),
                ));
            }
        }
    }
    let Some(&lowest) = flagged.flagged.first() else {
        return;
    };
    match site(world, ids, view_override) {
        Site::Overridden => {}
        Site::Missing => {
            let key = crate::diagnostics::offender_key(world, lowest);
            degraded.push((
                format!("{key}/viewmodel-no-camera"),
                Diagnostic::new(
                    code::VIEWMODEL_NO_CAMERA,
                    format!(
                        "{lowest} is a `Viewmodel` but the scene has no single perspective \
                         first-person camera; it draws as a world mesh"
                    ),
                )
                .with_help("give the scene's one camera a first-person `CameraRig`"),
            ));
        }
        Site::Camera(cam) => {
            if !lens(world, cam).1 {
                let key = crate::diagnostics::offender_key(world, cam);
                degraded.push((
                    format!("{key}/viewmodel-lens-invalid"),
                    Diagnostic::new(
                        code::VIEWMODEL_LENS_INVALID,
                        format!(
                            "{cam} carries an out-of-range `ViewmodelLens`; the defaults are used"
                        ),
                    )
                    .with_help("`ludion explain L1338`"),
                ));
            }
        }
    }
}

/// Marks each flagged mesh (`viewmodel`, no shadow) and returns the packet data with its
/// camera-local pose; `None` when the frame draws no composed item. `owners` is parallel to
/// `meshes`.
pub(crate) fn apply(
    world: &World,
    cam: EntityId,
    flagged: &FrameViewmodel,
    meshes: &mut [MeshInstance],
    owners: &[EntityId],
    viewport: (u32, u32),
) -> Option<ViewmodelData> {
    let camera = world.get::<Camera>(cam).ok().flatten().copied()?;
    let (lens, _) = lens(world, cam);
    let cam_model = crate::extract::camera_entity_model(world, cam);
    #[allow(clippy::disallowed_methods)] // render-side: GPU matrix, never reaches a hash
    let cam_inverse = cam_model.as_dmat4().inverse();
    let cam_rotation_t = Mat3::from_mat4(cam_model).transpose();
    let mut locals = Vec::new();
    for (index, (mesh, &owner)) in meshes.iter_mut().zip(owners).enumerate() {
        if !flagged.contains(owner) {
            continue;
        }
        mesh.viewmodel = true;
        mesh.cast_shadows = false;
        locals.push(ViewmodelLocal {
            mesh: u32::try_from(index).ok()?,
            model: (cam_inverse * mesh.model.as_dmat4()).as_mat4(),
            normal_matrix: cam_rotation_t * mesh.normal_matrix,
        });
    }
    if locals.is_empty() {
        return None;
    }
    let (_, far) = camera.depth_range();
    Some(ViewmodelData {
        fov_y_deg: lens.fov_y_deg,
        near: lens.near,
        depth_slice: lens.depth_slice,
        projection: Mat4::perspective_rh(
            lens.fov_y_deg.to_radians(),
            aspect_ratio(viewport),
            lens.near,
            far,
        ),
        locals,
    })
}
