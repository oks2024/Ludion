//! [`UiShape`] resolved to a [`UiInstance::Shape`], the SDF the draw and the hit test share, and
//! [`ui_shape_hit`] (`Plan/ui-hud.md` decisions 53–61).
//!
//! `shape_sdf` is the Rust twin of `ui_shape.wgsl`: both fold a sector about its mid ray and take the
//! distance to the cap segment, so the draw and the hit agree to the anti-aliasing band. Everything
//! here uses `ludion_core::math` for roots and trig.

use ludion_core::math::{hypot, sin_cos};
use ludion_renderer::{
    UiClip, UiInstance, UiPathJoin, UiPathStroke, UiRotation, UiShapeGeometry, UiShapeStroke,
};

use super::group::cos_sin;
use super::polygon::{PolygonFault, dedup_points, polygon_contains, stroke_contains, triangulate};
use super::{canvas_anchor_px, pixel_scale};
use crate::components::{UiLineCap, UiLineJoin, UiShape, UiShapeType};

const TAU: f32 = std::f32::consts::TAU;

/// The shape as `extract` draws it before tweens and groups act: origin and geometry in pixels, the
/// fill and stroke. `None` when nothing draws: an invisible shape, no fill and no stroke, a zero
/// sweep or a zero-length butt line.
///
/// `line` and `arc` carry no stroke in the instance: their geometry is the stroke band, drawn as a
/// fill of the stroke colour (decision 56).
pub(crate) fn shape_instance(
    shape: &UiShape,
    viewport: (u32, u32),
) -> Result<Option<UiInstance>, PolygonFault> {
    match &shape.shape {
        UiShapeType::Polygon { points, join } => {
            path_instance(shape, points, None, *join, UiLineCap::Butt, viewport)
        }
        UiShapeType::Polyline {
            points,
            closed,
            join,
            cap,
        } => path_instance(shape, points, Some(*closed), *join, *cap, viewport),
        _ => Ok(analytic_instance(shape, viewport)),
    }
}

/// The kind name a path fault is reported under.
pub(crate) fn path_kind(shape: &UiShapeType) -> &'static str {
    match shape {
        UiShapeType::Polyline { .. } => "polyline",
        _ => "polygon",
    }
}

/// Whether `local` (relative to the path's origin) is on a polygonal path: inside its fill, or on its
/// stroke. The one test the draw's hit check and `ui_hit` share (decision 79).
pub(crate) fn path_contains(
    points: &[[f32; 2]],
    closed: bool,
    has_fill: bool,
    stroke: Option<&UiPathStroke>,
    local: [f32; 2],
) -> bool {
    (has_fill && polygon_contains(points, local))
        || stroke.is_some_and(|s| {
            // A stroke under 1 px draws at 1 px, so it hits as one.
            let half = s.width_px.max(1.0) * 0.5;
            stroke_contains(points, closed, half, s.join, s.round_cap, local)
        })
}

