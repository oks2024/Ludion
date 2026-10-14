//! The signed-distance-field glyph generator: exact distance to the flattened outline, sign from
//! the nonzero winding number.

use super::geometry::TexelBox;

/// One glyph's SDF alpha bytes, row-major, top row first. `s` is font units per texel; `spread_px`
/// the falloff in texels. Encoding: `0.5 + d / (2 * spread_px)` clamped to `[0, 1]`, `d > 0`
/// inside, so the edge sits at 127.5.
pub(super) fn sdf_glyph(
    contours: &[Vec<[f32; 2]>],
    tbox: &TexelBox,
    s: f32,
    spread_px: u32,
) -> Vec<u8> {
    let (w, h) = (tbox.width(), tbox.height());
    let mut out = Vec::with_capacity(w as usize * h as usize);
    let denom = 2.0 * spread_px as f32;
    let boundary = boundary_segments(contours);
    for j in 0..h {
        let v = (tbox.py1 as f32 - j as f32 - 0.5) * s;
        for i in 0..w {
            let u = (tbox.px0 as f32 + i as f32 + 0.5) * s;
            let (d2, winding) = probe(contours, &boundary, u, v);
            let d = d2.sqrt() / s;
            let signed = if winding != 0 { d } else { -d };
            let value = (0.5 + signed / denom).clamp(0.0, 1.0);
            out.push((value * 255.0 + 0.5) as u8);
        }
    }
    out
}

/// This edge's signed contribution to the nonzero winding number at `(u, v)`. Half-open on y so a
/// vertex on the scanline counts once.
fn crossing(a: [f32; 2], b: [f32; 2], u: f32, v: f32) -> i32 {
    let cross = (b[0] - a[0]) * (v - a[1]) - (u - a[0]) * (b[1] - a[1]);
    if a[1] <= v {
        i32::from(b[1] > v && cross > 0.0)
    } else {
        -i32::from(b[1] <= v && cross < 0.0)
    }
}

fn winding(contour: &[[f32; 2]], u: f32, v: f32) -> i32 {
    let Some(&last) = contour.last() else {
        return 0;
    };
    let mut a = last;
    let mut total = 0;
    for &b in contour {
        total += crossing(a, b, u, v);
        a = b;
    }
    total
}

/// `+1` for a contour wound the way the nonzero rule counts as positive, `-1` for the other way.
fn orientation(contour: &[[f32; 2]]) -> i32 {
    let Some(&last) = contour.last() else {
        return 1;
    };
    let mut a = last;
    let mut twice_area = 0.0;
    for &b in contour {
        twice_area += a[0] * b[1] - b[0] * a[1];
        a = b;
    }
    if twice_area < 0.0 { -1 } else { 1 }
}

/// Per contour segment: whether it lies on the filled region's outline. Contours that overlap
/// leave edges inside the other's ink; their distance is zero but they are not the glyph's edge,
/// and drawing them would put a seam through the ink.
fn boundary_segments(contours: &[Vec<[f32; 2]>]) -> Vec<Vec<bool>> {
    contours
        .iter()
        .enumerate()
        .map(|(k, contour)| {
            let sign = orientation(contour);
            let Some(&last) = contour.last() else {
                return Vec::new();
            };
            let mut a = last;
            contour
                .iter()
                .map(|&b| {
                    let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                    a = b;
                    let others: i32 = contours
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != k)
                        .map(|(_, c)| winding(c, mid[0], mid[1]))
                        .sum();
                    (others != 0) != (others + sign != 0)
                })
                .collect()
        })
        .collect()
}

/// The minimum squared distance from `(u, v)` to any boundary segment, and the nonzero winding
/// number over every contour.
fn probe(contours: &[Vec<[f32; 2]>], boundary: &[Vec<bool>], u: f32, v: f32) -> (f32, i32) {
    let mut best = f32::INFINITY;
    let mut total = 0;
    for (contour, keep) in contours.iter().zip(boundary) {
        let Some(&last) = contour.last() else {
            continue;
        };
        let mut a = last;
        for (&b, &on_boundary) in contour.iter().zip(keep) {
            if on_boundary {
                best = best.min(segment_distance_sq(a, b, u, v));
            }
            total += crossing(a, b, u, v);
            a = b;
        }
    }
    (best, total)
}

fn segment_distance_sq(a: [f32; 2], b: [f32; 2], u: f32, v: f32) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((u - a[0]) * dx + (v - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (u - (a[0] + t * dx), v - (a[1] + t * dy));
    ex * ex + ey * ey
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 10-unit square at upem == em_px (s = 1), padded by 4 texels.
    fn square() -> Vec<Vec<[f32; 2]>> {
        vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]]
    }

    fn render(contours: &[Vec<[f32; 2]>]) -> (Vec<u8>, u32) {
        let b = TexelBox::new([0, 0, 10, 10], 1000, 1000, 4)
            .unwrap()
            .unwrap();
        (sdf_glyph(contours, &b, 1.0, 4), b.width())
    }

    #[test]
    fn inside_is_high_outside_is_low_and_the_edge_is_near_the_middle() {
        let (px, w) = render(&square());
        let at = |x: usize, y: usize| px[y * w as usize + x];
        assert!(at(9, 9) > 128, "centre {}", at(9, 9));
        assert!(at(0, 0) < 128, "corner {}", at(0, 0));
        // Texel centre x = 0.5 is 0.5 inside the left edge; row 9 is mid-height.
        let near = at(4, 9);
        assert!(near.abs_diff(128) <= 40, "{near}");
        let row: Vec<u8> = (0..w as usize).map(|x| at(x, 9)).collect();
        assert!(row[..=9].windows(2).all(|p| p[0] <= p[1]), "{row:?}");
    }

    #[test]
    fn a_counter_wound_inner_contour_reads_outside() {
        let mut o = square();
        o.push(vec![[3.0, 3.0], [3.0, 7.0], [7.0, 7.0], [7.0, 3.0]]);
        let (px, w) = render(&o);
        assert!(px[9 * w as usize + 9] < 128);
        assert!(px[9 * w as usize + 5] > 128);
    }

    #[test]
    fn an_edge_inside_an_overlapping_contour_is_not_a_seam() {
        let mut o = vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]];
        o.push(vec![[5.5, 5.5], [15.5, 5.5], [15.5, 15.5], [5.5, 15.5]]);
        let b = TexelBox::new([0, 0, 20, 20], 1000, 1000, 4)
            .unwrap()
            .unwrap();
        let px = sdf_glyph(&o, &b, 1.0, 4);
        // The texel centred on the inner square's left edge, inside the outer square.
        let at = px[13 * b.width() as usize + 10];
        assert!(at >= 230, "seam: {at}");
    }
}
