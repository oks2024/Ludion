//! `UiGroup` resolution (`Plan/ui-hud.md` decisions 18-24): a group's box, its composed affine,
//! opacity and visibility, memoized per extract, and the `check-scene` pass for its references.
//!
//! A group resolves like a `UiRect` (tween offset and size first), moved by [`group_delta`] into its
//! parent's box. Every group's `box_px` lives in one shared layout space; only the affines
//! (`p' = p * scale + offset`) differ, so a child element moves into the box first and takes the
//! group's affine last.

use std::collections::BTreeMap;

use ludion_components::ui::{UiClipShape, UiGroup, UiWorldAnchor};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_renderer::{UiClip, UiClipBox};
use ludion_scene::Scene;

use super::hud::{HudFrame, pre_group, world_anchor_delta};
use super::{canvas_anchor_px, group_delta, pixel_scale, resolve_ui_rect};
use crate::hierarchy::WorldAffines;

/// The most groups one element may sit inside.
pub(crate) const MAX_GROUP_DEPTH: usize = 8;

/// The most clipping groups that apply to one element (the innermost are kept).
pub(crate) const MAX_CLIPS: usize = 2;

/// `(cos, sin)` of a UI rotation in radians. Exactly 0 is `(1, 0)` with no trig, so an unrotated
/// chain never reaches the libm.
pub(crate) fn cos_sin(radians: f32) -> (f32, f32) {
    if radians == 0.0 {
        (1.0, 0.0)
    } else {
        let (sin, cos) = ludion_core::math::sin_cos(radians);
        (cos, sin)
    }
}

/// A resolved group: its box in layout pixels and the composition it gives every descendant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GroupXf {
    /// `[x, y, w, h]` in the shared layout space, before any group's scale.
    pub box_px: [f32; 4],
    /// The composed uniform scale of this group and every ancestor.
    pub scale: f32,
    /// The composed clockwise rotation, radians, of this group and every ancestor.
    pub rotation: f32,
    /// The composed translation: layout pixel `p` lands at `S · R(rotation) · p + offset`, which is
    /// `p * scale + offset` when `rotation` is 0.
    pub offset: [f32; 2],
    /// The product of this group's and every ancestor's opacity.
    pub opacity: f32,
    /// This group and every ancestor are visible.
    pub visible: bool,
    /// How many groups this chain holds, this one included.
    pub depth: usize,
    /// The clip this group and its ancestors give a descendant, innermost box first.
    pub clip: UiClip,
    /// The outermost group's entity when its world anchor placed this chain.
    pub anchor: Option<EntityId>,
}

/// One extract's memo of resolved groups (`None`: not a live, acyclic `UiGroup` chain within the
/// depth limit).
#[derive(Default)]
pub(crate) struct UiGroups {
    memo: BTreeMap<EntityId, Option<GroupXf>>,
    chain: Vec<EntityId>,
}

impl UiGroups {
    /// Resolves group `id`, or `None` when it is dead, has no `UiGroup`, sits on a cycle, or is
    /// more than [`MAX_GROUP_DEPTH`] groups deep.
    pub(crate) fn resolve(
        &mut self,
        frame: &HudFrame<'_>,
        poses: &mut WorldAffines,
        id: EntityId,
        degraded: &mut Vec<(String, Diagnostic)>,
    ) -> Option<GroupXf> {
        if let Some(done) = self.memo.get(&id) {
            return *done;
        }
        // Walk outward to the first memoized group (or the canvas), refusing a cycle, a missing
        // `UiGroup` and a chain past the depth limit; then compose back inward.
        self.chain.clear();
        let mut base: Option<GroupXf> = None;
        let mut next = Some(id);
        let mut ok = true;
        while let Some(at) = next {
            if let Some(done) = self.memo.get(&at) {
                ok = done.is_some();
                base = *done;
                break;
            }
            if self.chain.contains(&at) || self.chain.len() >= MAX_GROUP_DEPTH {
                ok = false;
                break;
            }
            let Some(group) = group_of(frame.world, at) else {
                ok = false;
                break;
            };
            self.chain.push(at);
            next = group.group;
        }
        if ok && base.map_or(0, |b| b.depth) + self.chain.len() > MAX_GROUP_DEPTH {
            ok = false;
        }
        if !ok {
            self.memo.insert(id, None);
            return None;
        }
        let mut parent = base;
        for at in std::mem::take(&mut self.chain).into_iter().rev() {
            let group = group_of(frame.world, at).expect("walked above");
            let xf = compose(frame, poses, at, group, parent, degraded);
            self.memo.insert(at, Some(xf));
            parent = Some(xf);
        }
        parent
    }
}

