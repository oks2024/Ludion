//! The screen-space UI polygon vertex stream: the packed vertex, its attribute table, and the
//! tessellation of a filled polygon with a one-pixel feathered fringe that `ui_polygon.wgsl` draws
//! (`Plan/ui-hud.md` decision 72).

use super::{ui_clip_raw, ui_cos_sin};
use crate::scene::{UiClip, UiPathJoin, UiPathStroke, UiRotation};
use bytemuck::{Pod, Zeroable};
use std::f32::consts::PI;

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

/// Maps a path-local point to screen pixels: translated by `origin_px`, then turned about the pivot.
fn placer(origin_px: [f32; 2], rotation: &UiRotation) -> impl Fn([f32; 2]) -> [f32; 2] {
    let turn = (rotation.radians != 0.0).then(|| ui_cos_sin(rotation.radians));
    let pivot = rotation.pivot_px;
    move |p| {
        let (x, y) = (origin_px[0] + p[0], origin_px[1] + p[1]);
        match turn {
            None => [x, y],
            Some((cos, sin)) => {
                let (dx, dy) = (x - pivot[0], y - pivot[1]);
                [
                    pivot[0] + cos * dx - sin * dy,
                    pivot[1] + sin * dx + cos * dy,
                ]
            }
        }
    }
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
    let place = placer(origin_px, rotation);
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

type Pt = [f32; 2];

/// A miter longer than this many half-widths from the vertex falls back to a bevel (SVG's default 4).
const MITER_LIMIT: f32 = 4.0;
/// Coverage of a stroke row's four rails, outermost first: fringe, core, core, fringe.
const ROW_COVERAGE: [f32; 4] = [0.0, 1.0, 1.0, 0.0];

fn dot2(a: Pt, b: Pt) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross2(a: Pt, b: Pt) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn mad(p: Pt, v: Pt, k: f32) -> Pt {
    [p[0] + v[0] * k, p[1] + v[1] * k]
}

fn rotate(v: Pt, radians: f32) -> Pt {
    let (sin, cos) = ludion_core::math::sin_cos(radians);
    [cos * v[0] - sin * v[1], sin * v[0] + cos * v[1]]
}

/// The arc segment count of a round join or cap sweeping `theta` radians at outer radius `outer`: the
/// chord's sagitta stays under a quarter pixel.
fn arc_segments(theta: f32, outer: f32) -> usize {
    let step = 2.0 * ludion_core::math::acos((1.0 - 0.25 / outer).clamp(-1.0, 1.0));
    ((theta.abs() / step.max(1e-3)).ceil() as usize).clamp(1, 32)
}

/// Drops a vertex a dash run keeps within `reach` of its start or end when the path barely turns there
/// (a flattened curve's facet): a join next to a segment that short clamps its inner edge into a notch.
/// A real corner stays.
fn trim_short_ends(mut run: Vec<Pt>, reach: f32) -> Vec<Pt> {
    let gentle = |a: Pt, b: Pt, c: Pt| {
        let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
        let (lu, lv) = (dot2(u, u).sqrt(), dot2(v, v).sqrt());
        lu > 0.0 && lv > 0.0 && dot2(u, v) > 0.9 * lu * lv
    };
    let len = |a: Pt, b: Pt| dot2([b[0] - a[0], b[1] - a[1]], [b[0] - a[0], b[1] - a[1]]).sqrt();
    while run.len() >= 3 && len(run[0], run[1]) < reach && gentle(run[0], run[1], run[2]) {
        run.remove(1);
    }
    while run.len() >= 3 {
        let n = run.len();
        if len(run[n - 2], run[n - 1]) < reach && gentle(run[n - 3], run[n - 2], run[n - 1]) {
            run.remove(n - 2);
        } else {
            break;
        }
    }
    run
}

/// Appends one path's stroke to `verts`: per segment a strip of four rails (a 1 px fringe on each side
/// of the core, so the 0.5-coverage contour is the stroke edge), with joins and caps between them.
/// Miter joins keep the shared point up to a miter limit of 4 half-widths, then bevel; round joins and
/// caps fan an arc. A stroke under 1 px draws at 1 px with its alpha scaled by the width. `points` are
/// pixels relative to `origin_px`; `rotation` turns every vertex here, as the fill does.
pub(super) fn push_ui_stroke_verts(
    verts: &mut Vec<UiPolygonVertexRaw>,
    origin_px: [f32; 2],
    points: &[[f32; 2]],
    closed: bool,
    stroke: &UiPathStroke,
    clip: &UiClip,
    rotation: &UiRotation,
) {
    if let Some([on, off]) = stroke.dash {
        let solid = UiPathStroke {
            dash: None,
            ..*stroke
        };
        let reach = stroke.width_px.max(1.0) * 0.5 + 1.0;
        for run in crate::ui_dash::ui_dash_runs(points, closed, on, off) {
            let run = trim_short_ends(run, reach);
            push_ui_stroke_verts(verts, origin_px, &run, false, &solid, clip, rotation);
        }
        return;
    }
    let mut pts: Vec<Pt> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    if closed && pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    if n < 2 || stroke.width_px.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return;
    }
    let closed = closed && n >= 3;
    let segments = if closed { n } else { n - 1 };
    let half = stroke.width_px.max(1.0) * 0.5;
    let dists = [half - 0.5, half + 0.5];
    let alpha = stroke.color[3] * stroke.width_px.min(1.0);

    let [clip_a, clip_b, clip_r, clip_t] = ui_clip_raw(clip);
    let place = placer(origin_px, rotation);
    let vert = |p: Pt, coverage: f32| {
        let c = stroke.color;
        UiPolygonVertexRaw {
            position: place(p),
            color: [
                c[0] * alpha * coverage,
                c[1] * alpha * coverage,
                c[2] * alpha * coverage,
                alpha * coverage,
            ],
            clip_a,
            clip_b,
            clip_r,
            clip_t,
        }
    };
    let tri = |verts: &mut Vec<UiPolygonVertexRaw>, t: [(Pt, f32); 3]| {
        verts.extend(t.iter().map(|&(p, c)| vert(p, c)));
    };
    let band = |verts: &mut Vec<UiPolygonVertexRaw>, a: [(Pt, f32); 2], b: [(Pt, f32); 2]| {
        tri(verts, [a[0], a[1], b[1]]);
        tri(verts, [a[0], b[1], b[0]]);
    };
    // The fan from `u0` to `u1` about `p` sweeping `theta` in `nseg` steps: a core triangle each and a
    // fringe quad on the outside.
    let fan =
        |verts: &mut Vec<UiPolygonVertexRaw>, p: Pt, u0: Pt, u1: Pt, theta: f32, nseg: usize| {
            let dir = |k: usize| match k {
                0 => u0,
                k if k == nseg => u1,
                k => rotate(u0, theta * k as f32 / nseg as f32),
            };
            for k in 0..nseg {
                let (d0, d1) = (dir(k), dir(k + 1));
                let (a0, a1) = (mad(p, d0, dists[0]), mad(p, d1, dists[0]));
                let (b0, b1) = (mad(p, d0, dists[1]), mad(p, d1, dists[1]));
                tri(verts, [(p, 1.0), (a0, 1.0), (a1, 1.0)]);
                band(verts, [(a0, 1.0), (b0, 0.0)], [(a1, 1.0), (b1, 0.0)]);
            }
        };

    let dirs: Vec<Pt> = (0..segments)
        .map(|k| {
            let (from, to) = (pts[k], pts[(k + 1) % n]);
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1e-12);
            [dx / len, dy / len]
        })
        .collect();
    let lens: Vec<f32> = (0..segments)
        .map(|k| {
            let (from, to) = (pts[k], pts[(k + 1) % n]);
            ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt()
        })
        .collect();
    let normals: Vec<Pt> = dirs.iter().map(|d| [d[1], -d[0]]).collect();
    let sides = [1.0f32, -1.0];

    // Per vertex, per side, per distance: where the incoming and the outgoing segment's rail ends.
    let mut incoming = vec![[[[0.0f32; 2]; 2]; 2]; n];
    let mut outgoing = incoming.clone();
    for i in 0..n {
        let p = pts[i];
        let prev = if i > 0 {
            Some(i - 1)
        } else if closed {
            Some(segments - 1)
        } else {
            None
        };
        let next = (i < segments).then_some(i);
        match (prev, next) {
            (Some(pv), Some(nx)) => {
                let (n0, n1, d0, d1) = (normals[pv], normals[nx], dirs[pv], dirs[nx]);
                let dot = dot2(n0, n1);
                let reversal = 1.0 + dot < 1e-4;
                let miter = if reversal {
                    [0.0, 0.0]
                } else {
                    let k = 1.0 / (1.0 + dot);
                    [(n0[0] + n1[0]) * k, (n0[1] + n1[1]) * k]
                };
                let miter_len = if reversal {
                    f32::INFINITY
                } else {
                    dot2(miter, miter).sqrt()
                };
                let limit = lens[pv].min(lens[nx]);
                let mut sharp_sides = [false; 2];
                for (si, &s) in sides.iter().enumerate() {
                    let outer = if reversal {
                        si == 0
                    } else {
                        s * dot2(d1, n0) < -1e-6
                    };
                    let sharp = outer
                        && (reversal
                            || stroke.join != UiPathJoin::Miter
                            || miter_len > MITER_LIMIT);
                    for (k, &dist) in dists.iter().enumerate() {
                        let (a, b) = if sharp || (reversal && !outer) {
                            (mad(p, n0, s * dist), mad(p, n1, s * dist))
                        } else {
                            let mut v = [miter[0] * dist, miter[1] * dist];
                            let len = dot2(v, v).sqrt();
                            if !outer && len > limit {
                                v = [v[0] * limit / len, v[1] * limit / len];
                            }
                            let q = mad(p, v, s);
                            (q, q)
                        };
                        incoming[i][si][k] = a;
                        outgoing[i][si][k] = b;
                    }
                    if sharp {
                        sharp_sides[si] = true;
                        let (u0, u1) = ([n0[0] * s, n0[1] * s], [n1[0] * s, n1[1] * s]);
                        let theta = if reversal {
                            if cross2(u0, d0) >= 0.0 { PI } else { -PI }
                        } else {
                            ludion_core::math::atan2(cross2(u0, u1), dot2(u0, u1))
                        };
                        let nseg = if stroke.join == UiPathJoin::Round {
                            arc_segments(theta, dists[1])
                        } else {
                            1
                        };
                        fan(verts, p, u0, u1, theta, nseg);
                    }
                }
                // A sharp outer side leaves the strips' end edges, which run from its rail to the inner
                // miter point, missing `p`: the two wedges either side of `p` close the gap.
                for si in (0..2).filter(|&si| sharp_sides[si] && !reversal) {
                    let inner = outgoing[i][1 - si][0];
                    for outer in [incoming[i][si][0], outgoing[i][si][0]] {
                        tri(verts, [(p, 1.0), (outer, 1.0), (inner, 1.0)]);
                    }
                }
            }
            (None, Some(nx)) | (Some(nx), None) => {
                let at_start = prev.is_none();
                let (nm, d) = (normals[nx], dirs[nx]);
                // A butt end is flush: its 1 px fringe straddles the end point, the core starting half
                // a pixel inside it. A round cap is a half-disc about the end point.
                let shift = if stroke.round_cap {
                    0.0
                } else {
                    0.5f32.min(lens[nx] * 0.5)
                };
                let toward = if at_start { shift } else { -shift };
                let row = |offset: f32| {
                    let base = mad(p, d, offset);
                    let mut r = [[[0.0f32; 2]; 2]; 2];
                    for (si, &s) in sides.iter().enumerate() {
                        for (k, &dist) in dists.iter().enumerate() {
                            r[si][k] = mad(base, nm, s * dist);
                        }
                    }
                    r
                };
                let rails = row(toward);
                incoming[i] = rails;
                outgoing[i] = rails;
                if stroke.round_cap {
                    let u0 = nm;
                    let u1 = [-nm[0], -nm[1]];
                    let theta = if at_start { -PI } else { PI };
                    fan(verts, p, u0, u1, theta, arc_segments(PI, dists[1]));
                } else {
                    let flat = row(-toward);
                    let pair = |r: &[[[f32; 2]; 2]; 2], c: [f32; 4]| {
                        [
                            (r[0][1], c[0]),
                            (r[0][0], c[1]),
                            (r[1][0], c[2]),
                            (r[1][1], c[3]),
                        ]
                    };
                    let (hi, lo) = (pair(&rails, ROW_COVERAGE), pair(&flat, [0.0; 4]));
                    for j in 0..3 {
                        band(verts, [hi[j], hi[j + 1]], [lo[j], lo[j + 1]]);
                    }
                }
            }
            (None, None) => {}
        }
    }

    #[allow(clippy::needless_range_loop)]
    for k in 0..segments {
        let j = (k + 1) % n;
        let row = |r: &[[Pt; 2]; 2]| {
            [
                (r[0][1], ROW_COVERAGE[0]),
                (r[0][0], ROW_COVERAGE[1]),
                (r[1][0], ROW_COVERAGE[2]),
                (r[1][1], ROW_COVERAGE[3]),
            ]
        };
        let (from, to) = (row(&outgoing[k]), row(&incoming[j]));
        for q in 0..3 {
            band(verts, [from[q], from[q + 1]], [to[q], to[q + 1]]);
        }
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

    fn stroke_verts(
        points: &[[f32; 2]],
        closed: bool,
        width_px: f32,
        join: UiPathJoin,
        round_cap: bool,
    ) -> Vec<UiPolygonVertexRaw> {
        let stroke = UiPathStroke {
            width_px,
            color: [1.0; 4],
            join,
            round_cap,
            dash: None,
        };
        verts_of(points, closed, &stroke)
    }

    fn verts_of(
        points: &[[f32; 2]],
        closed: bool,
        stroke: &UiPathStroke,
    ) -> Vec<UiPolygonVertexRaw> {
        let mut verts = Vec::new();
        push_ui_stroke_verts(
            &mut verts,
            [0.0, 0.0],
            points,
            closed,
            stroke,
            &UiClip::NONE,
            &UiRotation::NONE,
        );
        verts
    }

    #[test]
    fn a_dashed_stroke_packs_the_verts_of_its_runs_stroked_open() {
        let points = [[0.0, 0.0], [20.0, 0.0], [20.0, 20.0]];
        let dashed = UiPathStroke {
            width_px: 4.0,
            color: [1.0; 4],
            join: UiPathJoin::Round,
            round_cap: true,
            dash: Some([12.0, 6.0]),
        };
        let solid = UiPathStroke {
            dash: None,
            ..dashed
        };
        let mut expected = Vec::new();
        for run in crate::ui_dash::ui_dash_runs(&points, false, 12.0, 6.0) {
            expected.extend(verts_of(&run, false, &solid));
        }
        let got = verts_of(&points, false, &dashed);
        assert!(!got.is_empty());
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&got),
            bytemuck::cast_slice::<_, u8>(&expected)
        );
    }

    #[test]
    fn a_dash_run_drops_a_gentle_vertex_near_its_end_and_keeps_a_corner() {
        let facet = trim_short_ends(vec![[0.0, 0.0], [10.0, 0.0], [11.0, 0.1]], 3.0);
        assert_eq!(facet, vec![[0.0, 0.0], [11.0, 0.1]]);
        let corner = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 1.0]];
        assert_eq!(trim_short_ends(corner.clone(), 3.0), corner);
        let far = vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.1]];
        assert_eq!(trim_short_ends(far.clone(), 3.0), far);
    }

    #[test]
    fn an_undashed_stroke_packs_exactly_the_solid_path() {
        // The vertex count of one butt segment, pinned so a dash-free path can't drift.
        let verts = stroke_verts(
            &[[0.0, 0.0], [20.0, 0.0]],
            false,
            4.0,
            UiPathJoin::Miter,
            false,
        );
        assert_eq!(verts.len(), 54);
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4
    }

    #[test]
    fn a_butt_stroke_has_its_half_coverage_edge_on_the_path_and_its_ends() {
        let verts = stroke_verts(
            &[[0.0, 0.0], [20.0, 0.0]],
            false,
            4.0,
            UiPathJoin::Miter,
            false,
        );
        // Core rails at 1.5 (coverage 1), fringe rails at 2.5 (coverage 0): 0.5 at 2 = half the width.
        for v in &verts {
            let y = v.position[1].abs();
            assert!(
                (v.color[3] == 1.0 && (y - 1.5).abs() < 1e-5)
                    || (v.color[3] == 0.0 && ((y - 2.5).abs() < 1e-5 || (y - 1.5).abs() < 1e-5)),
                "{:?} {}",
                v.position,
                v.color[3]
            );
        }
        // The end fringe straddles the end points: coverage 1 half a pixel inside, 0 half outside.
        let min_x = |covered: bool| {
            verts
                .iter()
                .filter(|v| (v.color[3] == 1.0) == covered)
                .map(|v| v.position[0])
                .fold(f32::INFINITY, f32::min)
        };
        let max_x = |covered: bool| {
            verts
                .iter()
                .filter(|v| (v.color[3] == 1.0) == covered)
                .map(|v| v.position[0])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        assert!((min_x(true) - 0.5).abs() < 1e-5 && (min_x(false) + 0.5).abs() < 1e-5);
        assert!((max_x(true) - 19.5).abs() < 1e-5 && (max_x(false) - 20.5).abs() < 1e-5);
    }

    #[test]
    fn a_right_angle_miter_reaches_the_corner_of_the_offset_box() {
        let verts = stroke_verts(
            &[[-20.0, 0.0], [0.0, 0.0], [0.0, 20.0]],
            false,
            4.0,
            UiPathJoin::Miter,
            false,
        );
        let has = |p: [f32; 2], coverage: f32| {
            verts
                .iter()
                .any(|v| near(v.position, p) && v.color[3] == coverage)
        };
        // h = 2: the 0.5 contour's corner is (h, h) from the vertex, between the rails at 1.5 and 2.5.
        assert!(
            has([1.5, -1.5], 1.0),
            "{:?}",
            verts.iter().map(|v| v.position).collect::<Vec<_>>()
        );
        assert!(has([2.5, -2.5], 0.0));
    }

    #[test]
    fn a_sharp_miter_falls_back_to_the_bevel() {
        // An interior angle of 20 degrees: the miter is 5.8 half-widths long, past the limit of 4.
        let (sin, cos) = ludion_core::math::sin_cos(160f32.to_radians());
        let path = [[0.0, 0.0], [40.0, 0.0], [40.0 + 40.0 * cos, 40.0 * sin]];
        let miter = stroke_verts(&path, false, 4.0, UiPathJoin::Miter, false);
        let bevel = stroke_verts(&path, false, 4.0, UiPathJoin::Bevel, false);
        assert_eq!(miter.len(), bevel.len());
        assert!(
            miter
                .iter()
                .zip(&bevel)
                .all(|(a, b)| near(a.position, b.position))
        );
        // A 90 degree corner stays mitred: no fan triangles beyond the strips and end fringes.
        let right = [[0.0, 0.0], [40.0, 0.0], [40.0, 40.0]];
        assert!(
            stroke_verts(&right, false, 4.0, UiPathJoin::Miter, false).len()
                < stroke_verts(&right, false, 4.0, UiPathJoin::Bevel, false).len()
        );
    }

    #[test]
    fn round_joins_and_caps_follow_the_segment_formula() {
        assert_eq!(arc_segments(0.001, 1.0), 1);
        assert!(arc_segments(PI, 0.6) >= 1);
        assert!(arc_segments(100.0, 1000.0) <= 32);
        let right = [[0.0, 0.0], [60.0, 0.0], [60.0, 60.0]];
        let (round, bevel) = (
            stroke_verts(&right, false, 40.0, UiPathJoin::Round, false),
            stroke_verts(&right, false, 40.0, UiPathJoin::Bevel, false),
        );
        // Nine vertices per fan step (a core triangle and a fringe quad).
        let steps = arc_segments(std::f32::consts::FRAC_PI_2, 20.5);
        assert!(steps > 1);
        assert_eq!(round.len() - bevel.len(), 9 * (steps - 1));
        let capped = stroke_verts(&right[..2], false, 40.0, UiPathJoin::Miter, true);
        let flush = stroke_verts(&right[..2], false, 40.0, UiPathJoin::Miter, false);
        let cap = arc_segments(PI, 20.5);
        // Two caps of nine vertices a step, instead of the two butt-end fringes (18 vertices each).
        assert_eq!(capped.len() + 2 * 18, flush.len() + 2 * 9 * cap);
    }

    #[test]
    fn a_thin_stroke_draws_at_one_pixel_with_scaled_alpha() {
        let path = [[0.0, 0.0], [20.0, 0.0]];
        let thin = stroke_verts(&path, false, 0.5, UiPathJoin::Miter, false);
        let one = stroke_verts(&path, false, 1.0, UiPathJoin::Miter, false);
        assert_eq!(thin.len(), one.len());
        assert!(
            thin.iter()
                .zip(&one)
                .all(|(t, o)| near(t.position, o.position))
        );
        assert!(thin.iter().all(|v| v.color[3] <= 0.5 + 1e-6));
        assert!(thin.iter().any(|v| (v.color[3] - 0.5).abs() < 1e-6));
    }

    #[test]
    fn a_closed_path_has_no_caps_and_a_degenerate_one_draws_nothing() {
        let square = [[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]];
        let closed = stroke_verts(&square, true, 4.0, UiPathJoin::Miter, true);
        // Four segments of three bands each (18 vertices), nothing else with mitred corners.
        assert_eq!(closed.len(), 4 * 18);
        assert!(
            stroke_verts(
                &[[1.0, 1.0], [1.0, 1.0]],
                false,
                4.0,
                UiPathJoin::Miter,
                false
            )
            .is_empty()
        );
        assert!(stroke_verts(&square, false, 0.0, UiPathJoin::Miter, false).is_empty());
    }

    #[test]
    fn an_unclipped_polygon_packs_the_identity_clip() {
        let verts = square(&UiClip::NONE, &UiRotation::NONE);
        assert!(verts.iter().all(|v| v.clip_t == [1.0, 0.0, 1.0, 0.0]));
    }
}