/// A `polygon` (`polyline` is `Some(closed)`) as `extract` draws it before tweens and groups act: the
/// points in pixels about the origin, the ear-clipped triangles of a fill and the outline. `Ok(None)`
/// when it is invisible or has neither; `Err` when it cannot be drawn (decisions 70, 75, 76).
fn path_instance(
    shape: &UiShape,
    points: &[[f32; 2]],
    polyline: Option<bool>,
    join: UiLineJoin,
    cap: UiLineCap,
    viewport: (u32, u32),
) -> Result<Option<UiInstance>, PolygonFault> {
    let closed = polyline.unwrap_or(true);
    let fill = shape.fill.filter(|_| polyline.is_none());
    if !shape.visible || (fill.is_none() && shape.stroke.is_none()) {
        return Ok(None);
    }
    let ps = pixel_scale(viewport);
    let anchor = canvas_anchor_px(shape.anchor, viewport);
    let distinct = if closed || polyline.is_none() {
        dedup_points(points)
    } else {
        let mut open: Vec<[f32; 2]> = Vec::with_capacity(points.len());
        for &p in points {
            if open.last() != Some(&p) {
                open.push(p);
            }
        }
        open
    };
    let points: Vec<[f32; 2]> = distinct.iter().map(|p| [p[0] * ps, p[1] * ps]).collect();
    let triangles = if polyline.is_some() {
        if points.len() < 2 {
            return Err(PolygonFault::TooFew { needed: 2 });
        }
        Vec::new()
    } else {
        let triangles = triangulate(&points)?;
        if fill.is_some() {
            triangles
        } else {
            Vec::new()
        }
    };
    let stroke = shape.stroke.map(|s| UiPathStroke {
        width_px: s.width * ps,
        color: s.color,
        join: match join {
            UiLineJoin::Miter => UiPathJoin::Miter,
            UiLineJoin::Round => UiPathJoin::Round,
            UiLineJoin::Bevel => UiPathJoin::Bevel,
        },
        round_cap: cap == UiLineCap::Round && !closed,
    });
    Ok(Some(UiInstance::Polygon {
        origin_px: [
            anchor[0] + shape.offset[0] * ps,
            anchor[1] + shape.offset[1] * ps,
        ],
        points,
        closed,
        triangles,
        fill,
        stroke,
        clip: UiClip::NONE,
        rotation: UiRotation::NONE,
    }))
}

fn analytic_instance(shape: &UiShape, viewport: (u32, u32)) -> Option<UiInstance> {
    if !shape.visible {
        return None;
    }
    let ps = pixel_scale(viewport);
    let anchor = canvas_anchor_px(shape.anchor, viewport);
    let origin_px = [
        anchor[0] + shape.offset[0] * ps,
        anchor[1] + shape.offset[1] * ps,
    ];
    let sweep = |start_deg: f32, sweep_deg: f32| {
        let sweep = if sweep_deg >= 360.0 {
            TAU
        } else {
            sweep_deg.to_radians()
        };
        (start_deg.to_radians(), sweep)
    };
    let stroke = shape.stroke.map(|s| UiShapeStroke {
        width_px: s.width * ps,
        color: s.color,
    });
    let (geometry, fill, stroke) = match &shape.shape {
        &UiShapeType::Line { from, to, cap } => {
            let stroke = stroke?;
            let (a, b) = ([from[0] * ps, from[1] * ps], [to[0] * ps, to[1] * ps]);
            let round = cap == UiLineCap::Round;
            if !round && a == b {
                return None;
            }
            let geometry = UiShapeGeometry::Segment {
                a,
                b,
                half_width: stroke.width_px * 0.5,
                round,
            };
            (geometry, Some(stroke.color), None)
        }
        &UiShapeType::Circle { radius } => (
            UiShapeGeometry::Sector {
                radius_outer: radius * ps,
                radius_inner: 0.0,
                start: 0.0,
                sweep: TAU,
            },
            shape.fill,
            stroke,
        ),
        &UiShapeType::Ring {
            radius,
            thickness,
            start_deg,
            sweep_deg,
        } => {
            if sweep_deg <= 0.0 {
                return None;
            }
            let (start, sweep) = sweep(start_deg, sweep_deg);
            (
                UiShapeGeometry::Sector {
                    radius_outer: radius * ps,
                    radius_inner: (radius - thickness) * ps,
                    start,
                    sweep,
                },
                shape.fill,
                stroke,
            )
        }
        &UiShapeType::Arc {
            radius,
            start_deg,
            sweep_deg,
        } => {
            let stroke = stroke?;
            if sweep_deg <= 0.0 {
                return None;
            }
            let (start, sweep) = sweep(start_deg, sweep_deg);
            let half = stroke.width_px * 0.5;
            let centre = radius * ps;
            (
                UiShapeGeometry::Sector {
                    radius_outer: centre + half,
                    radius_inner: (centre - half).max(0.0),
                    start,
                    sweep,
                },
                Some(stroke.color),
                None,
            )
        }
        &UiShapeType::Wedge {
            radius,
            inner_radius,
            start_deg,
            sweep_deg,
        } => {
            if sweep_deg <= 0.0 {
                return None;
            }
            let (start, sweep) = sweep(start_deg, sweep_deg);
            (
                UiShapeGeometry::Sector {
                    radius_outer: radius * ps,
                    radius_inner: inner_radius * ps,
                    start,
                    sweep,
                },
                shape.fill,
                stroke,
            )
        }
        &UiShapeType::RoundedRect {
            size,
            corner_radius,
        } => {
            let half_size = [size[0] * ps * 0.5, size[1] * ps * 0.5];
            let radius = (corner_radius * ps).min(half_size[0]).min(half_size[1]);
            (
                UiShapeGeometry::RoundedBox { half_size, radius },
                shape.fill,
                stroke,
            )
        }
        UiShapeType::Polygon { .. } | UiShapeType::Polyline { .. } => return None,
    };
    if fill.is_none() && stroke.is_none() {
        return None;
    }
    Some(UiInstance::Shape {
        origin_px,
        geometry,
        fill,
        stroke,
        clip: UiClip::NONE,
        rotation: UiRotation::NONE,
    })
}