fn group_of(world: &World, id: EntityId) -> Option<&UiGroup> {
    if world.is_alive(id) {
        world.get::<UiGroup>(id).ok().flatten()
    } else {
        None
    }
}

fn compose(
    frame: &HudFrame<'_>,
    poses: &mut WorldAffines,
    id: EntityId,
    group: &UiGroup,
    parent: Option<GroupXf>,
    degraded: &mut Vec<(String, Diagnostic)>,
) -> GroupXf {
    let motion = pre_group(frame, id, group, degraded);
    let mut box_px = resolve_ui_rect(group.anchor, motion.offset, motion.size, frame.viewport);
    if let Some(outer) = &parent {
        let delta = group_delta(group.anchor, outer.box_px, frame.viewport);
        box_px[0] += delta[0];
        box_px[1] += delta[1];
    }
    let mut opacity = motion.opacity;
    let mut visible = group.visible;
    let mut anchor_scale = 1.0;
    let mut anchor_turn_deg = 0.0;
    let mut anchored = None;
    if let Some(anchor) = frame.world.get::<UiWorldAnchor>(id).ok().flatten() {
        if parent.is_some() {
            let key = crate::diagnostics::offender_key(frame.world, id);
            degraded.push((
                format!("{key}/ui-anchor-group"),
                Diagnostic::new(
                    code::UI_MOTION_UNSUPPORTED,
                    format!(
                        "{id} carries a `UiWorldAnchor` on a nested group, so its anchor is skipped"
                    ),
                )
                .with_help("anchor the outermost group; `ludion explain L2300`"),
            ));
        } else {
            match world_anchor_delta(frame, poses, id, anchor, group.anchor, degraded) {
                None => visible = false,
                Some(place) => {
                    anchored = Some(id);
                    box_px[0] += place.delta[0];
                    box_px[1] += place.delta[1];
                    opacity *= place.alpha;
                    if place.scale != 1.0 {
                        // Scale about the projected point: shift the box so scaling about its centre
                        // lands the same.
                        let pivot = canvas_anchor_px(group.anchor, frame.viewport);
                        let q = [pivot[0] + place.delta[0], pivot[1] + place.delta[1]];
                        let c = [box_px[0] + box_px[2] * 0.5, box_px[1] + box_px[3] * 0.5];
                        box_px[0] += (1.0 - place.scale) * (q[0] - c[0]);
                        box_px[1] += (1.0 - place.scale) * (q[1] - c[1]);
                        anchor_scale = place.scale;
                    }
                    anchor_turn_deg = place.turn_deg;
                }
            }
        }
    }
    let centre = [box_px[0] + box_px[2] * 0.5, box_px[1] + box_px[3] * 0.5];
    let outer = parent.unwrap_or(GroupXf {
        box_px,
        scale: 1.0,
        rotation: 0.0,
        offset: [0.0, 0.0],
        opacity: 1.0,
        visible: true,
        depth: 0,
        clip: UiClip::NONE,
        anchor: None,
    });
    let scale = if anchor_scale != 1.0 {
        motion.scale * anchor_scale
    } else {
        motion.scale
    };
    let composed_scale = outer.scale * scale;
    let own_rotation = if anchor_turn_deg != 0.0 {
        (motion.rotation + anchor_turn_deg).to_radians()
    } else {
        motion.rotation.to_radians()
    };
    let (composed_rotation, composed_offset) = if outer.rotation == 0.0 && own_rotation == 0.0 {
        (
            0.0,
            [
                outer.scale * centre[0] * (1.0 - scale) + outer.offset[0],
                outer.scale * centre[1] * (1.0 - scale) + outer.offset[1],
            ],
        )
    } else {
        // `O = A_outer(centre) − S · R(Θ) · centre`, so the group turns about its own centre.
        let (oc, os) = cos_sin(outer.rotation);
        let outer_at = [
            outer.scale * (oc * centre[0] - os * centre[1]) + outer.offset[0],
            outer.scale * (os * centre[0] + oc * centre[1]) + outer.offset[1],
        ];
        let rotation = outer.rotation + own_rotation;
        let (c, s) = cos_sin(rotation);
        (
            rotation,
            [
                outer_at[0] - composed_scale * (c * centre[0] - s * centre[1]),
                outer_at[1] - composed_scale * (s * centre[0] + c * centre[1]),
            ],
        )
    };
    let clip = compose_clip(
        frame,
        id,
        group,
        outer.clip,
        box_px,
        composed_scale,
        composed_rotation,
        composed_offset,
        degraded,
    );
    GroupXf {
        box_px,
        scale: composed_scale,
        rotation: composed_rotation,
        offset: composed_offset,
        opacity: outer.opacity * opacity,
        visible: outer.visible && visible,
        depth: outer.depth + 1,
        clip,
        anchor: anchored.or(outer.anchor),
    }
}

