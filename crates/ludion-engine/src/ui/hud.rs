//! The HUD motion pass: [`UiTween`] and [`UiWorldAnchor`] resolved onto the screen-space elements
//! `extract` emits (`Plan/ui-hud.md`).
//!
//! Two entry points bracket an element's resolve. The `pre_*` functions return the element as it
//! should be resolved — a [`Cow`], borrowed when nothing animates it — with the tween's offset, size,
//! colour and opacity applied. [`post`] then edits the instances the element just emitted: scale
//! about the centre of their box, the world anchor's translation, `show`, `behind`, `fade`. Neither
//! writes the world, and an element with neither component is returned and left untouched.

use std::borrow::Cow;

use ludion_components::ui::{
    UiAnchorBehind, UiAnchorShow, UiGroup, UiTween, UiTweenField, UiWorldAnchor,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::time::TickRate;
use ludion_ecs::World;
use ludion_renderer::{UiClip, UiInstance};

use super::group::{GroupXf, UiGroups};
use super::{
    canvas_anchor_px, group_delta, nine_slice, pixel_scale, place_world_anchor, sub_uv,
    ui_tween_sample,
};
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
    Group,
}

impl HudElement {
    fn supports(self, field: UiTweenField) -> bool {
        match field {
            UiTweenField::Opacity | UiTweenField::Offset => true,
            UiTweenField::Scale => !matches!(self, HudElement::List),
            UiTweenField::Color => matches!(self, HudElement::Rect { .. } | HudElement::Text),
            UiTweenField::Size => matches!(self, HudElement::Rect { .. } | HudElement::Group),
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
        HudElement::Group => "UiGroup",
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
            .with_help("`color` needs a `UiRect` or `UiText`, `scale` a `UiRect`, `UiText` or `UiGroup`, `size` a `UiRect` or `UiGroup`; `ludion explain L2300`"),
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

/// A group's own numbers at this frame, its tracks applied: the offset and size to resolve, and the
/// scale and opacity it composes onto its descendants.
pub(crate) struct GroupMotion {
    pub offset: [f32; 2],
    pub size: [f32; 2],
    pub scale: f32,
    pub opacity: f32,
}

/// The group's numbers with its tracks at this frame applied.
pub(crate) fn pre_group(
    frame: &HudFrame<'_>,
    id: EntityId,
    group: &UiGroup,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> GroupMotion {
    let s = sample(frame, id, HudElement::Group, degraded);
    let [x, y] = s.offset.unwrap_or([0.0, 0.0]);
    GroupMotion {
        offset: [group.offset[0] + x, group.offset[1] + y],
        size: s.size.unwrap_or(group.size),
        scale: group.scale * s.scale.unwrap_or(1.0),
        opacity: group.opacity * s.opacity.unwrap_or(1.0),
    }
}

/// The bounding box `[l, t, r, b]` of every rect and glyph quad in `instances`, or `None` when empty.
pub(crate) fn bounds(instances: &[UiEntry]) -> Option<[f32; 4]> {
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
            UiInstance::Rect { color, .. } => color[3] *= opacity,
            UiInstance::Text { color, outline, .. } => {
                color[3] *= opacity;
                if let Some(outline) = outline {
                    outline.color[3] *= opacity;
                }
            }
        }
    }
}

/// Transforms the instances `entries[start..]` that one element just emitted: its tween's `scale`,
/// then its world anchor. An element the anchor hides has its instances removed.
///
/// The scale pivots on the centre of the instances' bounding box, the anchor then moves the box by
/// `projected − canvas_anchor_point` so the element's `anchor` is its pivot on the projected point.
/// A target that is dead or has no `Transform` draws nothing.
///
/// An element inside a [`UiGroup`] first moves by [`group_delta`] into the group's box, takes the
/// same tween scale, then the group's affine and opacity; its own world anchor is skipped (`L2300`)
/// and an invisible group removes it. An element with no `group` takes exactly the path above.
#[allow(clippy::too_many_arguments)]
pub(crate) fn post(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    id: EntityId,
    element: HudElement,
    anchor: Anchor,
    entries: &mut Vec<UiEntry>,
    start: usize,
    grouping: Grouping<'_>,
) {
    if start >= entries.len() {
        return;
    }
    let world = frame.world;
    let group_xf = resolve_grouping(frame, poses, id, element, grouping);
    if let Some((xf, in_box)) = &group_xf {
        if !xf.visible {
            return entries.truncate(start);
        }
        if !in_box {
            let delta = group_delta(anchor, xf.box_px, frame.viewport);
            transform_instances(&mut entries[start..], [0.0, 0.0], 1.0, delta);
        }
    }
    let group_xf = group_xf.map(|(xf, _)| xf);
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
        _ if group_xf.is_some() => None,
        _ => world.get::<UiWorldAnchor>(id).ok().flatten(),
    };
    if scale.is_none() && world_anchor.is_none() {
        return apply_group(entries, start, group_xf);
    }
    let Some(b) = bounds(&entries[start..]) else {
        return;
    };
    let centre = [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5];
    let mut delta = [0.0, 0.0];
    let mut opacity = 1.0;
    if let Some(a) = world_anchor {
        match world_anchor_delta(frame, poses, a, anchor) {
            None => return entries.truncate(start),
            Some((d, alpha)) => {
                delta = d;
                opacity = alpha;
            }
        }
    }
    transform_instances(&mut entries[start..], centre, scale.unwrap_or(1.0), delta);
    if opacity < 1.0 {
        multiply_alpha(&mut entries[start..], opacity);
    }
    apply_group(entries, start, group_xf);
}

/// What a grouped element needs from [`post`]: its group, whether it covers the group's box
/// (`fill_group`, so no delta), the shared memo and the degraded list.
pub(crate) struct Grouping<'a> {
    pub group: Option<EntityId>,
    pub fill_group: bool,
    pub groups: &'a mut UiGroups,
    pub degraded: &'a mut Vec<(String, Diagnostic)>,
}