/// A sector about `origin_px` as `extract` draws it: `start_deg` and `sweep_deg` are clock degrees, a
/// sweep of 360 or more is a full turn, and `stroke` is already in pixels. `rotation` and `clip` are
/// left for the group and `hud::post` to apply.
pub(crate) fn sector_instance(
    origin_px: [f32; 2],
    radius_outer: f32,
    radius_inner: f32,
    start_deg: f32,
    sweep_deg: f32,
    fill: Option<[f32; 4]>,
    stroke: Option<UiShapeStroke>,
) -> UiInstance {
    let sweep = if sweep_deg >= 360.0 {
        TAU
    } else {
        sweep_deg.to_radians()
    };
    UiInstance::Shape {
        origin_px,
        geometry: UiShapeGeometry::Sector {
            radius_outer,
            radius_inner,
            start: start_deg.to_radians(),
            sweep,
        },
        fill,
        stroke,
        clip: UiClip::NONE,
        rotation: UiRotation::NONE,
    }
}

/// Scales every length of `geometry` by `k` (angles are untouched).
pub(crate) fn scale_geometry(geometry: &mut UiShapeGeometry, k: f32) {
    match geometry {
        UiShapeGeometry::Sector {
            radius_outer,
            radius_inner,
            ..
        } => {
            *radius_outer *= k;
            *radius_inner *= k;
        }
        UiShapeGeometry::Segment {
            a, b, half_width, ..
        } => {
            *a = [a[0] * k, a[1] * k];
            *b = [b[0] * k, b[1] * k];
            *half_width *= k;
        }
        UiShapeGeometry::RoundedBox { half_size, radius } => {
            *half_size = [half_size[0] * k, half_size[1] * k];
            *radius *= k;
        }
    }
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn length(v: [f32; 2]) -> f32 {
    hypot(v[0], v[1])
}

/// A box of half extents `half` centred on 0, its corners rounded by `radius`.
fn box_sdf(p: [f32; 2], half: [f32; 2], radius: f32) -> f32 {
    let q = [p[0].abs() - half[0] + radius, p[1].abs() - half[1] + radius];
    length([q[0].max(0.0), q[1].max(0.0)]) + q[0].max(q[1]).min(0.0) - radius
}

/// The signed distance, in pixels, from `local` (relative to the shape's origin, +Y down) to the
/// geometry's edge: negative inside. The Rust twin of `ui_shape.wgsl`'s three SDFs.
pub(crate) fn shape_sdf(geometry: &UiShapeGeometry, local: [f32; 2]) -> f32 {
    match *geometry {
        UiShapeGeometry::Sector {
            radius_outer,
            radius_inner,
            start,
            sweep,
        } => {
            let half = sweep * 0.5;
            let (sin_m, cos_m) = sin_cos(start + half);
            let (sin_h, cos_h) = sin_cos(half);
            let q = [
                dot(local, [cos_m, sin_m]).abs(),
                dot(local, [sin_m, -cos_m]),
            ];
            let len = length(q);
            let d_r = if radius_inner <= 0.0 {
                len - radius_outer
            } else {
                (len - radius_outer).max(radius_inner - len)
            };
            if sweep >= TAU {
                return d_r;
            }
            let e = [sin_h, cos_h];
            let t = dot(q, e).clamp(radius_inner, radius_outer);
            let d_c = length([q[0] - e[0] * t, q[1] - e[1] * t]);
            let side = if cos_h * q[0] - sin_h * q[1] > 0.0 {
                1.0
            } else {
                -1.0
            };
            d_r.max(side * d_c)
        }
        UiShapeGeometry::Segment {
            a,
            b,
            half_width,
            round,
        } => {
            let ba = [b[0] - a[0], b[1] - a[1]];
            if round {
                let pa = [local[0] - a[0], local[1] - a[1]];
                let h = (dot(pa, ba) / dot(ba, ba).max(1e-12)).clamp(0.0, 1.0);
                return length([pa[0] - ba[0] * h, pa[1] - ba[1] * h]) - half_width;
            }
            let len = length(ba).max(1e-12);
            let u = [ba[0] / len, ba[1] / len];
            let pm = [
                local[0] - (a[0] + b[0]) * 0.5,
                local[1] - (a[1] + b[1]) * 0.5,
            ];
            let p = [dot(pm, u), dot(pm, [-u[1], u[0]])];
            box_sdf(p, [len * 0.5, half_width], 0.0)
        }
        UiShapeGeometry::RoundedBox { half_size, radius } => box_sdf(local, half_size, radius),
    }
}

/// Whether `local` is on the drawn shape: inside its fill, or on its stroke band. A stroke-only kind
/// carries its band as a fill, so it is the same test.
pub(crate) fn shape_contains(
    geometry: &UiShapeGeometry,
    fill: bool,
    stroke_width: Option<f32>,
    local: [f32; 2],
) -> bool {
    let d = shape_sdf(geometry, local);
    (fill && d < 0.0) || stroke_width.is_some_and(|w| d.abs() < w * 0.5)
}

/// Whether `cursor` (render-target pixels, top-left origin, +Y down) is on `shape` as it is drawn
/// when ungrouped and untweened: its rotation about the origin turned back, then the SDF. A fill is
/// hit inside the edge, a stroke on its band; a stroke-only kind (`line`, `arc`) on its band. `false`
/// for an invisible shape or one that draws nothing. `ui_hit` is the grouped, tweened, clipped test.
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiShape, UiShapeType};
/// use ludion_engine::ui::ui_shape_hit;
///
/// let disc = UiShape {
///     shape: UiShapeType::Circle { radius: 50.0 },
///     fill: Some([1.0; 4]),
///     ..UiShape::default()
/// };
/// assert!(ui_shape_hit(&disc, (1920, 1080), [10.0, 10.0]));
/// assert!(!ui_shape_hit(&disc, (1920, 1080), [200.0, 10.0]));
/// ```
pub fn ui_shape_hit(shape: &UiShape, viewport: (u32, u32), cursor: [f32; 2]) -> bool {
    if !cursor.iter().all(|c| c.is_finite()) {
        return false;
    }
    let Ok(Some(instance)) = shape_instance(shape, viewport) else {
        return false;
    };
    let origin_px = match &instance {
        UiInstance::Shape { origin_px, .. } | UiInstance::Polygon { origin_px, .. } => *origin_px,
        _ => return false,
    };
    let (dx, dy) = (cursor[0] - origin_px[0], cursor[1] - origin_px[1]);
    let local = if shape.rotation == 0.0 {
        [dx, dy]
    } else {
        let (c, s) = cos_sin(shape.rotation.to_radians());
        [c * dx + s * dy, -s * dx + c * dy]
    };
    match &instance {
        UiInstance::Shape {
            geometry,
            fill,
            stroke,
            ..
        } => shape_contains(geometry, fill.is_some(), stroke.map(|s| s.width_px), local),
        UiInstance::Polygon {
            points,
            closed,
            fill,
            stroke,
            ..
        } => path_contains(points, *closed, fill.is_some(), stroke.as_ref(), local),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(
        radius_outer: f32,
        radius_inner: f32,
        start_deg: f32,
        sweep_deg: f32,
    ) -> UiShapeGeometry {
        UiShapeGeometry::Sector {
            radius_outer,
            radius_inner,
            start: start_deg.to_radians(),
            sweep: if sweep_deg >= 360.0 {
                TAU
            } else {
                sweep_deg.to_radians()
            },
        }
    }

    /// A point at clock angle `deg` (0 is 12 o'clock, clockwise) and radius `r`, +Y down.
    fn clock(deg: f32, r: f32) -> [f32; 2] {
        let (s, c) = sin_cos(deg.to_radians());
        [r * s, -r * c]
    }

    #[test]
    fn a_discs_centre_is_minus_its_radius() {
        let disc = sector(20.0, 0.0, 0.0, 360.0);
        assert!((shape_sdf(&disc, [0.0, 0.0]) + 20.0).abs() < 1e-4);
        assert!(shape_sdf(&disc, [30.0, 0.0]) > 0.0);
    }

    #[test]
    fn a_wedge_is_inside_on_its_mid_ray_and_outside_past_its_edge() {
        // 90 degrees clockwise from 12 o'clock: the upper-right quadrant, mid ray at 45 degrees.
        let wedge = sector(40.0, 0.0, 0.0, 90.0);
        assert!(shape_sdf(&wedge, clock(45.0, 20.0)) < 0.0);
        assert!(shape_sdf(&wedge, clock(100.0, 20.0)) > 0.0);
        assert!(shape_sdf(&wedge, clock(-10.0, 20.0)) > 0.0);
        assert!(shape_sdf(&wedge, clock(45.0, 50.0)) > 0.0);
    }

    #[test]
    fn a_full_ring_has_no_seam_at_six_oclock() {
        let ring = sector(40.0, 30.0, 0.0, 360.0);
        for deg in [179.0, 180.0, 181.0, 0.0, 90.0, 270.0] {
            assert!(shape_sdf(&ring, clock(deg, 35.0)) < 0.0, "{deg}");
        }
        // A near-full sweep still has its gap.
        let almost = sector(40.0, 30.0, 0.0, 350.0);
        assert!(shape_sdf(&almost, clock(355.0, 35.0)) > 0.0);
        assert!(shape_sdf(&almost, clock(170.0, 35.0)) < 0.0);
    }

    #[test]
    fn a_wedge_wider_than_a_half_turn_keeps_its_far_side() {
        let wedge = sector(40.0, 0.0, 0.0, 270.0);
        assert!(shape_sdf(&wedge, clock(200.0, 20.0)) < 0.0);
        assert!(shape_sdf(&wedge, clock(315.0, 20.0)) > 0.0);
    }

    #[test]
    fn a_butt_line_ends_flush_and_a_round_line_overhangs() {
        let line = |round| UiShapeGeometry::Segment {
            a: [-10.0, 0.0],
            b: [10.0, 0.0],
            half_width: 2.0,
            round,
        };
        assert!(shape_sdf(&line(false), [0.0, 0.0]) < 0.0);
        assert!(shape_sdf(&line(false), [10.5, 0.0]) > 0.0);
        assert!(shape_sdf(&line(false), [9.5, 1.9]) < 0.0);
        assert!(shape_sdf(&line(true), [11.5, 0.0]) < 0.0);
        assert!(shape_sdf(&line(true), [12.5, 0.0]) > 0.0);
    }

    #[test]
    fn a_rounded_box_rounds_its_corners() {
        let rounded = UiShapeGeometry::RoundedBox {
            half_size: [20.0, 10.0],
            radius: 8.0,
        };
        assert!(shape_sdf(&rounded, [0.0, 0.0]) < 0.0);
        assert!(shape_sdf(&rounded, [19.5, 9.5]) > 0.0);
        assert!(shape_sdf(&rounded, [19.5, 0.0]) < 0.0);
    }

    fn line_shape(rotation: f32) -> UiShape {
        UiShape {
            shape: UiShapeType::Line {
                from: [-20.0, 0.0],
                to: [20.0, 0.0],
                cap: UiLineCap::Butt,
            },
            stroke: Some(crate::components::UiStroke {
                width: 6.0,
                color: [1.0; 4],
            }),
            rotation,
            ..UiShape::default()
        }
    }

    #[test]
    fn hit_turns_the_cursor_back_about_the_origin() {
        let viewport = (1920, 1080);
        // The line is along +X through the origin (top-left corner); rotated 90 degrees clockwise it
        // is along +Y, so a point straight below the origin hits and one to the right does not.
        let rotated = line_shape(90.0);
        assert!(ui_shape_hit(&rotated, viewport, [0.0, 10.0]));
        assert!(!ui_shape_hit(&rotated, viewport, [10.0, 0.0]));
        let flat = line_shape(0.0);
        assert!(ui_shape_hit(&flat, viewport, [10.0, 0.0]));
        assert!(!ui_shape_hit(&flat, viewport, [0.0, 10.0]));
    }

    #[test]
    fn a_zero_sweep_or_an_empty_shape_draws_nothing() {
        let viewport = (1920, 1080);
        let ring = UiShape {
            shape: UiShapeType::Ring {
                radius: 20.0,
                thickness: 4.0,
                start_deg: 0.0,
                sweep_deg: 0.0,
            },
            fill: Some([1.0; 4]),
            ..UiShape::default()
        };
        assert!(shape_instance(&ring, viewport).unwrap().is_none());
        assert!(
            shape_instance(&UiShape::default(), viewport)
                .unwrap()
                .is_none()
        );
        let zero_line = UiShape {
            shape: UiShapeType::Line {
                from: [3.0, 3.0],
                to: [3.0, 3.0],
                cap: UiLineCap::Butt,
            },
            stroke: Some(crate::components::UiStroke {
                width: 4.0,
                color: [1.0; 4],
            }),
            ..UiShape::default()
        };
        assert!(shape_instance(&zero_line, viewport).unwrap().is_none());
    }

    #[test]
    fn a_stroke_only_kind_is_its_band_drawn_as_a_fill() {
        let arc = UiShape {
            shape: UiShapeType::Arc {
                radius: 30.0,
                start_deg: 0.0,
                sweep_deg: 360.0,
            },
            stroke: Some(crate::components::UiStroke {
                width: 10.0,
                color: [0.0, 1.0, 0.0, 1.0],
            }),
            ..UiShape::default()
        };
        let Ok(Some(UiInstance::Shape {
            geometry,
            fill,
            stroke,
            ..
        })) = shape_instance(&arc, (1080, 1080))
        else {
            panic!("an arc draws");
        };
        assert_eq!(fill, Some([0.0, 1.0, 0.0, 1.0]));
        assert!(stroke.is_none());
        assert_eq!(
            geometry,
            UiShapeGeometry::Sector {
                radius_outer: 35.0,
                radius_inner: 25.0,
                start: 0.0,
                sweep: TAU,
            }
        );
    }
}
