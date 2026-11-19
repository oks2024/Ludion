//! The screen-space UI text vertex stream: the packed vertex, its attribute table, and the glyph-quad
//! packing (including the outline's quad growth) that `ui_text.wgsl` reads.

use super::{ui_clip_raw, ui_cos_sin};
use crate::scene::{GlyphOutline, UiClip, UiRotation};
use bytemuck::{Pod, Zeroable};
use ludion_font::GlyphQuad;

/// One screen-space UI text vertex: a pixel-space position, an atlas UV, a linear RGBA color, the
/// SDF flag (0.0 bitmap, 1.0 signed-distance atlas), the packed clip and the outline (colour, the
/// glyph's original uv rect, and `[on, sdf edge or bitmap radius, 0, 0]`; all zero when unoutlined).
/// Matches `ui_text.wgsl`'s `VsIn`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct UiTextVertexRaw {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    sdf: f32,
    clip_a: [f32; 4],
    clip_b: [f32; 4],
    clip_r: [f32; 4],
    ol_color: [f32; 4],
    ol_rect: [f32; 4],
    ol_param: [f32; 4],
    clip_t: [f32; 4],
}

/// UI-text per-vertex attributes (`UiTextVertexRaw`: position/uv/color/sdf/clip a, b, r/outline colour, rect, param/clip t), locations 0-10.
pub(super) const UI_TEXT_VERTEX_ATTRS: [wgpu::VertexAttribute; 11] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4];

