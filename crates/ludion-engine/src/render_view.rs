//! The host-side **view override**: a free camera pose the stream draws through instead of the
//! scene's own `Camera`.
//!
//! The override is one `Option<ViewPose>` field on the [`Engine`](crate::Engine) — this module is the
//! half that decides whether a requested pose is one a camera can look through, and turns an accepted
//! pose into the [`RenderCamera`] a frame is extracted against.
//!
//! **It is host render state, like the render extent** (`render_size`, and
//! `ludion devdocs determinism`): never written to a scene, never folded into a `WorldHash`, never
//! captured by `engine.snapshot`. Unlike the extent, which a recording carries in its header, the
//! override is never recorded into a replay. Nothing in the simulation can read it, so a session
//! flown to a rooftop hashes, records and replays identically to one that never moved — and a
//! frame rendered after the override is cleared is byte-identical to one rendered before it was
//! ever set.
//!
//! **It moves the stream, not the query.** `render.screenshot` and `render.bounds` choose their
//! camera per call ([`CameraChoice`](ludion_protocol::command::CameraChoice)) and default to the
//! scene's, so every agent call, test and CLI path answers through the *game's* framing whether or
//! not a human is flying the viewport somewhere else.
//!
//! **A malformed pose is refused** ([`code::PROTOCOL_RENDER_VIEW_INVALID`], `L3032`) and leaves the
//! override exactly as it was, which is what makes the verb safe to drive from a gesture loop: one
//! bad frame of input leaves the human looking at the last good pose rather than at nothing.
//!
//! The pose carries geometry alone. Clip planes — and the field of view it does not name — come from
//! the scene's own `Camera`, so a flown view keeps the game's depth range instead of inventing one;
//! the override is always a *perspective* camera, and an orthographic scene camera contributes its
//! near and far and nothing else.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_ecs::World;
use ludion_protocol::command::ViewPose;
use ludion_renderer::{RenderCamera, aspect_ratio};

use crate::components::{Camera, Projection};

/// Accepts a requested pose, or refuses it.
///
/// Every coordinate and every angle must be finite, and a named `fov_y_deg` must be strictly inside
/// `0..180` — the open interval a perspective projection is defined on.
///
/// # Errors
/// One `L3032` [`Diagnostic`], its JSON Pointer naming the offending field. The caller leaves the
/// override untouched.
pub(crate) fn accept(pose: ViewPose) -> Result<ViewPose, Diagnostic> {
    finite(pose.translation, "translation")?;
    finite(pose.rotation, "rotation")?;
    if let Some(fov) = pose.fov_y_deg
        && !(fov.is_finite() && fov > 0.0 && fov < 180.0)
    {
        return Err(invalid(
            format!(
                "render.view asked for a vertical field of view of {fov} degrees; the range is the open interval 0..180"
            ),
            "fov_y_deg",
        ));
    }
    Ok(pose)
}

/// One three-component field, all of whose numbers must be finite.
fn finite(values: [f32; 3], field: &str) -> Result<(), Diagnostic> {
    match values.iter().position(|value| !value.is_finite()) {
        None => Ok(()),
        // A NaN or an infinity does not aim a camera somewhere strange: it makes every product of
        // the view matrix NaN, and the frame comes back blank with nothing in it to say why.
        Some(axis) => Err(invalid(
            format!(
                "render.view's `{field}` is {:?}, whose {} component is not a finite number",
                values,
                ["x", "y", "z"][axis],
            ),
            field,
        )),
    }
}

/// An `L3032` pointing at the offending field, carrying the help every refusal needs: the form, the
/// clear verb, and the read verb.
fn invalid(message: String, field: &str) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_RENDER_VIEW_INVALID, message)
        .with_path(format!("/params/pose/{field}"))
        .with_help(
            "send `translation` and `rotation` (Euler-XYZ degrees) as finite numbers, with an \
             optional `fov_y_deg` inside 0..180; send `{\"pose\": null}` to clear the override and \
             `{}` to read it without changing it",
        )
}

/// The [`RenderCamera`] an override in force produces at `viewport` pixels.
///
/// `scene` is the world's single camera entity when it has one: its near and far planes are used
/// under the same guard a scene render applies ([`Camera::depth_range`]), and its field of view
/// fills in a pose that named none. A world with no camera (or one whose camera is orthographic,
/// which has no field of view to lend) falls back to
/// [`Camera::default`]'s 60° perspective framing — the override supplies the viewpoint either way,
/// so a scene without a camera still answers here.
#[allow(clippy::disallowed_methods)] // render-side: GPU view matrix, never reaches a hash
pub(crate) fn camera(
    world: &World,
    scene: Option<EntityId>,
    pose: &ViewPose,
    viewport: (u32, u32),
) -> RenderCamera {
    let scene_camera = scene
        .and_then(|id| world.get::<Camera>(id).ok().flatten())
        .copied()
        .unwrap_or_default();
    let fov_y_deg = pose.fov_y_deg.unwrap_or(match scene_camera.projection {
        Projection::Perspective { fov_y_deg } => fov_y_deg,
        Projection::Orthographic { .. } => default_fov_y_deg(),
    });
    let (near, far) = scene_camera.depth_range();
    let translation = Vec3::from(pose.translation);
    // The same decomposition `build_camera` applies to a camera entity's `Transform`, so a pose a
    // client read off the scene camera reproduces that camera's framing exactly.
    let model =
        Mat4::from_rotation_translation(crate::extract::rotation_quat(pose.rotation), translation);
    RenderCamera {
        view: model.inverse(),
        projection: Mat4::perspective_rh(fov_y_deg.to_radians(), aspect_ratio(viewport), near, far),
        position: translation,
    }
}

