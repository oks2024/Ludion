//! Deterministic screen→world math for aiming: [`camera_ray`] and [`ray_ground_plane`].
//!
//! Games aim at the ground from a mouse position or a stick direction: turn a render-target pixel
//! into a world-space [`Ray`], then intersect that ray with the `y = 0` ground plane. Both helpers
//! are pure functions of plain data, so their output can flow straight into hashed sim state.
//!
//! **Cross-platform bit-stable by construction.** Every transcendental here routes through
//! [`ludion_core::math`] (pure-Rust `libm`), never the `f32` inherent methods or `glam`'s
//! trig-bearing constructors (`Mat4::perspective_rh`, `Quat::from_euler`) — those call the platform C
//! `libm`, which differs in the last bit between Windows and macOS. Matrix inverses go through
//! [`ludion_core::math::mat4_inverse`] (a fixed scalar order; `glam`'s `Mat4::inverse` sums in a
//! different order on SSE2 and NEON), so only plain matrix arithmetic touches `glam`. The formulas mirror the renderer's camera
//! (`crates/ludion-engine/src/extract.rs`) and the editor gizmo's projection
//! (`editor/app/src/editor/gizmo/camera.ts`); the deterministic (libm-trig) build differs from the
//! renderer's glam-std-trig matrix by well under a pixel — sim determinism outranks last-bit
//! agreement with the render matrix.
//!
//! This module also re-exports [`ludion_core::math`], so one `use ludion::math` brings the
//! deterministic transcendentals (`math::sin`, `math::sqrt`, `math::glam`, …) alongside these helpers.
//!
//! # Screen-space UI hit-testing
//!
//! [`resolve_ui_rect`] is the same anchor arithmetic the renderer's overlay pass draws with — one of
//! five re-exports here (each takes the resolved [`FontFace`] first), alongside [`measure_ui_text`], [`resolve_ui_text`] and [`wrap_ui_text`] for
//! [`UiText`](crate::prelude::UiText), and [`list_hit`] for [`UiList`](crate::prelude::UiList): given
//! a viewport and a cursor position it returns the item index under the cursor, or `None`.
//! [`InputState`](crate::prelude::InputState)'s `mouse().position()` is already reported in
//! render-target pixels, origin top-left, `+Y` down — the exact space `resolve_ui_rect` resolves an
//! anchored [`UiRect`](crate::prelude::UiRect) to — so hit-testing a HUD element under the cursor is
//! a plain point-in-rect comparison, no unprojection and no camera involved:
//!
//! ```
//! use ludion::math::resolve_ui_rect;
//! use ludion::prelude::Anchor;
//!
//! // A bottom-left button, 24 canvas units in from the corner, 200x48.
//! let rect = resolve_ui_rect(Anchor::BottomLeft, [24.0, -24.0], [200.0, 48.0], (1280, 720));
//! let cursor = [80.0, 700.0]; // render-target pixels, from `mouse().position()`
//! let hit = cursor[0] >= rect[0] && cursor[0] < rect[0] + rect[2]
//!     && cursor[1] >= rect[1] && cursor[1] < rect[1] + rect[3];
//! assert!(hit);
//! ```
//!
//! A rect with `fill: true` covers the whole target, which `resolve_ui_rect` knows nothing of: hit-test
//! a [`UiRect`](crate::prelude::UiRect) value with [`resolve_ui_rect_of`], which returns
//! `[0, 0, width, height]` for a filled rect and the anchored rect otherwise.
//!
//! Once an element sits in a [`UiGroup`](crate::prelude::UiGroup), is tweened, world-anchored or
//! clipped, the point-in-rect above no longer matches what is drawn: [`ui_hit`] takes the world, the
//! entity, the viewport, the cursor and the sim time, and answers over the element as `extract` draws
//! it (group delta, tweens, world anchor, group affine, visibility and the clip). It reads the world
//! and is not part of the sim's hashed state.
//!
//! A rotated `UiRect` (`rotation` in degrees, clockwise on screen, about its centre) is hit-tested
//! with [`ui_point_in`], and [`ui_rect_corners`] gives its four drawn corners; both use
//! `ludion_core::math` trig only when the rotation is non-zero.
//!
//! These four are pure arithmetic (no trig, no transcendentals) exactly like [`camera_ray`]/
//! [`ray_ground_plane`] above, so their output is as safe to fold into hashed sim state as any other
//! deterministic helper here.
//!
//! A [`UiShape`](crate::prelude::UiShape) is hit-tested with [`ui_shape_hit`]: its fill, or its stroke band,
//! with the shape's own `rotation` turned back about its origin; grouped, tweened or clipped, ask [`ui_hit`].
//!
//! A [`UiRadial`](crate::prelude::UiRadial) wheel has its own pair: [`radial_layout`] places the centre,
//! radii and each item's label point, and [`radial_hit`] returns the item under a cursor (the gap and the
//! hole are `None`, a disabled item is returned). Both are ungrouped and use `libm` trig through
//! `ludion_core::math`, so they are as deterministic as everything else here.
//!
//! # Euler angles
//!
//! [`euler_xyz_degrees_to_quat`] and [`quat_to_euler_xyz_degrees`] convert between `Transform.rotation`'s
//! Euler XYZ degrees and a quaternion `[x, y, z, w]`, with libm trig — the same conversion the engine
//! uses everywhere, so a game's physics-side poses agree with the renderer's.
//!
//! # Picking on a model sheet
//!
//! [`model_camera`] returns the camera `ludion render --model` draws one cell with — the same
//! arithmetic the sheet itself frames by, not a copy — in exactly the plain-data spelling
//! [`camera_ray`] takes. Given a view, a model's AABB and the cell size, composing the two turns a
//! pixel of a rendered sheet into a world ray, so an author can point at what they are looking at.
//! [`ModelCamera`] carries the normalize transform alongside, because the cell draws the model
//! scaled into a unit sphere rather than at its authored size. [`world_to_pixel`] is the inverse of
//! [`camera_ray`], for the other direction: where a point of the normalized model lands in the cell.
//!
//! # Example
//!
//! ```
//! use ludion::math::{camera_ray, ray_ground_plane, Projection};
//!
//! // A follow camera above and behind the origin, tilted 58° down toward the ground.
//! let ray = camera_ray(
//!     [0.0, 25.44, 15.90],            // translation
//!     [-58.0, 0.0, 0.0],              // Euler XYZ degrees (pitch down), matching Transform.rotation
//!     Projection::Perspective { fov_y_deg: 50.0 },
//!     0.1,
//!     1000.0,
//!     [640.0, 360.0],                 // screen centre, render-target pixels
//!     (1280, 720),
//! )
//! .expect("a valid camera");
//! let aim = ray_ground_plane(&ray).expect("looking down at the ground");
//! // The centre ray of a camera aimed at the origin hits the ground back at the origin.
//! assert!(aim[0].abs() < 0.1 && aim[1].abs() < 0.1);
//! ```
//!
//! [`nine_slice`] splits a resolved `UiRect` into the nine pieces `extract` draws for its `slice`;
//! [`sub_uv`] narrows an image region to a `uv_rect`.
//!
//! ```
//! use ludion::math::nine_slice;
//! use ludion_engine::components::ui::{UiSlice, UiSliceCenter};
//!
//! let slice = UiSlice { border: [4.0; 4], scale: 1.0, center: UiSliceCenter::Hidden };
//! let nine = nine_slice([0.0, 0.0, 120.0, 60.0], [0.0, 0.0, 1.0, 1.0], [12.0, 12.0], &slice, 1.0);
//! assert_eq!(nine.pieces.len(), 8); // the hidden centre is skipped
//! ```

