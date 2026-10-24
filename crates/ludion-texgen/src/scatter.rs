//! The tile sampler: a stamp scattered once per cell of a grid that wraps around the tile, each
//! instance jittered by a hash of its wrapped cell so the result is seamless by construction.

use ludion_core::math::{hypot, sin_cos};

use crate::hash::{fold32, hash2, lowbias32, unit};
use crate::image::Image;
use crate::ops::{Combine, StampShape, TileSpec};

/// One cell's instance, with everything but its cell position resolved.
struct Instance {
    keep: bool,
    /// Offset of the centre from the cell centre, in tile units.
    jitter: (f32, f32),
    /// Size relative to the cell.
    size: f32,
    sin: f32,
    cos: f32,
    value: f32,
}

/// The `k`-th random of an instance hash, in `[0, 1)`.
fn random(h: u32, k: u32) -> f32 {
    unit(lowbias32(h ^ k.wrapping_mul(0x9E37_79B9)))
}

fn instances(spec: &TileSpec, seed: u32) -> Vec<Instance> {
    let (cw, ch) = (1.0 / spec.columns as f32, 1.0 / spec.rows as f32);
    let mut out = Vec::with_capacity(spec.columns as usize * spec.rows as usize);
    for j in 0..spec.rows {
        for i in 0..spec.columns {
            let h = hash2(i, j, seed);
            let degrees = spec.rotation + (random(h, 4) - 0.5) * 360.0 * spec.rotation_jitter;
            let (sin, cos) = sin_cos(degrees.to_radians());
            out.push(Instance {
                keep: random(h, 6) < spec.density,
                jitter: (
                    (random(h, 1) - 0.5) * spec.position_jitter * cw,
                    (random(h, 2) - 0.5) * spec.position_jitter * ch,
                ),
                size: spec.scale * (1.0 - spec.scale_jitter * random(h, 3)),
                sin,
                cos,
                value: 1.0 - spec.value_jitter * random(h, 5),
            });
        }
    }
    out
}

/// Cells to visit on each side of a texel's own cell so no instance that reaches it is missed.
fn reach(spec: &TileSpec) -> (i64, i64) {
    let (cw, ch) = (1.0 / spec.columns as f32, 1.0 / spec.rows as f32);
    let turns = spec.rotation != 0.0 || spec.rotation_jitter > 0.0;
    let (ext_x, ext_y) = if turns {
        let e = 0.5 * spec.scale * hypot(cw, ch);
        (e, e)
    } else {
        (0.5 * spec.scale * cw, 0.5 * spec.scale * ch)
    };
    let cells = |ext: f32, cell: f32| {
        ((ext + 0.5 * spec.position_jitter * cell) / cell + 0.5).ceil() as i64
    };
    (
        cells(ext_x, cw) + i64::from(spec.offset > 0.0),
        cells(ext_y, ch),
    )
}

