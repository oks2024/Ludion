//! Neighbourhood ops: warp, blur, height-to-normal, occlusion and curvature, all wrapping around
//! the tile.

use ludion_core::math::{cos, sin, sqrt};
use ludion_core::par;

use crate::image::Image;
use crate::noise::GRADIENTS;

/// Displaces `input` by `by`, in tile units.
///
/// A gray `by` moves along `angle` degrees by `(by - 0.5) * amount`; a color `by` moves by
/// `(r - 0.5, g - 0.5) * amount`.
pub fn warp(input: &Image, by: &Image, amount: f32, angle: f32) -> Image {
    let s = input.size as f32;
    let radians = angle.to_radians();
    let (dir_x, dir_y) = (cos(radians), sin(radians));
    Image::generate(input.size, input.channels, |x, y, t| {
        let (dx, dy) = if by.channels == 1 {
            let d = (by.channel(x, y, 0) - 0.5) * amount;
            (d * dir_x, d * dir_y)
        } else {
            (
                (by.channel(x, y, 0) - 0.5) * amount,
                (by.channel(x, y, 1) - 0.5) * amount,
            )
        };
        input.sample((x as f32 + 0.5) / s + dx, (y as f32 + 0.5) / s + dy, t);
    })
}

/// One wrap-around box pass along each row, window `2 * r + 1`.
fn box_rows(input: &Image, r: usize) -> Image {
    let size = input.size as usize;
    let channels = input.channels;
    let window = (2 * r + 1) as f64;
    let mut out = Image::filled(input.size, channels, 0.0);
    let row_len = size * channels;
    par::par_chunks(&mut out.data, row_len, |y, row| {
        let src = &input.data[y * row_len..(y + 1) * row_len];
        let at =
            |x: i64, c: usize| f64::from(src[x.rem_euclid(size as i64) as usize * channels + c]);
        for c in 0..channels {
            let mut sum: f64 = (-(r as i64)..=r as i64).map(|k| at(k, c)).sum();
            for x in 0..size {
                row[x * channels + c] = (sum / window) as f32;
                sum += at((x + r + 1) as i64, c) - at(x as i64 - r as i64, c);
            }
        }
    });
    out
}

fn transpose(input: &Image) -> Image {
    Image::generate(input.size, input.channels, |x, y, t| {
        t.copy_from_slice(input.texel(i64::from(y), i64::from(x)));
    })
}

/// Three separable box passes per axis, `radius` in tile units; zero radius is the identity.
pub fn blur(input: &Image, radius: f32) -> Image {
    blur_px(input, (radius * input.size as f32).round() as usize)
}

/// [`blur`] with the radius in texels.
pub fn blur_px(input: &Image, r: usize) -> Image {
    if r == 0 {
        return input.clone();
    }
    let mut image = input.clone();
    for _ in 0..3 {
        image = transpose(&box_rows(&transpose(&box_rows(&image, r)), r));
    }
    image
}

/// A glTF +Y-up tangent-space normal map from a gray height, 3×3 Sobel slopes with wrap.
pub fn normal_from_height(input: &Image, depth: f32) -> Image {
    let scale = input.size as f32 / 8.0 * depth;
    let h = |x: i64, y: i64| input.texel(x, y)[0];
    Image::generate(input.size, 3, |x, y, t| {
        let (xi, yi) = (i64::from(x), i64::from(y));
        let column = |dx: i64| h(xi + dx, yi - 1) + 2.0 * h(xi + dx, yi) + h(xi + dx, yi + 1);
        let row = |dy: i64| h(xi - 1, yi + dy) + 2.0 * h(xi, yi + dy) + h(xi + 1, yi + dy);
        let dhdx = (column(1) - column(-1)) * scale;
        let dhdrow = (row(1) - row(-1)) * scale;
        let (nx, ny, nz) = (-dhdx, dhdrow, 1.0f32);
        let inv = 1.0 / sqrt(nx * nx + ny * ny + nz * nz);
        t[0] = nx * inv * 0.5 + 0.5;
        t[1] = ny * inv * 0.5 + 0.5;
        t[2] = nz * inv * 0.5 + 0.5;
    })
}

/// Horizon-based occlusion of a gray height: 1 where open, toward `1 - strength` in pits.
///
/// Eight directions each look up to `radius` (tile units) out, at up to 16 samples, and take the
/// steepest rise over the centre texel; `depth` is the world height of value 1, as in
/// [`normal_from_height`].
pub fn ambient_occlusion(input: &Image, radius: f32, depth: f32, strength: f32) -> Image {
    let size = input.size as f32;
    let reach = radius * size;
    if reach < 1.0 {
        return Image::filled(input.size, 1, 1.0);
    }
    let samples = (reach.ceil() as usize).clamp(1, 16);
    Image::generate(input.size, 1, |x, y, t| {
        let centre = input.channel(x, y, 0);
        let (u, v) = ((x as f32 + 0.5) / size, (y as f32 + 0.5) / size);
        let mut total = 0.0f32;
        for (dx, dy) in GRADIENTS {
            let mut rise = 0.0f32;
            let mut at = [0.0f32];
            for k in 1..=samples {
                let step = reach * k as f32 / samples as f32;
                input.sample(u + step * dx / size, v + step * dy / size, &mut at);
                rise = rise.max((at[0] - centre) * depth * size / step);
            }
            total += rise / sqrt(1.0 + rise * rise);
        }
        t[0] = 1.0 - strength * (total / GRADIENTS.len() as f32);
    })
}

