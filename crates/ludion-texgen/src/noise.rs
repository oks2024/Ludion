//! Tileable lattice noise: value, gradient (Perlin) and cellular, summed into fbm.
//!
//! Every lattice coordinate wraps modulo the octave's integer period, which is what makes a tile
//! seamless: texel `size` samples the same lattice point as texel 0.

use crate::hash::{fold32, hash2, mix, unit};
use crate::image::Image;
use crate::ops::{Axis, CellReturn, NoiseKind, NoiseSpec};

/// The eight fixed unit gradients; no trigonometry, so no libm.
pub(crate) const GRADIENTS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (0.707_106_77, 0.707_106_77),
    (-0.707_106_77, 0.707_106_77),
    (0.707_106_77, -0.707_106_77),
    (-0.707_106_77, -0.707_106_77),
];

struct Octave {
    /// Lattice cells per tile along `u` and along `v`.
    period: (u32, u32),
    seed: u32,
    amplitude: f32,
}

/// A noise spec bound to a step seed, ready to sample.
pub struct Sampler<'a> {
    spec: &'a NoiseSpec,
    octaves: Vec<Octave>,
    norm: f32,
}

impl<'a> Sampler<'a> {
    /// Binds `spec` to `step_seed` (already `mix(recipe_seed, op.seed)`).
    pub fn new(spec: &'a NoiseSpec, step_seed: u64) -> Self {
        let mut octaves = Vec::new();
        let mut period = match spec.stretch_axis {
            Axis::U => (spec.scale, spec.scale.saturating_mul(spec.stretch)),
            Axis::V => (spec.scale.saturating_mul(spec.stretch), spec.scale),
        };
        let (mut amplitude, mut total) = (1.0f32, 0.0f32);
        for i in 0..spec.octaves {
            octaves.push(Octave {
                period,
                seed: fold32(mix(step_seed, u64::from(i))),
                amplitude,
            });
            total += amplitude;
            amplitude *= spec.gain;
            period = (
                period.0.saturating_mul(spec.lacunarity),
                period.1.saturating_mul(spec.lacunarity),
            );
        }
        Sampler {
            spec,
            octaves,
            norm: total,
        }
    }