/// The clip a group gives its descendants: its own box (under its composed affine) in front of the
/// two innermost boxes it inherits, capped at [`MAX_CLIPS`] with `L2303` when one is dropped.
#[allow(clippy::too_many_arguments)]
fn compose_clip(
    frame: &HudFrame<'_>,
    id: EntityId,
    group: &UiGroup,
    inherited: UiClip,
    box_px: [f32; 4],
    scale: f32,
    rotation: f32,
    offset: [f32; 2],
    degraded: &mut Vec<(String, Diagnostic)>,
) -> UiClip {
    let size = [box_px[2] * scale, box_px[3] * scale];
    let rect = if rotation == 0.0 {
        [
            box_px[0] * scale + offset[0],
            box_px[1] * scale + offset[1],
            size[0],
            size[1],
        ]
    } else {
        // Decision 51: the box is centred on where the group maps its own centre, then turned.
        let (c, s) = cos_sin(rotation);
        let centre = [box_px[0] + box_px[2] * 0.5, box_px[1] + box_px[3] * 0.5];
        let at = [
            scale * (c * centre[0] - s * centre[1]) + offset[0],
            scale * (s * centre[0] + c * centre[1]) + offset[1],
        ];
        [
            at[0] - size[0] * 0.5,
            at[1] - size[1] * 0.5,
            size[0],
            size[1],
        ]
    };
    let shorter = rect[2].min(rect[3]);
    let own = match group.clip {
        UiClipShape::None => return inherited,
        UiClipShape::Rect => UiClipBox {
            rect_px: rect,
            radius_px: 0.0,
            radians: rotation,
        },
        UiClipShape::RoundedRect { corner_radius } => UiClipBox {
            rect_px: rect,
            radius_px: (corner_radius * pixel_scale(frame.viewport) * scale).min(shorter * 0.5),
            radians: rotation,
        },
        UiClipShape::Circle => UiClipBox {
            rect_px: [
                rect[0] + (rect[2] - shorter) * 0.5,
                rect[1] + (rect[3] - shorter) * 0.5,
                shorter,
                shorter,
            ],
            radius_px: shorter * 0.5,
            radians: rotation,
        },
    };
    if inherited.boxes[MAX_CLIPS - 1].is_some() {
        let key = crate::diagnostics::offender_key(frame.world, id);
        degraded.push((
            format!("{key}/ui-clip-depth"),
            Diagnostic::new(
                code::UI_CLIP_NESTING,
                format!(
                    "{id} is a third clipping group in a chain, so the outermost clip is dropped"
                ),
            )
            .with_help("keep at most two clipping groups in a chain; `ludion explain L2303`"),
        ));
    }
    UiClip {
        boxes: [Some(own), inherited.boxes[0]],
    }
}

/// Whether the pixel point `p` is inside `clip`: inside every present box, the Rust twin of
/// `ui_rect.wgsl`'s `clip_cov` (`d < 0`).
pub(crate) fn clip_contains(clip: &UiClip, p: [f32; 2]) -> bool {
    clip.boxes
        .iter()
        .flatten()
        .all(|b| rounded_box_sdf(p, b) < 0.0)
}

