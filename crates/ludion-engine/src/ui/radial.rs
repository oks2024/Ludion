//! [`radial_layout`] and [`radial_hit`]: the pixel geometry of a [`UiRadial`], and what `extract`
//! draws of it (`Plan/ui-hud.md` decisions 42–43).
//!
//! Both helpers are pure and ungrouped, like [`list_hit`](super::list_hit): a wheel inside a
//! `UiGroup` is placed by the group, which only [`ui_hit`](super::ui_hit) resolves. The trigonometry
//! is `ludion_core::math` (pure-Rust libm), the same on every platform.

use ludion_core::math;
use ludion_font::{GlyphQuad, YAxis, fold_line_breaks, layout};
use ludion_renderer::{FontDraw, GlyphOutline, UiClip, UiInstance, UiRotation};

use super::{FontFace, pixel_scale, resolve_ui_rect};
use crate::components::{UiRadial, UiRadialItem};

const RADIANS_PER_DEGREE: f32 = 0.017_453_292;

/// One item's sector and label anchor, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadialItemLayout {
    /// Clock degrees where the drawn sector starts (the gap already taken off).
    pub start_deg: f32,
    /// Degrees the drawn sector sweeps (`360 / items - gap_deg`).
    pub sweep_deg: f32,
    /// The label point: mid-radius on the item's centre angle, `+Y` down.
    pub mid_px: [f32; 2],
}

/// The pixel geometry of a [`UiRadial`] at a viewport, before any group.
#[derive(Clone, Debug, PartialEq)]
pub struct RadialLayout {
    /// The wheel centre, the centre of its `2 * radius_outer` box.
    pub center_px: [f32; 2],
    /// The inner radius in pixels.
    pub radius_inner_px: f32,
    /// The outer radius in pixels.
    pub radius_outer_px: f32,
    /// One entry per item, in item order.
    pub items: Vec<RadialItemLayout>,
}

/// Lays a wheel out in render-target pixels: the box is placed by `anchor` and `offset` under the
/// `UiRect` pivot rule, and each item's label sits at the mid-radius on its centre angle (clock degrees,
/// 0 at 12 o'clock, clockwise).
///
/// ```
/// use ludion_engine::components::{Anchor, UiRadial, UiRadialItem};
/// use ludion_engine::ui::radial_layout;
///
/// let wheel = UiRadial {
///     anchor: Anchor::Center,
///     items: vec![UiRadialItem::default(); 4],
///     ..UiRadial::default()
/// };
/// let layout = radial_layout(&wheel, (1920, 1080));
/// assert_eq!(layout.center_px, [960.0, 540.0]);
/// // Item 0 is at 12 o'clock, mid-radius (90 + 220) / 2 = 155 above the centre.
/// assert!((layout.items[0].mid_px[1] - (540.0 - 155.0)).abs() < 1e-3);
/// ```
pub fn radial_layout(radial: &UiRadial, viewport: (u32, u32)) -> RadialLayout {
    let scale = pixel_scale(viewport);
    let side = 2.0 * radial.radius_outer;
    let rect = resolve_ui_rect(radial.anchor, radial.offset, [side, side], viewport);
    let center_px = [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5];
    let mid_radius = (radial.radius_inner + radial.radius_outer) * 0.5 * scale;
    let count = radial.items.len();
    let span = if count == 0 {
        0.0
    } else {
        360.0 / count as f32
    };
    let items = (0..count)
        .map(|i| {
            let centre = radial.start_deg + i as f32 * span;
            let radians = centre * RADIANS_PER_DEGREE;
            RadialItemLayout {
                start_deg: centre - span * 0.5 + radial.gap_deg * 0.5,
                sweep_deg: span - radial.gap_deg,
                mid_px: [
                    center_px[0] + mid_radius * math::sin(radians),
                    center_px[1] - mid_radius * math::cos(radians),
                ],
            }
        })
        .collect();
    RadialLayout {
        center_px,
        radius_inner_px: radial.radius_inner * scale,
        radius_outer_px: radial.radius_outer * scale,
        items,
    }
}

/// The item whose sector (gap excluded) and annulus `[radius_inner, radius_outer)` contain `cursor_px`,
/// in render-target pixels, `+Y` down. It returns disabled items too: the caller decides what a click on
/// one means. Agrees with [`UiRadial::navigate`]'s sector for the same direction.
///
/// ```
/// use ludion_engine::components::{Anchor, UiRadial, UiRadialItem};
/// use ludion_engine::ui::radial_hit;
///
/// let wheel = UiRadial {
///     anchor: Anchor::Center,
///     items: vec![UiRadialItem::default(); 4],
///     ..UiRadial::default()
/// };
/// let viewport = (1920, 1080);
/// assert_eq!(radial_hit(&wheel, viewport, [960.0, 540.0 - 150.0]), Some(0)); // above the centre
/// assert_eq!(radial_hit(&wheel, viewport, [960.0 + 150.0, 540.0]), Some(1)); // to the right
/// assert_eq!(radial_hit(&wheel, viewport, [960.0, 540.0 - 20.0]), None); // inside the hole
/// ```
pub fn radial_hit(radial: &UiRadial, viewport: (u32, u32), cursor_px: [f32; 2]) -> Option<u32> {
    if radial.items.is_empty() || !cursor_px.iter().all(|c| c.is_finite()) {
        return None;
    }
    let layout = radial_layout(radial, viewport);
    let dx = cursor_px[0] - layout.center_px[0];
    let dy = cursor_px[1] - layout.center_px[1];
    let distance = math::hypot(dx, dy);
    if distance < layout.radius_inner_px || distance >= layout.radius_outer_px {
        return None;
    }
    radial.sector_hit(dx, -dy).map(|i| i as u32)
}

