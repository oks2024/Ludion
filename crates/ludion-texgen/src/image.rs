//! The evaluation image: interleaved `f32` samples, gray or linear color, sampled with wrap-around.

use ludion_core::par;

/// Rows evaluated per parallel chunk; fixed so chunking never depends on the thread count.
const ROWS_PER_CHUNK: usize = 16;

/// A square image of `channels` (1 gray or 3 linear color) `f32` samples per texel, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// Width and height in texels.
    pub size: u32,
    /// Samples per texel: 1 (gray) or 3 (color).
    pub channels: usize,
    /// `size * size * channels` samples.
    pub data: Vec<f32>,
}

impl Image {
    /// An image filled with `value` in every sample.
    pub fn filled(size: u32, channels: usize, value: f32) -> Image {
        Image {
            size,
            channels,
            data: vec![value; size as usize * size as usize * channels],
        }
    }

    /// Builds an image by calling `f(x, y, texel)` for every texel, rows in parallel.
    ///
    /// `f` must write only its `texel` slice; each chunk owns disjoint rows, so the result does not
    /// depend on the thread count.
    pub fn generate(size: u32, channels: usize, f: impl Fn(u32, u32, &mut [f32]) + Sync) -> Image {
        let mut image = Image::filled(size, channels, 0.0);
        let row_len = size as usize * channels;
        par::par_chunks(&mut image.data, ROWS_PER_CHUNK * row_len, |chunk, rows| {
            for (r, row) in rows.chunks_mut(row_len).enumerate() {
                let y = (chunk * ROWS_PER_CHUNK + r) as u32;
                for (x, texel) in row.chunks_mut(channels).enumerate() {
                    f(x as u32, y, texel);
                }
            }
        });
        image
    }

    /// The texel at `(x, y)`, wrapping both coordinates around the tile.
    pub fn texel(&self, x: i64, y: i64) -> &[f32] {
        let size = i64::from(self.size);
        let (x, y) = (x.rem_euclid(size) as usize, y.rem_euclid(size) as usize);
        let at = (y * self.size as usize + x) * self.channels;
        &self.data[at..at + self.channels]
    }

    /// Bilinear sample at tile coordinates `(u, v)` (texel centres at `(x + 0.5) / size`), wrapping.
    pub fn sample(&self, u: f32, v: f32, out: &mut [f32]) {
        let size = self.size as f32;
        let (px, py) = (u * size - 0.5, v * size - 0.5);
        let (fx0, fy0) = (px.floor(), py.floor());
        let (fx, fy) = (px - fx0, py - fy0);
        let (x0, y0) = (fx0 as i64, fy0 as i64);
        let (t00, t10) = (self.texel(x0, y0), self.texel(x0 + 1, y0));
        let (t01, t11) = (self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1));
        for (c, o) in out.iter_mut().enumerate() {
            let top = t00[c] + (t10[c] - t00[c]) * fx;
            let bottom = t01[c] + (t11[c] - t01[c]) * fx;
            *o = top + (bottom - top) * fy;
        }
    }

    /// An area average down to `size` texels a side; a clone when `size` is not smaller.
    ///
    /// Output texel `x` averages source columns `x*s/t .. (x+1)*s/t` (at least one), rows likewise,
    /// summed sequentially in `f64`.
    pub fn downsample(&self, size: u32) -> Image {
        if size >= self.size {
            return self.clone();
        }
        let (s, t) = (self.size as usize, size as usize);
        let bounds = |i: usize| {
            let lo = i * s / t;
            (lo, ((i + 1) * s / t).max(lo + 1))
        };
        Image::generate(size, self.channels, |x, y, texel| {
            let ((x0, x1), (y0, y1)) = (bounds(x as usize), bounds(y as usize));
            let count = ((x1 - x0) * (y1 - y0)) as f64;
            for (c, out) in texel.iter_mut().enumerate() {
                let mut sum = 0.0f64;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        sum += f64::from(self.data[(sy * s + sx) * self.channels + c]);
                    }
                }
                *out = (sum / count) as f32;
            }
        })
    }

    /// Sample `c` of the texel at `(x, y)`, broadcasting a gray image across color channels.
    pub fn channel(&self, x: u32, y: u32, c: usize) -> f32 {
        let at = (y as usize * self.size as usize + x as usize) * self.channels;
        self.data[at + if self.channels == 1 { 0 } else { c }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_visits_every_texel_once() {
        let image = Image::generate(40, 1, |x, y, t| t[0] = (y * 40 + x) as f32);
        for (i, v) in image.data.iter().enumerate() {
            assert_eq!(*v, i as f32);
        }
    }

    #[test]
    fn texel_wraps_both_axes() {
        let image = Image::generate(4, 1, |x, y, t| t[0] = (y * 4 + x) as f32);
        assert_eq!(image.texel(-1, -1)[0], 15.0);
        assert_eq!(image.texel(4, 5)[0], 4.0);
    }

    #[test]
    fn downsample_averages_blocks_exactly() {
        let image = Image::generate(4, 1, |x, y, t| t[0] = (y * 4 + x) as f32);
        let small = image.downsample(2);
        assert_eq!(small.data, vec![2.5, 4.5, 10.5, 12.5]);
        assert_eq!(image.downsample(8), image);
    }

    #[test]
    fn sample_at_texel_centre_is_exact_and_blends_between() {
        let image = Image::generate(4, 1, |x, _, t| t[0] = x as f32);
        let mut out = [0.0];
        image.sample(1.5 / 4.0, 0.5, &mut out);
        assert_eq!(out[0], 1.0);
        image.sample(2.0 / 4.0, 0.5, &mut out);
        assert_eq!(out[0], 1.5);
    }
}