pub use ludion_core::math::*;
pub use ludion_engine::ui::{
    FontFace, NineSlice, RadialItemLayout, RadialLayout, WorldAnchorPlacement, canvas_anchor_px,
    group_delta, list_hit, measure_ui_list, measure_ui_text, nine_slice, place_world_anchor,
    radial_hit, radial_layout, resolve_ui_list, resolve_ui_rect, resolve_ui_rect_of,
    resolve_ui_text, sub_uv, ui_hit, ui_list_window, ui_point_in, ui_rect_corners, ui_shape_hit,
    ui_tween_sample, wrap_ui_text,
};
pub use ludion_engine::{ModelCamera, ModelView, fit_distance_for_aspect, model_camera};

use ludion_core::math::{
    self,
    glam::{Mat4, Quat, Vec3, Vec4},
};

/// Degrees→radians, in `f32` (pure arithmetic — deterministic, unlike `f32::to_radians`'s cousins).
const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

/// A camera's projection, matching the wire `Camera.projection` enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A perspective projection with a vertical field of view, in degrees.
    Perspective {
        /// Vertical field of view, in degrees.
        fov_y_deg: f32,
    },
    /// An orthographic projection with a fixed visible world height (aspect sets the width).
    Orthographic {
        /// The visible world height; the width follows from the viewport aspect.
        height: f32,
    },
}

/// A world-space ray: an `origin` on the near plane and a unit-length `dir` toward the far plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// The ray's world-space origin (on the camera's near plane).
    pub origin: [f32; 3],
    /// The ray's world-space direction, a unit vector pointing away from the camera.
    pub dir: [f32; 3],
}

