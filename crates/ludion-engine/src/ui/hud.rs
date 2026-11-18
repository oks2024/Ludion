//! The HUD motion pass: [`UiTween`] and [`UiWorldAnchor`] resolved onto the screen-space elements
//! `extract` emits (`Plan/ui-hud.md`).
//!
//! Two entry points bracket an element's resolve. The `pre_*` functions return the element as it
//! should be resolved — a [`Cow`], borrowed when nothing animates it — with the tween's offset, size,
//! colour and opacity applied. [`post`] then edits the instances the element just emitted: scale
//! about the centre of their box, the world anchor's translation, `show`, `behind`, `fade`. Neither
//! writes the world, and an element with neither component is returned and left untouched.

use std::borrow::Cow;

use ludion_components::ui::{UiAnchorBehind, UiAnchorShow, UiTween, UiTweenField, UiWorldAnchor};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::time::TickRate;
use ludion_ecs::World;
use ludion_renderer::UiInstance;

use super::{canvas_anchor_px, place_world_anchor, ui_tween_sample};
use crate::components::{Anchor, UiList, UiRect, UiText};
use crate::hierarchy::WorldAffines;

/// One `(order, entity, instance)` row of `extract`'s UI list, before the final sort.
pub(crate) type UiEntry = (i32, EntityId, UiInstance);

/// What the pass reads about the frame being extracted.
pub(crate) struct HudFrame<'w> {
    pub world: &'w World,
    /// The world→view matrix of the camera as drawn.
    pub view: Mat4,
    /// The view→clip matrix of the camera as drawn.
    pub projection: Mat4,
    pub viewport: (u32, u32),
    /// The sim frame, for `clock: real` tracks.
    pub real_frame: u64,
    /// The gameplay frame, for `clock: game` tracks.
    pub game_frame: u64,
    pub rate: TickRate,
}

/// Which element a tween or anchor sits on, which decides the fields it can take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HudElement {
    Rect { fill: bool },
    Text,
    List,
}

impl HudElement {
    fn supports(self, field: UiTweenField) -> bool {
        match field {
            UiTweenField::Opacity | UiTweenField::Offset => true,
            UiTweenField::Scale | UiTweenField::Color => !matches!(self, HudElement::List),
            UiTweenField::Size => matches!(self, HudElement::Rect { .. }),
        }
    }
}

/// The tween's samples for one element, after the unsupported fields were reported and dropped.
#[derive(Default)]
struct Samples {
    opacity: Option<f32>,
    offset: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    color: Option<[f32; 4]>,
    scale: Option<f32>,
}

fn field_name(field: UiTweenField) -> &'static str {
    match field {
        UiTweenField::Opacity => "opacity",
        UiTweenField::Offset => "offset",
        UiTweenField::Scale => "scale",
        UiTweenField::Size => "size",
        UiTweenField::Color => "color",
    }
}

fn element_name(element: HudElement) -> &'static str {
    match element {
        HudElement::Rect { .. } => "UiRect",
        HudElement::Text => "UiText",
        HudElement::List => "UiList",
    }
}