/// The element's resolved group and whether the element already sits in the group's box. `None` for
/// an ungrouped element, a `fill: true` rect (which ignores `group`), or a group that cannot be
/// resolved (`L2302`, drawn ungrouped).
fn resolve_grouping(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    id: EntityId,
    element: HudElement,
    grouping: Grouping<'_>,
) -> Option<(GroupXf, bool)> {
    let group = grouping.group?;
    if matches!(element, HudElement::Rect { fill: true }) {
        return None;
    }
    let key = crate::diagnostics::offender_key(frame.world, id);
    let Some(xf) = grouping
        .groups
        .resolve(frame, poses, group, grouping.degraded)
    else {
        grouping.degraded.push((
            format!("{key}/ui-group"),
            Diagnostic::new(
                code::UI_GROUP_UNRESOLVED,
                format!("{id} names {group} as its `group`, which is not a live `UiGroup` within 8 levels with no cycle; the element is drawn ungrouped"),
            )
            .with_help("point `group` at a live entity with a `UiGroup`; `ludion explain L2302`"),
        ));
        return None;
    };
    if frame.world.has::<UiWorldAnchor>(id) {
        grouping.degraded.push((
            format!("{key}/ui-anchor-group"),
            Diagnostic::new(
                code::UI_MOTION_UNSUPPORTED,
                format!("{id} carries a `UiWorldAnchor` inside a group, so its anchor is skipped"),
            )
            .with_help("put the `UiWorldAnchor` on the outermost group; `ludion explain L2300`"),
        ));
    }
    Some((xf, grouping.fill_group))
}

/// Applies the group's scale, translation and opacity to the instances `entries[start..]`.
fn apply_group(entries: &mut [UiEntry], start: usize, group: Option<GroupXf>) {
    let Some(xf) = group else {
        return;
    };
    for (_, _, instance) in &mut entries[start..] {
        match instance {
            UiInstance::Rect { clip, .. } | UiInstance::Text { clip, .. } => *clip = xf.clip,
        }
    }
    if xf.scale.to_bits() != 1.0f32.to_bits() || xf.offset != [0.0, 0.0] {
        transform_instances(&mut entries[start..], [0.0, 0.0], xf.scale, xf.offset);
    }
    if xf.opacity < 1.0 {
        multiply_alpha(&mut entries[start..], xf.opacity);
    }
}