/// Appends one screen-space text run's glyph quads (6 vertices each) to `verts`, in pixel space with
/// a top-left origin and **+Y down**. The quads come from `ludion_font::layout` via extract — the
/// same layout `ludion_engine::ui::measure_ui_text` measures, so a block measured/centered
/// engine-side draws exactly where the resolve function said it would.
///
/// An `outline` (with the face's SDF spread and the atlas size in texels) rides every vertex. An SDF
/// quad already carries the spread as padding and is not grown; a bitmap quad grows by the whole-texel
/// radius per side, and every vertex carries the original uv rect so the shader masks out neighbours.
///
/// A `rotation` with `radians != 0` turns every corner about its pivot here, on the CPU; an unrotated
/// run's positions are untouched.
pub(super) fn push_ui_text_verts(
    verts: &mut Vec<UiTextVertexRaw>,
    quads: &[GlyphQuad],
    color: [f32; 4],
    sdf: bool,
    clip: &UiClip,
    rotation: &UiRotation,
    outline: Option<(&GlyphOutline, u32, [u32; 2])>,
) {
    let [clip_a, clip_b, clip_r, clip_t] = ui_clip_raw(clip);
    let turn = (rotation.radians != 0.0).then(|| ui_cos_sin(rotation.radians));
    let place = |x: f32, y: f32| match turn {
        None => [x, y],
        Some((cos, sin)) => {
            let [px, py] = rotation.pivot_px;
            let (dx, dy) = (x - px, y - py);
            [px + cos * dx - sin * dy, py + sin * dx + cos * dy]
        }
    };
    let (ol_color, ol_param) = match outline {
        None => ([0.0; 4], [0.0; 4]),
        Some((o, spread_px, _)) => {
            let edge = if sdf {
                0.5 - o.texels / (2.0 * spread_px.max(1) as f32)
            } else {
                o.texels
            };
            (o.color, [1.0, edge, 0.0, 0.0])
        }
    };
    for q in quads {
        let [mut l, mut t, mut r, mut b] = q.pos;
        let [mut u0, mut v0, mut u1, mut v1] = q.uv;
        let mut ol_rect = [0.0; 4];
        if let Some((o, _, dims)) = outline {
            ol_rect = [u0.min(u1), v0.min(v1), u0.max(u1), v0.max(v1)];
            if !sdf {
                // Grow one axis outward by `texels`: `d` is the uv step (signed like the span), and
                // the same step scaled by the quad's pixels per uv unit is the position step.
                let grow = |lo: &mut f32, hi: &mut f32, p_lo: &mut f32, p_hi: &mut f32, n: u32| {
                    let span = *hi - *lo;
                    if span == 0.0 || n == 0 {
                        return;
                    }
                    let d = (o.texels / n as f32).copysign(span);
                    let px = d * (*p_hi - *p_lo) / span;
                    *lo -= d;
                    *hi += d;
                    *p_lo -= px;
                    *p_hi += px;
                };
                grow(&mut u0, &mut u1, &mut l, &mut r, dims[0]);
                grow(&mut v0, &mut v1, &mut t, &mut b, dims[1]);
            }
        }
        let vert = |x: f32, y: f32, u: f32, v: f32| UiTextVertexRaw {
            position: place(x, y),
            uv: [u, v],
            color,
            sdf: f32::from(sdf),
            clip_a,
            clip_b,
            clip_r,
            ol_color,
            ol_rect,
            ol_param,
            clip_t,
        };
        let tl = vert(l, t, u0, v0);
        let tr = vert(r, t, u1, v0);
        let br = vert(r, b, u1, v1);
        let bl = vert(l, b, u0, v1);
        verts.extend_from_slice(&[tl, tr, br, tl, br, bl]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screen-space quads for `text` at `cell` px from the origin, on the embedded face.
    fn test_quads(text: &str, cell: f32) -> Vec<GlyphQuad> {
        let face = ludion_font::FontFace::embedded();
        ludion_font::layout(face, text, cell, 0.0).quads([0.0, 0.0], cell, ludion_font::YAxis::Down)
    }

    /// The bounding box of the glyph quads `push_ui_text_verts` emits equals the extent
    /// `ludion_engine::ui::measure_ui_text` computes for the same string — `[longest_line_glyphs *
    /// cell, line_count * cell]` from the origin. This is the cross-crate layout contract (the two
    /// crates can't share a test — `ludion-renderer` is a dependency of `ludion-engine` — so both
    /// encode the same formula and this pins the renderer half). No GPU needed.
    #[test]
    fn ui_text_verts_bounding_box_matches_the_measure_formula() {
        let cell = 8.0_f32;
        for (text, glyphs, lines) in [("HI", 2, 1), ("HELLO", 5, 1), ("HI\nTHERE\nYOU", 5, 3)] {
            let mut verts = Vec::new();
            let quads = test_quads(text, cell);
            push_ui_text_verts(
                &mut verts,
                &quads,
                [1.0; 4],
                false,
                &UiClip::NONE,
                &UiRotation::NONE,
                None,
            );
            let (mut max_x, mut max_y) = (0.0_f32, 0.0_f32);
            for v in &verts {
                max_x = max_x.max(v.position[0]);
                max_y = max_y.max(v.position[1]);
            }
            assert_eq!(
                [max_x, max_y],
                [glyphs as f32 * cell, lines as f32 * cell],
                "extent of {text:?}",
            );
        }
    }

    /// An empty string emits no vertices, and an **unmapped** code point (no atlas cell) is skipped
    /// entirely — matching the world-text path. (A mapped-but-blank glyph like space still emits a
    /// transparent quad, exactly as `build_text_draw` does; only glyphs with no cell are dropped.)
    #[test]
    fn ui_text_verts_skip_empty_and_unmapped() {
        let mut verts = Vec::new();
        push_ui_text_verts(
            &mut verts,
            &test_quads("", 8.0),
            [1.0; 4],
            false,
            &UiClip::NONE,
            &UiRotation::NONE,
            None,
        );
        assert!(verts.is_empty(), "empty string emits nothing");
        // 'é' has no glyph in the embedded face → no quad.
        push_ui_text_verts(
            &mut verts,
            &test_quads("ééé", 8.0),
            [1.0; 4],
            false,
            &UiClip::NONE,
            &UiRotation::NONE,
            None,
        );
        assert!(verts.is_empty(), "unmapped code points emit no quads");
    }

    /// Unoutlined text packs as before with zeroed outline fields; an outlined bitmap quad grows by
    /// `k` texels in uv and pixels and carries the original rect; an outlined SDF quad is not grown
    /// and carries the edge `0.5 - texels / (2 * spread)`.
    #[test]
    fn ui_text_verts_pack_the_outline() {
        let quads = test_quads("H", 8.0);
        let mut plain = Vec::new();
        push_ui_text_verts(
            &mut plain,
            &quads,
            [1.0; 4],
            false,
            &UiClip::NONE,
            &UiRotation::NONE,
            None,
        );
        assert!(
            plain
                .iter()
                .all(|v| v.ol_param == [0.0; 4] && v.ol_color == [0.0; 4] && v.ol_rect == [0.0; 4])
        );

        let outline = GlyphOutline {
            color: [1.0, 0.0, 0.0, 1.0],
            texels: 2.0,
        };
        let dims = [128, 64];
        let mut grown = Vec::new();
        push_ui_text_verts(
            &mut grown,
            &quads,
            [1.0; 4],
            false,
            &UiClip::NONE,
            &UiRotation::NONE,
            Some((&outline, 0, dims)),
        );
        let q = &quads[0];
        let (du, dv) = (2.0 / 128.0, 2.0 / 64.0);
        let (sx, sy) = (
            (q.pos[2] - q.pos[0]) / (q.uv[2] - q.uv[0]),
            (q.pos[3] - q.pos[1]) / (q.uv[3] - q.uv[1]),
        );
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let tl = &grown[0];
        assert!(near(tl.uv[0], q.uv[0] - du) && near(tl.uv[1], q.uv[1] - dv));
        assert!(near(tl.position[0], q.pos[0] - du * sx));
        assert!(near(tl.position[1], q.pos[1] - dv * sy));
        assert_eq!(tl.ol_rect, [q.uv[0], q.uv[1], q.uv[2], q.uv[3]]);
        assert_eq!(tl.ol_param, [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(tl.ol_color, outline.color);

        let mut sdf = Vec::new();
        push_ui_text_verts(
            &mut sdf,
            &quads,
            [1.0; 4],
            true,
            &UiClip::NONE,
            &UiRotation::NONE,
            Some((&outline, 8, dims)),
        );
        assert_eq!(sdf[0].uv, q.uv[..2]);
        assert_eq!(sdf[0].position, [q.pos[0], q.pos[1]]);
        assert_eq!(sdf[0].ol_param, [1.0, 0.5 - 2.0 / 16.0, 0.0, 0.0]);
    }

    /// An unrotated run keeps the axis-aligned corners and `clip_t = [1, 0, 1, 0]`; a 90° rotation
    /// about a pivot (clockwise on the +Y-down screen) moves the corners to the hand-computed points.
    #[test]
    fn ui_text_verts_rotate_about_the_pivot() {
        let quads = test_quads("H", 8.0);
        let q = &quads[0];
        let mut plain = Vec::new();
        let push = |verts: &mut Vec<UiTextVertexRaw>, rotation: &UiRotation| {
            push_ui_text_verts(
                verts,
                &quads,
                [1.0; 4],
                false,
                &UiClip::NONE,
                rotation,
                None,
            );
        };
        push(&mut plain, &UiRotation::NONE);
        assert_eq!(plain[0].position, [q.pos[0], q.pos[1]]);
        assert_eq!(plain[1].position, [q.pos[2], q.pos[1]]);
        assert_eq!(plain[0].clip_t, [1.0, 0.0, 1.0, 0.0]);

        let pivot = [10.0, 10.0];
        let mut turned = Vec::new();
        push(
            &mut turned,
            &UiRotation {
                radians: std::f32::consts::FRAC_PI_2,
                pivot_px: pivot,
            },
        );
        // R(90°)(x, y) = (-y, x): the top-left corner offset (dx, dy) lands at (-dy, dx).
        let want = |x: f32, y: f32| [pivot[0] - (y - pivot[1]), pivot[1] + (x - pivot[0])];
        let near =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(near(turned[0].position, want(q.pos[0], q.pos[1])));
        assert!(near(turned[1].position, want(q.pos[2], q.pos[1])));
        assert!(near(turned[2].position, want(q.pos[2], q.pos[3])));
    }
}
