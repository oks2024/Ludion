//! The coverage rasterizer: signed-area accumulation (font-rs style) over the flattened outline.
//! Segments and rows run in a fixed order with `+ - * /` only, so the bytes are reproducible.

use super::geometry::TexelBox;

/// One glyph's coverage bytes, row-major, top row first.
pub(super) fn coverage_glyph(
    contours: &[Vec<[f32; 2]>],
    tbox: &TexelBox,
    em_px: u32,
    upem: u32,
) -> Vec<u8> {
    let (w, h) = (tbox.width() as usize, tbox.height() as usize);
    let scale = em_px as f32 / upem as f32;
    let mut acc = Accumulator::new(w, h);
    for contour in contours {
        let to_px = |p: [f32; 2]| {
            [
                p[0] * scale - tbox.px0 as f32,
                tbox.py1 as f32 - p[1] * scale,
            ]
        };
        let Some(&last) = contour.last() else {
            continue;
        };
        let mut a = to_px(last);
        for &p in contour {
            let b = to_px(p);
            acc.line(a, b);
            a = b;
        }
    }
    acc.finish()
}

struct Accumulator {
    w: usize,
    h: usize,
    stride: usize,
    area: Vec<f32>,
}

impl Accumulator {
    fn new(w: usize, h: usize) -> Self {
        let stride = w + 2;
        Self {
            w,
            h,
            stride,
            area: vec![0.0; stride * h],
        }
    }

    fn line(&mut self, p0: [f32; 2], p1: [f32; 2]) {
        if p0[1] == p1[1] {
            return;
        }
        let (dir, p0, p1) = if p0[1] < p1[1] {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let dxdy = (p1[0] - p0[0]) / (p1[1] - p0[1]);
        let mut x = p0[0];
        let top = p0[1].max(0.0);
        if p0[1] < 0.0 {
            x -= p0[1] * dxdy;
        }
        let wf = self.w as f32;
        let y_first = top as usize;
        let y_last = (p1[1].ceil().max(0.0) as usize).min(self.h);
        for y in y_first..y_last {
            let dy = ((y + 1) as f32).min(p1[1]) - (y as f32).max(p0[1]);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (lo, hi) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let (x0, x1) = (lo.clamp(0.0, wf), hi.clamp(0.0, wf));
            self.row(y, x0, x1, d);
            x = xnext;
        }
    }

    fn row(&mut self, y: usize, x0: f32, x1: f32, d: f32) {
        let base = y * self.stride;
        let a = &mut self.area[base..base + self.stride];
        let x0floor = x0.floor();
        let x0i = x0floor as usize;
        let x1ceil = x1.ceil();
        let x1i = x1ceil as usize;
        if x1i <= x0i + 1 {
            let xmf = 0.5 * (x0 + x1) - x0floor;
            a[x0i] += d - d * xmf;
            a[x0i + 1] += d * xmf;
        } else {
            let s = 1.0 / (x1 - x0);
            let x0f = x0 - x0floor;
            let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
            let x1f = x1 - x1ceil + 1.0;
            let am = 0.5 * s * x1f * x1f;
            a[x0i] += d * a0;
            if x1i == x0i + 2 {
                a[x0i + 1] += d * (1.0 - a0 - am);
            } else {
                let a1 = s * (1.5 - x0f);
                a[x0i + 1] += d * (a1 - a0);
                for cell in &mut a[x0i + 2..x1i - 1] {
                    *cell += d * s;
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                a[x1i - 1] += d * (1.0 - a2 - am);
            }
            a[x1i] += d * am;
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h);
        for row in self.area.chunks(self.stride) {
            let mut sum = 0.0f32;
            for &cell in &row[..self.w] {
                sum += cell;
                out.push((sum.abs().min(1.0) * 255.0 + 0.5) as u8);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Vec<[f32; 2]>> {
        vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]]
    }

    // upem == em_px, so font units are pixels.
    fn render(c: &[Vec<[f32; 2]>], b: [i32; 4]) -> (Vec<u8>, usize) {
        let tb = TexelBox::new(b, 8, 8, 0).unwrap().unwrap();
        (coverage_glyph(c, &tb, 8, 8), tb.width() as usize)
    }

    #[test]
    fn a_pixel_aligned_square_is_exactly_binary() {
        let (px, w) = render(&rect(1.0, 1.0, 4.0, 5.0), [1, 1, 4, 5]);
        assert_eq!(w, 3);
        assert!(px.iter().all(|&b| b == 255), "{px:?}");
        let (px, _) = render(&rect(1.0, 1.0, 4.0, 5.0), [0, 0, 5, 6]);
        let inside = px.iter().filter(|&&b| b == 255).count();
        assert_eq!(inside, 12);
        assert!(px.iter().all(|&b| b == 0 || b == 255));
    }

    #[test]
    fn a_half_pixel_edge_is_half_covered() {
        let (px, w) = render(&rect(1.5, 1.0, 4.0, 5.0), [1, 1, 4, 5]);
        assert!(px[0].abs_diff(128) <= 1, "{}", px[0]);
        assert_eq!(px[1], 255);
        assert_eq!(px.len(), w * 4);
    }
}