/// Renders the sampler at `size`×`size`. `pattern` is the gray stamp image when the spec has one;
/// `step_seed` is the recipe seed mixed with the op's `seed`.
pub fn render(spec: &TileSpec, pattern: Option<&Image>, size: u32, step_seed: u64) -> Image {
    let table = instances(spec, fold32(step_seed));
    let (columns, rows) = (i64::from(spec.columns), i64::from(spec.rows));
    let (cw, ch) = (1.0 / spec.columns as f32, 1.0 / spec.rows as f32);
    let (kx, ky) = reach(spec);
    let n = size as f32;
    Image::generate(size, 1, |x, y, t| {
        let (tx, ty) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);
        let (i0, j0) = ((tx / cw).floor() as i64, (ty / ch).floor() as i64);
        let mut acc = 0.0f32;
        for j in j0 - ky..=j0 + ky {
            let row = j.rem_euclid(rows);
            let shift = if row % 2 == 1 { spec.offset } else { 0.0 };
            for i in i0 - kx..=i0 + kx {
                let inst = &table[(row * columns + i.rem_euclid(columns)) as usize];
                if !inst.keep || inst.size <= 0.0 {
                    continue;
                }
                let cx = (i as f32 + 0.5 + shift) * cw + inst.jitter.0;
                let cy = (j as f32 + 0.5) * ch + inst.jitter.1;
                let (dx, dy) = (tx - cx, ty - cy);
                let lx = (dx * inst.cos + dy * inst.sin) / (0.5 * inst.size * cw);
                let ly = (dy * inst.cos - dx * inst.sin) / (0.5 * inst.size * ch);
                if lx.abs() > 1.0 || ly.abs() > 1.0 {
                    continue;
                }
                let stamp = match (pattern, spec.shape) {
                    (Some(p), _) => {
                        let mut v = [0.0];
                        p.sample((lx + 1.0) * 0.5, (ly + 1.0) * 0.5, &mut v);
                        v[0]
                    }
                    (None, StampShape::Box) => 1.0,
                    (None, StampShape::Disc) => f32::from(lx * lx + ly * ly <= 1.0),
                    (None, StampShape::Bell) => {
                        let r2 = lx * lx + ly * ly;
                        if r2 < 1.0 {
                            (1.0 - r2) * (1.0 - r2)
                        } else {
                            0.0
                        }
                    }
                    (None, StampShape::Pyramid) => 1.0 - lx.abs().max(ly.abs()),
                };
                let v = stamp * inst.value;
                acc = match spec.combine {
                    Combine::Max => acc.max(v),
                    Combine::Add => acc + v,
                };
            }
        }
        t[0] = acc;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(columns: u32, rows: u32) -> TileSpec {
        TileSpec {
            columns,
            rows,
            shape: StampShape::Box,
            pattern: None,
            scale: 1.0,
            scale_jitter: 0.0,
            position_jitter: 0.0,
            rotation_jitter: 0.0,
            value_jitter: 0.0,
            rotation: 0.0,
            offset: 0.0,
            density: 1.0,
            combine: Combine::Max,
            seed: 0,
        }
    }

    #[test]
    fn a_single_box_fills_the_tile() {
        let out = render(&spec(1, 1), None, 32, 0);
        assert!(out.data.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn a_disc_covers_pi_over_four() {
        let mut s = spec(1, 1);
        s.shape = StampShape::Disc;
        let out = render(&s, None, 128, 0);
        let covered = out.data.iter().sum::<f32>() / out.data.len() as f32;
        assert!(
            (covered - std::f32::consts::FRAC_PI_4).abs() < 0.02,
            "{covered}"
        );
    }

    #[test]
    fn zero_density_is_empty() {
        let mut s = spec(4, 4);
        s.density = 0.0;
        assert!(render(&s, None, 32, 3).data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn offset_shifts_odd_rows_by_that_part_of_a_cell() {
        let mut s = spec(4, 2);
        s.scale = 0.5;
        s.offset = 0.5;
        let out = render(&s, None, 64, 0);
        // Cells are 16 texels wide; the stamp spans the middle half of each.
        assert_eq!(out.texel(8, 16)[0], 1.0);
        assert_eq!(out.texel(16, 16)[0], 0.0);
        assert_eq!(out.texel(8, 48)[0], 0.0);
        assert_eq!(out.texel(16, 48)[0], 1.0);
        // The shifted row wraps: its last instance reaches across the right edge.
        assert_eq!(out.texel(0, 48)[0], 1.0);
    }

    #[test]
    fn a_pattern_at_scale_one_on_one_cell_reproduces_it() {
        let pattern = Image::generate(16, 1, |x, y, t| t[0] = (x * 3 + y) as f32 / 64.0);
        let mut s = spec(1, 1);
        s.pattern = Some("p".into());
        let out = render(&s, Some(&pattern), 16, 0);
        for (a, b) in out.data.iter().zip(&pattern.data) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn jitter_is_a_function_of_the_wrapped_cell() {
        let mut s = spec(3, 3);
        s.shape = StampShape::Bell;
        s.scale = 1.6;
        s.position_jitter = 1.0;
        s.rotation_jitter = 1.0;
        s.value_jitter = 0.5;
        let a = render(&s, None, 48, 11);
        let b = render(&s, None, 48, 11);
        assert_eq!(a, b);
        assert_ne!(a, render(&s, None, 48, 12));
    }
}
