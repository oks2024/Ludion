//! The centre-line outlines a dashed analytic [`UiShape`](crate::components::UiShape) stroke is cut
//! from (`Plan/ui-hud.md` decisions 83–84). Pure: pixels about the shape's origin, before tweens and
//! groups act.

use ludion_core::math::{acos, sin_cos};

use crate::components::UiShapeType;

const TAU: f32 = std::f32::consts::TAU;

/// One flattened outline: its points and whether it closes back on its first.
pub(crate) type Outline = (Vec<[f32; 2]>, bool);

/// A point at clock angle `theta` (radians clockwise from 12 o'clock) and radius `r`, +Y down.
fn clock(centre: [f32; 2], r: f32, theta: f32) -> [f32; 2] {
    let (s, c) = sin_cos(theta);
    [centre[0] + r * s, centre[1] - r * c]
}

/// Segments for an arc of radius `r` px turning `theta` radians: the chord's sagitta stays under 0.1 px.
fn segments(r: f32, theta: f32) -> usize {
    if r <= 0.1 {
        return 4;
    }
    let step = 2.0 * acos((1.0 - 0.1 / r).clamp(-1.0, 1.0));
    ((theta / step.max(1e-6)).ceil() as usize).clamp(4, 512)
}

/// The arc from `start` turning `sweep` radians clockwise, both ends included.
fn arc_points(centre: [f32; 2], r: f32, start: f32, sweep: f32) -> Vec<[f32; 2]> {
    let n = segments(r, sweep.abs());
    (0..=n)
        .map(|i| clock(centre, r, start + sweep * i as f32 / n as f32))
        .collect()
}

/// A full circle starting at `start`, the first point not repeated at the end.
fn circle_points(r: f32, start: f32) -> Vec<[f32; 2]> {
    let n = segments(r, TAU);
    (0..n)
        .map(|i| clock([0.0, 0.0], r, start + TAU * i as f32 / n as f32))
        .collect()
}

/// The ring or wedge outline: a sector between `inner` and `outer` radii.
fn sector(outer: f32, inner: f32, start_deg: f32, sweep_deg: f32) -> Vec<Outline> {
    if sweep_deg <= 0.0 {
        return Vec::new();
    }
    let start = start_deg.to_radians();
    if sweep_deg >= 360.0 {
        let mut loops = vec![(circle_points(outer, start), true)];
        if inner > 0.0 {
            loops.push((circle_points(inner, start), true));
        }
        return loops;
    }
    let sweep = sweep_deg.to_radians();
    let mut points = arc_points([0.0, 0.0], outer, start, sweep);
    if inner > 0.0 {
        let mut back = arc_points([0.0, 0.0], inner, start, sweep);
        back.reverse();
        points.extend(back);
    } else {
        points.push([0.0, 0.0]);
    }
    vec![(points, true)]
}

/// The outlines of an analytic `shape` at pixel scale `ps`, in drawing order, for the kinds that take a
/// dash. Empty for a `polygon`/`polyline` (they dash their own points) or when nothing would draw.
pub(crate) fn outlines(shape: &UiShapeType, ps: f32) -> Vec<Outline> {
    match *shape {
        UiShapeType::Line { from, to, .. } => {
            let (a, b) = ([from[0] * ps, from[1] * ps], [to[0] * ps, to[1] * ps]);
            if a == b {
                return Vec::new();
            }
            vec![(vec![a, b], false)]
        }
        UiShapeType::Circle { radius } => vec![(circle_points(radius * ps, 0.0), true)],
        UiShapeType::Ring {
            radius,
            thickness,
            start_deg,
            sweep_deg,
        } => sector(radius * ps, (radius - thickness) * ps, start_deg, sweep_deg),
        UiShapeType::Wedge {
            radius,
            inner_radius,
            start_deg,
            sweep_deg,
        } => sector(radius * ps, inner_radius * ps, start_deg, sweep_deg),
        UiShapeType::Arc {
            radius,
            start_deg,
            sweep_deg,
        } => {
            if sweep_deg <= 0.0 {
                return Vec::new();
            }
            let start = start_deg.to_radians();
            if sweep_deg >= 360.0 {
                vec![(circle_points(radius * ps, start), true)]
            } else {
                let sweep = sweep_deg.to_radians();
                vec![(arc_points([0.0, 0.0], radius * ps, start, sweep), false)]
            }
        }
        UiShapeType::RoundedRect {
            size,
            corner_radius,
        } => {
            let (hw, hh) = (size[0] * ps * 0.5, size[1] * ps * 0.5);
            let r = (corner_radius * ps).min(hw).min(hh);
            let quarter = TAU * 0.25;
            let mut points: Vec<[f32; 2]> = Vec::new();
            // Clockwise from the top edge's left end: each corner arc, then the edge to the next one.
            for (centre, from) in [
                ([hw - r, -hh + r], 0.0),
                ([hw - r, hh - r], quarter),
                ([-hw + r, hh - r], 2.0 * quarter),
                ([-hw + r, -hh + r], 3.0 * quarter),
            ] {
                points.extend(arc_points(centre, r, from, quarter));
            }
            // The last point of the top-left arc is the start `(−hw + r, −hh)`: rotate it to the front.
            points.rotate_right(1);
            points.dedup();
            if points.first() == points.last() && points.len() > 1 {
                points.pop();
            }
            vec![(points, true)]
        }
        UiShapeType::Polygon { .. } | UiShapeType::Polyline { .. } => Vec::new(),
    }
}

