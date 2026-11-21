//! The screen-space UI polygon vertex stream: the packed vertex, its attribute table, and the
//! tessellation of a filled polygon with a one-pixel feathered fringe that `ui_polygon.wgsl` draws
//! (`Plan/ui-hud.md` decision 72).

use super::{ui_clip_raw, ui_cos_sin};
use crate::scene::{UiClip, UiRotation};
use bytemuck::{Pod, Zeroable};

/// One screen-space UI polygon vertex: a pixel-space position, a premultiplied linear RGBA colour with
/// the edge coverage folded in, and the packed clip. Matches `ui_polygon.wgsl`'s `VsIn`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct UiPolygonVertexRaw {
    pub(super) position: [f32; 2],
    pub(super) color: [f32; 4],
    clip_a: [f32; 4],
    clip_b: [f32; 4],
    clip_r: [f32; 4],
    clip_t: [f32; 4],
}

/// UI-polygon per-vertex attributes (`UiPolygonVertexRaw`: position, colour, clip a, b, r, t), locations 0-5.
pub(super) const UI_POLYGON_VERTEX_ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4];

fn unit_normal(from: [f32; 2], to: [f32; 2], sign: f32) -> [f32; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return [0.0, 0.0];
    }
    [sign * dy / len, -sign * dx / len]
}

/// Appends one polygon's vertices to `verts`: the interior `triangles` over the outline inset by half a
/// pixel, and a quad per edge between that inset outline (coverage 1) and the outline pushed out by half
/// a pixel (coverage 0), so the 0.5-coverage contour is the authored outline. `points` are pixels
/// relative to `origin_px`; the miter at each vertex is capped at 2x. A `rotation` with `radians != 0`
/// turns every vertex about its pivot here, on the CPU.
pub(super) fn push_ui_polygon_verts(
    verts: &mut Vec<UiPolygonVertexRaw>,
    origin_px: [f32; 2],
    points: &[[f32; 2]],
    triangles: &[[u16; 3]],
    fill: [f32; 4],
    clip: &UiClip,
    rotation: &UiRotation,
) {
    let n = points.len();
    if n < 3 || triangles.is_empty() {
        return;
    }
    let [clip_a, clip_b, clip_r, clip_t] = ui_clip_raw(clip);
    let turn = (rotation.radians != 0.0).then(|| ui_cos_sin(rotation.radians));
    let place = |p: [f32; 2]| {
        let (x, y) = (origin_px[0] + p[0], origin_px[1] + p[1]);
        match turn {
            None => [x, y],
            Some((cos, sin)) => {
                let [px, py] = rotation.pivot_px;
                let (dx, dy) = (x - px, y - py);
                [px + cos * dx - sin * dy, py + sin * dx + cos * dy]
            }
        }
    };
    let vert = |p: [f32; 2], coverage: f32| UiPolygonVertexRaw {
        position: place(p),
        color: [
            fill[0] * fill[3] * coverage,
            fill[1] * fill[3] * coverage,
            fill[2] * fill[3] * coverage,
            fill[3] * coverage,
        ],
        clip_a,
        clip_b,
        clip_r,
        clip_t,
    };

    let area2: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let sign = if area2 >= 0.0 { 1.0 } else { -1.0 };
    let normals: Vec<[f32; 2]> = (0..n)
        .map(|i| unit_normal(points[i], points[(i + 1) % n], sign))
        .collect();
    let (mut inner, mut outer) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for (i, p) in points.iter().enumerate() {
        let (prev, next) = (normals[(i + n - 1) % n], normals[i]);
        let half = [(prev[0] + next[0]) * 0.5, (prev[1] + next[1]) * 0.5];
        let k = 1.0 / (half[0] * half[0] + half[1] * half[1]).max(0.25);
        let m = [half[0] * k, half[1] * k];
        inner.push(vert([p[0] - 0.5 * m[0], p[1] - 0.5 * m[1]], 1.0));
        outer.push(vert([p[0] + 0.5 * m[0], p[1] + 0.5 * m[1]], 0.0));
    }

    for t in triangles {
        verts.extend(t.iter().map(|&i| inner[usize::from(i)]));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        verts.extend_from_slice(&[inner[i], outer[i], outer[j], inner[i], outer[j], inner[j]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQUARE: [[f32; 2]; 4] = [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
    const TRIANGLES: [[u16; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

    fn square(clip: &UiClip, rotation: &UiRotation) -> Vec<UiPolygonVertexRaw> {
        let mut verts = Vec::new();
        push_ui_polygon_verts(
            &mut verts,
            [0.0, 0.0],
            &SQUARE,
            &TRIANGLES,
            [1.0, 0.5, 0.25, 1.0],
            clip,
            rotation,
        );
        verts
    }

    #[test]
    fn a_square_has_interior_and_fringe_vertices() {
        let verts = square(&UiClip::NONE, &UiRotation::NONE);
        assert_eq!(verts.len(), 2 * 3 + 4 * 6);
    }

    #[test]
    fn inner_corners_are_inset_and_outer_corners_pushed_out() {
        let verts = square(&UiClip::NONE, &UiRotation::NONE);
        for v in &verts[..6] {
            assert!(
                v.position.iter().all(|c| (c.abs() - 4.5).abs() < 1e-5),
                "{:?}",
                v.position
            );
            assert_eq!(v.color[3], 1.0);
        }
        let outer: Vec<_> = verts[6..].iter().filter(|v| v.color[3] == 0.0).collect();
        assert!(!outer.is_empty());
        for v in outer {
            assert!(
                v.position.iter().all(|c| (c.abs() - 5.5).abs() < 1e-5),
                "{:?}",
                v.position
            );
            assert_eq!(v.color, [0.0; 4]);
        }
    }

    #[test]
    fn the_colour_is_premultiplied() {
        let mut verts = Vec::new();
        push_ui_polygon_verts(
            &mut verts,
            [0.0, 0.0],
            &SQUARE,
            &TRIANGLES,
            [1.0, 0.5, 0.25, 0.5],
            &UiClip::NONE,
            &UiRotation::NONE,
        );
        assert_eq!(verts[0].color, [0.5, 0.25, 0.125, 0.5]);
    }

    #[test]
    fn the_winding_does_not_change_the_fringe_side() {
        let mut flipped = SQUARE;
        flipped.reverse();
        let mut verts = Vec::new();
        push_ui_polygon_verts(
            &mut verts,
            [0.0, 0.0],
            &flipped,
            &[[0, 1, 2], [0, 2, 3]],
            [1.0; 4],
            &UiClip::NONE,
            &UiRotation::NONE,
        );
        assert!(
            verts[..6]
                .iter()
                .all(|v| v.position.iter().all(|c| c.abs() < 4.6))
        );
    }

    #[test]
    fn a_quarter_turn_moves_a_vertex_about_the_pivot() {
        let rotation = UiRotation {
            radians: std::f32::consts::FRAC_PI_2,
            pivot_px: [0.0, 0.0],
        };
        let plain = square(&UiClip::NONE, &UiRotation::NONE);
        let turned = square(&UiClip::NONE, &rotation);
        let [x, y] = plain[0].position;
        let [tx, ty] = turned[0].position;
        assert!((tx + y).abs() < 1e-4 && (ty - x).abs() < 1e-4, "{tx} {ty}");
    }

    #[test]
    fn an_unclipped_polygon_packs_the_identity_clip() {
        let verts = square(&UiClip::NONE, &UiRotation::NONE);
        assert!(verts.iter().all(|v| v.clip_t == [1.0, 0.0, 1.0, 0.0]));
    }
}