/// A world-space ray through a render-target pixel, or `None` on a degenerate camera.
///
/// The pixel is unprojected at NDC depth 0 (near plane) for the origin and depth 1 (far plane) for
/// the direction — uniform across perspective and orthographic. `screen_pos` is in render-target
/// pixels, origin top-left, `+Y` down; `cam_rotation_deg` is Euler XYZ degrees, matching
/// `Transform.rotation`.
///
/// Returns `None` when the camera cannot produce a ray: a zero-size `viewport`, a non-finite or
/// degenerate projection (e.g. `fov_y_deg` of 0 or a non-positive orthographic `height`), or a
/// non-invertible view·projection matrix. A `near`/`far` that is not finite with `0 < near < far`
/// takes the default range (`0.1`, `1000.0`), exactly as a render does.
///
/// # Example
///
/// ```
/// use ludion::math::{camera_ray, Projection};
///
/// let ray = camera_ray(
///     [0.0, 10.0, 0.0],
///     [-90.0, 0.0, 0.0], // straight down
///     Projection::Orthographic { height: 20.0 },
///     0.1,
///     100.0,
///     [400.0, 300.0],
///     (800, 600),
/// )
/// .unwrap();
/// assert!(ray.dir[1] < 0.0); // pointing at the ground
/// ```
pub fn camera_ray(
    cam_translation: [f32; 3],
    cam_rotation_deg: [f32; 3],
    projection: Projection,
    near: f32,
    far: f32,
    screen_pos: [f32; 2],
    viewport: (u32, u32),
) -> Option<Ray> {
    let (vw, vh) = viewport;
    let view_proj = view_proj_matrix(
        cam_translation,
        cam_rotation_deg,
        projection,
        near,
        far,
        viewport,
    )?;
    let inv = math::mat4_inverse(view_proj);
    if !inv.is_finite() {
        return None;
    }

    let width = vw as f32;
    let height = vh as f32;
    let near_pt = unproject(screen_pos, 0.0, width, height, inv)?;
    let far_pt = unproject(screen_pos, 1.0, width, height, inv)?;

    let delta = far_pt - near_pt;
    // libm sqrt: correctly-rounded and platform-identical, like every irrational op on the sim path.
    // `dot` of a finite vector is non-negative and finite, so `len` is finite here.
    let len = math::sqrt(delta.dot(delta));
    if len <= 1e-12 {
        return None;
    }
    Some(Ray {
        origin: near_pt.to_array(),
        dir: (delta / len).to_array(),
    })
}

/// The render-target pixel a world-space `point` projects to, or `None` when it cannot.
///
/// The inverse of [`camera_ray`], with the same argument order: pixels are render-target pixels,
/// origin top-left, `+Y` down, and edge-relative — the centre of pixel `i` is `i + 0.5`. The result
/// may lie outside the `viewport`; the caller decides what to do with it.
///
/// Returns `None` when the camera is degenerate (as [`camera_ray`]) or the point is at or behind the
/// camera plane (non-positive or non-finite homogeneous `w`).
///
/// # Example
///
/// ```
/// use ludion::math::{camera_ray, world_to_pixel, Projection};
///
/// let (translation, rotation) = ([0.0, 0.0, 5.0], [0.0, 0.0, 0.0]);
/// let projection = Projection::Perspective { fov_y_deg: 50.0 };
/// let point = [0.5, 0.25, 0.0];
///
/// let pixel = world_to_pixel(translation, rotation, projection, 0.1, 100.0, point, (800, 600))
///     .expect("in front of the camera");
/// // The ray back through that pixel passes through the point.
/// let ray = camera_ray(translation, rotation, projection, 0.1, 100.0, pixel, (800, 600)).unwrap();
/// let t = (point[2] - ray.origin[2]) / ray.dir[2];
/// assert!((ray.origin[0] + t * ray.dir[0] - point[0]).abs() < 1e-3);
/// assert!((ray.origin[1] + t * ray.dir[1] - point[1]).abs() < 1e-3);
/// ```
pub fn world_to_pixel(
    cam_translation: [f32; 3],
    cam_rotation_deg: [f32; 3],
    projection: Projection,
    near: f32,
    far: f32,
    point: [f32; 3],
    viewport: (u32, u32),
) -> Option<[f32; 2]> {
    let view_proj = view_proj_matrix(
        cam_translation,
        cam_rotation_deg,
        projection,
        near,
        far,
        viewport,
    )?;
    let clip = view_proj * Vec3::from_array(point).extend(1.0);
    if !clip.is_finite() || clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some([
        (ndc.x * 0.5 + 0.5) * viewport.0 as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport.1 as f32,
    ])
}

/// Where a world-space `point` lands on screen for a HUD element pinned to it, clamped to the viewport
/// edge inset by `margin` canvas units: the same placement `extract` gives a `UiWorldAnchor` (see
/// [`place_world_anchor`]).
///
/// Arguments follow [`world_to_pixel`], plus the `margin`. `None` when the camera is degenerate or the
/// projection is not finite. The result carries the pixel, whether it is `on_screen`, whether the point
/// is `behind` the camera and its `distance`.
///
/// # Example
///
/// ```
/// use ludion::math::{world_anchor, Projection};
///
/// let projection = Projection::Perspective { fov_y_deg: 50.0 };
/// let at = |point| {
///     world_anchor([0.0, 0.0, 5.0], [0.0; 3], projection, 0.1, 100.0, point, (800, 600), 48.0)
///         .expect("a valid camera")
/// };
/// assert!(at([0.0, 0.0, 0.0]).on_screen);
/// let far_right = at([40.0, 0.0, 0.0]);
/// assert!(!far_right.on_screen);
/// assert!((far_right.pixel[0] - (800.0 - 48.0 * 600.0 / 1080.0)).abs() < 1e-3);
/// ```
#[allow(clippy::too_many_arguments)]
pub fn world_anchor(
    cam_translation: [f32; 3],
    cam_rotation_deg: [f32; 3],
    projection: Projection,
    near: f32,
    far: f32,
    point: [f32; 3],
    viewport: (u32, u32),
    margin: f32,
) -> Option<WorldAnchorPlacement> {
    let (view, proj) = view_and_projection(
        cam_translation,
        cam_rotation_deg,
        projection,
        near,
        far,
        viewport,
    )?;
    place_world_anchor(view, proj, point, viewport, margin)
}

