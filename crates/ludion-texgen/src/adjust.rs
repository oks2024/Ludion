//! Per-texel ops: generators, blends and tonal adjustments.

use ludion_core::math::{cos, powf};

use crate::image::Image;
use crate::ops::{Axis, BlendMode, Const, GradientShape};

/// A flat image.
pub fn constant(size: u32, value: &Const) -> Image {
    match value {
        Const::Gray(v) => Image::filled(size, 1, *v),
        Const::Color(c) => Image::generate(size, 3, |_, _, t| t.copy_from_slice(c)),
    }
}

/// A periodic gradient along `axis`.
pub fn gradient(size: u32, axis: Axis, shape: GradientShape, repeat: u32) -> Image {
    let s = size as f32;
    Image::generate(size, 1, |x, y, t| {
        let along = match axis {
            Axis::U => x,
            Axis::V => y,
        };
        let phase = ((along as f32 + 0.5) / s * repeat as f32).rem_euclid(1.0);
        t[0] = match shape {
            GradientShape::Ramp => phase,
            GradientShape::Triangle => 1.0 - (2.0 * phase - 1.0).abs(),
            GradientShape::Sine => 0.5 - 0.5 * cos(std::f32::consts::TAU * phase),
        };
    })
}

fn combine(mode: BlendMode, a: f32, b: f32) -> f32 {
    match mode {
        BlendMode::Mix => b,
        BlendMode::Add => a + b,
        BlendMode::Subtract => a - b,
        BlendMode::Multiply => a * b,
        BlendMode::Screen => 1.0 - (1.0 - a) * (1.0 - b),
        BlendMode::Overlay => {
            if a < 0.5 {
                2.0 * a * b
            } else {
                1.0 - 2.0 * (1.0 - a) * (1.0 - b)
            }
        }
        BlendMode::Min => a.min(b),
        BlendMode::Max => a.max(b),
        BlendMode::Difference => (a - b).abs(),
    }
}

/// `lerp(a, mode(a, b), opacity * mask)`; a gray operand broadcasts over a color one.
pub fn blend(a: &Image, b: &Image, mask: Option<&Image>, mode: BlendMode, opacity: f32) -> Image {
    let channels = a.channels.max(b.channels);
    Image::generate(a.size, channels, |x, y, t| {
        let weight = opacity * mask.map_or(1.0, |m| m.channel(x, y, 0));
        for (c, out) in t.iter_mut().enumerate() {
            let (av, bv) = (a.channel(x, y, c), b.channel(x, y, c));
            *out = av + (combine(mode, av, bv) - av) * weight;
        }
    })
}

fn map(input: &Image, f: impl Fn(f32) -> f32 + Sync) -> Image {
    Image::generate(input.size, input.channels, |x, y, t| {
        for (c, out) in t.iter_mut().enumerate() {
            *out = f(input.channel(x, y, c));
        }
    })
}

/// Input range, gamma, then output range; the input range clamps.
pub fn levels(
    input: &Image,
    in_low: f32,
    in_high: f32,
    gamma: f32,
    out_low: f32,
    out_high: f32,
) -> Image {
    map(input, |v| {
        let mut t = ((v - in_low) / (in_high - in_low)).clamp(0.0, 1.0);
        if gamma != 1.0 {
            t = powf(t, 1.0 / gamma);
        }
        out_low + t * (out_high - out_low)
    })
}

/// A piecewise-linear curve through `points`, flat outside them.
pub fn curve(input: &Image, points: &[(f32, f32)]) -> Image {
    map(input, |v| {
        let first = points[0];
        if v <= first.0 {
            return first.1;
        }
        for pair in points.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            if v < x1 {
                return y0 + (y1 - y0) * ((v - x0) / (x1 - x0));
            }
        }
        points[points.len() - 1].1
    })
}