/// The default `Camera`'s vertical field of view — what a pose that names none inherits when the
/// scene's camera cannot lend one.
fn default_fov_y_deg() -> f32 {
    match Projection::default() {
        Projection::Perspective { fov_y_deg } => fov_y_deg,
        // `Projection`'s default is the 60° perspective; an orthographic default would mean this
        // module has no angle to fall back on at all.
        Projection::Orthographic { .. } => 60.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose() -> ViewPose {
        ViewPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [-15.0, 45.0, 0.0],
            fov_y_deg: None,
        }
    }

    /// A well-formed pose is accepted verbatim, at both ends of the field-of-view interval's
    /// interior — the bound guards nothing a client could want.
    #[test]
    fn a_well_formed_pose_is_accepted() {
        assert_eq!(accept(pose()).expect("the pose is accepted"), pose());
        for fov in [0.001_f32, 60.0, 179.999] {
            let mut p = pose();
            p.fov_y_deg = Some(fov);
            assert_eq!(
                accept(p).expect("the pose is accepted").fov_y_deg,
                Some(fov)
            );
        }
    }

    /// Every non-finite coordinate is refused, on either field and on any axis, with the field named
    /// by the diagnostic's path.
    #[test]
    fn a_non_finite_coordinate_is_refused_naming_its_field() {
        for (field, index) in [("translation", 0), ("rotation", 2)] {
            for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut p = pose();
                if field == "translation" {
                    p.translation[index] = bad;
                } else {
                    p.rotation[index] = bad;
                }
                let diagnostic = accept(p).expect_err("a non-finite pose must be refused");
                assert_eq!(diagnostic.code, code::PROTOCOL_RENDER_VIEW_INVALID);
                assert_eq!(
                    diagnostic.path.as_deref(),
                    Some(format!("/params/pose/{field}").as_str()),
                );
            }
        }
    }

    /// A degenerate field of view is refused at both ends of the closed interval, and past them.
    #[test]
    fn a_degenerate_field_of_view_is_refused() {
        for fov in [0.0_f32, -60.0, 180.0, 360.0, f32::NAN] {
            let mut p = pose();
            p.fov_y_deg = Some(fov);
            let diagnostic = accept(p).expect_err("a degenerate fov must be refused");
            assert_eq!(diagnostic.code, code::PROTOCOL_RENDER_VIEW_INVALID);
            assert_eq!(diagnostic.path.as_deref(), Some("/params/pose/fov_y_deg"));
        }
    }

    /// Every refusal carries the same help — the pose's form, the clear verb, and the read verb — so
    /// a client that sent any of the wrong shapes learns the right one from the diagnostic alone.
    #[test]
    fn every_refusal_carries_the_form_and_both_verbs_as_help() {
        let mut p = pose();
        p.fov_y_deg = Some(0.0);
        let help = accept(p).expect_err("refused").help.expect("carries help");
        assert!(help.contains("Euler-XYZ"), "{help}");
        assert!(help.contains("\"pose\": null"), "{help}");
        assert!(help.contains("0..180"), "{help}");
    }

    /// The override supplies the viewpoint, so a world with no camera at all still yields one — at
    /// the default framing's clip planes, and looking from where the pose stands.
    #[test]
    fn a_world_with_no_camera_still_yields_an_override_camera() {
        let world = World::new();
        let built = camera(&world, None, &pose(), (1280, 720));
        assert_eq!(built.position, Vec3::new(1.0, 2.0, 3.0));
        assert!(built.projection.is_finite());
        assert!(built.view.is_finite());
    }

    fn world_with_camera(near: f32, far: f32) -> (World, EntityId) {
        let mut world = World::new();
        let id = world.spawn();
        world
            .insert(
                id,
                Camera {
                    near,
                    far,
                    ..Camera::default()
                },
            )
            .unwrap();
        (world, id)
    }

    /// A scene camera whose near and far are equal projects through a finite matrix: the flown view
    /// takes the same guarded depth range a scene render does.
    #[test]
    fn an_equal_near_and_far_stays_finite() {
        let (world, id) = world_with_camera(1.0, 1.0);
        let built = camera(&world, Some(id), &pose(), (256, 256));
        assert!(built.projection.is_finite());
    }

    /// An inverted range falls back to the default camera's planes.
    #[test]
    fn an_inverted_depth_range_takes_the_default_planes() {
        let (world, id) = world_with_camera(5.0, 1.0);
        let built = camera(&world, Some(id), &pose(), (256, 256));
        let default = Camera::default();
        let expected = Mat4::perspective_rh(
            default_fov_y_deg().to_radians(),
            1.0,
            default.near,
            default.far,
        );
        assert_eq!(built.projection, expected);
    }
}
