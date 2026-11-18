//! [`ui_hit`]: whether a pixel is over a screen-space UI element as `extract` draws it
//! (`Plan/ui-hud.md` decision 30).
//!
//! It runs the same per-element path as `extract` — `pre_*`, the emitted rows, [`hud::post`] — on a
//! one-element list, then tests the cursor against the bounding box of what `post` left and the
//! clip it stamped, so tweens, world anchors, group affines, visibility and clips cannot diverge
//! from what is drawn.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::{GameClock, GameTime, SimTime};
use ludion_ecs::World;
use ludion_renderer::{SpriteInstance, UiClip, UiInstance};

use super::group::{UiGroups, clip_contains};
use super::hud::{self, Grouping, HudElement, HudFrame, UiEntry};
use super::{FontFace, radial_hit, radial_layout, ui_list_draw, ui_text_draw};
use crate::components::{Anchor, UiList, UiRadial, UiRect, UiText};
use crate::fonts::FontLibrary;
use crate::hierarchy::WorldAffines;

/// Whether `cursor` (render-target pixels, top-left origin, `+Y` down) is over the UI element on
/// `entity` as [`extract`](crate::extract) draws it at `now`: the group delta, tweens, world anchor,
/// group affine, visibility and clip all applied.
///
/// A `UiRect` is its one resolved rect, a `UiText` the bounding box of its glyph quads, a `UiList`
/// its background panel (use [`list_hit`](crate::ui::list_hit) for the item index of an ungrouped
/// list), a `UiRadial` its sectors and annulus ([`radial_hit`]; the group-placed box when grouped).
/// An entity holding several UI components is hit when any of them is. The box is half-open
/// and the clip test is `d < 0`, so a cursor exactly on a clip edge is outside.
///
/// `false` for an invisible element or group, a world anchor that hides the element, a dead
/// entity, a non-finite cursor or a world whose camera does not resolve. The camera is the
/// scene's, without `pixel_snap`; fonts come from the world's [`FontLibrary`] resource, else the
/// embedded face.
///
/// # Example
///
/// ```
/// use ludion_core::time::{SimTime, TickRate};
/// use ludion_ecs::World;
/// use ludion_engine::components::{UiRect, UiGroup, UiClipShape};
/// use ludion_engine::ui::ui_hit;
///
/// let mut world = World::new();
/// let group = world.spawn();
/// world.insert(group, UiGroup { clip: UiClipShape::Circle, ..UiGroup::default() }).unwrap();
/// let rect = world.spawn();
/// world.insert(rect, UiRect { group: Some(group), ..UiRect::default() }).unwrap();
///
/// let now = SimTime::from_ticks(0, TickRate::DEFAULT);
/// let viewport = (1920, 1080);
/// assert!(ui_hit(&world, rect, viewport, [50.0, 50.0], now)); // the centre
/// assert!(!ui_hit(&world, rect, viewport, [2.0, 2.0], now)); // the box corner, outside the circle
/// ```
pub fn ui_hit(
    world: &World,
    entity: EntityId,
    viewport: (u32, u32),
    cursor: [f32; 2],
    now: SimTime,
) -> bool {
    if !world.is_alive(entity) || !cursor.iter().all(|c| c.is_finite()) {
        return false;
    }
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let Ok(camera) = crate::extract::resolve_camera(world, &ids, viewport, None) else {
        return false;
    };
    let game_now = world
        .resource::<GameClock>()
        .map_or_else(|| GameTime::from(now), |clock| clock.game_instant(now));
    let frame = HudFrame {
        world,
        view: camera.view,
        projection: camera.projection,
        viewport,
        real_frame: now.ticks(),
        game_frame: game_now.ticks_floor(),
        rate: now.rate(),
    };
    let fonts = world.resource::<FontLibrary>();
    let face = |name: Option<&str>| face_of(fonts, name);
    let mut probe = Probe {
        frame,
        entity,
        cursor,
        poses: WorldAffines::new(),
        groups: UiGroups::default(),
        // Extract reports every degraded warning; a hit test only needs the geometry.
        degraded: Vec::new(),
    };

    if let Some(rect) = world.get::<UiRect>(entity).ok().flatten()
        && rect.visible
    {
        let rect = hud::pre_rect(&probe.frame, entity, rect, &mut probe.degraded);
        let rect_px = hud::rect_box(
            &probe.frame,
            &mut probe.poses,
            &mut probe.groups,
            &rect,
            &mut probe.degraded,
        );
        let row = UiInstance::Rect {
            rect_px,
            texture: None,
            color: rect.color,
            uv: SpriteInstance::FULL_UV,
            clip: UiClip::NONE,
            view: None,
        };
        let element = HudElement::Rect { fill: rect.fill };
        if probe.hit(element, rect.anchor, rect.group, rect.fill_group, row) {
            return true;
        }
    }
    if let Some(text) = world.get::<UiText>(entity).ok().flatten()
        && text.visible
    {
        let text = hud::pre_text(&probe.frame, entity, text, &mut probe.degraded);
        let row = UiInstance::Text {
            color: text.color,
            text: String::new(),
            quads: ui_text_draw(face(text.font.as_deref()), &text, viewport),
            font: None,
            clip: UiClip::NONE,
            outline: None,
        };
        if probe.hit(HudElement::Text, text.anchor, text.group, false, row) {
            return true;
        }
    }
    if let Some(list) = world.get::<UiList>(entity).ok().flatten()
        && list.visible
        && !list.items.is_empty()
    {
        let list = hud::pre_list(&probe.frame, entity, list, &mut probe.degraded);
        let (layout, _) = ui_list_draw(face(list.font.as_deref()), &list, viewport);
        let row = UiInstance::Rect {
            rect_px: layout.background_px,
            texture: None,
            color: list.background,
            uv: SpriteInstance::FULL_UV,
            clip: UiClip::NONE,
            view: None,
        };
        if probe.hit(HudElement::List, list.anchor, list.group, false, row) {
            return true;
        }
    }
    if let Some(radial) = world.get::<UiRadial>(entity).ok().flatten()
        && radial.visible
        && !radial.items.is_empty()
    {
        hud::pre_radial(&probe.frame, entity, &mut probe.degraded);
        // Ungrouped, the wheel is its sectors; grouped, the box the group places and clips.
        if radial.group.is_none() {
            return radial_hit(radial, viewport, cursor).is_some();
        }
        let layout = radial_layout(radial, viewport);
        let [cx, cy] = layout.center_px;
        let r = layout.radius_outer_px;
        let row = UiInstance::Rect {
            rect_px: [cx - r, cy - r, 2.0 * r, 2.0 * r],
            texture: None,
            color: radial.color,
            uv: SpriteInstance::FULL_UV,
            clip: UiClip::NONE,
            view: None,
        };
        if probe.hit(HudElement::Radial, radial.anchor, radial.group, false, row) {
            return true;
        }
    }
    false
}