/// The colour an item's text and icon draw in: disabled outranks selected.
fn item_color(radial: &UiRadial, item: usize) -> [f32; 4] {
    if !radial.items[item].enabled {
        radial.disabled_color
    } else if radial.selected.is_some_and(|s| s as usize == item) {
        radial.selected_color
    } else {
        radial.color
    }
}

/// One text line centred on `centre_px`, laid out unwrapped like a `UiList` line.
fn centred_line(
    face: &FontFace,
    text: &str,
    size: f32,
    centre_px: [f32; 2],
    scale: f32,
) -> Vec<GlyphQuad> {
    let laid = layout(face, &fold_line_breaks(text), size, 0.0);
    let extent = laid.extent();
    let top_left = [
        centre_px[0] - extent[0] * scale * 0.5,
        centre_px[1] - extent[1] * scale * 0.5,
    ];
    laid.quads(top_left, size * scale, YAxis::Down)
}

/// Resolves an item's icon to a texture index and uv, or `None` for no icon.
pub(crate) type IconResolver<'a> =
    dyn FnMut(usize, &UiRadialItem) -> Option<(usize, [f32; 4])> + 'a;

/// What `extract` draws of a wheel, in draw order: per item, its icon (a rect `icon_size` square,
/// `0.5 * (size + icon_size)` above the label point), its `text` centred on the label point, and its
/// `detail` centred `size` below it at `0.75 * size`. `icon` resolves an item's atlas frame to a
/// texture index and uv, or `None` to skip the icon. An empty or invisible wheel draws nothing.
pub(crate) fn radial_draw(
    face: &FontFace,
    radial: &UiRadial,
    viewport: (u32, u32),
    font: Option<FontDraw>,
    outline: Option<GlyphOutline>,
    icon: &mut IconResolver<'_>,
) -> Vec<UiInstance> {
    let scale = pixel_scale(viewport);
    let layout = radial_layout(radial, viewport);
    let mut out = Vec::new();
    for (i, (item, placed)) in radial.items.iter().zip(&layout.items).enumerate() {
        let color = item_color(radial, i);
        let [mx, my] = placed.mid_px;
        if let Some((texture, uv)) = icon(i, item) {
            let edge = radial.icon_size * scale;
            let lift = 0.5 * (radial.size + radial.icon_size) * scale;
            out.push(UiInstance::Rect {
                rect_px: [mx - edge * 0.5, my - lift - edge * 0.5, edge, edge],
                texture: Some(texture),
                color,
                uv,
                clip: UiClip::NONE,
                view: None,
                rotation: UiRotation::NONE,
            });
        }
        let mut line = |text: &str, size: f32, centre: [f32; 2]| {
            if text.is_empty() {
                return;
            }
            out.push(UiInstance::Text {
                color,
                text: text.to_string(),
                quads: centred_line(face, text, size, centre, scale),
                font,
                clip: UiClip::NONE,
                outline,
                rotation: UiRotation::NONE,
            });
        };
        line(&item.text, radial.size, [mx, my]);
        if let Some(detail) = &item.detail {
            line(detail, 0.75 * radial.size, [mx, my + radial.size * scale]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Anchor;

    fn wheel(n: usize) -> UiRadial {
        UiRadial {
            anchor: Anchor::Center,
            items: (0..n)
                .map(|i| UiRadialItem {
                    text: format!("I{i}"),
                    ..UiRadialItem::default()
                })
                .collect(),
            ..UiRadial::default()
        }
    }

    fn close(a: [f32; 2], b: [f32; 2]) {
        assert!(
            (a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2,
            "{a:?} vs {b:?}"
        );
    }

    #[test]
    fn mid_points_for_four_items_at_two_resolutions() {
        let w = wheel(4);
        let l = radial_layout(&w, (1920, 1080));
        close(l.center_px, [960.0, 540.0]);
        close(l.items[0].mid_px, [960.0, 385.0]);
        close(l.items[1].mid_px, [1115.0, 540.0]);
        close(l.items[2].mid_px, [960.0, 695.0]);
        close(l.items[3].mid_px, [805.0, 540.0]);
        assert_eq!((l.items[0].start_deg, l.items[0].sweep_deg), (-44.0, 88.0));
        let l = radial_layout(&w, (960, 540));
        close(l.center_px, [480.0, 270.0]);
        close(l.items[1].mid_px, [480.0 + 77.5, 270.0]);
        assert_eq!((l.radius_inner_px, l.radius_outer_px), (45.0, 110.0));
    }

    #[test]
    fn hit_is_none_in_the_gap_the_hole_and_outside() {
        let w = wheel(4);
        let viewport = (1920, 1080);
        let at = |deg: f32, radius: f32| {
            let r = deg * RADIANS_PER_DEGREE;
            [960.0 + radius * math::sin(r), 540.0 - radius * math::cos(r)]
        };
        assert_eq!(radial_hit(&w, viewport, at(0.0, 150.0)), Some(0));
        assert_eq!(
            radial_hit(&w, viewport, at(45.0, 150.0)),
            None,
            "in the gap"
        );
        assert_eq!(radial_hit(&w, viewport, at(40.0, 150.0)), Some(0));
        assert_eq!(radial_hit(&w, viewport, at(50.0, 150.0)), Some(1));
        assert_eq!(radial_hit(&w, viewport, at(0.0, 89.0)), None, "inside r_in");
        assert_eq!(radial_hit(&w, viewport, at(0.0, 91.0)), Some(0));
        assert_eq!(
            radial_hit(&w, viewport, at(0.0, 220.5)),
            None,
            "outside r_out"
        );
        assert_eq!(
            radial_hit(&UiRadial::default(), viewport, at(0.0, 150.0)),
            None
        );
    }

    #[test]
    fn hit_agrees_with_navigate_on_a_five_degree_sweep() {
        for (n, start) in [(4usize, 0.0f32), (6, 30.0), (5, 10.0)] {
            let mut w = wheel(n);
            w.start_deg = start;
            w.gap_deg = 0.0;
            let viewport = (1920, 1080);
            let mut deg = 0.0f32;
            while deg < 360.0 {
                let r = deg * RADIANS_PER_DEGREE;
                let cursor = [960.0 + 150.0 * math::sin(r), 540.0 - 150.0 * math::cos(r)];
                let hit = radial_hit(&w, viewport, cursor).expect("gap 0 always hits");
                let mut nav = w.clone();
                nav.hysteresis_deg = 0.0;
                nav.selected = None;
                nav.navigate(crate::components::UiRadialNav::from_cursor(
                    [cursor[0] - 960.0, cursor[1] - 540.0],
                    1.0,
                ));
                assert_eq!(nav.selected, Some(hit), "n={n} start={start} deg={deg}");
                deg += 5.0;
            }
        }
    }

    #[test]
    fn draw_places_text_detail_and_icon_around_the_label_point() {
        let mut w = wheel(4);
        w.items[0].detail = Some("x3".into());
        w.items[0].atlas = Some("icons.atlas.yaml".into());
        w.items[1].enabled = false;
        w.selected = Some(0);
        let mut asked = Vec::new();
        let drawn = radial_draw(
            FontFace::embedded(),
            &w,
            (1920, 1080),
            None,
            None,
            &mut |i, item| {
                asked.push(i);
                item.atlas.as_ref().map(|_| (0usize, [0.0, 0.0, 1.0, 1.0]))
            },
        );
        assert_eq!(asked, [0, 1, 2, 3]);
        // Item 0: icon, text, detail; the others one text each.
        assert_eq!(drawn.len(), 3 + 3);
        let UiInstance::Rect { rect_px, color, .. } = &drawn[0] else {
            panic!("the icon comes first")
        };
        // Centred on x = 960, lifted 0.5 * (24 + 48) = 36 above the label point (y = 385).
        close(
            [rect_px[0] + 24.0, rect_px[1] + 24.0],
            [960.0, 385.0 - 36.0],
        );
        assert_eq!(*color, w.selected_color);
        let UiInstance::Text {
            color, text, quads, ..
        } = &drawn[1]
        else {
            panic!("the label")
        };
        assert_eq!((text.as_str(), *color), ("I0", w.selected_color));
        // "I0" at size 24 is two 24-wide glyphs: centred on x = 960.
        let (left, right) = (quads[0].pos[0], quads[1].pos[2]);
        assert!(
            ((left + right) * 0.5 - 960.0).abs() < 1e-2,
            "{left} {right}"
        );
        let UiInstance::Text { text, quads, .. } = &drawn[2] else {
            panic!("the detail")
        };
        assert_eq!(text, "x3");
        // 0.75 * 24 = 18 canvas units tall, centred 24 below the label point.
        let (top, bottom) = (quads[0].pos[1], quads[0].pos[3]);
        assert!(
            ((top + bottom) * 0.5 - (385.0 + 24.0)).abs() < 1e-2,
            "{top} {bottom}"
        );
        assert!(((bottom - top) - 18.0).abs() < 1e-2);
        let UiInstance::Text { color, .. } = &drawn[3] else {
            panic!("item 1")
        };
        assert_eq!(*color, w.disabled_color);
        let UiInstance::Text { color, .. } = &drawn[4] else {
            panic!("item 2")
        };
        assert_eq!(*color, w.color);
    }

    #[test]
    fn an_empty_wheel_draws_nothing() {
        let drawn = radial_draw(
            FontFace::embedded(),
            &UiRadial::default(),
            (1920, 1080),
            None,
            None,
            &mut |_, _| None,
        );
        assert!(drawn.is_empty());
    }
}