/// The translation a world anchor gives an element whose canvas anchor is `anchor`, and the fade
/// alpha it multiplies in; `None` when the anchor hides the element (a dead target, behind the
/// camera, outside its `show` window, or faded out).
pub(crate) fn world_anchor_delta(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    a: &UiWorldAnchor,
    anchor: Anchor,
) -> Option<([f32; 2], f32)> {
    let (p, alpha) = placement(frame, poses, a)?;
    let hidden = (p.behind && a.behind == UiAnchorBehind::Hide)
        || (a.show == UiAnchorShow::OnScreen && !p.on_screen)
        || (a.show == UiAnchorShow::OffScreen && p.on_screen)
        || alpha <= 0.0;
    if hidden {
        return None;
    }
    let pivot = canvas_anchor_px(anchor, frame.viewport);
    Some(([p.pixel[0] - pivot[0], p.pixel[1] - pivot[1]], alpha))
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

/// The pixel box a rect resolves to, before `post`: its canvas rect, or the group's box for a
/// `fill_group` rect (a group that cannot resolve leaves the canvas rect, and `post` reports it).
pub(crate) fn rect_box(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    groups: &mut UiGroups,
    rect: &UiRect,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> [f32; 4] {
    let rect_px = super::resolve_ui_rect_of(rect, frame.viewport);
    match rect.group {
        Some(group) if rect.fill_group && !rect.fill => groups
            .resolve(frame, poses, group, degraded)
            .map_or(rect_px, |g| g.box_px),
        _ => rect_px,
    }
}

/// Pushes the draw rows of one resolved `UiRect`: a nine-slice of nine rects when it carries a
/// `slice` and a texture, else one rect whose uv is `uv_rect` narrowed into `uv` (the loose
/// texture or atlas-region uv). `region_texels` is the sampled region size in texels. A `uv_rect`
/// beside a `slice` is ignored, and so are both without a texture.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_rect(
    frame: &HudFrame<'_>,
    id: EntityId,
    rect: &UiRect,
    rect_px: [f32; 4],
    texture: Option<usize>,
    uv: [f32; 4],
    region_texels: Option<[f32; 2]>,
    entries: &mut Vec<UiEntry>,
    degraded: &mut Vec<(String, Diagnostic)>,
) {
    let row = |rect_px: [f32; 4], uv: [f32; 4]| {
        (
            rect.order,
            id,
            UiInstance::Rect {
                rect_px,
                texture,
                color: rect.color,
                uv,
                clip: UiClip::NONE,
            },
        )
    };
    let (Some(_), Some(texels)) = (texture, region_texels) else {
        entries.push(row(rect_px, uv));
        return;
    };
    if let Some(slice) = &rect.slice {
        let nine = nine_slice(rect_px, uv, texels, slice, pixel_scale(frame.viewport));
        if nine.clamped {
            let key = crate::diagnostics::offender_key(frame.world, id);
            degraded.push((
                format!("{key}/ui-slice-clamped"),
                Diagnostic::new(
                    code::UI_SLICE_CLAMPED,
                    format!("{id} has a `UiRect` `slice` larger than its region or rect; it is drawn clamped"),
                )
                .with_help("shrink the `border` or `scale`, or enlarge the rect; `ludion explain L2301`"),
            ));
        }
        entries.extend(nine.pieces.into_iter().map(|(r, uv)| row(r, uv)));
    } else {
        let uv = rect.uv_rect.map_or(uv, |sub| sub_uv(uv, sub));
        entries.push(row(rect_px, uv));
    }
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
                clip: UiClip::NONE,
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
                clip: UiClip::NONE,
                outline: None,
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

    fn emit(
        rect: &UiRect,
        texture: Option<usize>,
        texels: Option<[f32; 2]>,
    ) -> (Vec<UiEntry>, Vec<(String, Diagnostic)>) {
        let world = World::new();
        let frame = HudFrame {
            world: &world,
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            viewport: (1920, 1080),
            real_frame: 0,
            game_frame: 0,
            rate: TickRate::DEFAULT,
        };
        let (mut entries, mut degraded) = (Vec::new(), Vec::new());
        emit_rect(
            &frame,
            EntityId::new(3, 0),
            rect,
            [0.0, 0.0, 120.0, 60.0],
            texture,
            [0.0, 0.0, 1.0, 1.0],
            texels,
            &mut entries,
            &mut degraded,
        );
        (entries, degraded)
    }

    fn slice(border: f32) -> ludion_components::ui::UiSlice {
        ludion_components::ui::UiSlice {
            border: [border; 4],
            scale: 1.0,
            center: ludion_components::ui::UiSliceCenter::Stretch,
        }
    }

    #[test]
    fn uv_rect_narrows_one_rect() {
        let rect = UiRect {
            uv_rect: Some([0.0, 0.0, 0.5, 1.0]),
            ..UiRect::default()
        };
        let (entries, degraded) = emit(&rect, Some(0), Some([12.0, 12.0]));
        assert_eq!(entries.len(), 1);
        let UiInstance::Rect { uv, rect_px, .. } = &entries[0].2 else {
            unreachable!()
        };
        assert_eq!(*uv, [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(*rect_px, [0.0, 0.0, 120.0, 60.0]);
        assert!(degraded.is_empty());
    }

    #[test]
    fn slice_emits_nine_and_ignores_uv_rect() {
        let rect = UiRect {
            uv_rect: Some([0.0, 0.0, 0.5, 1.0]),
            slice: Some(slice(4.0)),
            ..UiRect::default()
        };
        let (entries, degraded) = emit(&rect, Some(0), Some([12.0, 12.0]));
        assert_eq!(entries.len(), 9);
        let UiInstance::Rect { uv, .. } = &entries[8].2 else {
            unreachable!()
        };
        assert_eq!(uv[2], 1.0);
        assert!(degraded.is_empty());
    }

    #[test]
    fn untextured_rect_is_unchanged() {
        let rect = UiRect {
            uv_rect: Some([0.0, 0.0, 0.5, 1.0]),
            slice: Some(slice(4.0)),
            ..UiRect::default()
        };
        let (entries, _) = emit(&rect, None, None);
        assert_eq!(entries.len(), 1);
        let UiInstance::Rect { uv, texture, .. } = &entries[0].2 else {
            unreachable!()
        };
        assert_eq!((*uv, *texture), ([0.0, 0.0, 1.0, 1.0], None));
    }

    #[test]
    fn clamped_slice_reports_l2301_once() {
        let rect = UiRect {
            slice: Some(slice(40.0)),
            ..UiRect::default()
        };
        let (entries, degraded) = emit(&rect, Some(0), Some([12.0, 12.0]));
        assert!(!entries.is_empty());
        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].1.code.as_str(), "L2301");
        assert!(degraded[0].0.ends_with("/ui-slice-clamped"));
    }
}