/// Convexity of a gray height: `0.5 + strength * (height - blur)`, clamped. Crests read above 0.5
/// and cavities below; `radius` is the blur radius in tile units, at least one texel.
pub fn curvature(input: &Image, radius: f32, strength: f32) -> Image {
    let soft = blur_px(
        input,
        ((radius * input.size as f32).round() as usize).max(1),
    );
    Image::generate(input.size, 1, |x, y, t| {
        let diff = input.channel(x, y, 0) - soft.channel(x, y, 0);
        t[0] = (0.5 + strength * diff).clamp(0.0, 1.0);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(size: u32) -> Image {
        Image::generate(size, 1, |x, y, t| {
            t[0] = if x == 4 && y == 4 { 1.0 } else { 0.0 }
        })
    }

    #[test]
    fn zero_radius_blur_is_identity() {
        let image = dot(16);
        assert_eq!(blur(&image, 0.0), image);
    }

    #[test]
    fn blur_conserves_energy_and_wraps() {
        let image = Image::generate(16, 1, |x, y, t| {
            t[0] = if x == 0 && y == 0 { 1.0 } else { 0.0 }
        });
        let out = blur(&image, 2.0 / 16.0);
        let sum: f32 = out.data.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4, "{sum}");
        assert!(out.texel(15, 15)[0] > 0.0, "energy wraps across the seam");
    }

    #[test]
    fn flat_height_gives_straight_up_normal() {
        let n = normal_from_height(&Image::filled(8, 1, 0.3), 0.05);
        assert!(n.data.chunks(3).all(|t| t == [0.5, 0.5, 1.0]));
    }

    #[test]
    fn normal_follows_gltf_y_up() {
        // Height falling toward larger x tilts the red channel above 0.5; rising toward the top
        // (smaller row index) tilts green below 0.5.
        let by_x = Image::generate(16, 1, |x, _, t| t[0] = 1.0 - x as f32 / 16.0);
        assert!(normal_from_height(&by_x, 0.5).texel(8, 8)[0] > 0.5);
        let by_row = Image::generate(16, 1, |_, y, t| t[0] = 1.0 - y as f32 / 16.0);
        assert!(normal_from_height(&by_row, 0.5).texel(8, 8)[1] < 0.5);
    }

    #[test]
    fn warp_by_neutral_gray_is_identity() {
        let image = dot(16);
        let out = warp(&image, &Image::filled(16, 1, 0.5), 0.3, 40.0);
        assert_eq!(out, image);
    }

    fn roll(image: &Image, dx: i64, dy: i64) -> Image {
        Image::generate(image.size, image.channels, |x, y, t| {
            t.copy_from_slice(image.texel(i64::from(x) - dx, i64::from(y) - dy));
        })
    }

    fn max_diff(a: &Image, b: &Image) -> f32 {
        a.data
            .iter()
            .zip(&b.data)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0, f32::max)
    }

    /// A smooth bump (positive height) centred at (32, 32) on a 64² tile.
    fn bump(sign: f32) -> Image {
        Image::generate(64, 1, |x, y, t| {
            let (dx, dy) = (x as f32 - 32.0, y as f32 - 32.0);
            t[0] = 0.5 + sign * 0.4 / (1.0 + (dx * dx + dy * dy) / 40.0);
        })
    }

    #[test]
    fn flat_height_is_fully_open_and_flat_curvature() {
        let flat = Image::filled(32, 1, 0.3);
        assert!(
            ambient_occlusion(&flat, 0.1, 0.1, 1.0)
                .data
                .iter()
                .all(|v| *v == 1.0)
        );
        assert!(curvature(&flat, 0.05, 4.0).data.iter().all(|v| *v == 0.5));
    }

    #[test]
    fn a_pit_is_occluded_at_its_centre() {
        let pit = bump(-1.0);
        let ao = ambient_occlusion(&pit, 0.15, 0.2, 1.0);
        assert!(ao.texel(32, 32)[0] < 0.9, "{}", ao.texel(32, 32)[0]);
        assert!(ao.texel(0, 0)[0] > ao.texel(32, 32)[0]);
    }

    #[test]
    fn a_crest_is_convex_and_its_foot_concave() {
        let ridge = Image::generate(64, 1, |x, _, t| {
            let d = (x as f32 - 32.0).abs();
            t[0] = (1.0 - d / 8.0).max(0.0);
        });
        let c = curvature(&ridge, 0.06, 4.0);
        assert!(c.texel(32, 5)[0] > 0.5, "{}", c.texel(32, 5)[0]);
        assert!(c.texel(24, 5)[0] < 0.5, "{}", c.texel(24, 5)[0]);
    }

    #[test]
    fn occlusion_and_curvature_commute_with_rolling() {
        let height = Image::generate(32, 1, |x, y, t| {
            t[0] = ((x * 7 + y * 13) % 11) as f32 / 11.0;
        });
        let rolled = roll(&height, 5, 9);
        let ao = ambient_occlusion(&height, 0.1, 0.1, 1.0);
        let ao_rolled = ambient_occlusion(&rolled, 0.1, 0.1, 1.0);
        assert!(max_diff(&roll(&ao, 5, 9), &ao_rolled) < 1e-4);
        let cv = curvature(&height, 0.05, 4.0);
        assert!(max_diff(&roll(&cv, 5, 9), &curvature(&rolled, 0.05, 4.0)) < 1e-6);
    }
}
