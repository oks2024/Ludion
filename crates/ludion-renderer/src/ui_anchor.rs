//! World-anchor placement: where a world point lands in a pixel box, and how [`blend`](crate::blend)
//! re-places the world-anchored UI rows of the newer tick against the blended camera and point.
//!
//! Extract and blend share [`place_world_anchor_in`], so a blended row lands where a fresh extract at
//! that camera and point would have put it.

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};

use crate::blend::{BlendData, keys};
use crate::scene::{RenderScene, UiInstance};

/// Where a world point lands on screen, from [`place_world_anchor_in`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldAnchorPlacement {
    /// Render-target pixel, top-left origin, +Y down: the projected point, or its clamp onto the edge
    /// inset.
    pub pixel: [f32; 2],
    /// The point is in front of the camera and inside the inset rectangle. Always `false` when the
    /// inset is degenerate.
    pub on_screen: bool,
    /// The point is at or behind the camera plane (perspective only).
    pub behind: bool,
    /// Distance from the camera to the point, in world units.
    pub distance: f32,
    /// Clockwise degrees from 12 o'clock, in `[0, 360)`, of the direction from the box centre toward
    /// the point (behind the camera: the clamp direction); 0 when there is none.
    pub angle_deg: f32,
}

/// Projects `point` through `view` and `projection` into a pixel box `[x, y, width, height]` inset by
/// `inset_px` pixels: the box stands where the viewport stood (a view's rect, a radar). `None` for a
/// non-finite projection or a box with no area.
///
/// In front of the camera the pixel is the usual projection; inside the inset rect it is `on_screen`,
/// otherwise the vector from the box centre is scaled until it meets the inset rect. Behind the camera
/// (`clip.w <= 1e-6`, a perspective camera) the direction is `(view.x, -view.y)` so the marker sits on
/// the edge the target lies toward; a point straight behind has no direction and goes straight down. A
/// degenerate inset (at least half the box) places every point at the box centre.
///
/// # Example
///
/// ```
/// use ludion_core::math::glam::{Mat4, Vec3};
/// use ludion_renderer::place_world_anchor_in;
///
/// let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
/// let projection = Mat4::perspective_rh(1.0, 2.0, 0.1, 100.0);
/// let hit = place_world_anchor_in(view, projection, [0.0; 3], [100.0, 50.0, 200.0, 100.0], 4.0).unwrap();
/// assert!((hit.pixel[0] - 200.0).abs() < 1e-3 && (hit.pixel[1] - 100.0).abs() < 1e-3);
/// ```
pub fn place_world_anchor_in(
    view: Mat4,
    projection: Mat4,
    point: [f32; 3],
    box_px: [f32; 4],
    inset_px: f32,
) -> Option<WorldAnchorPlacement> {
    let [x, y, width, height] = box_px;
    if !(width > 0.0 && height > 0.0) {
        return None;
    }
    let v = view * Vec3::from_array(point).extend(1.0);
    let clip = projection * v;
    if !clip.is_finite() {
        return None;
    }
    let distance = ludion_core::math::sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    let half = [width * 0.5 - inset_px, height * 0.5 - inset_px];
    let centre = [x + width * 0.5, y + height * 0.5];
    let behind = clip.w <= 1e-6;
    let px = [
        (clip.x / clip.w * 0.5 + 0.5) * width + x,
        (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * height + y,
    ];
    let mut dir = if behind {
        [v.x, -v.y]
    } else {
        [px[0] - centre[0], px[1] - centre[1]]
    };
    if behind && dir[0].abs() < 1e-6 && dir[1].abs() < 1e-6 {
        dir = [0.0, 1.0];
    }
    let angle_deg = clock_angle_deg(dir);
    if half[0] <= 0.0 || half[1] <= 0.0 {
        return Some(WorldAnchorPlacement {
            pixel: centre,
            on_screen: false,
            behind,
            distance,
            angle_deg,
        });
    }
    let clamp = |dir: [f32; 2]| {
        let sx = half[0] / dir[0].abs();
        let sy = half[1] / dir[1].abs();
        let s = sx.min(sy);
        [centre[0] + dir[0] * s, centre[1] + dir[1] * s]
    };
    if behind {
        return Some(WorldAnchorPlacement {
            pixel: clamp(dir),
            on_screen: false,
            behind: true,
            distance,
            angle_deg,
        });
    }
    let on_screen = dir[0].abs() <= half[0] && dir[1].abs() <= half[1];
    let pixel = if on_screen { px } else { clamp(dir) };
    pixel
        .iter()
        .all(|c| c.is_finite())
        .then_some(WorldAnchorPlacement {
            pixel,
            on_screen,
            behind: false,
            distance,
            angle_deg,
        })
}

/// Clockwise degrees from 12 o'clock of a +Y-down direction, in `[0, 360)`; 0 for none.
fn clock_angle_deg(dir: [f32; 2]) -> f32 {
    if !(dir[0].is_finite() && dir[1].is_finite()) || (dir[0] == 0.0 && dir[1] == 0.0) {
        return 0.0;
    }
    let degrees = ludion_core::math::atan2(dir[0], -dir[1]).to_degrees();
    if degrees < 0.0 {
        degrees + 360.0
    } else if degrees >= 360.0 {
        degrees - 360.0
    } else {
        degrees
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Re-places every UI row keyed to a world anchor in `out` (a clone of `next`, its cameras already
/// blended) against the blended camera and point: each row moves by the screen delta between the
/// anchor's placement at the blended state and at the newer tick's.
pub(crate) fn blend_ui_anchors(
    prev: &BlendData,
    next: &RenderScene,
    next_data: &BlendData,
    alpha: f32,
    out: &mut RenderScene,
) {
    if next_data.ui_anchors.is_empty() {
        return;
    }
    let mut deltas: Vec<(EntityId, [f32; 2])> = Vec::new();
    for a in &next_data.ui_anchors {
        let point = match prev
            .ui_anchors
            .binary_search_by_key(&a.anchor, |p| p.anchor)
        {
            Ok(i)
                if prev.ui_anchors[i].target == a.target
                    && next_data.snaps.binary_search(&a.target).is_err() =>
            {
                lerp3(prev.ui_anchors[i].point, a.point, alpha)
            }
            _ => a.point,
        };
        let (drawn, newer) = match a.view {
            None => (&out.camera, &next.camera),
            Some(i) => match (out.views.get(i as usize), next.views.get(i as usize)) {
                (Some(o), Some(n)) => (&o.camera, &n.camera),
                _ => continue,
            },
        };
        if point == a.point && drawn.view == newer.view && drawn.projection == newer.projection {
            continue;
        }
        let place = |view, projection, point| {
            place_world_anchor_in(view, projection, point, a.box_px, a.inset_px)
        };
        let (Some(now), Some(then)) = (
            place(drawn.view, drawn.projection, point),
            place(newer.view, newer.projection, a.point),
        ) else {
            continue;
        };
        let delta = [now.pixel[0] - then.pixel[0], now.pixel[1] - then.pixel[1]];
        if delta.iter().all(|d| d.is_finite()) && delta != [0.0, 0.0] {
            deltas.push((a.anchor, delta));
        }
    }
    if deltas.is_empty() {
        return;
    }
    deltas.sort_by_key(|(id, _)| *id);
    for (row, key) in out.ui.iter_mut().zip(keys(&next_data.ui, next.ui.len())) {
        let Some(key) = key else { continue };
        if let Ok(i) = deltas.binary_search_by_key(&key, |(id, _)| *id) {
            translate(row, deltas[i].1);
        }
    }
}

/// Moves one row by `d` pixels: geometry origins, a rotation's pivot (only when it turns) and every
/// clip box. Offsets and sizes are relative and stay.
fn translate(row: &mut UiInstance, d: [f32; 2]) {
    let (clip, rotation) = match row {
        UiInstance::Rect {
            rect_px,
            clip,
            rotation,
            ..
        } => {
            rect_px[0] += d[0];
            rect_px[1] += d[1];
            (clip, rotation)
        }
        UiInstance::Text {
            quads,
            clip,
            rotation,
            ..
        } => {
            for q in quads {
                q.pos = [
                    q.pos[0] + d[0],
                    q.pos[1] + d[1],
                    q.pos[2] + d[0],
                    q.pos[3] + d[1],
                ];
            }
            (clip, rotation)
        }
        UiInstance::Shape {
            origin_px,
            clip,
            rotation,
            ..
        }
        | UiInstance::Polygon {
            origin_px,
            clip,
            rotation,
            ..
        } => {
            *origin_px = [origin_px[0] + d[0], origin_px[1] + d[1]];
            (clip, rotation)
        }
    };
    if rotation.radians != 0.0 {
        rotation.pivot_px = [rotation.pivot_px[0] + d[0], rotation.pivot_px[1] + d[1]];
    }
    for b in clip.boxes.iter_mut().flatten() {
        b.rect_px[0] += d[0];
        b.rect_px[1] += d[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::{UiAnchorBlend, blend};
    use crate::scene::{UiClip, UiClipBox, UiRotation};
    use ludion_font::GlyphQuad;

    const BOX: [f32; 4] = [0.0, 0.0, 200.0, 100.0];

    fn camera_at(x: f32) -> (Mat4, Mat4) {
        (
            Mat4::look_at_rh(Vec3::new(x, 0.0, 5.0), Vec3::new(x, 0.0, 0.0), Vec3::Y),
            Mat4::perspective_rh(1.0, 2.0, 0.1, 100.0),
        )
    }

    fn scene(cam_x: f32, rows: Vec<UiInstance>) -> RenderScene {
        let mut s = RenderScene::empty((200, 100));
        let (v, p) = camera_at(cam_x);
        s.camera.view = v;
        s.camera.projection = p;
        s.ui = rows;
        s
    }

    fn rect_at(x: f32, y: f32, clip: UiClip, rotation: UiRotation) -> UiInstance {
        UiInstance::Rect {
            rect_px: [x, y, 10.0, 10.0],
            texture: None,
            color: [1.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            clip,
            view: None,
            rotation,
        }
    }

    fn plain(x: f32, y: f32) -> UiInstance {
        rect_at(x, y, UiClip::NONE, UiRotation::NONE)
    }

    fn rect_xy(s: &RenderScene, i: usize) -> [f32; 2] {
        match &s.ui[i] {
            UiInstance::Rect { rect_px, .. } => [rect_px[0], rect_px[1]],
            other => panic!("not a rect: {other:?}"),
        }
    }

    fn id(n: u32) -> EntityId {
        EntityId::new(n, 0)
    }

    fn anchor_at(a: u32, target: u32, point: [f32; 3]) -> UiAnchorBlend {
        UiAnchorBlend {
            anchor: id(a),
            target: id(target),
            point,
            view: None,
            box_px: BOX,
            inset_px: 4.0,
        }
    }

    fn data(ui: Vec<Option<EntityId>>, anchors: Vec<UiAnchorBlend>) -> BlendData {
        BlendData {
            ui,
            ui_anchors: anchors,
            ..BlendData::default()
        }
    }

    /// Where `point` lands from the camera at `cam_x`, with the rect sitting on that placement.
    fn placed(cam_x: f32, point: [f32; 3]) -> [f32; 2] {
        let (v, p) = camera_at(cam_x);
        place_world_anchor_in(v, p, point, BOX, 4.0).unwrap().pixel
    }

    fn near(a: [f32; 2], b: [f32; 2]) {
        assert!(
            (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3,
            "{a:?} != {b:?}"
        );
    }

    /// The previous and newer tick: a rect on `id(1)`'s anchor, the point moving x 0 -> 2.
    fn moving_point() -> (RenderScene, BlendData, RenderScene, BlendData) {
        let [x0, y0] = placed(0.0, [0.0; 3]);
        let [x1, y1] = placed(0.0, [2.0, 0.0, 0.0]);
        let prev_scene = scene(0.0, vec![plain(x0, y0)]);
        let next_scene = scene(0.0, vec![plain(x1, y1)]);
        let prev = data(vec![Some(id(1))], vec![anchor_at(1, 7, [0.0; 3])]);
        let next = data(vec![Some(id(1))], vec![anchor_at(1, 7, [2.0, 0.0, 0.0])]);
        (prev_scene, prev, next_scene, next)
    }

    #[test]
    fn a_moving_point_places_the_rect_at_the_lerped_point() {
        let (ps, p, ns, n) = moving_point();
        let out = blend(&ps, &p, &ns, &n, 0.5);
        near(rect_xy(&out, 0), placed(0.0, [1.0, 0.0, 0.0]));
        assert_eq!(blend(&ps, &p, &ns, &n, 1.0), ns);
    }

    #[test]
    fn text_quads_and_a_shape_origin_move_by_the_same_delta() {
        let (ps, p, mut ns, n) = moving_point();
        let [x1, y1] = rect_xy(&ns, 0);
        let quad = |l: f32| GlyphQuad {
            ch: 'a',
            pos: [l, y1, l + 8.0, y1 + 8.0],
            uv: [0.0; 4],
            face: 0,
        };
        ns.ui = vec![
            UiInstance::Text {
                color: [1.0; 4],
                text: "ab".into(),
                quads: vec![quad(x1), quad(x1 + 8.0)],
                font: None,
                clip: UiClip::NONE,
                outline: None,
                shadow: None,
                rotation: UiRotation::NONE,
            },
            UiInstance::Shape {
                origin_px: [x1, y1],
                geometry: crate::scene::UiShapeGeometry::Sector {
                    radius_outer: 5.0,
                    radius_inner: 0.0,
                    start: 0.0,
                    sweep: std::f32::consts::TAU,
                },
                fill: Some([1.0; 4]),
                stroke: None,
                clip: UiClip::NONE,
                rotation: UiRotation::NONE,
            },
        ];
        let mut n = n;
        n.ui = vec![Some(id(1)), Some(id(1))];
        let out = blend(&ps, &p, &ns, &n, 0.5);
        let want = placed(0.0, [1.0, 0.0, 0.0]);
        let dx = want[0] - x1;
        match (&out.ui[0], &out.ui[1]) {
            (UiInstance::Text { quads, .. }, UiInstance::Shape { origin_px, .. }) => {
                assert!((quads[0].pos[0] - (x1 + dx)).abs() < 1e-3);
                assert!((quads[1].pos[2] - (x1 + 16.0 + dx)).abs() < 1e-3);
                near(*origin_px, want);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_snapped_target_keeps_the_newer_point() {
        let (ps, p, ns, mut n) = moving_point();
        n.snaps = vec![id(7)];
        assert_eq!(blend(&ps, &p, &ns, &n, 0.5), ns);
    }

    #[test]
    fn a_target_swap_keeps_the_newer_point() {
        let (ps, mut p, ns, n) = moving_point();
        p.ui_anchors[0].target = id(8);
        assert_eq!(blend(&ps, &p, &ns, &n, 0.5), ns);
    }

    #[test]
    fn a_blended_camera_moves_the_row_with_a_fixed_point() {
        let point = [0.0; 3];
        let [x, y] = placed(1.0, point);
        let next_scene = scene(1.0, vec![plain(x, y)]);
        let a = anchor_at(1, 7, point);
        let prev = data(vec![Some(id(1))], vec![a]);
        let next = data(vec![Some(id(1))], vec![a]);
        let mut out = next_scene.clone();
        let (v, _) = camera_at(0.5);
        out.camera.view = v;
        blend_ui_anchors(&prev, &next_scene, &next, 0.5, &mut out);
        near(rect_xy(&out, 0), placed(0.5, point));
    }

    #[test]
    fn unkeyed_rows_and_a_mismatched_key_list_are_untouched() {
        let (ps, p, mut ns, mut n) = moving_point();
        ns.ui.push(plain(3.0, 4.0));
        n.ui = vec![Some(id(1)), None];
        let out = blend(&ps, &p, &ns, &n, 0.5);
        assert_eq!(rect_xy(&out, 1), [3.0, 4.0]);
        assert_ne!(rect_xy(&out, 0), rect_xy(&ns, 0));
        n.ui = vec![Some(id(1))];
        assert_eq!(blend(&ps, &p, &ns, &n, 0.5), ns);
    }

    #[test]
    fn a_rotated_pivot_and_a_clip_box_move_by_the_delta() {
        let (ps, p, mut ns, n) = moving_point();
        let [x1, y1] = rect_xy(&ns, 0);
        let clip = UiClip {
            boxes: [
                Some(UiClipBox {
                    rect_px: [x1, y1, 30.0, 30.0],
                    radius_px: 0.0,
                    radians: 0.0,
                }),
                None,
            ],
        };
        let turned = UiRotation {
            radians: 0.5,
            pivot_px: [x1 + 5.0, y1 + 5.0],
        };
        ns.ui = vec![
            rect_at(x1, y1, clip, turned),
            rect_at(
                x1,
                y1,
                UiClip::NONE,
                UiRotation {
                    radians: 0.0,
                    pivot_px: [9.0, 9.0],
                },
            ),
        ];
        let mut n = n;
        n.ui = vec![Some(id(1)), Some(id(1))];
        let out = blend(&ps, &p, &ns, &n, 0.5);
        let dx = placed(0.0, [1.0, 0.0, 0.0])[0] - x1;
        match (&out.ui[0], &out.ui[1]) {
            (
                UiInstance::Rect { clip, rotation, .. },
                UiInstance::Rect {
                    rotation: still, ..
                },
            ) => {
                assert!((rotation.pivot_px[0] - (x1 + 5.0 + dx)).abs() < 1e-3);
                assert!((clip.boxes[0].unwrap().rect_px[0] - (x1 + dx)).abs() < 1e-3);
                assert_eq!(still.pivot_px, [9.0, 9.0]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_view_anchor_follows_the_blended_view_camera() {
        let point = [0.0; 3];
        let mut next_scene = scene(0.0, vec![plain(0.0, 0.0)]);
        let mut view_camera = next_scene.camera;
        (view_camera.view, view_camera.projection) = camera_at(1.0);
        next_scene.views.push(crate::scene::RenderViewData {
            name: "v".into(),
            size: (64, 64),
            camera: view_camera,
            layers: u32::MAX,
            lights: u32::MAX,
            sun: true,
            background: None,
            unlit: false,
            fog: true,
            post: crate::scene::ViewPost::Scene,
            screen_effects: Vec::new(),
        });
        let [x, y] = placed(1.0, point);
        next_scene.ui = vec![plain(x, y)];
        let mut a = anchor_at(1, 7, point);
        a.view = Some(0);
        let next = data(vec![Some(id(1))], vec![a]);
        let mut out = next_scene.clone();
        out.views[0].camera.view = camera_at(0.5).0;
        blend_ui_anchors(&BlendData::default(), &next_scene, &next, 0.5, &mut out);
        near(rect_xy(&out, 0), placed(0.5, point));
    }

    #[test]
    fn an_edge_clamped_placement_stays_on_the_inset_edge() {
        let point = [50.0, 0.0, 0.0];
        let [x0, y0] = placed(0.0, point);
        let [x1, y1] = placed(0.0, [60.0, 0.0, 0.0]);
        assert_eq!([x0, y0], [x1, y1]);
        let prev_scene = scene(0.0, vec![plain(x0, y0)]);
        let next_scene = scene(0.0, vec![plain(x1, y1)]);
        let prev = data(vec![Some(id(1))], vec![anchor_at(1, 7, point)]);
        let next = data(vec![Some(id(1))], vec![anchor_at(1, 7, [60.0, 0.0, 0.0])]);
        let out = blend(&prev_scene, &prev, &next_scene, &next, 0.5);
        assert_eq!(out, next_scene);
        assert!((x1 - (BOX[2] - 4.0)).abs() < 1e-3);
    }
}