/// The signed distance from `p` to the rounded box, negative inside. A rotated box evaluates `p` in
/// its own frame (`R(−θ)` about its centre), as `ui_rect.wgsl`'s `box_point` does.
pub(crate) fn rounded_box_sdf(p: [f32; 2], b: &UiClipBox) -> f32 {
    let [x, y, w, h] = b.rect_px;
    let half = [w * 0.5, h * 0.5];
    let p = if b.radians == 0.0 {
        p
    } else {
        let (c, s) = cos_sin(b.radians);
        let d = [p[0] - (x + half[0]), p[1] - (y + half[1])];
        [
            x + half[0] + c * d[0] + s * d[1],
            y + half[1] - s * d[0] + c * d[1],
        ]
    };
    let q = [
        (p[0] - (x + half[0])).abs() - half[0] + b.radius_px,
        (p[1] - (y + half[1])).abs() - half[1] + b.radius_px,
    ];
    let outside = ludion_core::math::hypot(q[0].max(0.0), q[1].max(0.0));
    outside + q[0].max(q[1]).min(0.0) - b.radius_px
}

/// `L1615` (a `group` chain that loops), `L1616` (a chain of more than [`MAX_GROUP_DEPTH`] groups),
/// `L2302` (a `group` naming an entity with no `UiGroup`) and `L2303` (more than [`MAX_CLIPS`]
/// clipping groups in a chain) for every `UiRect`, `UiText`, `UiList`, `UiRadial`,
/// `UiShape` and `UiGroup` in the scene. Pointers are `/entities/{i}/components/{Component}/group`.
pub fn scene_ui_group_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let group_of = |index: usize| -> Option<Option<usize>> {
        let entity = scene.entities.get(index)?;
        let (_, value) = entity
            .components
            .iter()
            .find(|(name, _)| name == "UiGroup")?;
        Some(reference(value))
    };
    let clips = |value: &serde_json::Value| {
        value
            .get("clip")
            .is_some_and(|clip| clip.as_str() != Some("none"))
    };
    let mut out = Vec::new();
    for (i, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            if !matches!(
                name.as_str(),
                "UiRect" | "UiText" | "UiList" | "UiRadial" | "UiShape" | "UiGroup"
            ) {
                continue;
            }
            let Some(first) = reference(value) else {
                continue;
            };
            let pointer = format!("/entities/{i}/components/{name}/group");
            if first >= scene.entities.len() {
                continue;
            }
            let mut seen: Vec<usize> = Vec::new();
            let mut next = Some(first);
            let mut count = 0;
            let mut clipping = usize::from(name == "UiGroup" && clips(value));
            while let Some(at) = next {
                let Some(outer) = group_of(at) else {
                    if count == 0 {
                        out.push(
                            Diagnostic::new(
                                code::UI_GROUP_UNRESOLVED,
                                format!("`group` names entity {at}, which has no `UiGroup`"),
                            )
                            .with_path(pointer.clone())
                            .with_help("point `group` at an entity with a `UiGroup`"),
                        );
                    }
                    break;
                };
                count += 1;
                if let Some((_, outer_value)) = scene
                    .entities
                    .get(at)
                    .and_then(|e| e.components.iter().find(|(n, _)| n == "UiGroup"))
                    && clips(outer_value)
                {
                    clipping += 1;
                }
                if count > MAX_GROUP_DEPTH {
                    out.push(
                        Diagnostic::new(
                            code::UI_GROUP_TOO_DEEP,
                            format!("this chain of `group` references is more than {MAX_GROUP_DEPTH} groups deep"),
                        )
                        .with_path(pointer.clone())
                        .with_help("flatten the nesting; `ludion explain L1616`"),
                    );
                    break;
                }
                if seen.contains(&at) || (name == "UiGroup" && at == i) {
                    out.push(
                        Diagnostic::new(
                            code::UI_GROUP_CYCLE,
                            "this `group` reference loops back on itself",
                        )
                        .with_path(pointer.clone())
                        .with_help("point `group` at an outer group; `ludion explain L1615`"),
                    );
                    break;
                }
                seen.push(at);
                next = outer;
            }
            if clipping > MAX_CLIPS {
                out.push(
                    Diagnostic::new(
                        code::UI_CLIP_NESTING,
                        format!("this element sits under {clipping} clipping groups; only the innermost {MAX_CLIPS} clip"),
                    )
                    .with_path(pointer.clone())
                    .with_help("keep at most two clipping groups in a chain; `ludion explain L2303`"),
                );
            }
        }
    }
    out
}