/// Clamps every sample to `[min, max]`.
pub fn clamp(input: &Image, min: f32, max: f32) -> Image {
    map(input, |v| v.clamp(min, max))
}

/// Linearly remaps `[from_low, from_high]` onto `[to_low, to_high]`, unclamped.
pub fn remap(input: &Image, from_low: f32, from_high: f32, to_low: f32, to_high: f32) -> Image {
    map(input, |v| {
        to_low + (v - from_low) / (from_high - from_low) * (to_high - to_low)
    })
}

/// Gray to color through `stops`, interpolated in linear.
pub fn colorize(input: &Image, stops: &[(f32, [f32; 3])]) -> Image {
    Image::generate(input.size, 3, |x, y, t| {
        let v = input.channel(x, y, 0);
        let first = &stops[0];
        if v <= first.0 {
            t.copy_from_slice(&first.1);
            return;
        }
        for pair in stops.windows(2) {
            let ((a0, c0), (a1, c1)) = (&pair[0], &pair[1]);
            if v < *a1 {
                let k = (v - a0) / (a1 - a0);
                for i in 0..3 {
                    t[i] = c0[i] + (c1[i] - c0[i]) * k;
                }
                return;
            }
        }
        t.copy_from_slice(&stops[stops.len() - 1].1);
    })
}

/// Rec.709 luminance of a color image.
pub fn grayscale(input: &Image) -> Image {
    Image::generate(input.size, 1, |x, y, t| {
        t[0] = 0.2126 * input.channel(x, y, 0)
            + 0.7152 * input.channel(x, y, 1)
            + 0.0722 * input.channel(x, y, 2);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(size: u32) -> Image {
        gradient(size, Axis::U, GradientShape::Ramp, 1)
    }

    #[test]
    fn gradient_shapes_are_periodic() {
        let tri = gradient(8, Axis::U, GradientShape::Triangle, 1);
        assert!(tri.data[0] < tri.data[3] && tri.data[7] < tri.data[4]);
        let two = gradient(8, Axis::U, GradientShape::Ramp, 2);
        assert_eq!(two.data[0], two.data[4]);
        let sine = gradient(8, Axis::V, GradientShape::Sine, 1);
        assert!((sine.data[0] - sine.data[7 * 8]).abs() < 1e-5);
    }

    #[test]
    fn blend_modes_and_mask() {
        let a = Image::filled(8, 1, 0.25);
        let b = Image::filled(8, 1, 0.5);
        assert_eq!(blend(&a, &b, None, BlendMode::Add, 1.0).data[0], 0.75);
        assert_eq!(blend(&a, &b, None, BlendMode::Multiply, 0.0).data[0], 0.25);
        let zero = Image::filled(8, 1, 0.0);
        assert_eq!(
            blend(&a, &b, Some(&zero), BlendMode::Mix, 1.0).data[0],
            0.25
        );
        let color = Image::filled(8, 3, 1.0);
        assert_eq!(blend(&a, &color, None, BlendMode::Mix, 1.0).channels, 3);
    }

    #[test]
    fn levels_curve_remap_clamp() {
        let r = ramp(4);
        let l = levels(&r, 0.0, 0.5, 1.0, 0.0, 1.0);
        assert_eq!(l.data[3], 1.0);
        let c = curve(&r, &[(0.0, 1.0), (1.0, 0.0)]);
        assert_eq!(c.data[0], 1.0 - r.data[0]);
        let m = remap(&r, 0.0, 1.0, 1.0, 3.0);
        assert_eq!(m.data[0], 1.0 + r.data[0] * 2.0);
        assert!(clamp(&m, 0.0, 2.0).data.iter().all(|v| *v <= 2.0));
    }

    #[test]
    fn colorize_then_grayscale_orders_by_luminance() {
        let stops = [(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])];
        let g = grayscale(&colorize(&ramp(8), &stops));
        assert!(g.data[..8].windows(2).all(|w| w[0] <= w[1]));
    }
}