/// The `(x, z)` point where `ray` crosses the `y = 0` ground plane, or `None` when it cannot.
///
/// Solves `origin.y + t · dir.y = 0` and returns `[x, z]` at that `t`. `None` when the ray is
/// parallel to the plane (`dir.y ≈ 0`) or the intersection is not strictly ahead of the origin
/// (`t ≤ 0` — the ray points away from, or starts on, the plane); the strict `t > 0` boundary also
/// rejects a `NaN` `t`.
///
/// # Example
///
/// ```
/// use ludion::math::{ray_ground_plane, Ray};
///
/// // Straight down from 5 units up lands at the origin; straight up never reaches the ground.
/// let down = Ray { origin: [2.0, 5.0, -1.0], dir: [0.0, -1.0, 0.0] };
/// assert_eq!(ray_ground_plane(&down), Some([2.0, -1.0]));
/// let up = Ray { origin: [2.0, 5.0, -1.0], dir: [0.0, 1.0, 0.0] };
/// assert_eq!(ray_ground_plane(&up), None);
/// ```
pub fn ray_ground_plane(ray: &Ray) -> Option<[f32; 2]> {
    let oy = ray.origin[1];
    let dy = ray.dir[1];
    if dy.abs() < 1e-9 {
        return None; // parallel to the ground plane
    }
    // Strictly ahead of the origin. `partial_cmp` keeps this NaN-safe (a non-finite `Ray` → None)
    // without the negated float comparison clippy flags.
    let t = -oy / dy;
    if t.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater) {
        return None; // on, or behind, the ray origin (pointing away), or non-finite
    }
    Some([
        ray.origin[0] + t * ray.dir[0],
        ray.origin[2] + t * ray.dir[2],
    ])
}

/// Builds the view·projection matrix, or `None` on a degenerate viewport/projection. Shared by
/// [`camera_ray`] and its round-trip parity test — the one place the deterministic camera is assembled.
fn view_proj_matrix(
    cam_translation: [f32; 3],
    cam_rotation_deg: [f32; 3],
    projection: Projection,
    near: f32,
    far: f32,
    viewport: (u32, u32),
) -> Option<Mat4> {
    let (view, proj) = view_and_projection(
        cam_translation,
        cam_rotation_deg,
        projection,
        near,
        far,
        viewport,
    )?;
    let view_proj = proj * view;
    view_proj.is_finite().then_some(view_proj)
}

/// The view and projection matrices of the deterministic camera, or `None` on a degenerate
/// viewport/projection: [`view_proj_matrix`] and [`world_anchor`] build on it.
fn view_and_projection(
    cam_translation: [f32; 3],
    cam_rotation_deg: [f32; 3],
    projection: Projection,
    near: f32,
    far: f32,
    viewport: (u32, u32),
) -> Option<(Mat4, Mat4)> {
    let (vw, vh) = viewport;
    if vw == 0 || vh == 0 {
        return None;
    }
    let aspect = vw as f32 / vh as f32;

    let quat = Quat::from_array(euler_xyz_degrees_to_quat(cam_rotation_deg));
    let model = Mat4::from_rotation_translation(quat, Vec3::from_array(cam_translation));
    let view = math::mat4_inverse(model);
    let proj = build_projection(projection, aspect, near, far)?;
    (view.is_finite() && proj.is_finite()).then_some((view, proj))
}

/// The projection matrix, mirroring `build_camera` (`extract.rs`) / `perspectiveRh` (`camera.ts`) but
/// with the perspective cotangent from libm `tan` — never `Mat4::perspective_rh` (std trig, non-
/// deterministic across OS). `Mat4::orthographic_rh` is pure arithmetic, so it is used as-is.
fn build_projection(projection: Projection, aspect: f32, near: f32, far: f32) -> Option<Mat4> {
    let (near, far) = ludion_engine::Camera {
        near,
        far,
        ..Default::default()
    }
    .depth_range();
    let m = match projection {
        Projection::Perspective { fov_y_deg } => {
            if !fov_y_deg.is_finite() {
                return None;
            }
            let half_fov = fov_y_deg * DEG_TO_RAD * 0.5;
            let tan = math::tan(half_fov);
            if !tan.is_finite() || tan.abs() < 1e-12 {
                return None; // fov of 0 / 180° etc. — no finite cotangent
            }
            let h = 1.0 / tan;
            let w = h / aspect;
            let r = far / (near - far);
            Mat4::from_cols(
                Vec4::new(w, 0.0, 0.0, 0.0),
                Vec4::new(0.0, h, 0.0, 0.0),
                Vec4::new(0.0, 0.0, r, -1.0),
                Vec4::new(0.0, 0.0, r * near, 0.0),
            )
        }
        Projection::Orthographic { height } => {
            if !(height.is_finite() && height > 0.0) {
                return None;
            }
            let half_h = height * 0.5;
            let half_w = half_h * aspect;
            Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, near, far)
        }
    };
    m.is_finite().then_some(m)
}