fn sample(
    frame: &HudFrame<'_>,
    id: EntityId,
    element: HudElement,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Samples {
    let mut out = Samples::default();
    if let HudElement::Rect { fill: true } = element
        && frame.world.has::<UiWorldAnchor>(id)
    {
        let key = crate::diagnostics::offender_key(frame.world, id);
        degraded.push((
            format!("{key}/ui-anchor-fill"),
            Diagnostic::new(
                code::UI_MOTION_UNSUPPORTED,
                format!("{id} carries a `UiWorldAnchor` on a `fill: true` rect, which ignores its anchor"),
            )
            .with_help("drop `fill` and size the rect, or remove the `UiWorldAnchor`; `ludion explain L2300`"),
        ));
    }
    let Some(tween) = frame.world.get::<UiTween>(id).ok().flatten() else {
        return out;
    };
    let mut unsupported: Vec<&'static str> = Vec::new();
    for track in &tween.tracks {
        if !element.supports(track.field) && !unsupported.contains(&field_name(track.field)) {
            unsupported.push(field_name(track.field));
        }
    }
    if !unsupported.is_empty() {
        let key = crate::diagnostics::offender_key(frame.world, id);
        degraded.push((
            format!("{key}/ui-tween-field"),
            Diagnostic::new(
                code::UI_MOTION_UNSUPPORTED,
                format!(
                    "{id} carries a `UiTween` track on `{}`, which a `{}` cannot take; the track is skipped",
                    unsupported.join("`, `"),
                    element_name(element),
                ),
            )
            .with_help("`scale` and `color` need a `UiRect` or `UiText`, `size` a `UiRect`; `ludion explain L2300`"),
        ));
    }
    let at = |field: UiTweenField| {
        element
            .supports(field)
            .then(|| ui_tween_sample(tween, field, frame.real_frame, frame.game_frame, frame.rate))
            .flatten()
    };
    out.opacity = at(UiTweenField::Opacity).map(|v| v[0]);
    out.offset = at(UiTweenField::Offset).map(|v| [v[0], v[1]]);
    out.size = at(UiTweenField::Size).map(|v| [v[0], v[1]]);
    out.color = at(UiTweenField::Color);
    out.scale = at(UiTweenField::Scale).map(|v| v[0]);
    out
}

fn fade(color: &mut [f32; 4], opacity: f32) {
    color[3] *= opacity;
}

/// The rect to resolve: `rect` with its tracks at this frame applied.
pub(crate) fn pre_rect<'a>(
    frame: &HudFrame<'_>,
    id: EntityId,
    rect: &'a UiRect,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Cow<'a, UiRect> {
    let s = sample(frame, id, HudElement::Rect { fill: rect.fill }, degraded);
    if s.opacity.is_none() && s.offset.is_none() && s.size.is_none() && s.color.is_none() {
        return Cow::Borrowed(rect);
    }
    let mut out = rect.clone();
    if let Some([x, y]) = s.offset {
        out.offset = [out.offset[0] + x, out.offset[1] + y];
    }
    if let Some(size) = s.size {
        out.size = size;
    }
    if let Some(color) = s.color {
        out.color = color;
    }
    if let Some(opacity) = s.opacity {
        fade(&mut out.color, opacity);
    }
    Cow::Owned(out)
}

/// The text to resolve: `text` with its tracks at this frame applied.
pub(crate) fn pre_text<'a>(
    frame: &HudFrame<'_>,
    id: EntityId,
    text: &'a UiText,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Cow<'a, UiText> {
    let s = sample(frame, id, HudElement::Text, degraded);
    if s.opacity.is_none() && s.offset.is_none() && s.color.is_none() {
        return Cow::Borrowed(text);
    }
    let mut out = text.clone();
    if let Some([x, y]) = s.offset {
        out.offset = [out.offset[0] + x, out.offset[1] + y];
    }
    if let Some(color) = s.color {
        out.color = color;
    }
    if let Some(opacity) = s.opacity {
        fade(&mut out.color, opacity);
    }
    Cow::Owned(out)
}

/// The list to resolve: `list` with its tracks at this frame applied. Opacity scales the alpha of
/// every colour the list draws, the panel included.
pub(crate) fn pre_list<'a>(
    frame: &HudFrame<'_>,
    id: EntityId,
    list: &'a UiList,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> Cow<'a, UiList> {
    let s = sample(frame, id, HudElement::List, degraded);
    if s.opacity.is_none() && s.offset.is_none() {
        return Cow::Borrowed(list);
    }
    let mut out = list.clone();
    if let Some([x, y]) = s.offset {
        out.offset = [out.offset[0] + x, out.offset[1] + y];
    }
    if let Some(opacity) = s.opacity {
        for color in [
            &mut out.color,
            &mut out.selected_color,
            &mut out.disabled_color,
            &mut out.background,
        ] {
            fade(color, opacity);
        }
    }
    Cow::Owned(out)
}

/// The bounding box `[l, t, r, b]` of every rect and glyph quad in `instances`, or `None` when empty.
fn bounds(instances: &[UiEntry]) -> Option<[f32; 4]> {
    let mut out: Option<[f32; 4]> = None;
    let mut grow = |b: [f32; 4]| {
        out = Some(match out {
            None => b,
            Some(o) => [
                o[0].min(b[0]),
                o[1].min(b[1]),
                o[2].max(b[2]),
                o[3].max(b[3]),
            ],
        });
    };
    for (_, _, instance) in instances {
        match instance {
            UiInstance::Rect { rect_px: r, .. } => grow([r[0], r[1], r[0] + r[2], r[1] + r[3]]),
            UiInstance::Text { quads, .. } => quads.iter().for_each(|q| grow(q.pos)),
        }
    }
    out
}

/// Scales every instance about `centre` by `scale`, then translates it by `delta`, in pixels.
pub(crate) fn transform_instances(
    instances: &mut [UiEntry],
    centre: [f32; 2],
    scale: f32,
    delta: [f32; 2],
) {
    let x = |v: f32| (v - centre[0]) * scale + centre[0] + delta[0];
    let y = |v: f32| (v - centre[1]) * scale + centre[1] + delta[1];
    for (_, _, instance) in instances {
        match instance {
            UiInstance::Rect { rect_px: r, .. } => {
                *r = [x(r[0]), y(r[1]), r[2] * scale, r[3] * scale];
            }
            UiInstance::Text { quads, .. } => {
                for q in quads {
                    q.pos = [x(q.pos[0]), y(q.pos[1]), x(q.pos[2]), y(q.pos[3])];
                }
            }
        }
    }
}

fn multiply_alpha(instances: &mut [UiEntry], opacity: f32) {
    for (_, _, instance) in instances {
        match instance {
            UiInstance::Rect { color, .. } | UiInstance::Text { color, .. } => color[3] *= opacity,
        }
    }
}

/// Transforms the instances `entries[start..]` that one element just emitted: its tween's `scale`,
/// then its world anchor. An element the anchor hides has its instances removed.
///
/// The scale pivots on the centre of the instances' bounding box, the anchor then moves the box by
/// `projected − canvas_anchor_point` so the element's `anchor` is its pivot on the projected point.
/// A target that is dead or has no `Transform` draws nothing.
pub(crate) fn post(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    id: EntityId,
    element: HudElement,
    anchor: Anchor,
    entries: &mut Vec<UiEntry>,
    start: usize,
) {
    if start >= entries.len() {
        return;
    }
    let world = frame.world;
    let scale = match world.get::<UiTween>(id).ok().flatten() {
        Some(tween) if element.supports(UiTweenField::Scale) => ui_tween_sample(
            tween,
            UiTweenField::Scale,
            frame.real_frame,
            frame.game_frame,
            frame.rate,
        )
        .map(|v| v[0]),
        _ => None,
    };
    let world_anchor = match element {
        HudElement::Rect { fill: true } => None,
        _ => world.get::<UiWorldAnchor>(id).ok().flatten(),
    };
    if scale.is_none() && world_anchor.is_none() {
        return;
    }
    let Some(b) = bounds(&entries[start..]) else {
        return;
    };
    let centre = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
    let mut delta = [0.0, 0.0];
    let mut opacity = 1.0;
    if let Some(a) = world_anchor {
        match placement(frame, poses, a) {
            None => return entries.truncate(start),
            Some((p, alpha)) => {
                let hidden = (p.behind && a.behind == UiAnchorBehind::Hide)
                    || (a.show == UiAnchorShow::OnScreen && !p.on_screen)
                    || (a.show == UiAnchorShow::OffScreen && p.on_screen)
                    || alpha <= 0.0;
                if hidden {
                    return entries.truncate(start);
                }
                let pivot = canvas_anchor_px(anchor, frame.viewport);
                delta = [p.pixel[0] - pivot[0], p.pixel[1] - pivot[1]];
                opacity = alpha;
            }
        }
    }
    transform_instances(&mut entries[start..], centre, scale.unwrap_or(1.0), delta);
    if opacity < 1.0 {
        multiply_alpha(&mut entries[start..], opacity);
    }
}

/// The anchor's placement and its fade alpha, or `None` when the target cannot be drawn against.
fn placement(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    a: &UiWorldAnchor,
) -> Option<(super::WorldAnchorPlacement, f32)> {
    let world = frame.world;
    if !world.is_alive(a.target) || !world.has::<crate::components::Transform>(a.target) {
        return None;
    }
    let (model, _) = crate::extract::render_model(world, poses, a.target);
    let at = model.w_axis.truncate() + Vec3::from_array(a.world_offset);
    let placed = place_world_anchor(
        frame.view,
        frame.projection,
        at.to_array(),
        frame.viewport,
        a.margin,
    )?;
    let alpha = match a.fade {
        Some([near, far]) => ((far - placed.distance) / (far - near)).clamp(0.0, 1.0),
        None => 1.0,
    };
    Some((placed, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_font::GlyphQuad;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> UiEntry {
        (
            0,
            EntityId::new(0, 0),
            UiInstance::Rect {
                rect_px: [x, y, w, h],
                texture: None,
                color: [1.0; 4],
                uv: [0.0, 0.0, 1.0, 1.0],
            },
        )
    }

    fn text(pos: [f32; 4]) -> UiEntry {
        (
            0,
            EntityId::new(0, 0),
            UiInstance::Text {
                color: [1.0; 4],
                text: "A".into(),
                quads: vec![GlyphQuad {
                    ch: 'A',
                    pos,
                    uv: [0.0; 4],
                }],
                font: None,
            },
        )
    }

    #[test]
    fn scale_pivots_on_the_box_centre() {
        let mut rows = vec![rect(100.0, 100.0, 40.0, 20.0)];
        transform_instances(&mut rows, [120.0, 110.0], 2.0, [0.0, 0.0]);
        let UiInstance::Rect { rect_px, .. } = &rows[0].2 else {
            unreachable!()
        };
        assert_eq!(*rect_px, [80.0, 90.0, 80.0, 40.0]);
    }

    #[test]
    fn translation_moves_rects_and_glyph_quads() {
        let mut rows = vec![rect(0.0, 0.0, 10.0, 10.0), text([2.0, 2.0, 6.0, 8.0])];
        transform_instances(&mut rows, [5.0, 5.0], 1.0, [10.0, -4.0]);
        let UiInstance::Rect { rect_px, .. } = &rows[0].2 else {
            unreachable!()
        };
        assert_eq!(*rect_px, [10.0, -4.0, 10.0, 10.0]);
        let UiInstance::Text { quads, .. } = &rows[1].2 else {
            unreachable!()
        };
        assert_eq!(quads[0].pos, [12.0, -2.0, 16.0, 4.0]);
    }

    #[test]
    fn bounds_cover_rects_and_quads() {
        let rows = vec![rect(10.0, 10.0, 10.0, 10.0), text([0.0, 15.0, 4.0, 40.0])];
        assert_eq!(bounds(&rows), Some([0.0, 10.0, 20.0, 40.0]));
        assert_eq!(bounds(&[]), None);
    }

    #[test]
    fn alpha_multiplies_every_instance() {
        let mut rows = vec![rect(0.0, 0.0, 1.0, 1.0), text([0.0; 4])];
        multiply_alpha(&mut rows, 0.5);
        for (_, _, instance) in &rows {
            let (UiInstance::Rect { color, .. } | UiInstance::Text { color, .. }) = instance;
            assert_eq!(color[3], 0.5);
        }
    }

    #[test]
    fn element_support_matrix() {
        let rect = HudElement::Rect { fill: false };
        assert!(rect.supports(UiTweenField::Size));
        assert!(!HudElement::Text.supports(UiTweenField::Size));
        assert!(HudElement::Text.supports(UiTweenField::Scale));
        assert!(!HudElement::List.supports(UiTweenField::Color));
        assert!(HudElement::List.supports(UiTweenField::Opacity));
    }
}