fn face_of<'a>(fonts: Option<&'a FontLibrary>, name: Option<&str>) -> &'a FontFace {
    match fonts {
        Some(library) => library.face(name),
        None => FontFace::embedded(),
    }
}

/// The state one [`ui_hit`] walk shares across the entity's UI components.
struct Probe<'w> {
    frame: HudFrame<'w>,
    entity: EntityId,
    cursor: [f32; 2],
    poses: WorldAffines,
    groups: UiGroups,
    degraded: Vec<(String, Diagnostic)>,
}

impl Probe<'_> {
    /// Runs `row` through [`hud::post`] as the entity's only emitted instance, then tests the cursor
    /// against what is left: its bounding box (half-open) and its clip.
    fn hit(
        &mut self,
        element: HudElement,
        anchor: Anchor,
        group: Option<EntityId>,
        fill_group: bool,
        row: UiInstance,
    ) -> bool {
        let mut entries: Vec<UiEntry> = vec![(0, self.entity, row)];
        hud::post(
            &self.frame,
            &mut self.poses,
            self.entity,
            element,
            anchor,
            &mut entries,
            0,
            Grouping {
                group,
                fill_group,
                groups: &mut self.groups,
                degraded: &mut self.degraded,
            },
        );
        let Some(b) = hud::bounds(&entries) else {
            return false;
        };
        let (UiInstance::Rect { clip, .. } | UiInstance::Text { clip, .. }) = &entries[0].2;
        let [x, y] = self.cursor;
        x >= b[0] && x < b[2] && y >= b[1] && y < b[3] && clip_contains(clip, self.cursor)
    }
}