/// Unprojects a render-target pixel at NDC depth `ndc_z` to a world point, or `None` on a degenerate
/// homogeneous `w`. `width`/`height` are the (non-zero) viewport extents.
fn unproject(screen: [f32; 2], ndc_z: f32, width: f32, height: f32, inv: Mat4) -> Option<Vec3> {
    let ndc_x = (screen[0] / width) * 2.0 - 1.0;
    let ndc_y = (1.0 - screen[1] / height) * 2.0 - 1.0;
    let h = inv * Vec4::new(ndc_x, ndc_y, ndc_z, 1.0);
    if h.w.abs() < 1e-12 {
        return None;
    }
    Some(h.truncate() / h.w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The G0 tilted follow camera (roadmap defaults): pitch 58° down, distance 30, fov 50°. Placed so
    /// it looks at the world origin — translation = -30 · forward from the origin.
    const G0_PITCH_DEG: f32 = -58.0;
    const G0_FOV_DEG: f32 = 50.0;
    const G0_NEAR: f32 = 0.1;
    const G0_FAR: f32 = 1000.0;
    const G0_VIEWPORT: (u32, u32) = (1280, 720);

    fn g0_translation() -> [f32; 3] {
        // forward = (0, -sin58, -cos58); position = origin - 30·forward.
        let s = math::sin(58.0 * DEG_TO_RAD);
        let c = math::cos(58.0 * DEG_TO_RAD);
        [0.0, 30.0 * s, 30.0 * c]
    }

    fn g0_ray(screen_pos: [f32; 2]) -> Ray {
        camera_ray(
            g0_translation(),
            [G0_PITCH_DEG, 0.0, 0.0],
            Projection::Perspective {
                fov_y_deg: G0_FOV_DEG,
            },
            G0_NEAR,
            G0_FAR,
            screen_pos,
            G0_VIEWPORT,
        )
        .expect("the G0 camera is well-formed")
    }

    fn center() -> [f32; 2] {
        [G0_VIEWPORT.0 as f32 / 2.0, G0_VIEWPORT.1 as f32 / 2.0]
    }

    #[test]
    fn g0_center_ray_hits_the_look_at_point() {
        // A camera aimed at the origin: its centre ray hits the ground back at the origin (0, 0).
        let aim = ray_ground_plane(&g0_ray(center())).expect("looking down at the ground");
        assert!(aim[0].abs() < 1e-2, "x = {}", aim[0]);
        assert!(aim[1].abs() < 1e-2, "z = {}", aim[1]);
    }

    #[test]
    fn top_down_orthographic_axes() {
        // Straight-down ortho at height 20 over an 800×600 viewport: visible half-extents are
        // height/2 = 10 vertically and height·aspect/2 ≈ 13.33 horizontally.
        let cam = [0.0, 20.0, 0.0];
        let rot = [-90.0, 0.0, 0.0]; // look down -Y
        let proj = Projection::Orthographic { height: 20.0 };
        let vp = (800, 600);
        let aspect = 800.0 / 600.0;
        let ray = |px: [f32; 2]| {
            camera_ray(cam, rot, proj, 0.1, 100.0, px, vp).expect("well-formed ortho camera")
        };

        let mid = ray_ground_plane(&ray([400.0, 300.0])).unwrap();
        assert!(mid[0].abs() < 1e-3 && mid[1].abs() < 1e-3, "centre {mid:?}");

        // Right edge → +X = +height·aspect/2; screen-up (top) → -Z.
        let right = ray_ground_plane(&ray([800.0, 300.0])).unwrap();
        assert!(
            (right[0] - 20.0 * aspect / 2.0).abs() < 1e-2,
            "right {right:?}"
        );
        assert!(right[1].abs() < 1e-2, "right {right:?}");

        let top = ray_ground_plane(&ray([400.0, 0.0])).unwrap();
        assert!(top[0].abs() < 1e-2, "top {top:?}");
        assert!((top[1] - -10.0).abs() < 1e-2, "top {top:?}");
    }

    #[test]
    fn symmetric_screen_offsets_give_symmetric_ground_offsets() {
        // The G0 camera has no yaw/roll, so horizontal screen offsets mirror in world X, same Z.
        let [cx, cy] = center();
        let left = ray_ground_plane(&g0_ray([cx - 200.0, cy])).unwrap();
        let right = ray_ground_plane(&g0_ray([cx + 200.0, cy])).unwrap();
        assert!(
            (left[0] + right[0]).abs() < 1e-2,
            "x mirror {left:?} {right:?}"
        );
        assert!(
            (left[1] - right[1]).abs() < 1e-2,
            "z equal {left:?} {right:?}"
        );
    }

    #[test]
    fn parallel_and_away_rays_have_no_ground_hit() {
        // Horizontal look (forward ≈ -Z): parallel to the ground → None.
        let parallel = camera_ray(
            [0.0, 5.0, 0.0],
            [0.0, 0.0, 0.0],
            Projection::Perspective { fov_y_deg: 50.0 },
            0.1,
            1000.0,
            center(),
            G0_VIEWPORT,
        )
        .unwrap();
        assert_eq!(ray_ground_plane(&parallel), None);

        // Pitched up from above the plane: the ray points away → None.
        let away = camera_ray(
            [0.0, 5.0, 0.0],
            [30.0, 0.0, 0.0],
            Projection::Perspective { fov_y_deg: 50.0 },
            0.1,
            1000.0,
            center(),
            G0_VIEWPORT,
        )
        .unwrap();
        assert_eq!(ray_ground_plane(&away), None);
    }

    #[test]
    fn a_degenerate_depth_range_takes_the_default_range() {
        let ray = |near: f32, far: f32| {
            camera_ray(
                g0_translation(),
                [G0_PITCH_DEG, 0.0, 0.0],
                Projection::Perspective {
                    fov_y_deg: G0_FOV_DEG,
                },
                near,
                far,
                [300.0, 200.0],
                G0_VIEWPORT,
            )
            .expect("a degenerate depth range still yields a ray")
        };
        let default = ray(0.1, 1000.0);
        for (near, far) in [(5.0, 1.0), (1.0, 1.0), (0.0, 10.0), (f32::NAN, 10.0)] {
            assert_eq!(ray(near, far), default, "near {near}, far {far}");
        }
    }

    #[test]
    fn degenerate_camera_returns_none() {
        let ok_rot = [G0_PITCH_DEG, 0.0, 0.0];
        let persp = Projection::Perspective { fov_y_deg: 50.0 };

        // Zero-size viewport (either extent).
        assert_eq!(
            camera_ray(
                g0_translation(),
                ok_rot,
                persp,
                0.1,
                1000.0,
                center(),
                (0, 0)
            ),
            None
        );
        assert_eq!(
            camera_ray(
                g0_translation(),
                ok_rot,
                persp,
                0.1,
                1000.0,
                [0.0, 0.0],
                (1280, 0)
            ),
            None
        );
        // Degenerate perspective fov.
        assert_eq!(
            camera_ray(
                g0_translation(),
                ok_rot,
                Projection::Perspective { fov_y_deg: 0.0 },
                0.1,
                1000.0,
                center(),
                G0_VIEWPORT
            ),
            None
        );
        assert_eq!(
            camera_ray(
                g0_translation(),
                ok_rot,
                Projection::Perspective {
                    fov_y_deg: f32::NAN
                },
                0.1,
                1000.0,
                center(),
                G0_VIEWPORT
            ),
            None
        );
        // Non-positive orthographic height.
        assert_eq!(
            camera_ray(
                g0_translation(),
                ok_rot,
                Projection::Orthographic { height: 0.0 },
                0.1,
                1000.0,
                center(),
                G0_VIEWPORT
            ),
            None
        );
    }

    #[test]
    fn round_trip_parity_recovers_the_ground_point() {
        // Project a known ground point to a pixel with bounds.rs's forward projection, then unproject
        // that pixel and re-intersect the ground. Tolerance: 1e-2 world units — the deterministic
        // libm-trig matrix vs. its own inverse round-trips to far under a pixel of aim (the parity
        // budget decision 7 documents).
        let pixel = world_to_pixel(
            g0_translation(),
            [G0_PITCH_DEG, 0.0, 0.0],
            Projection::Perspective {
                fov_y_deg: G0_FOV_DEG,
            },
            G0_NEAR,
            G0_FAR,
            [5.0, 0.0, -3.0],
            G0_VIEWPORT,
        )
        .expect("the parity point must be in front of the camera");
        let aim = ray_ground_plane(&g0_ray(pixel)).unwrap();
        assert!((aim[0] - 5.0).abs() < 1e-2, "x recovered {}", aim[0]);
        assert!((aim[1] - -3.0).abs() < 1e-2, "z recovered {}", aim[1]);
    }

    /// **The sheet camera composes with the pick ray.** A ray through the centre of a cell drawn
    /// by [`model_camera`] passes through the centre of the normalized model — the composition
    /// `ludion mesh pick` is built on, checked on the CPU with no device involved.
    ///
    /// The centre of a model's *box* is not where the camera looks (the rig targets the box centre
    /// only in `x`/`z`, with `y` half the scaled height above the ground the model stands on), so
    /// the target is taken from the camera itself rather than re-derived — which is the point of
    /// returning it.
    #[test]
    fn the_model_sheet_camera_rays_through_the_subject() {
        let (min, max) = ([-2.0, 0.0, -1.0], [2.0, 6.0, 1.0]);
        let (cell_w, cell_h) = (1920u32, 1080u32);
        for view in ModelView::ALL {
            let camera = model_camera(view, min, max, cell_w, cell_h);
            let ray = camera_ray(
                camera.translation,
                camera.rotation_deg,
                Projection::Perspective {
                    fov_y_deg: camera.fov_y_deg,
                },
                camera.near,
                camera.far,
                [cell_w as f32 * 0.5, cell_h as f32 * 0.5],
                (cell_w, cell_h),
            )
            .expect("a well-formed sheet camera");

            // Distance from the target to the ray: |(target - origin) x dir|, dir being unit length.
            let origin = Vec3::from(ray.origin);
            let offset = Vec3::from(camera.target) - origin;
            let miss = offset.cross(Vec3::from(ray.dir)).length();
            assert!(miss < 1e-4, "{}: centre ray missed by {miss}", view.name());
            assert!(
                offset.dot(Vec3::from(ray.dir)) > 0.0,
                "{}: the subject is behind the camera",
                view.name()
            );

            // And the model really is normalized into that frame: its box centre maps inside the
            // unit sphere the camera was fitted to.
            let normalized = |p: [f32; 3]| -> Vec3 {
                Vec3::from(std::array::from_fn::<f32, 3, _>(|i| {
                    p[i] * camera.normalize_scale + camera.normalize_translation[i]
                }))
            };
            let centre = normalized([0, 1, 2].map(|i| 0.5 * (min[i] + max[i])));
            assert!(centre.length() <= 1.0 + 1e-4, "{centre:?}");
        }
    }

    /// `world_anchor` agrees with `world_to_pixel` for a point inside the margin, and is the engine's
    /// `place_world_anchor` over the same camera matrices.
    #[test]
    fn world_anchor_matches_world_to_pixel_and_the_engine_placement() {
        let projection = Projection::Perspective { fov_y_deg: 50.0 };
        let (translation, rotation, viewport) = ([1.0, 2.0, 9.0], [-10.0, 15.0, 0.0], (1280, 720));
        let point = [0.5, 0.25, 0.0];
        let placed = world_anchor(
            translation,
            rotation,
            projection,
            0.1,
            100.0,
            point,
            viewport,
            48.0,
        )
        .expect("a valid camera");
        let pixel = world_to_pixel(
            translation,
            rotation,
            projection,
            0.1,
            100.0,
            point,
            viewport,
        )
        .expect("in front");
        assert!(placed.on_screen && !placed.behind);
        assert_eq!(placed.pixel, pixel);

        let (view, proj) =
            view_and_projection(translation, rotation, projection, 0.1, 100.0, viewport).unwrap();
        assert_eq!(
            place_world_anchor(view, proj, [40.0, 0.0, 0.0], viewport, 48.0),
            world_anchor(
                translation,
                rotation,
                projection,
                0.1,
                100.0,
                [40.0, 0.0, 0.0],
                viewport,
                48.0
            )
        );
    }

    /// `world_to_pixel` is the inverse of `camera_ray` on every sheet camera: a point inside the
    /// (normalized) model, projected and cast back, lies on the ray.
    #[test]
    fn world_to_pixel_inverts_camera_ray_on_every_sheet_view() {
        let (min, max) = ([-2.0, 0.0, -1.0], [2.0, 6.0, 1.0]);
        let (cell_w, cell_h) = (1920u32, 1080u32);
        for view in ModelView::ALL {
            let camera = model_camera(view, min, max, cell_w, cell_h);
            let projection = Projection::Perspective {
                fov_y_deg: camera.fov_y_deg,
            };
            let model_point = [0.7, 2.2, -0.3];
            let point: [f32; 3] = std::array::from_fn(|i| {
                model_point[i] * camera.normalize_scale + camera.normalize_translation[i]
            });
            let pixel = world_to_pixel(
                camera.translation,
                camera.rotation_deg,
                projection,
                camera.near,
                camera.far,
                point,
                (cell_w, cell_h),
            )
            .expect("the point is in front of the sheet camera");
            let ray = camera_ray(
                camera.translation,
                camera.rotation_deg,
                projection,
                camera.near,
                camera.far,
                pixel,
                (cell_w, cell_h),
            )
            .expect("a well-formed sheet camera");
            let miss = (Vec3::from(point) - Vec3::from(ray.origin))
                .cross(Vec3::from(ray.dir))
                .length();
            assert!(
                miss < 1e-3,
                "{}: ray missed the point by {miss}",
                view.name()
            );
        }
    }

    #[test]
    fn world_to_pixel_refuses_a_point_behind_the_camera() {
        let behind = world_to_pixel(
            [0.0, 0.0, 5.0],
            [0.0, 0.0, 0.0],
            Projection::Perspective { fov_y_deg: 50.0 },
            0.1,
            100.0,
            [0.0, 0.0, 9.0],
            (800, 600),
        );
        assert_eq!(behind, None);
    }

    #[test]
    fn double_run_is_bit_identical() {
        let a = g0_ray(center());
        let b = g0_ray(center());
        for i in 0..3 {
            assert_eq!(a.origin[i].to_bits(), b.origin[i].to_bits());
            assert_eq!(a.dir[i].to_bits(), b.dir[i].to_bits());
        }
        let ga = ray_ground_plane(&a).unwrap();
        let gb = ray_ground_plane(&b).unwrap();
        assert_eq!(ga[0].to_bits(), gb[0].to_bits());
        assert_eq!(ga[1].to_bits(), gb[1].to_bits());
    }

    /// `world_to_pixel` is a pure function of the pose it is handed: the rig's components give the
    /// no-offset answer, and `CameraOffset::applied_to` gives the pixel the drawn frame shows.
    #[test]
    fn world_to_pixel_follows_the_offset_only_through_applied_to() {
        use crate::prelude::World;
        use ludion_engine::{Camera, CameraOffset, Transform};

        let viewport = (320, 180);
        let mut engine = ludion_engine::Engine::new().with_render_size(viewport.0, viewport.1);
        let world: &mut World = engine.world_mut();
        let camera = world.spawn();
        let rig = Transform {
            translation: [1.0, 2.0, 10.0],
            rotation: [-5.0, 12.0, 0.0],
            ..Transform::default()
        };
        world.insert(camera, rig).unwrap();
        world.insert(camera, Camera::default()).unwrap();
        let offset = CameraOffset {
            translation: [0.5, -0.25, 4.0],
            rotation_deg: [0.0, 7.0, 0.0],
            fov_delta: -10.0,
        };
        let intro = world.spawn_presentation();
        world.insert(intro, offset).unwrap();

        let drawn = engine.extract_present_scene().expect("extracts").0.camera;
        let point = [0.5, 0.25, 0.0];
        let through_extract = {
            let clip = drawn.projection * drawn.view * Vec3::from_array(point).extend(1.0);
            let ndc = clip.truncate() / clip.w;
            [
                (ndc.x * 0.5 + 0.5) * viewport.0 as f32,
                (1.0 - (ndc.y * 0.5 + 0.5)) * viewport.1 as f32,
            ]
        };
        let project = |transform: &ludion_engine::Transform, camera: &Camera| {
            let ludion_engine::Projection::Perspective { fov_y_deg } = camera.projection else {
                unreachable!("the camera is perspective")
            };
            world_to_pixel(
                transform.translation,
                transform.rotation,
                Projection::Perspective { fov_y_deg },
                camera.near,
                camera.far,
                point,
                viewport,
            )
            .expect("in front of the camera")
        };

        let by_rig = project(&rig, &Camera::default());
        let (moved, moved_camera) = offset.applied_to(&rig, &Camera::default());
        let by_offset = project(&moved, &moved_camera);
        assert!(
            (by_rig[0] - by_offset[0]).abs() > 1.0,
            "the offset moves the pixel: {by_rig:?} vs {by_offset:?}"
        );
        for axis in 0..2 {
            assert!(
                (by_offset[axis] - through_extract[axis]).abs() < 0.05,
                "applied_to matches extract: {by_offset:?} vs {through_extract:?}"
            );
        }
    }

    /// `camera_ray` built from the camera's own components ignores a presentation offset, so a
    /// pick ray is the same with or without one; only `applied_to`'s pose gives the drawn ray.
    #[test]
    fn camera_ray_ignores_the_offset_unless_applied() {
        use crate::prelude::World;
        use ludion_engine::{Camera, CameraOffset, Transform};

        let viewport = (320, 180);
        let rig = Transform {
            translation: [1.0, 2.0, 10.0],
            rotation: [-5.0, 12.0, 0.0],
            ..Transform::default()
        };
        let build = |offset: Option<CameraOffset>| {
            let mut engine = ludion_engine::Engine::new().with_render_size(viewport.0, viewport.1);
            let world: &mut World = engine.world_mut();
            let camera = world.spawn();
            world.insert(camera, rig).unwrap();
            world.insert(camera, Camera::default()).unwrap();
            if let Some(offset) = offset {
                let intro = world.spawn_presentation();
                world.insert(intro, offset).unwrap();
            }
            engine.extract_present_scene().expect("extracts");
            (engine, camera)
        };
        let offset = CameraOffset {
            translation: [0.5, -0.25, 4.0],
            rotation_deg: [0.0, 7.0, 0.0],
            fov_delta: -10.0,
        };
        let pixel = [100.0, 60.0];
        let ray_of = |transform: &Transform, camera: &Camera| {
            let ludion_engine::Projection::Perspective { fov_y_deg } = camera.projection else {
                unreachable!("the camera is perspective")
            };
            camera_ray(
                transform.translation,
                transform.rotation,
                Projection::Perspective { fov_y_deg },
                camera.near,
                camera.far,
                pixel,
                viewport,
            )
            .expect("a well-formed camera")
        };
        let read = |engine: &ludion_engine::Engine, camera| {
            let world = engine.world();
            (
                *world.get::<Transform>(camera).unwrap().unwrap(),
                world.get::<Camera>(camera).unwrap().unwrap().clone(),
            )
        };

        let (with, with_cam) = build(Some(offset));
        let (without, without_cam) = build(None);
        let (t, c) = read(&with, with_cam);
        let (t0, c0) = read(&without, without_cam);
        let (ray, plain) = (ray_of(&t, &c), ray_of(&t0, &c0));
        assert_eq!(ray.origin, plain.origin);
        assert_eq!(ray.dir, plain.dir);

        let (moved, moved_camera) = offset.applied_to(&t, &c);
        let applied = ray_of(&moved, &moved_camera);
        assert_ne!(applied.origin, plain.origin);
        assert_ne!(applied.dir, plain.dir);
    }
}
