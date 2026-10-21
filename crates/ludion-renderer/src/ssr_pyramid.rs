//! The screen-space reflection trace's two input pyramids, built only when `PostChain::ssr` is on.
//!
//! - `hiz`: an `R32Float` **min-Z** pyramid of the scene depth. Depth is standard-Z (far = 1.0), and
//!   the trace may skip a cell only when the ray is in front of the *closest* surface in it, so each
//!   cell holds the per-cell minimum. The culling HZB (`gpu-driven-culling.md` rung 3b) needs the
//!   farthest depth instead and is built from its own occluder pre-pass; it reuses this machinery
//!   (the format, the footprint snippet, the reduce shader with `max`), not this instance.
//! - `ssr_color`: an `Rgba16Float` pyramid of the opaque scene, before `reflections` adds into it.
//!   Mip 0 is the sharp mirror level; each mip is a 4×4 tent over the one above.
//!
//! **Footprint rule** (`shaders/snippets/footprint.wgsl`): a child texel `i` covers source
//! `[i·r, (i+1)·r)` with `r = source / target` per axis and touches texels `floor(lo) ..= ceil(hi) - 1`,
//! clamped. It is conservative for odd sizes, where a floor-halving would drop the last row or column
//! of an odd parent. One rule serves the seeds and every Hi-Z reduce.
//!
//! **Blur link**: one `[1,3,3,1]/8` tent has variance 0.75 parent-texel²; stacked, σ_m ≈ 0.5·2^m base
//! pixels, so ±σ spans 2^m pixels — the cone diameter `lobe::pyramid_level` turns into level m.

use crate::graph::FrameSize;

/// The pyramids' base extent: the output extent × `ssr_scale` ([`QualityParams::ssr_scale`](crate::QualityParams::ssr_scale)),
/// rounded, at least 1.
pub(crate) fn trace_extent(size: FrameSize, ssr_scale: f32) -> (u32, u32) {
    let scale = |n: u32| ((n as f32 * ssr_scale).round() as u32).max(1);
    (scale(size.output.0), scale(size.output.1))
}

