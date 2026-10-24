//! Neighbourhood ops: warp, blur and height-to-normal, all wrapping around the tile.

use ludion_core::math::{cos, sin, sqrt};
use ludion_core::par;

use crate::image::Image;

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
    let r = (radius * input.size as f32).round() as usize;
    if r == 0 {
        return input.clone();
    }
    let mut image = input.clone();
    for _ in 0..3 {
        image = transpose(&box_rows(&transpose(&box_rows(&image, r)), r));
    }
    image
}

/// A glTF +Y-up tangent-space normal map from a gray height, central differences with wrap.
pub fn normal_from_height(input: &Image, depth: f32) -> Image {
    let scale = input.size as f32 * 0.5 * depth;
    Image::generate(input.size, 3, |x, y, t| {
        let (xi, yi) = (i64::from(x), i64::from(y));
        let dhdx = (input.texel(xi + 1, yi)[0] - input.texel(xi - 1, yi)[0]) * scale;
        let dhdrow = (input.texel(xi, yi + 1)[0] - input.texel(xi, yi - 1)[0]) * scale;
        let (nx, ny, nz) = (-dhdx, dhdrow, 1.0f32);
        let inv = 1.0 / sqrt(nx * nx + ny * ny + nz * nz);
        t[0] = nx * inv * 0.5 + 0.5;
        t[1] = ny * inv * 0.5 + 0.5;
        t[2] = nz * inv * 0.5 + 0.5;
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
}