/// The length of a path in the units of its points, the closing segment included when `closed`.
pub(crate) fn path_length(points: &[[f32; 2]], closed: bool) -> f32 {
    let mut total: f32 = points
        .windows(2)
        .map(|w| ludion_core::math::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1]))
        .sum();
    if closed && points.len() > 1 {
        let (a, b) = (points[points.len() - 1], points[0]);
        total += ludion_core::math::hypot(b[0] - a[0], b[1] - a[1]);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::UiShapeType;

    fn radius_of(p: [f32; 2]) -> f32 {
        ludion_core::math::hypot(p[0], p[1])
    }

    #[test]
    fn a_circle_outline_sits_on_its_radius_and_starts_at_twelve() {
        let r = 40.0;
        let loops = outlines(&UiShapeType::Circle { radius: 40.0 }, 1.0);
        assert_eq!(loops.len(), 1);
        let (points, closed) = &loops[0];
        assert!(closed);
        assert!(points.iter().all(|&p| (radius_of(p) - r).abs() < 1e-4));
        assert!(points[0][0].abs() < 1e-4 && (points[0][1] + r).abs() < 1e-4);
        let step = 2.0 * acos(1.0 - 0.1 / r);
        let n = ((TAU / step).ceil() as usize).clamp(4, 512);
        assert_eq!(points.len(), n);
    }

    #[test]
    fn a_tiny_radius_takes_four_segments() {
        assert_eq!(segments(0.05, TAU), 4);
        assert_eq!(segments(0.1, TAU), 4);
    }

    #[test]
    fn a_partial_wedge_passes_through_the_origin() {
        let wedge = UiShapeType::Wedge {
            radius: 30.0,
            inner_radius: 0.0,
            start_deg: 0.0,
            sweep_deg: 90.0,
        };
        let loops = outlines(&wedge, 1.0);
        assert_eq!(loops.len(), 1);
        assert!(loops[0].1);
        assert!(loops[0].0.contains(&[0.0, 0.0]));
        // Outer arc first: it starts at 12 o'clock.
        assert!((loops[0].0[0][1] + 30.0).abs() < 1e-4);
    }

    #[test]
    fn a_partial_ring_runs_out_then_back_along_the_inner_arc() {
        let ring = UiShapeType::Ring {
            radius: 30.0,
            thickness: 10.0,
            start_deg: 0.0,
            sweep_deg: 90.0,
        };
        let (points, _) = &outlines(&ring, 1.0)[0];
        let n = points
            .iter()
            .position(|&p| (radius_of(p) - 20.0).abs() < 1e-3)
            .unwrap();
        assert!((radius_of(points[0]) - 30.0).abs() < 1e-3);
        assert!((radius_of(points[n - 1]) - 30.0).abs() < 1e-3);
        assert!((radius_of(points[points.len() - 1]) - 20.0).abs() < 1e-3);
        // The inner arc comes back to the start angle: its last point is at 12 o'clock.
        assert!(points[points.len() - 1][0].abs() < 1e-3);
    }

    #[test]
    fn a_full_ring_is_two_loops_and_a_full_disc_wedge_one() {
        let ring = UiShapeType::Ring {
            radius: 30.0,
            thickness: 10.0,
            start_deg: 0.0,
            sweep_deg: 360.0,
        };
        let loops = outlines(&ring, 1.0);
        assert_eq!(loops.len(), 2);
        assert!(loops.iter().all(|l| l.1));
        assert!((radius_of(loops[0].0[0]) - 30.0).abs() < 1e-3);
        assert!((radius_of(loops[1].0[0]) - 20.0).abs() < 1e-3);
        let wedge = UiShapeType::Wedge {
            radius: 30.0,
            inner_radius: 0.0,
            start_deg: 0.0,
            sweep_deg: 360.0,
        };
        assert_eq!(outlines(&wedge, 1.0).len(), 1);
    }

    #[test]
    fn a_rounded_rect_starts_at_its_top_edges_left_end() {
        let rect = UiShapeType::RoundedRect {
            size: [80.0, 40.0],
            corner_radius: 10.0,
        };
        let (points, closed) = &outlines(&rect, 1.0)[0];
        assert!(closed);
        assert!((points[0][0] + 30.0).abs() < 1e-3 && (points[0][1] + 20.0).abs() < 1e-3);
        // The first move is along the top edge, to the right.
        assert!(points[1][0] > points[0][0]);
    }

    #[test]
    fn a_square_rounded_rect_has_no_duplicate_corners() {
        let rect = UiShapeType::RoundedRect {
            size: [20.0, 10.0],
            corner_radius: 0.0,
        };
        let (points, _) = &outlines(&rect, 1.0)[0];
        assert_eq!(points.len(), 4);
    }

    #[test]
    fn a_line_is_one_open_path_and_a_zero_line_none() {
        let line = UiShapeType::Line {
            from: [1.0, 2.0],
            to: [11.0, 2.0],
            cap: crate::components::UiLineCap::Round,
        };
        assert_eq!(
            outlines(&line, 2.0),
            vec![(vec![[2.0, 4.0], [22.0, 4.0]], false)]
        );
        let zero = UiShapeType::Line {
            from: [1.0, 2.0],
            to: [1.0, 2.0],
            cap: crate::components::UiLineCap::Round,
        };
        assert!(outlines(&zero, 1.0).is_empty());
    }
}