/// The full mip chain length for `extent`, the same formula as `TextureDesc::with_mip_levels`.
pub(crate) fn levels((w, h): (u32, u32)) -> u32 {
    32 - w.max(h).leading_zeros()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A child texel's source interval and the (unclamped) texels it touches, as the WGSL computes
    /// them.
    fn footprint(i: u32, r: f32) -> (f32, f32, i32, i32) {
        let lo = i as f32 * r;
        let hi = lo + r;
        (lo, hi, lo.floor() as i32, hi.ceil() as i32 - 1)
    }

    fn at(x: i32, n: u32) -> usize {
        x.clamp(0, n as i32 - 1) as usize
    }

    /// `hiz_reduce.wgsl` / `hiz_seed.wgsl` on a `sw`×`sh` grid into `dw`×`dh`.
    pub(crate) fn hiz_reduce(src: &[f32], (sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> Vec<f32> {
        let (rx, ry) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
        let mut out = Vec::new();
        for y in 0..dh {
            let (_, _, y0, y1) = footprint(y, ry);
            for x in 0..dw {
                let (_, _, x0, x1) = footprint(x, rx);
                let mut m = 1.0f32;
                for ty in y0..=y1 {
                    for tx in x0..=x1 {
                        m = m.min(src[at(ty, sh) * sw as usize + at(tx, sw)]);
                    }
                }
                out.push(m);
            }
        }
        out
    }

    fn hdr_safe(c: f32) -> f32 {
        if c.is_nan() {
            0.0
        } else {
            c.clamp(0.0, 60000.0)
        }
    }

    /// `ssr_color.wgsl` with `reduce == 0`: the area-weighted box plus `hdr_safe`.
    pub(crate) fn color_seed(
        src: &[[f32; 4]],
        (sw, sh): (u32, u32),
        (dw, dh): (u32, u32),
    ) -> Vec<[f32; 4]> {
        let (rx, ry) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
        let mut out = Vec::new();
        for y in 0..dh {
            let (ly, hy, y0, y1) = footprint(y, ry);
            for x in 0..dw {
                let (lx, hx, x0, x1) = footprint(x, rx);
                let (mut acc, mut total) = ([0.0f32; 3], 0.0f32);
                for ty in y0..=y1 {
                    let wy = ((hy.min((ty + 1) as f32)) - ly.max(ty as f32)).max(0.0);
                    for tx in x0..=x1 {
                        let wx = ((hx.min((tx + 1) as f32)) - lx.max(tx as f32)).max(0.0);
                        let t = src[at(ty, sh) * sw as usize + at(tx, sw)];
                        for c in 0..3 {
                            acc[c] += hdr_safe(t[c]) * wx * wy;
                        }
                        total += wx * wy;
                    }
                }
                out.push([acc[0] / total, acc[1] / total, acc[2] / total, 1.0]);
            }
        }
        out
    }

    /// `ssr_color.wgsl` with `reduce == 1`: the separable `[1,3,3,1]/8` tent.
    pub(crate) fn color_reduce(
        src: &[[f32; 4]],
        (sw, sh): (u32, u32),
        (dw, dh): (u32, u32),
    ) -> Vec<[f32; 4]> {
        let (rx, ry) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
        let tap = [0.125f32, 0.375, 0.375, 0.125];
        let base = |i: u32, r: f32| ((i as f32 + 0.5) * r - 0.5).floor() as i32 - 1;
        let mut out = Vec::new();
        for y in 0..dh {
            for x in 0..dw {
                let mut acc = [0.0f32; 3];
                for (j, wj) in tap.iter().enumerate() {
                    for (i, wi) in tap.iter().enumerate() {
                        let t = src[at(base(y, ry) + j as i32, sh) * sw as usize
                            + at(base(x, rx) + i as i32, sw)];
                        for c in 0..3 {
                            acc[c] += t[c] * wi * wj;
                        }
                    }
                }
                out.push([acc[0], acc[1], acc[2], 1.0]);
            }
        }
        out
    }

    #[test]
    fn trace_extent_is_the_scaled_output_never_the_render_extent() {
        let native = FrameSize::native(1920, 1080);
        assert_eq!(trace_extent(native, 0.5), (960, 540));
        assert_eq!(trace_extent(native, 0.25), (480, 270));
        let ssaa = FrameSize::scaled(1920, 1080, crate::RenderScale::new(2.0).unwrap());
        assert_eq!(trace_extent(ssaa, 0.5), (960, 540));
        assert_eq!(trace_extent(FrameSize::native(1, 1), 0.5), (1, 1));
        assert_eq!(levels((960, 540)), 10);
        assert_eq!(levels((1, 1)), 1);
    }

    /// (a) Every source texel lies in some child's footprint, for floor-halving and a spread of
    /// ratios — the rule is conservative.
    #[test]
    fn every_source_texel_is_covered_by_some_footprint() {
        for src in 1..=33u32 {
            let mut targets = vec![(src / 2).max(1)];
            for ratio in [0.5f32, 1.0, 1.5, 2.0, 3.0] {
                targets.push(((src as f32 / ratio).round() as u32).max(1));
            }
            for dst in targets {
                let r = src as f32 / dst as f32;
                let mut covered = vec![false; src as usize];
                for i in 0..dst {
                    let (_, _, first, last) = footprint(i, r);
                    for t in first..=last {
                        covered[at(t, src)] = true;
                    }
                }
                assert!(
                    covered.iter().all(|&c| c),
                    "src {src} -> dst {dst}: {covered:?}"
                );
            }
        }
    }

    /// (b) An impulse seeded at ratio 1 then tent-reduced m times has σ ≈ 0.5·2^m base pixels.
    #[test]
    fn stacked_tents_blur_by_half_a_pixel_per_level_width() {
        for x0 in 20..44usize {
            let mut img = vec![[0.0f32; 4]; 64];
            img[x0] = [1.0, 1.0, 1.0, 1.0];
            let mut cur = color_seed(&img, (64, 1), (64, 1));
            let mut w = 64u32;
            for m in 1..=4u32 {
                let next = (w / 2).max(1);
                cur = color_reduce(&cur, (w, 1), (next, 1));
                w = next;
                if m < 2 {
                    continue;
                }
                let scale = (1u32 << m) as f32;
                let total: f32 = cur.iter().map(|t| t[0]).sum();
                let pos = |j: usize| (j as f32 + 0.5) * scale;
                let mean: f32 = cur
                    .iter()
                    .enumerate()
                    .map(|(j, t)| t[0] * pos(j))
                    .sum::<f32>()
                    / total;
                let var: f32 = cur
                    .iter()
                    .enumerate()
                    .map(|(j, t)| t[0] * (pos(j) - mean).powi(2))
                    .sum::<f32>()
                    / total;
                let ratio = var.sqrt() / scale;
                assert!(
                    (0.45..=0.55).contains(&ratio),
                    "x0 {x0}, m {m}: sigma / 2^m = {ratio}"
                );
            }
        }
    }

    /// (c) A known grid reduces to the hand-computed minima (7×5 → 3×2, ratios 7/3 and 5/2).
    #[test]
    fn hiz_reduce_matches_hand_computed_minima() {
        #[rustfmt::skip]
        let grid = [
            0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3,
            0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2,
            0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1,
            0.6, 0.5, 0.4, 0.3, 0.2, 0.1, 0.05,
            0.5, 0.4, 0.3, 0.2, 0.1, 0.05, 0.9,
        ];
        // x ranges: [0,2], [2,4], [4,6]; y ranges: [0,2], [2,4] (rows 0..=2 and 2..=4).
        assert_eq!(
            hiz_reduce(&grid, (7, 5), (3, 2)),
            [0.5, 0.3, 0.1, 0.3, 0.1, 0.05]
        );
    }
}