/// The scene index a component's `group` field names, if it has one.
fn reference(component: &serde_json::Value) -> Option<usize> {
    let index = component.get("group")?.get("index")?.as_u64()?;
    usize::try_from(index).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::hud::HudFrame;
    use ludion_core::math::glam::Mat4;
    use ludion_core::time::TickRate;

    use crate::components::Anchor;

    fn frame(world: &World) -> HudFrame<'_> {
        HudFrame {
            world,
            view: Mat4::IDENTITY,
            projection: Mat4::IDENTITY,
            viewport: (1920, 1080),
            real_frame: 0,
            game_frame: 0,
            rate: TickRate::DEFAULT,
            anchor_views: Default::default(),
            anchor_record: None,
        }
    }

    fn spawn(world: &mut World, group: UiGroup) -> EntityId {
        let id = world.spawn();
        world.insert(id, group).unwrap();
        id
    }

    fn resolve(world: &World, groups: &mut UiGroups, id: EntityId) -> Option<GroupXf> {
        let mut degraded = Vec::new();
        groups.resolve(&frame(world), &mut WorldAffines::new(), id, &mut degraded)
    }

    #[test]
    fn group_delta_matches_resolving_inside_the_box() {
        let viewport = (1920, 1080);
        let anchors = [
            Anchor::TopLeft,
            Anchor::TopCenter,
            Anchor::TopRight,
            Anchor::CenterLeft,
            Anchor::Center,
            Anchor::CenterRight,
            Anchor::BottomLeft,
            Anchor::BottomCenter,
            Anchor::BottomRight,
        ];
        let group = resolve_ui_rect(Anchor::TopRight, [-24.0, 24.0], [360.0, 360.0], viewport);
        for anchor in anchors {
            let on_canvas = resolve_ui_rect(anchor, [12.0, -6.0], [40.0, 30.0], viewport);
            let delta = group_delta(anchor, group, viewport);
            // The same element resolved against the box: the box's own anchor fraction.
            let f = crate::ui::anchor_fraction(anchor);
            let expected = [
                group[0] + f[0] * group[2] + 12.0 - f[0] * 40.0,
                group[1] + f[1] * group[3] - 6.0 - f[1] * 30.0,
            ];
            assert_eq!(
                [on_canvas[0] + delta[0], on_canvas[1] + delta[1]],
                expected,
                "{anchor:?}"
            );
        }
    }

    #[test]
    fn nested_affine_composes_by_hand() {
        let mut world = World::new();
        let outer = spawn(
            &mut world,
            UiGroup {
                size: [400.0, 200.0],
                scale: 0.5,
                opacity: 0.5,
                ..UiGroup::default()
            },
        );
        let inner = spawn(
            &mut world,
            UiGroup {
                anchor: Anchor::Center,
                size: [100.0, 100.0],
                scale: 2.0,
                opacity: 0.5,
                group: Some(outer),
                ..UiGroup::default()
            },
        );
        let mut groups = UiGroups::default();
        let o = resolve(&world, &mut groups, outer).unwrap();
        assert_eq!(o.box_px, [0.0, 0.0, 400.0, 200.0]);
        assert_eq!((o.scale, o.offset), (0.5, [100.0, 50.0]));
        let i = resolve(&world, &mut groups, inner).unwrap();
        // Centred in the outer box: top-left (150, 50), centre (200, 100).
        assert_eq!(i.box_px, [150.0, 50.0, 100.0, 100.0]);
        // affine(outer) o scale_about(centre, 2): p -> 0.5 * (2p - c) + (100, 50)
        assert_eq!(i.scale, 1.0);
        assert_eq!(i.offset, [-(0.5 * 200.0) + 100.0, -(0.5 * 100.0) + 50.0]);
        assert_eq!(i.opacity, 0.25);
        assert!(i.visible);
    }

    #[test]
    fn an_invisible_parent_hides_the_child() {
        let mut world = World::new();
        let outer = spawn(
            &mut world,
            UiGroup {
                visible: false,
                ..UiGroup::default()
            },
        );
        let inner = spawn(
            &mut world,
            UiGroup {
                group: Some(outer),
                ..UiGroup::default()
            },
        );
        let mut groups = UiGroups::default();
        assert!(!resolve(&world, &mut groups, inner).unwrap().visible);
    }

    #[test]
    fn depth_eight_resolves_and_nine_does_not() {
        let mut world = World::new();
        let mut chain: Vec<EntityId> = Vec::new();
        for _ in 0..9 {
            let outer = chain.last().copied();
            chain.push(spawn(
                &mut world,
                UiGroup {
                    group: outer,
                    ..UiGroup::default()
                },
            ));
        }
        let mut groups = UiGroups::default();
        assert!(resolve(&world, &mut groups, chain[7]).is_some());
        assert!(resolve(&world, &mut groups, chain[8]).is_none());
    }

    #[test]
    fn a_cycle_and_a_missing_target_resolve_to_none() {
        let mut world = World::new();
        let a = spawn(&mut world, UiGroup::default());
        let b = spawn(
            &mut world,
            UiGroup {
                group: Some(a),
                ..UiGroup::default()
            },
        );
        world.get_mut::<UiGroup>(a).unwrap().unwrap().group = Some(b);
        let plain = world.spawn();
        let c = spawn(
            &mut world,
            UiGroup {
                group: Some(plain),
                ..UiGroup::default()
            },
        );
        let mut groups = UiGroups::default();
        assert!(resolve(&world, &mut groups, a).is_none());
        assert!(resolve(&world, &mut groups, b).is_none());
        assert!(resolve(&world, &mut groups, c).is_none());
    }

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn nested_quarter_turns_compose_to_a_half_turn() {
        let mut world = World::new();
        let outer = spawn(
            &mut world,
            UiGroup {
                size: [200.0, 100.0],
                rotation: 90.0,
                ..UiGroup::default()
            },
        );
        let inner = spawn(
            &mut world,
            UiGroup {
                anchor: Anchor::Center,
                size: [100.0, 100.0],
                rotation: 90.0,
                group: Some(outer),
                ..UiGroup::default()
            },
        );
        let mut groups = UiGroups::default();
        let o = resolve(&world, &mut groups, outer).unwrap();
        // Turned about its centre (100, 50): O = c − R(90°)c = (100, 50) − (−50, 100).
        assert!((o.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(near(o.offset, [150.0, -50.0]));
        let i = resolve(&world, &mut groups, inner).unwrap();
        // The inner box is centred at (100, 50), which the outer turn maps to itself; R(180°)c = −c.
        assert_eq!(i.box_px, [50.0, 0.0, 100.0, 100.0]);
        assert!((i.rotation - std::f32::consts::PI).abs() < 1e-6);
        assert!(near(i.offset, [200.0, 100.0]));
    }

    #[test]
    fn an_unrotated_chain_composes_with_the_scale_only_formula_bit_for_bit() {
        let mut world = World::new();
        let outer = spawn(
            &mut world,
            UiGroup {
                size: [400.0, 200.0],
                scale: 0.7,
                ..UiGroup::default()
            },
        );
        let inner = spawn(
            &mut world,
            UiGroup {
                anchor: Anchor::Center,
                size: [100.0, 100.0],
                scale: 1.3,
                group: Some(outer),
                ..UiGroup::default()
            },
        );
        let mut groups = UiGroups::default();
        let o = resolve(&world, &mut groups, outer).unwrap();
        let i = resolve(&world, &mut groups, inner).unwrap();
        assert_eq!((o.rotation.to_bits(), i.rotation.to_bits()), (0, 0));
        let centre = [
            i.box_px[0] + i.box_px[2] * 0.5,
            i.box_px[1] + i.box_px[3] * 0.5,
        ];
        let want = [
            o.scale * centre[0] * (1.0 - 1.3) + o.offset[0],
            o.scale * centre[1] * (1.0 - 1.3) + o.offset[1],
        ];
        assert_eq!(i.scale.to_bits(), (o.scale * 1.3).to_bits());
        assert_eq!(
            i.offset.map(f32::to_bits),
            want.map(f32::to_bits),
            "the unrotated formula runs unchanged"
        );
    }

    #[test]
    fn a_rotated_groups_clip_is_centred_on_its_mapped_centre() {
        let mut world = World::new();
        let id = spawn(
            &mut world,
            UiGroup {
                scale: 0.5,
                rotation: 90.0,
                ..clipping(UiClipShape::Rect, None)
            },
        );
        let mut groups = UiGroups::default();
        let xf = resolve(&world, &mut groups, id).unwrap();
        let b = xf.clip.boxes[0].unwrap();
        // A lone group turns about its own centre (100, 50), so the 200x100 box becomes 100x50 there.
        assert!(near([b.rect_px[0], b.rect_px[1]], [50.0, 25.0]));
        assert_eq!([b.rect_px[2], b.rect_px[3]], [100.0, 50.0]);
        assert_eq!(b.radians, xf.rotation);
        assert!((b.radians - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn a_rotated_box_is_evaluated_in_its_own_frame() {
        let b = UiClipBox {
            rect_px: [-50.0, -5.0, 100.0, 10.0],
            radius_px: 0.0,
            radians: std::f32::consts::FRAC_PI_4,
        };
        // (30, 30) is far above the flat box but on its long axis once the box is turned 45°.
        assert!(rounded_box_sdf([30.0, 30.0], &b) < 0.0);
        assert!(rounded_box_sdf([30.0, -30.0], &b) > 0.0);
        let flat = UiClipBox { radians: 0.0, ..b };
        assert!(rounded_box_sdf([30.0, 30.0], &flat) > 0.0);
    }

    fn clipping(clip: UiClipShape, group: Option<EntityId>) -> UiGroup {
        UiGroup {
            size: [200.0, 100.0],
            clip,
            group,
            ..UiGroup::default()
        }
    }

    #[test]
    fn rect_circle_and_rounded_clips_build_their_boxes() {
        let mut world = World::new();
        let rect = spawn(&mut world, clipping(UiClipShape::Rect, None));
        let circle = spawn(&mut world, clipping(UiClipShape::Circle, None));
        let rounded = spawn(
            &mut world,
            clipping(
                UiClipShape::RoundedRect {
                    corner_radius: 80.0,
                },
                None,
            ),
        );
        let plain = spawn(&mut world, clipping(UiClipShape::None, None));
        let mut groups = UiGroups::default();
        let boxes = |xf: GroupXf| xf.clip.boxes;
        let r = boxes(resolve(&world, &mut groups, rect).unwrap());
        assert_eq!(r[0].unwrap().rect_px, [0.0, 0.0, 200.0, 100.0]);
        assert_eq!((r[0].unwrap().radius_px, r[1]), (0.0, None));
        let c = boxes(resolve(&world, &mut groups, circle).unwrap())[0].unwrap();
        assert_eq!((c.rect_px, c.radius_px), ([50.0, 0.0, 100.0, 100.0], 50.0));
        // 80 canvas units at the 1080p canvas scale 1, clamped to half the shorter side.
        let rr = boxes(resolve(&world, &mut groups, rounded).unwrap())[0].unwrap();
        assert_eq!(rr.radius_px, 50.0);
        assert_eq!(
            resolve(&world, &mut groups, plain).unwrap().clip,
            UiClip::NONE
        );
    }

    #[test]
    fn a_scaled_group_scales_its_clip_box() {
        let mut world = World::new();
        let id = spawn(
            &mut world,
            UiGroup {
                scale: 0.5,
                ..clipping(
                    UiClipShape::RoundedRect {
                        corner_radius: 10.0,
                    },
                    None,
                )
            },
        );
        let mut groups = UiGroups::default();
        let b = resolve(&world, &mut groups, id).unwrap().clip.boxes[0].unwrap();
        // Scaled about the centre (100, 50): the 200x100 box becomes 100x50 at (50, 25).
        assert_eq!(b.rect_px, [50.0, 25.0, 100.0, 50.0]);
        assert_eq!(b.radius_px, 5.0);
    }

    #[test]
    fn nested_clips_keep_the_inner_box_first_and_a_third_is_dropped() {
        let mut world = World::new();
        let a = spawn(&mut world, clipping(UiClipShape::Rect, None));
        let b = spawn(&mut world, clipping(UiClipShape::Circle, Some(a)));
        let mut groups = UiGroups::default();
        let two = resolve(&world, &mut groups, b).unwrap().clip;
        assert_eq!(
            two.boxes[0].unwrap().radius_px,
            50.0,
            "the circle is innermost"
        );
        assert_eq!(two.boxes[1].unwrap().radius_px, 0.0);
        let c = spawn(&mut world, clipping(UiClipShape::Rect, Some(b)));
        let mut degraded = Vec::new();
        let mut poses = WorldAffines::new();
        let three = groups
            .resolve(&frame(&world), &mut poses, c, &mut degraded)
            .unwrap();
        assert_eq!(three.clip.boxes[0].unwrap().radius_px, 0.0);
        assert_eq!(
            three.clip.boxes[1], two.boxes[0],
            "the outermost box was dropped"
        );
        let codes: Vec<&str> = degraded.iter().map(|(_, d)| d.code.as_str()).collect();
        assert_eq!(codes, ["L2303"]);
    }

    #[test]
    fn clip_contains_is_half_open_at_the_edges() {
        let rect = UiClip {
            boxes: [
                Some(UiClipBox {
                    rect_px: [10.0, 10.0, 20.0, 20.0],
                    radius_px: 0.0,
                    radians: 0.0,
                }),
                None,
            ],
        };
        assert!(
            !clip_contains(&rect, [10.0, 20.0]),
            "an exact edge is outside (d < 0)"
        );
        assert!(clip_contains(&rect, [10.5, 20.0]));
        assert!(clip_contains(&rect, [20.0, 20.0]));
        assert!(!clip_contains(&rect, [30.5, 20.0]));
        assert!(!clip_contains(&rect, [9.5, 20.0]));
        let circle = UiClip {
            boxes: [
                Some(UiClipBox {
                    rect_px: [0.0, 0.0, 100.0, 100.0],
                    radius_px: 50.0,
                    radians: 0.0,
                }),
                None,
            ],
        };
        assert!(clip_contains(&circle, [50.0, 50.0]));
        assert!(
            !clip_contains(&circle, [2.0, 2.0]),
            "the box corner is outside the circle"
        );
        assert!(clip_contains(&UiClip::NONE, [-1e9, 1e9]));
    }

    fn scene(yaml: &str) -> Scene {
        ludion_scene::check(yaml, &crate::builtin_registry()).unwrap_or_else(|d| panic!("{d:?}"))
    }

    /// One scene entity holding `component`, grouped under entity `group` when given (block style:
    /// scene files refuse non-empty flow mappings).
    fn entity(component: &str, group: Option<usize>) -> String {
        match group {
            None => format!(
                "  - components:
      {component}: {{}}
"
            ),
            Some(index) => format!(
                "  - components:
      {component}:
        group:
          index: {index}
          generation: 0
"
            ),
        }
    }

    #[test]
    fn scene_pass_reports_cycles_depth_and_missing_groups() {
        // Entities 0 and 1 loop; entity 2 names an entity with no UiGroup.
        let looped = scene(&format!(
            "format: 1
entities:
{}{}{}{}",
            entity("UiGroup", Some(1)),
            entity("UiGroup", Some(0)),
            entity("UiRect", Some(3)),
            entity("Transform", None),
        ));
        let found = scene_ui_group_diagnostics(&looped);
        let codes: Vec<(&str, Option<&str>)> = found
            .iter()
            .map(|d| (d.code.as_str(), d.path.as_deref()))
            .collect();
        assert_eq!(
            codes,
            [
                ("L1615", Some("/entities/0/components/UiGroup/group")),
                ("L1615", Some("/entities/1/components/UiGroup/group")),
                ("L2302", Some("/entities/2/components/UiRect/group")),
            ]
        );
        // A chain of nine groups with a rect on the end.
        let mut yaml = String::from(
            "format: 1
entities:
",
        );
        yaml.push_str(&entity("UiGroup", None));
        for i in 1..9 {
            yaml.push_str(&entity("UiGroup", Some(i - 1)));
        }
        yaml.push_str(&entity("UiRect", Some(8)));
        let found = scene_ui_group_diagnostics(&scene(&yaml));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code.as_str(), "L1616");
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/9/components/UiRect/group")
        );
    }
}
