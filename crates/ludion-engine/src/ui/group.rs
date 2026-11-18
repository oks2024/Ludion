//! `UiGroup` resolution (`Plan/ui-hud.md` decisions 18-24): a group's box, its composed affine,
//! opacity and visibility, memoized per extract, and the `check-scene` pass for its references.
//!
//! A group resolves like a `UiRect` (tween offset and size first), moved by [`group_delta`] into its
//! parent's box. Every group's `box_px` lives in one shared layout space; only the affines
//! (`p' = p * scale + offset`) differ, so a child element moves into the box first and takes the
//! group's affine last.

use std::collections::BTreeMap;

use ludion_components::ui::{UiGroup, UiWorldAnchor};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_scene::Scene;

use super::hud::{HudFrame, pre_group, world_anchor_delta};
use super::{group_delta, resolve_ui_rect};
use crate::hierarchy::WorldAffines;

/// The most groups one element may sit inside.
pub(crate) const MAX_GROUP_DEPTH: usize = 8;

/// A resolved group: its box in layout pixels and the composition it gives every descendant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GroupXf {
    /// `[x, y, w, h]` in the shared layout space, before any group's scale.
    pub box_px: [f32; 4],
    /// The composed uniform scale of this group and every ancestor.
    pub scale: f32,
    /// The composed translation: layout pixel `p` lands at `p * scale + offset`.
    pub offset: [f32; 2],
    /// The product of this group's and every ancestor's opacity.
    pub opacity: f32,
    /// This group and every ancestor are visible.
    pub visible: bool,
    /// How many groups this chain holds, this one included.
    pub depth: usize,
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
            match world_anchor_delta(frame, poses, anchor, group.anchor) {
                None => visible = false,
                Some((delta, alpha)) => {
                    box_px[0] += delta[0];
                    box_px[1] += delta[1];
                    opacity *= alpha;
                }
            }
        }
    }
    let centre = [box_px[0] + box_px[2] * 0.5, box_px[1] + box_px[3] * 0.5];
    let outer = parent.unwrap_or(GroupXf {
        box_px,
        scale: 1.0,
        offset: [0.0, 0.0],
        opacity: 1.0,
        visible: true,
        depth: 0,
    });
    let scale = motion.scale;
    GroupXf {
        box_px,
        scale: outer.scale * scale,
        offset: [
            outer.scale * centre[0] * (1.0 - scale) + outer.offset[0],
            outer.scale * centre[1] * (1.0 - scale) + outer.offset[1],
        ],
        opacity: outer.opacity * opacity,
        visible: outer.visible && visible,
        depth: outer.depth + 1,
    }
}

/// `L1615` (a `group` chain that loops), `L1616` (a chain of more than [`MAX_GROUP_DEPTH`] groups)
/// and `L2302` (a `group` naming an entity with no `UiGroup`) for every `UiRect`, `UiText`, `UiList`
/// and `UiGroup` in the scene. Pointers are `/entities/{i}/components/{Component}/group`.
pub fn scene_ui_group_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let group_of = |index: usize| -> Option<Option<usize>> {
        let entity = scene.entities.get(index)?;
        let (_, value) = entity
            .components
            .iter()
            .find(|(name, _)| name == "UiGroup")?;
        Some(reference(value))
    };
    let mut out = Vec::new();
    for (i, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            if !matches!(name.as_str(), "UiRect" | "UiText" | "UiList" | "UiGroup") {
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
