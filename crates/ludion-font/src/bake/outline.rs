//! Glyph outlines: unscaled drawing and fixed-count curve flattening.

use skrifa::GlyphId;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlineGlyphCollection, OutlinePen};

/// Segments per quadratic curve. A constant, never adaptive, so the polyline is identical on every
/// machine.
const QUAD_STEPS: u32 = 8;
/// Segments per cubic curve.
const CUBIC_STEPS: u32 = 12;

#[derive(Default)]
struct Flattener {
    contours: Vec<Vec<[f32; 2]>>,
    current: Vec<[f32; 2]>,
}

impl Flattener {
    fn last(&self) -> [f32; 2] {
        self.current.last().copied().unwrap_or([0.0, 0.0])
    }

    fn finish(&mut self) {
        let mut contour = std::mem::take(&mut self.current);
        if contour.len() > 1 && contour.first() == contour.last() {
            contour.pop();
        }
        if contour.len() >= 3 {
            self.contours.push(contour);
        }
    }
}

impl OutlinePen for Flattener {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.current.push([x, y]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push([x, y]);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let [x0, y0] = self.last();
        for i in 1..QUAD_STEPS {
            let t = i as f32 / QUAD_STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c) = (u * u, 2.0 * u * t, t * t);
            self.current
                .push([a * x0 + b * cx + c * x, a * y0 + b * cy + c * y]);
        }
        self.current.push([x, y]);
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let [x0, y0] = self.last();
        for i in 1..CUBIC_STEPS {
            let t = i as f32 / CUBIC_STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.current.push([
                a * x0 + b * c0x + c * c1x + d * x,
                a * y0 + b * c0y + c * c1y + d * y,
            ]);
        }
        self.current.push([x, y]);
    }

    fn close(&mut self) {
        self.finish();
    }
}

/// Draws glyph `gid` unscaled and unhinted into closed polylines in font units. A glyph with no
/// outline (a space) yields no contours.
pub(super) fn extract(
    outlines: &OutlineGlyphCollection<'_>,
    gid: GlyphId,
) -> Result<Vec<Vec<[f32; 2]>>, String> {
    let Some(glyph) = outlines.get(gid) else {
        return Ok(Vec::new());
    };
    let mut pen = Flattener::default();
    glyph
        .draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut pen,
        )
        .map_err(|e| e.to_string())?;
    pen.finish();
    Ok(pen.contours)
}

/// The bounding box of `contours`, rounded outward (floor min, ceil max).
pub(super) fn bounds(contours: &[Vec<[f32; 2]>]) -> Option<[i32; 4]> {
    let mut points = contours.iter().flatten();
    let first = points.next()?;
    let (mut min, mut max) = (*first, *first);
    for p in points {
        for axis in 0..2 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    Some([
        min[0].floor() as i32,
        min[1].floor() as i32,
        max[0].ceil() as i32,
        max[1].ceil() as i32,
    ])
}

/// Whether a cmap entry names a real glyph: not `.notdef`, and inside the face's glyph count.
pub(super) fn is_real_glyph(gid: GlyphId, glyph_count: u32) -> bool {
    gid != GlyphId::NOTDEF && gid.to_u32() < glyph_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_id_past_the_glyph_count_is_not_real() {
        assert!(!is_real_glyph(GlyphId::NOTDEF, 10));
        assert!(is_real_glyph(GlyphId::new(9), 10));
        assert!(!is_real_glyph(GlyphId::new(10), 10));
    }
}