    /// The noise at tile coordinates `(u, v)`, in `[0, 1]`.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        let mut sum = 0.0;
        for o in &self.octaves {
            let n = match self.spec.kind {
                NoiseKind::Value => value(o, u, v),
                NoiseKind::Gradient => gradient(o, u, v),
                NoiseKind::Cellular => cellular(self.spec, o, u, v),
            };
            sum += o.amplitude * n;
        }
        (sum / self.norm).clamp(0.0, 1.0)
    }

    /// Renders the noise into a gray image.
    pub fn render(&self, size: u32) -> Image {
        let s = size as f32;
        Image::generate(size, 1, |x, y, t| {
            t[0] = self.at((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
        })
    }
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Splits a tile coordinate into a wrapped lattice cell and the fraction inside it.
fn cell(coord: f32, period: u32) -> (i64, f32) {
    let p = coord * period as f32;
    let f = p.floor();
    (f as i64, p - f)
}

fn wrap(c: i64, period: u32) -> u32 {
    c.rem_euclid(i64::from(period)) as u32
}

fn value(o: &Octave, u: f32, v: f32) -> f32 {
    let ((cx, fx), (cy, fy)) = (cell(u, o.period.0), cell(v, o.period.1));
    let corner = |dx: i64, dy: i64| {
        unit(hash2(
            wrap(cx + dx, o.period.0),
            wrap(cy + dy, o.period.1),
            o.seed,
        ))
    };
    let (sx, sy) = (fade(fx), fade(fy));
    let top = lerp(corner(0, 0), corner(1, 0), sx);
    let bottom = lerp(corner(0, 1), corner(1, 1), sx);
    lerp(top, bottom, sy)
}

fn gradient(o: &Octave, u: f32, v: f32) -> f32 {
    let ((cx, fx), (cy, fy)) = (cell(u, o.period.0), cell(v, o.period.1));
    let corner = |dx: i64, dy: i64| {
        let h = hash2(wrap(cx + dx, o.period.0), wrap(cy + dy, o.period.1), o.seed);
        let (gx, gy) = GRADIENTS[(h >> 29) as usize];
        gx * (fx - dx as f32) + gy * (fy - dy as f32)
    };
    let (sx, sy) = (fade(fx), fade(fy));
    let top = lerp(corner(0, 0), corner(1, 0), sx);
    let bottom = lerp(corner(0, 1), corner(1, 1), sx);
    let n = lerp(top, bottom, sy);
    (0.5 + n * std::f32::consts::SQRT_2 * 0.5).clamp(0.0, 1.0)
}

fn cellular(spec: &NoiseSpec, o: &Octave, u: f32, v: f32) -> f32 {
    let ((cx, fx), (cy, fy)) = (cell(u, o.period.0), cell(v, o.period.1));
    let (mut f1, mut f2) = (f32::MAX, f32::MAX);
    for dy in -1..=1i64 {
        for dx in -1..=1i64 {
            let (hx, hy) = (wrap(cx + dx, o.period.0), wrap(cy + dy, o.period.1));
            let px = 0.5 + spec.jitter * (unit(hash2(hx, hy, o.seed)) - 0.5);
            let py = 0.5 + spec.jitter * (unit(hash2(hx, hy, o.seed ^ 0x9E37_79B9)) - 0.5);
            let (ox, oy) = (dx as f32 + px - fx, dy as f32 + py - fy);
            let d = ox * ox + oy * oy;
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    let (d1, d2) = (ludion_core::math::sqrt(f1), ludion_core::math::sqrt(f2));
    match spec.ret {
        CellReturn::F1 => d1,
        CellReturn::F2 => d2,
        CellReturn::F2MinusF1 => d2 - d1,
    }
    .min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: NoiseKind) -> NoiseSpec {
        NoiseSpec {
            kind,
            scale: 4,
            octaves: 3,
            lacunarity: 2,
            gain: 0.5,
            seed: 0,
            ret: CellReturn::F1,
            jitter: 1.0,
            stretch: 1,
            stretch_axis: Axis::U,
        }
    }

    #[test]
    fn every_kind_stays_in_unit_range_and_varies() {
        for kind in [NoiseKind::Value, NoiseKind::Gradient, NoiseKind::Cellular] {
            let s = spec(kind);
            let image = Sampler::new(&s, 7).render(32);
            let (lo, hi) = image
                .data
                .iter()
                .fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
            assert!(lo >= 0.0 && hi <= 1.0, "{kind:?} range {lo}..{hi}");
            assert!(hi - lo > 0.2, "{kind:?} is flat: {lo}..{hi}");
        }
    }

    fn mean_steps(image: &Image) -> (f32, f32) {
        let n = image.size as i64;
        let (mut along_u, mut along_v) = (0.0, 0.0);
        for y in 0..n {
            for x in 0..n {
                let here = image.texel(x, y)[0];
                along_u += (image.texel(x + 1, y)[0] - here).abs();
                along_v += (image.texel(x, y + 1)[0] - here).abs();
            }
        }
        let count = (n * n) as f32;
        (along_u / count, along_v / count)
    }

    #[test]
    fn stretch_lengthens_features_along_its_axis() {
        for kind in [NoiseKind::Value, NoiseKind::Gradient, NoiseKind::Cellular] {
            for axis in [Axis::U, Axis::V] {
                let s = NoiseSpec {
                    stretch: 8,
                    stretch_axis: axis,
                    ..spec(kind)
                };
                let (du, dv) = mean_steps(&Sampler::new(&s, 7).render(64));
                let (along, across) = if axis == Axis::U { (du, dv) } else { (dv, du) };
                assert!(
                    along < across / 3.0,
                    "{kind:?} {axis:?}: along {along} across {across}"
                );
            }
        }
    }

    #[test]
    fn unit_stretch_is_the_plain_lattice() {
        for kind in [NoiseKind::Value, NoiseKind::Gradient, NoiseKind::Cellular] {
            let plain = Sampler::new(&spec(kind), 3).render(32);
            for axis in [Axis::U, Axis::V] {
                let s = NoiseSpec {
                    stretch_axis: axis,
                    ..spec(kind)
                };
                assert_eq!(Sampler::new(&s, 3).render(32), plain, "{kind:?} {axis:?}");
            }
        }
    }

    #[test]
    fn different_seeds_differ_and_same_seed_repeats() {
        let s = spec(NoiseKind::Value);
        let a = Sampler::new(&s, 1).render(16);
        assert_eq!(a, Sampler::new(&s, 1).render(16));
        assert_ne!(a, Sampler::new(&s, 2).render(16));
    }
}
