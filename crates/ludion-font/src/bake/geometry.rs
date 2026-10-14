//! Integer texel-box arithmetic: where a glyph's ink lands in the atlas and what `bounds` the
//! document records so layout places the quad over exactly that rect.

use super::pack::MAX_SIDE;

/// `floor(a / b)` for `b > 0`.
pub(super) fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// `ceil(a / b)` for `b > 0`.
pub(super) fn ceil_div(a: i64, b: i64) -> i64 {
    -(-a).div_euclid(b)
}

/// `a / b` rounded half away from zero, for `b > 0`.
pub(super) fn round_div(a: i64, b: i64) -> i64 {
    if a >= 0 {
        (2 * a + b) / (2 * b)
    } else {
        -((-2 * a + b) / (2 * b))
    }
}

/// A glyph's texel box in the baked pixel grid: x right, y up, baseline at `y = 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TexelBox {
    pub px0: i32,
    pub py0: i32,
    pub px1: i32,
    pub py1: i32,
}

impl TexelBox {
    /// The box of `bounds` (font units) at `em_px`, grown by `pad` texels on every side. `Ok(None)`
    /// when the box has no area; `Err([w, h])` when it is wider or taller than the atlas allows.
    pub fn new(
        bounds: [i32; 4],
        upem: u32,
        em_px: u32,
        pad: u32,
    ) -> Result<Option<Self>, [i64; 2]> {
        let (upem, em) = (i64::from(upem), i64::from(em_px));
        let pad = i64::from(pad);
        let lo = |v: i32| floor_div(i64::from(v) * em, upem) - pad;
        let hi = |v: i32| ceil_div(i64::from(v) * em, upem) + pad;
        let (x0, y0, x1, y1) = (lo(bounds[0]), lo(bounds[1]), hi(bounds[2]), hi(bounds[3]));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let max = i64::from(MAX_SIDE);
        if w > max || h > max {
            return Err([w, h]);
        }
        if w <= 0 || h <= 0 {
            return Ok(None);
        }
        // Within the side limit the coordinates fit i32 unless the box sits absurdly far from the
        // origin, which is as unusable as an oversize one.
        let fit = |v: i64| i32::try_from(v).map_err(|_| [w, h]);
        Ok(Some(Self {
            px0: fit(x0)?,
            py0: fit(y0)?,
            px1: fit(x1)?,
            py1: fit(y1)?,
        }))
    }

    pub fn width(&self) -> u32 {
        (i64::from(self.px1) - i64::from(self.px0)) as u32
    }

    pub fn height(&self) -> u32 {
        (i64::from(self.py1) - i64::from(self.py0)) as u32
    }

    /// The box mapped back to font units, for the document's `bounds`.
    pub fn font_bounds(&self, upem: u32, em_px: u32) -> [i32; 4] {
        let (upem, em) = (i64::from(upem), i64::from(em_px));
        let back = |v: i32| round_div(i64::from(v) * upem, em) as i32;
        [
            back(self.px0),
            back(self.py0),
            back(self.px1),
            back(self.py1),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_rounds_correctly_on_negatives() {
        assert_eq!(floor_div(-7, 2), -4);
        assert_eq!(floor_div(7, 2), 3);
        assert_eq!(ceil_div(-7, 2), -3);
        assert_eq!(ceil_div(7, 2), 4);
        assert_eq!(ceil_div(-6, 2), -3);
        assert_eq!(round_div(-5, 2), -3);
        assert_eq!(round_div(5, 2), 3);
        assert_eq!(round_div(-4, 3), -1);
        assert_eq!(round_div(0, 3), 0);
    }

    #[test]
    fn box_grows_outward_and_maps_back_within_half_a_unit() {
        // upem 2048 is not divisible by em_px 56.
        let b = TexelBox::new([-100, -30, 900, 1500], 2048, 56, 6)
            .unwrap()
            .unwrap();
        assert_eq!(b.px0, floor_div(-5600, 2048) as i32 - 6);
        assert_eq!(b.py1, ceil_div(1500 * 56, 2048) as i32 + 6);
        let back = b.font_bounds(2048, 56);
        let exact = [b.px0, b.py0, b.px1, b.py1];
        for (b, t) in back.iter().zip(exact) {
            let want = f64::from(t) * 2048.0 / 56.0;
            assert!((f64::from(*b) - want).abs() <= 0.5, "{b} vs {want}");
        }
    }

    #[test]
    fn an_empty_box_is_none() {
        assert_eq!(TexelBox::new([0, 0, 0, 10], 1000, 8, 0), Ok(None));
    }

    #[test]
    fn an_oversize_box_is_an_error_not_an_overflow() {
        let huge = TexelBox::new([i32::MIN, 0, i32::MAX, 10], 1, u32::MAX, 0);
        assert!(huge.is_err());
    }
}
