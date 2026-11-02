//! World transforms under a `Parent` hierarchy.
//!
//! [`Transform`] is local to its [`Parent`] when the entity has one, else world space; a root's
//! stored value *is* its world pose, bit for bit. [`world_transform`] composes root→leaf with
//! [`Trs::mul_trs`] (the sim result: shear under a non-uniform scale is dropped);
//! [`world_affine`] folds the exact [`Affine3`] per level (what render draws). An entity carrying
//! `ParentJoint` folds its parent skin's joint rest model transform as one more level before its own
//! `Transform`; a miss composes as a plain `Parent`.
//!
//! Three malformed chains degrade the queried entity instead of hanging or panicking, and
//! `report_hierarchy_faults` names each once: a `Parent` loop (`L1218`), a dead or presentation
//! parent of a sim entity (`L1219`, the chain stops at the last valid entity) and more than
//! [`MAX_HIERARCHY_DEPTH`] ancestors (`L1220`). A cycle or a too-deep chain makes the entity a root.

use std::collections::BTreeSet;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::{Affine3, Trs, euler_xyz_degrees_to_quat, quat_to_euler_xyz_degrees};
use ludion_ecs::{ChildMap, PRESENTATION_BASE, Parent, World, is_presentation};
use ludion_renderer::DebugSegment;
use ludion_spatial::{Body, Collider, ParentPose};

use crate::components::{Fade, Transform, Visibility};

/// The most ancestors a chain may have before the entity is treated as a root.
pub const MAX_HIERARCHY_DEPTH: usize = 64;

/// Why an entity's `Parent` chain could not be followed to its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HierarchyFault {
    /// The chain loops; the entity is a root.
    Cycle,
    /// This parent is dead, or a presentation entity above a sim one; the chain stops below it.
    InvalidParent(EntityId),
    /// More than [`MAX_HIERARCHY_DEPTH`] ancestors; the entity is a root.
    TooDeep,
}

/// An entity's composed world pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldPose {
    /// The composed translation, rotation and scale (shear dropped).
    pub trs: Trs,
    /// A level composed a rotated child under a non-uniform accumulated scale, so `trs` is not the
    /// exact world transform; [`world_affine`] keeps the shear.
    pub sheared: bool,
}

/// `entity`'s `Parent`. Records no access (neither the presence guard nor the unrecorded read), so
/// a system calling `world_transform` needs no `Parent` query declared for the query-drift gate.
pub(crate) fn parent_of(world: &World, entity: EntityId) -> Option<Parent> {
    if !world.has::<Parent>(entity) {
        return None;
    }
    world
        .get_unrecorded::<Parent>(entity)
        .ok()
        .flatten()
        .copied()
}

/// The entity's ancestors, root first and ending with the entity, plus the fault that cut the walk.
fn chain(world: &World, entity: EntityId) -> (Vec<EntityId>, Option<HierarchyFault>) {
    let mut up = vec![entity];
    let mut current = entity;
    let mut fault = None;
    while let Some(Parent(parent)) = parent_of(world, current) {
        if !world.is_alive(parent) || (is_presentation(parent) && !is_presentation(current)) {
            fault = Some(HierarchyFault::InvalidParent(parent));
            break;
        }
        if up.contains(&parent) {
            return (vec![entity], Some(HierarchyFault::Cycle));
        }
        if up.len() > MAX_HIERARCHY_DEPTH {
            return (vec![entity], Some(HierarchyFault::TooDeep));
        }
        up.push(parent);
        current = parent;
    }
    up.reverse();
    (up, fault)
}

/// The topmost ancestor of `entity` through `Parent`: the entity itself when it has none or its
/// chain is a cycle or too deep. A dead or presentation parent cuts the walk, so the result is the
/// last valid ancestor below it.
pub(crate) fn hierarchy_root(world: &World, entity: EntityId) -> EntityId {
    chain(world, entity).0[0]
}

/// `entity`'s own `Fade`, else its nearest ancestor's through `Parent`, else the default. An
/// unparented entity costs one read and one presence check.
pub(crate) fn inherited_fade(world: &World, entity: EntityId) -> Fade {
    let own = |id: EntityId| world.get::<Fade>(id).ok().flatten().copied();
    if let Some(fade) = own(entity) {
        return fade;
    }
    if !world.has::<Parent>(entity) {
        return Fade::default();
    }
    let (up, _) = chain(world, entity);
    up.iter()
        .rev()
        .skip(1)
        .find_map(|&ancestor| own(ancestor))
        .unwrap_or_default()
}

/// `entity`'s effective [`Visibility`]: the AND of its own and every `Parent` ancestor's (a hidden
/// ancestor hides the whole tree; a part's own `visible: true` never shows it). A world with no
/// `Visibility` anywhere answers the default after one O(1) count and never walks a chain.
pub(crate) fn inherited_visibility(world: &World, entity: EntityId) -> Visibility {
    if world.component_count::<Visibility>() == 0 {
        return Visibility::default();
    }
    let own = |id: EntityId| world.get::<Visibility>(id).ok().flatten().copied();
    let ids = if world.has::<Parent>(entity) {
        chain(world, entity).0
    } else {
        vec![entity]
    };
    ids.into_iter()
        .filter_map(own)
        .fold(Visibility::default(), |acc, v| Visibility {
            visible: acc.visible && v.visible,
            cast_shadows: acc.cast_shadows && v.cast_shadows,
        })
}

/// The entities hidden by [`Visibility`] this frame, computed once so each extract pass asks a set
/// instead of walking `Parent` chains. Empty, with no walk, when the world has no `Visibility`.
/// `shadowless` holds the visible entities that cast no shadow; a hidden entity is in `hidden` only.
pub(crate) struct FrameVisibility {
    hidden: BTreeSet<EntityId>,
    shadowless: BTreeSet<EntityId>,
}

impl FrameVisibility {
    pub(crate) fn of(world: &World, ids: &[EntityId]) -> Self {
        let mut hidden = BTreeSet::new();
        let mut shadowless = BTreeSet::new();
        if world.component_count::<Visibility>() != 0 {
            for &id in ids {
                let seen = inherited_visibility(world, id);
                if !seen.visible {
                    hidden.insert(id);
                } else if !seen.cast_shadows {
                    shadowless.insert(id);
                }
            }
        }
        Self { hidden, shadowless }
    }

    pub(crate) fn hidden(&self, id: EntityId) -> bool {
        self.hidden.contains(&id)
    }

    /// Whether `id` writes shadow maps: false when it, or an ancestor, sets `cast_shadows: false`.
    pub(crate) fn casts(&self, id: EntityId) -> bool {
        !self.shadowless.contains(&id)
    }
}

pub(crate) fn level_trs(transform: &Transform) -> Trs {
    Trs {
        translation: transform.translation,
        rotation: euler_xyz_degrees_to_quat(transform.rotation),
        scale: transform.scale,
    }
}

fn local(world: &World, id: EntityId) -> Option<Transform> {
    world.get::<Transform>(id).ok().flatten().copied()
}

/// `entity`'s world pose; `None` iff it has no `Transform`. An ancestor without one contributes
/// identity.
pub fn world_transform(world: &World, entity: EntityId) -> Option<WorldPose> {
    local(world, entity)?;
    chain_pose(world, entity)
}

/// The fold of [`world_transform`] without requiring `entity` itself to have a `Transform`.
pub(crate) fn chain_pose(world: &World, entity: EntityId) -> Option<WorldPose> {
    let (ids, _) = chain(world, entity);
    let mut acc: Option<Trs> = None;
    let mut sheared = false;
    let mut fold = |acc: &mut Option<Trs>, level: Trs| {
        *acc = Some(match *acc {
            None => level,
            Some(parent) => {
                let uniform =
                    parent.scale[0] == parent.scale[1] && parent.scale[1] == parent.scale[2];
                sheared |= !uniform && level.rotation != Trs::IDENTITY.rotation;
                parent.mul_trs(&level)
            }
        });
    };
    for (at, id) in ids.iter().enumerate() {
        if at > 0
            && let Some(joint) = crate::parent_joint::joint_rest(world, *id)
        {
            fold(&mut acc, joint);
        }
        if let Some(transform) = local(world, *id) {
            fold(&mut acc, level_trs(&transform));
        }
    }
    acc.map(|trs| WorldPose { trs, sheared })
}

/// `entity`'s exact world affine (shear kept); `None` iff it has no `Transform`.
pub fn world_affine(world: &World, entity: EntityId) -> Option<Affine3> {
    local(world, entity)?;
    let (ids, _) = chain(world, entity);
    let mut acc: Option<Affine3> = None;
    let fold = |acc: &mut Option<Affine3>, level: Affine3| {
        *acc = Some(match *acc {
            None => level,
            Some(parent) => parent.mul_affine(&level),
        });
    };
    for (at, id) in ids.iter().enumerate() {
        if at > 0
            && let Some(joint) = crate::parent_joint::joint_rest(world, *id)
        {
            fold(&mut acc, joint.to_affine());
        }
        if let Some(transform) = local(world, *id) {
            fold(&mut acc, level_trs(&transform).to_affine());
        }
    }
    acc
}

/// A value per entity slot, valid only for the generation it was stored under. Dense, so a lookup
/// is an index; the sim and presentation ranges each get their own vector.
struct SlotTable<T: Copy> {
    sim: Vec<Option<(u32, T)>>,
    presentation: Vec<Option<(u32, T)>>,
}

impl<T: Copy> SlotTable<T> {
    fn new() -> Self {
        Self {
            sim: Vec::new(),
            presentation: Vec::new(),
        }
    }

    fn get(&self, id: EntityId) -> Option<T> {
        let (slots, at) = self.slots(id);
        match slots.get(at) {
            Some(Some((generation, value))) if *generation == id.generation() => Some(*value),
            _ => None,
        }
    }

    fn set(&mut self, id: EntityId, value: T) {
        let generation = id.generation();
        let (slots, at) = self.slots_mut(id);
        if slots.len() <= at {
            slots.resize(at + 1, None);
        }
        slots[at] = Some((generation, value));
    }

    fn slots(&self, id: EntityId) -> (&Vec<Option<(u32, T)>>, usize) {
        if is_presentation(id) {
            (
                &self.presentation,
                (id.index() - PRESENTATION_BASE) as usize,
            )
        } else {
            (&self.sim, id.index() as usize)
        }
    }

    fn slots_mut(&mut self, id: EntityId) -> (&mut Vec<Option<(u32, T)>>, usize) {
        if is_presentation(id) {
            (
                &mut self.presentation,
                (id.index() - PRESENTATION_BASE) as usize,
            )
        } else {
            (&mut self.sim, id.index() as usize)
        }
    }
}

/// A memoized entity: its [`world_affine`] accumulator (`None` while no level up to it has a
/// `Transform`) and how many ancestors its chain has.
type Memo = (Option<Affine3>, u32);

/// A per-batch memo of [`world_affine`] for parented entities. Resuming the fold from a memoized
/// ancestor's accumulator is bit-identical to folding from the root: an ancestor's chain is a
/// prefix of its descendant's, and the fold is left-to-right. Only valid chains are memoized, so a
/// memoized entity is never on a cycle.
///
/// A node may carry a **posed local** ([`set_posed`](WorldAffines::set_posed)): a presentation-only
/// pose that replaces its stored `Transform` in the fold, so the entities below it follow. Poses are
/// set before the first lookup of a batch (a memo would hold the stale fold). Only the batch that
/// owns the table sees them: the extract batch (point and spot lights, rendered probes, decals,
/// particles) follows, while a camera, the sun, the `probes` debug lines and colliders are built
/// with a fresh table and draw from the stored pose. A `ParentJoint` child may likewise carry a
/// **posed joint** ([`set_posed_joint`](WorldAffines::set_posed_joint)) that replaces its rest
/// joint level; one set on an already memoized child is detected by `pose_sockets` (extract.rs),
/// which clears the memo and reruns.
pub(crate) struct WorldAffines {
    memo: SlotTable<Memo>,
    posed: SlotTable<Trs>,
    posed_joint: SlotTable<Affine3>,
}

impl WorldAffines {
    pub(crate) fn new() -> Self {
        Self {
            memo: SlotTable::new(),
            posed: SlotTable::new(),
            posed_joint: SlotTable::new(),
        }
    }

    /// Drops every memoized fold, keeping the posed locals and joints.
    pub(crate) fn clear_memo(&mut self) {
        self.memo = SlotTable::new();
    }

    /// Whether a lookup already memoized `entity`'s fold.
    pub(crate) fn is_memoized(&self, entity: EntityId) -> bool {
        self.memo.get(entity).is_some()
    }

    /// The posed joint level set on `entity`, if any.
    pub(crate) fn posed_joint(&self, entity: EntityId) -> Option<Affine3> {
        self.posed_joint.get(entity)
    }

    /// Poses `entity`'s `ParentJoint` level as `joint` for this batch, in place of the joint's rest.
    pub(crate) fn set_posed_joint(&mut self, entity: EntityId, joint: Affine3) {
        self.posed_joint.set(entity, joint);
    }

    /// Poses `entity`'s local transform as `trs` for this batch, in place of its stored `Transform`.
    pub(crate) fn set_posed(&mut self, entity: EntityId, trs: Trs) {
        self.posed.set(entity, trs);
    }

    /// The local `Trs` the fold uses for `node`: its posed local, else its stored `Transform`.
    fn level(&self, world: &World, node: EntityId) -> Option<Trs> {
        self.posed
            .get(node)
            .or_else(|| local(world, node).map(|transform| level_trs(&transform)))
    }

    /// `entity`'s world affine when it has a `Parent` and a `Transform` and its chain has more than
    /// itself, or when it is posed; `None` for an unposed root or an entity degraded to one, which
    /// the caller draws from its stored `Transform`. Otherwise equal to [`world_affine`], bit for
    /// bit. Walks up only to the first memoized ancestor, with no allocation.
    pub(crate) fn parented(&mut self, world: &World, entity: EntityId) -> Option<Affine3> {
        if !world.has::<Parent>(entity) {
            return self.posed.get(entity).map(|trs| trs.to_affine());
        }
        if self.posed.get(entity).is_none() {
            local(world, entity)?;
        }
        if let Some((acc, depth)) = self.memo.get(entity) {
            return if depth > 0 { acc } else { None };
        }
        let mut stack = [entity; MAX_HIERARCHY_DEPTH + 2];
        let mut len = 0;
        let mut current = entity;
        let mut base: Option<Memo> = None;
        loop {
            if len == stack.len() {
                return None;
            }
            stack[len] = current;
            len += 1;
            let Some(Parent(parent)) = parent_of(world, current) else {
                break;
            };
            if !world.is_alive(parent) || (is_presentation(parent) && !is_presentation(current)) {
                break;
            }
            if let Some(hit) = self.memo.get(parent) {
                base = Some(hit);
                break;
            }
            if stack[..len].contains(&parent) {
                return None;
            }
            current = parent;
        }
        let top_depth = base.map_or(0, |(_, depth)| depth + 1);
        if top_depth as usize + len - 1 > MAX_HIERARCHY_DEPTH {
            return None;
        }
        let mut acc = base.and_then(|(acc, _)| acc);
        for (step, node) in stack[..len].iter().rev().enumerate() {
            // The joint level needs the node's parent in the folded chain: not the top of a walk
            // that broke off at a root or an invalid parent.
            if (step > 0 || base.is_some())
                && let Some(joint) = self.posed_joint.get(*node).or_else(|| {
                    crate::parent_joint::joint_rest(world, *node).map(|rest| rest.to_affine())
                })
            {
                acc = Some(match acc {
                    None => joint,
                    Some(parent) => parent.mul_affine(&joint),
                });
            }
            if let Some(level) = self.level(world, *node) {
                let level = level.to_affine();
                acc = Some(match acc {
                    None => level,
                    Some(parent) => parent.mul_affine(&level),
                });
            }
            self.memo.set(*node, (acc, top_depth + step as u32));
        }
        if top_depth as usize + len - 1 == 0 {
            None
        } else {
            acc
        }
    }
}

/// The world pose as a [`Transform`]: the stored value verbatim for a root (or an entity degraded
/// to one), else the composed pose with the rotation back in Euler degrees.
pub fn world_pose_transform(world: &World, entity: EntityId) -> Option<Transform> {
    let own = local(world, entity)?;
    let (ids, _) = chain(world, entity);
    if ids.len() == 1 {
        return Some(own);
    }
    let pose = world_transform(world, entity)?;
    Some(Transform {
        translation: pose.trs.translation,
        rotation: quat_to_euler_xyz_degrees(pose.trs.rotation),
        scale: pose.trs.scale,
    })
}

/// The `hierarchy` debug-draw color: linear cyan.
const DEBUG_LINE_COLOR: [f32; 4] = [0.2, 0.8, 1.0, 1.0];

fn each_entity(world: &World) -> impl Iterator<Item = EntityId> + '_ {
    world.entities().chain(world.presentation_entities())
}

/// Whether any entity has a `Parent`.
pub(crate) fn any_parent(world: &World) -> bool {
    each_entity(world).any(|id| world.has::<Parent>(id))
}

/// One segment per parented entity, from its parent's world origin to its own, for the `hierarchy`
/// debug-draw category. An entity or parent with no `Transform` draws nothing.
pub(crate) fn debug_lines(world: &World) -> Vec<DebugSegment> {
    each_entity(world)
        .filter_map(|id| {
            let parent = parent_of(world, id)?.0;
            let a = world_affine(world, parent)?.transform_point([0.0; 3]);
            let b = world_affine(world, id)?.transform_point([0.0; 3]);
            Some(DebugSegment {
                a,
                b,
                color: DEBUG_LINE_COLOR,
            })
        })
        .collect()
}

/// What `reparent` does with the child's `Transform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Rewrite the local `Transform` so the world position is unchanged; rotation and scale are
    /// lossy only under a non-uniform scale with rotation, where the composed pose carries a shear
    /// a `Transform` cannot hold.
    World,
    /// Leave the `Transform` bits alone; the entity moves with its new parent.
    Local,
}

/// Re-points `child`'s `Parent` at `parent` (`None` removes it), refusing what the hierarchy rules
/// forbid: a dead entity (`L0002`), a cycle (`L1224`), a chain deeper than the limit (`L1260`), a sim
/// child under a presentation parent (`L1225`), a rigid or static solver-claimed body under a parent
/// (`L1221`/`L1222`). A refused call mutates nothing.
pub fn reparent(
    world: &mut World,
    child: EntityId,
    parent: Option<EntityId>,
    keep: Keep,
) -> Result<(), Diagnostic> {
    if !world.is_alive(child) {
        world.get::<Parent>(child)?;
    }
    if let Some(parent) = parent {
        if !world.is_alive(parent) {
            world.get::<Parent>(parent)?;
        }
        let mut walked = vec![parent];
        while let Some(&up) = walked.last() {
            if up == child {
                return Err(Diagnostic::new(
                    code::HIERARCHY_REPARENT_CYCLE,
                    format!("reparenting {child} under {parent} would make it its own ancestor"),
                )
                .with_help("pick a parent outside the entity's own subtree"));
            }
            let Some(Parent(next)) = parent_of(world, up) else {
                break;
            };
            // The walk stops where `chain` cuts one: an ancestor link to a dead entity, or to a
            // presentation entity above a sim one, is not counted.
            if is_invalid_parent(world, up, next) {
                break;
            }
            if walked.contains(&next) {
                return Err(Diagnostic::new(
                    code::HIERARCHY_REPARENT_CYCLE,
                    format!("{parent}'s ancestry already loops through {next}"),
                )
                .with_help(
                    "break the existing loop first: reparent one of its members with `parent: None`",
                ));
            }
            walked.push(next);
        }
        if walked.len() > MAX_HIERARCHY_DEPTH {
            return Err(Diagnostic::new(
                code::HIERARCHY_REPARENT_TOO_DEEP,
                format!(
                    "reparenting {child} under {parent} would give it {} ancestors, more than \
                     {MAX_HIERARCHY_DEPTH}",
                    walked.len()
                ),
            )
            .with_help("pick a shallower parent"));
        }
        // Phase-free: a presentation descendant counts against the cap like a sim one.
        let height = if world.has_children(child) {
            subtree_height(&ChildMap::build_full(world), child)
        } else {
            0
        };
        if walked.len() + height > MAX_HIERARCHY_DEPTH {
            return Err(Diagnostic::new(
                code::HIERARCHY_REPARENT_TOO_DEEP,
                format!(
                    "reparenting {child} under {parent} would give its deepest descendant {} \
                     ancestors, more than {MAX_HIERARCHY_DEPTH}",
                    walked.len() + height
                ),
            )
            .with_help("pick a shallower parent, or reparent the deep descendants first"));
        }
        if is_presentation(parent) && !is_presentation(child) {
            return Err(Diagnostic::new(
                code::HIERARCHY_REPARENT_PRESENTATION,
                format!("{child} is a sim entity and {parent} is a presentation entity"),
            )
            .with_help("parent the entity under another sim entity, or under nothing"));
        }
        if let Ok(Some(collider)) = world.get::<Collider>(child)
            && ludion_physics::claims(collider)
        {
            let refusal = match collider.body {
                Body::Rigid(_) => Some((
                    code::HIERARCHY_RIGID_BODY,
                    "a rigid body",
                    "un-parent it, or join it to the parent with a `Joint`",
                )),
                Body::Static(_) => Some((
                    code::HIERARCHY_STATIC_COLLIDER,
                    "a static solver collider",
                    "make it `body: kinematic`, or un-parent it",
                )),
                Body::Kinematic(_) => None,
            };
            if let Some((diagnostic, kind, help)) = refusal {
                return Err(Diagnostic::new(
                    diagnostic,
                    format!("{child} is {kind} and would get no physics body under {parent}"),
                )
                .with_help(help));
            }
        }
    }

    let new_local = match (keep, local(world, child)) {
        (Keep::World, Some(_)) => Some(match parent {
            Some(parent) => {
                let parent_world = chain_pose(world, parent).map_or(Trs::IDENTITY, |p| p.trs);
                let child_world = world_transform(world, child).map(|p| p.trs);
                child_world.map(|trs| {
                    let local = parent_world.localize(&trs);
                    Transform {
                        translation: local.translation,
                        rotation: quat_to_euler_xyz_degrees(local.rotation),
                        scale: local.scale,
                    }
                })
            }
            None => world_pose_transform(world, child),
        }),
        _ => None,
    }
    .flatten();

    match parent {
        Some(parent) => {
            world.insert(child, Parent(parent))?;
        }
        None => {
            world.remove::<Parent>(child)?;
        }
    }
    if let Some(transform) = new_local {
        world.insert(child, transform)?;
    }
    Ok(())
}

/// Levels below `root` in its subtree (0 for a leaf); stops past the depth cap, and a cycle
/// terminates through the visited set.
fn subtree_height(map: &ChildMap, root: EntityId) -> usize {
    let mut seen = BTreeSet::from([root]);
    let mut level = vec![root];
    let mut height = 0;
    while height <= MAX_HIERARCHY_DEPTH {
        let next: Vec<EntityId> = level
            .iter()
            .flat_map(|&node| map.children(node))
            .copied()
            .filter(|child| seen.insert(*child))
            .collect();
        if next.is_empty() {
            break;
        }
        height += 1;
        level = next;
    }
    height
}

/// How many live ancestors `entity`'s chain has (0 for a root or an entity degraded to one).
pub(crate) fn ancestor_count(world: &World, entity: EntityId) -> usize {
    chain(world, entity).0.len() - 1
}

/// `entity`'s composed world pose when it has a live ancestor; `None` for a root or an entity
/// degraded to one (a cycle, a too-deep chain, a dead direct parent — `L1218`–`L1220`).
pub(crate) fn parented_world_pose(world: &World, entity: EntityId) -> Option<WorldPose> {
    if !world.has::<Parent>(entity) {
        return None;
    }
    if chain(world, entity).0.len() == 1 {
        return None;
    }
    world_transform(world, entity)
}

/// The [`ParentPose`] resolver for `Transform`: composes the world pose through the `Parent` chain.
pub struct TransformHierarchy;

impl ParentPose for TransformHierarchy {
    fn world_pose(world: &World, entity: EntityId) -> Option<([f32; 3], [f32; 4])> {
        parented_world_pose(world, entity).map(|p| (p.trs.translation, p.trs.rotation))
    }
}

/// Reports `L1218`–`L1220` once per entity (once per loop for a cycle) through the `Diagnostics`
/// latch. Costs one empty query when no entity has a `Parent`.
pub(crate) fn report_hierarchy_faults(world: &mut World) {
    let ids: Vec<EntityId> = world.query_ids::<(&Parent,)>().map(|(id, _)| id).collect();
    let mut scratch = Vec::new();
    let mut good = SlotTable::new();
    for id in ids {
        let Some(fault) = fault_of(world, id, &mut scratch, &mut good) else {
            continue;
        };
        // The latch is probed before the message is formatted: a fault persists every tick.
        match fault {
            HierarchyFault::InvalidParent(parent) => {
                let key = format!("{}/Parent", crate::diagnostics::offender_key(world, id));
                if !crate::diagnostics::is_latched(world, code::HIERARCHY_PARENT_INVALID, &key) {
                    crate::diagnostics::report(
                        world,
                        code::HIERARCHY_PARENT_INVALID,
                        key,
                        format!(
                            "{id}'s Parent {parent} is dead or a presentation entity; the chain \
                             stops there and {id} composes only with its live ancestors"
                        ),
                    );
                }
            }
            HierarchyFault::TooDeep => {
                let key = format!("{}/Parent", crate::diagnostics::offender_key(world, id));
                if !crate::diagnostics::is_latched(world, code::HIERARCHY_TOO_DEEP, &key) {
                    crate::diagnostics::report(
                        world,
                        code::HIERARCHY_TOO_DEEP,
                        key,
                        format!(
                            "{id} has more than {MAX_HIERARCHY_DEPTH} ancestors; it is treated \
                             as a root"
                        ),
                    );
                }
            }
            HierarchyFault::Cycle => {
                let members = cycle_members(world, id);
                let key = members.iter().min().copied().unwrap_or(id);
                let key = format!("{}/Parent", crate::diagnostics::offender_key(world, key));
                if !crate::diagnostics::is_latched(world, code::HIERARCHY_CYCLE, &key) {
                    crate::diagnostics::report(
                        world,
                        code::HIERARCHY_CYCLE,
                        key,
                        format!(
                            "the Parent chain of {id} loops through {members:?}; {id} is treated \
                             as a root"
                        ),
                    );
                }
            }
        }
    }
}

/// Whether `child`'s `parent` is dead, or a presentation entity above a sim one.
fn is_invalid_parent(world: &World, child: EntityId, parent: EntityId) -> bool {
    !world.is_alive(parent) || (is_presentation(parent) && !is_presentation(child))
}

/// The fault [`chain`] would report for `id`, without its allocation: walks up only to the first
/// entity in `good` (a fault-free chain, stored with its ancestor count), then marks the walked path.
fn fault_of(
    world: &World,
    id: EntityId,
    scratch: &mut Vec<EntityId>,
    good: &mut SlotTable<u32>,
) -> Option<HierarchyFault> {
    scratch.clear();
    if let Some(Parent(parent)) = parent_of(world, id)
        && is_invalid_parent(world, id, parent)
    {
        return Some(HierarchyFault::InvalidParent(parent));
    }
    let mut current = id;
    let top_depth = loop {
        if let Some(depth) = good.get(current) {
            if depth as usize + scratch.len() > MAX_HIERARCHY_DEPTH {
                return Some(HierarchyFault::TooDeep);
            }
            break depth + 1;
        }
        scratch.push(current);
        let Some(Parent(parent)) = parent_of(world, current) else {
            break 0;
        };
        if is_invalid_parent(world, current, parent) {
            // An ancestor's broken link is that ancestor's to report; the chain just stops there.
            break 0;
        }
        if scratch.contains(&parent) {
            return Some(HierarchyFault::Cycle);
        }
        if scratch.len() > MAX_HIERARCHY_DEPTH {
            return Some(HierarchyFault::TooDeep);
        }
        current = parent;
    };
    let len = scratch.len() as u32;
    for (step, node) in scratch.iter().enumerate() {
        good.set(*node, top_depth + len - 1 - step as u32);
    }
    None
}

/// The entities on the loop the walk from `start` runs into (the tail leading to it excluded).
fn cycle_members(world: &World, start: EntityId) -> Vec<EntityId> {
    let mut seen = vec![start];
    let mut current = start;
    while let Some(Parent(parent)) = parent_of(world, current) {
        if let Some(at) = seen.iter().position(|id| *id == parent) {
            return seen.split_off(at);
        }
        seen.push(parent);
        current = parent;
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Transform {
        Transform {
            translation: [x, 0.0, 0.0],
            ..Transform::default()
        }
    }

    fn spawn(world: &mut World, transform: Transform, parent: Option<EntityId>) -> EntityId {
        let id = world.spawn();
        world.insert(id, transform).unwrap();
        if let Some(parent) = parent {
            world.insert(id, Parent(parent)).unwrap();
        }
        id
    }

    #[test]
    fn parented_world_pose_is_none_for_roots_and_degraded_entities() {
        let mut world = World::new();
        let root = spawn(&mut world, at(1.0), None);
        let child = spawn(&mut world, at(2.0), Some(root));
        let gone = spawn(&mut world, at(0.0), None);
        world.despawn(gone).unwrap();
        let orphan = spawn(&mut world, at(5.0), Some(gone));
        assert!(parented_world_pose(&world, root).is_none());
        assert_eq!(
            parented_world_pose(&world, child).unwrap().trs.translation,
            [3.0, 0.0, 0.0]
        );
        assert!(parented_world_pose(&world, orphan).is_none());
    }

    #[test]
    fn root_is_verbatim() {
        let mut world = World::new();
        let t = Transform {
            translation: [1.5, -2.0, 3.25],
            rotation: [10.0, 20.0, 30.0],
            scale: [1.0, 2.0, 3.0],
        };
        let id = spawn(&mut world, t, None);
        assert_eq!(world_pose_transform(&world, id), Some(t));
        assert_eq!(
            world_transform(&world, id).unwrap().trs.translation,
            t.translation
        );
    }

    #[test]
    fn three_level_chain_adds_translations() {
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(2.0), Some(a));
        let c = spawn(&mut world, at(4.0), Some(b));
        assert_eq!(
            world_transform(&world, c).unwrap().trs.translation,
            [7.0, 0.0, 0.0]
        );
        assert_eq!(world_affine(&world, c).unwrap().cols[3], [7.0, 0.0, 0.0]);
        assert_eq!(
            world_pose_transform(&world, c).unwrap().translation,
            [7.0, 0.0, 0.0]
        );
    }

    #[test]
    fn ancestor_without_transform_is_identity() {
        let mut world = World::new();
        let a = world.spawn();
        let b = spawn(&mut world, at(2.0), Some(a));
        assert_eq!(
            world_transform(&world, b).unwrap().trs.translation,
            [2.0, 0.0, 0.0]
        );
        assert!(world_transform(&world, a).is_none());
    }

    fn hide(world: &mut World, id: EntityId, visible: bool, cast_shadows: bool) {
        world
            .insert(
                id,
                Visibility {
                    visible,
                    cast_shadows,
                },
            )
            .unwrap();
    }

    #[test]
    fn visibility_ands_down_a_three_level_chain() {
        let mut world = World::new();
        let a = spawn(&mut world, at(0.0), None);
        let b = spawn(&mut world, at(0.0), Some(a));
        let c = spawn(&mut world, at(0.0), Some(b));
        hide(&mut world, b, true, false);
        let seen = inherited_visibility(&world, c);
        assert!(seen.visible && !seen.cast_shadows);
        hide(&mut world, a, false, true);
        let seen = inherited_visibility(&world, c);
        assert!(!seen.visible && !seen.cast_shadows);
        let frame = FrameVisibility::of(&world, &[a, b, c]);
        assert!(frame.hidden(a) && frame.hidden(b) && frame.hidden(c));
    }

    #[test]
    fn a_shadowless_root_makes_its_child_shadowless_and_hidden_wins() {
        let mut world = World::new();
        let root = spawn(&mut world, at(0.0), None);
        let child = spawn(&mut world, at(0.0), Some(root));
        let other = spawn(&mut world, at(0.0), None);
        hide(&mut world, root, true, false);
        let frame = FrameVisibility::of(&world, &[root, child, other]);
        assert!(!frame.casts(root) && !frame.casts(child) && frame.casts(other));
        assert!(!frame.hidden(child));
        hide(&mut world, root, false, false);
        let frame = FrameVisibility::of(&world, &[root, child]);
        assert!(frame.hidden(child), "hidden wins");
        assert!(frame.casts(child), "a hidden entity is not in `shadowless`");
    }

    #[test]
    fn a_visible_part_stays_hidden_under_a_hidden_root() {
        let mut world = World::new();
        let root = spawn(&mut world, at(0.0), None);
        let part = spawn(&mut world, at(0.0), Some(root));
        hide(&mut world, root, false, true);
        hide(&mut world, part, true, true);
        assert!(!inherited_visibility(&world, part).visible);
        assert!(FrameVisibility::of(&world, &[root, part]).hidden(part));
    }

    #[test]
    fn visibility_on_a_parent_cycle_degrades_without_hanging() {
        let mut world = World::new();
        let a = spawn(&mut world, at(0.0), None);
        let b = spawn(&mut world, at(0.0), Some(a));
        world.insert(a, Parent(b)).unwrap();
        hide(&mut world, b, false, true);
        assert!(
            inherited_visibility(&world, a).visible,
            "a cycle makes a a root"
        );
        assert!(!inherited_visibility(&world, b).visible);
    }

    #[test]
    fn visibility_without_the_component_is_the_default_and_hides_nothing() {
        let mut world = World::new();
        let a = spawn(&mut world, at(0.0), None);
        let b = spawn(&mut world, at(0.0), Some(a));
        assert_eq!(inherited_visibility(&world, b), Visibility::default());
        assert!(!FrameVisibility::of(&world, &[a, b]).hidden(b));
    }

    #[test]
    fn cycle_degrades_to_root() {
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(2.0), Some(a));
        world.insert(a, Parent(b)).unwrap();
        assert_eq!(chain(&world, a), (vec![a], Some(HierarchyFault::Cycle)));
        assert_eq!(
            world_transform(&world, b).unwrap().trs.translation,
            [2.0, 0.0, 0.0]
        );
        let selfish = spawn(&mut world, at(5.0), None);
        world.insert(selfish, Parent(selfish)).unwrap();
        assert_eq!(world_pose_transform(&world, selfish), Some(at(5.0)));
    }

    #[test]
    fn dead_parent_stops_the_chain() {
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(2.0), None);
        world.despawn(a).unwrap();
        world.insert(b, Parent(a)).unwrap();
        assert_eq!(
            chain(&world, b),
            (vec![b], Some(HierarchyFault::InvalidParent(a)))
        );
        assert_eq!(world_pose_transform(&world, b), Some(at(2.0)));
    }

    #[test]
    fn too_deep_degrades_to_root() {
        let mut world = World::new();
        let mut last = spawn(&mut world, at(1.0), None);
        for _ in 0..MAX_HIERARCHY_DEPTH {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        assert_eq!(chain(&world, last).0.len(), MAX_HIERARCHY_DEPTH + 1);
        assert!(chain(&world, last).1.is_none());
        let deeper = spawn(&mut world, at(1.0), Some(last));
        assert_eq!(
            chain(&world, deeper),
            (vec![deeper], Some(HierarchyFault::TooDeep))
        );
        assert_eq!(world_pose_transform(&world, deeper), Some(at(1.0)));
    }

    #[test]
    fn presentation_parent_of_a_sim_entity_is_invalid() {
        let mut world = World::new();
        let presentation = world.spawn_presentation();
        let sim = spawn(&mut world, at(1.0), Some(presentation));
        assert_eq!(
            chain(&world, sim),
            (vec![sim], Some(HierarchyFault::InvalidParent(presentation)))
        );
    }

    #[test]
    fn memoized_affines_equal_world_affine_bitwise() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 40) as f32 / (1u64 << 24) as f32
        };
        let mut world = World::new();
        let mut ids: Vec<EntityId> = Vec::new();
        for _ in 0..200 {
            let parent = if ids.is_empty() || next() < 0.2 {
                None
            } else {
                Some(ids[(next() * ids.len() as f32) as usize % ids.len()])
            };
            let transform = Transform {
                translation: [next() * 4.0 - 2.0, next() * 4.0 - 2.0, next() * 4.0 - 2.0],
                rotation: [next() * 360.0, next() * 360.0, next() * 360.0],
                scale: [0.5 + next(), 0.5 + next(), 0.5 + next()],
            };
            ids.push(spawn(&mut world, transform, parent));
        }
        let mut poses = WorldAffines::new();
        for &id in ids.iter().rev() {
            let expected = world_affine(&world, id);
            let got = poses.parented(&world, id);
            if chain(&world, id).0.len() > 1 {
                assert_eq!(got.map(|a| a.cols), expected.map(|a| a.cols), "{id}");
            } else {
                assert_eq!(got, None);
            }
        }
    }

    /// A `ParentJoint` chain over a real rig: `parented` equals `world_affine` bit for bit, with the
    /// skin queried first so the child's walk resumes from the memo.
    #[test]
    fn memoized_affines_equal_world_affine_bitwise_through_a_joint() {
        let dir = crate::parent_joint::tests::rig_dir("memo");
        let mut world = World::new();
        let (skin, child) = crate::parent_joint::tests::skin_and_child(&mut world, "child");
        let grandchild = spawn(&mut world, at(0.25), Some(child));
        // The skin is parented too, so querying it memoizes it and the child's walk resumes there.
        let anchor = spawn(&mut world, at(3.0), None);
        world.insert(skin, Parent(anchor)).unwrap();
        crate::parent_joint::install(&mut world, &dir);
        let plain = world_affine(&world, grandchild).unwrap();
        for query_skin_first in [true, false] {
            let mut poses = WorldAffines::new();
            if query_skin_first {
                let _ = poses.parented(&world, skin);
            }
            for id in [grandchild, child] {
                let expected = world_affine(&world, id);
                assert_eq!(
                    poses.parented(&world, id).map(|a| a.cols),
                    expected.map(|a| a.cols),
                    "{id} skin first: {query_skin_first}"
                );
            }
        }
        world
            .remove::<crate::components::ParentJoint>(child)
            .unwrap();
        assert_ne!(world_affine(&world, grandchild).unwrap().cols, plain.cols);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memo_and_fault_walk_cut_the_chain_at_the_depth_limit() {
        for forward in [true, false] {
            let mut world = World::new();
            let mut chain_ids = vec![spawn(&mut world, at(1.0), None)];
            for _ in 0..MAX_HIERARCHY_DEPTH + 1 {
                let last = *chain_ids.last().unwrap();
                chain_ids.push(spawn(&mut world, at(1.0), Some(last)));
            }
            let mut order: Vec<usize> = (0..chain_ids.len()).collect();
            if !forward {
                order.reverse();
            }
            let mut poses = WorldAffines::new();
            let (mut scratch, mut good) = (Vec::new(), SlotTable::new());
            for at in order {
                let id = chain_ids[at];
                let got = poses.parented(&world, id);
                let fault = fault_of(&world, id, &mut scratch, &mut good);
                assert_eq!(fault, chain(&world, id).1, "{at}");
                if at == 0 {
                    assert_eq!(got, None);
                } else if at <= MAX_HIERARCHY_DEPTH {
                    assert_eq!(
                        got.map(|a| a.cols),
                        world_affine(&world, id).map(|a| a.cols)
                    );
                } else {
                    assert_eq!((got, fault), (None, Some(HierarchyFault::TooDeep)));
                }
            }
        }
    }

    #[test]
    fn memo_returns_none_for_degraded_chains() {
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(2.0), Some(a));
        world.insert(a, Parent(b)).unwrap();
        let dead = spawn(&mut world, at(3.0), None);
        let orphan = spawn(&mut world, at(4.0), None);
        world.despawn(dead).unwrap();
        world.insert(orphan, Parent(dead)).unwrap();
        let mut poses = WorldAffines::new();
        assert_eq!(poses.parented(&world, a), None);
        assert_eq!(poses.parented(&world, b), None);
        assert_eq!(poses.parented(&world, orphan), None);
    }

    fn translated(x: f32) -> Trs {
        level_trs(&at(x))
    }

    #[test]
    fn an_empty_posed_table_changes_no_bit() {
        let mut world = World::new();
        let root = spawn(&mut world, at(1.0), None);
        let child = spawn(&mut world, at(2.0), Some(root));
        let grandchild = spawn(&mut world, at(3.0), Some(child));
        let mut poses = WorldAffines::new();
        assert_eq!(poses.parented(&world, root), None);
        for id in [child, grandchild] {
            assert_eq!(
                poses.parented(&world, id).map(|a| a.cols),
                world_affine(&world, id).map(|a| a.cols)
            );
        }
    }

    #[test]
    fn a_posed_root_and_a_posed_child_carry_their_descendants() {
        let mut world = World::new();
        let root = spawn(&mut world, at(1.0), None);
        let child = spawn(&mut world, at(2.0), Some(root));
        let grandchild = spawn(&mut world, at(3.0), Some(child));
        let mut poses = WorldAffines::new();
        poses.set_posed(root, translated(10.0));
        poses.set_posed(child, translated(20.0));
        // A posed root has no `Parent` yet reports its pose, so the caller stops reading the stored one.
        assert_eq!(
            poses
                .parented(&world, root)
                .map(|a| a.transform_point([0.0; 3])),
            Some([10.0, 0.0, 0.0])
        );
        assert_eq!(
            poses
                .parented(&world, child)
                .map(|a| a.transform_point([0.0; 3])),
            Some([30.0, 0.0, 0.0])
        );
        assert_eq!(
            poses
                .parented(&world, grandchild)
                .map(|a| a.transform_point([0.0; 3])),
            Some([33.0, 0.0, 0.0])
        );
        // The stored transforms are untouched.
        assert_eq!(
            world_pose_transform(&world, grandchild)
                .unwrap()
                .translation,
            [6.0, 0.0, 0.0]
        );
    }

    #[test]
    fn a_pose_set_on_a_parent_before_the_first_lookup_reaches_a_child_looked_up_first() {
        let mut world = World::new();
        let root = spawn(&mut world, at(1.0), None);
        let child = spawn(&mut world, at(2.0), Some(root));
        let mut poses = WorldAffines::new();
        poses.set_posed(root, translated(10.0));
        assert_eq!(
            poses
                .parented(&world, child)
                .map(|a| a.transform_point([0.0; 3])),
            Some([12.0, 0.0, 0.0])
        );
    }

    fn moved_parent(world: &mut World) -> EntityId {
        spawn(
            world,
            Transform {
                translation: [3.0, -1.0, 2.0],
                rotation: [10.0, 40.0, -25.0],
                scale: [2.0, 2.0, 2.0],
            },
            None,
        )
    }

    fn assert_near(a: [f32; 3], b: [f32; 3]) {
        for axis in 0..3 {
            assert!((a[axis] - b[axis]).abs() < 1e-5, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn keep_world_preserves_the_world_pose() {
        let mut world = World::new();
        let parent = moved_parent(&mut world);
        let child = spawn(&mut world, at(5.0), None);
        let before = world_pose_transform(&world, child).unwrap();
        reparent(&mut world, child, Some(parent), Keep::World).unwrap();
        let after = world_pose_transform(&world, child).unwrap();
        assert_near(after.translation, before.translation);
        assert_near(after.scale, before.scale);
        assert_ne!(world.get::<Transform>(child).unwrap(), Some(&before));
        reparent(&mut world, child, None, Keep::World).unwrap();
        let back = world.get::<Transform>(child).unwrap().copied().unwrap();
        assert_near(back.translation, before.translation);
    }

    #[test]
    fn keep_local_keeps_the_transform_bits() {
        let mut world = World::new();
        let parent = moved_parent(&mut world);
        let child = spawn(&mut world, at(5.0), None);
        reparent(&mut world, child, Some(parent), Keep::Local).unwrap();
        assert_eq!(world.get::<Transform>(child).unwrap(), Some(&at(5.0)));
        assert_eq!(world.get::<Parent>(child).unwrap(), Some(&Parent(parent)));
    }

    #[test]
    fn unparent_keep_world_writes_the_world_pose_transform() {
        let mut world = World::new();
        let parent = moved_parent(&mut world);
        let child = spawn(&mut world, at(5.0), Some(parent));
        let expected = world_pose_transform(&world, child).unwrap();
        reparent(&mut world, child, None, Keep::World).unwrap();
        assert_eq!(world.get::<Transform>(child).unwrap(), Some(&expected));
        assert!(!world.has::<Parent>(child));
        reparent(&mut world, child, None, Keep::World).unwrap();
    }

    #[test]
    fn refusals_mutate_nothing() {
        let rigid = Collider {
            shape: ludion_spatial::Shape::Sphere { radius: 0.5 },
            body: Body::Rigid(Default::default()),
            layer: None,
            sensor: false,
        };
        let wall = Collider {
            body: Body::new_static(),
            ..rigid.clone()
        };
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(2.0), Some(a));
        let c = spawn(&mut world, at(3.0), Some(b));
        let dead = spawn(&mut world, at(0.0), None);
        world.despawn(dead).unwrap();
        let presentation = world.spawn_presentation();
        let ball = spawn(&mut world, at(4.0), None);
        world.insert(ball, rigid).unwrap();
        let slab = spawn(&mut world, at(4.0), None);
        world.insert(slab, wall).unwrap();

        let cases = [
            (a, Some(c), code::HIERARCHY_REPARENT_CYCLE),
            (a, Some(a), code::HIERARCHY_REPARENT_CYCLE),
            (c, Some(presentation), code::HIERARCHY_REPARENT_PRESENTATION),
            (ball, Some(a), code::HIERARCHY_RIGID_BODY),
            (slab, Some(a), code::HIERARCHY_STATIC_COLLIDER),
            (dead, Some(a), code::DEAD_ENTITY),
            (a, Some(dead), code::DEAD_ENTITY),
        ];
        for (child, parent, expected) in cases {
            let before = (
                world.get::<Parent>(child).ok().flatten().copied(),
                world.get::<Transform>(child).ok().flatten().copied(),
            );
            let err = reparent(&mut world, child, parent, Keep::World).unwrap_err();
            assert_eq!(err.code, expected, "{child} -> {parent:?}");
            let after = (
                world.get::<Parent>(child).ok().flatten().copied(),
                world.get::<Transform>(child).ok().flatten().copied(),
            );
            assert_eq!(before, after);
        }
        reparent(&mut world, ball, None, Keep::Local).unwrap();
    }

    #[test]
    fn keep_world_under_a_transformless_parent_composes_its_ancestors() {
        let mut world = World::new();
        let a = spawn(&mut world, at(10.0), None);
        let b = world.spawn();
        world.insert(b, Parent(a)).unwrap();
        let c = spawn(&mut world, at(5.0), None);
        reparent(&mut world, c, Some(b), Keep::World).unwrap();
        assert_eq!(
            world_transform(&world, c).unwrap().trs.translation,
            [5.0, 0.0, 0.0]
        );
        assert_eq!(world.get::<Transform>(c).unwrap(), Some(&at(-5.0)));
    }

    #[test]
    fn reparent_refuses_a_chain_deeper_than_the_limit() {
        let mut world = World::new();
        let mut last = spawn(&mut world, at(1.0), None);
        for _ in 0..MAX_HIERARCHY_DEPTH - 1 {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        let child = spawn(&mut world, at(1.0), None);
        reparent(&mut world, child, Some(last), Keep::Local).unwrap();
        let other = spawn(&mut world, at(1.0), None);
        let err = reparent(&mut world, other, Some(child), Keep::Local).unwrap_err();
        assert_eq!(err.code, code::HIERARCHY_REPARENT_TOO_DEEP);
        assert!(!world.has::<Parent>(other));
        assert_eq!(
            ludion_core::codes::explain_code(code::HIERARCHY_REPARENT_TOO_DEEP)
                .expect("registered")
                .severity,
            ludion_core::diagnostics::Severity::Error
        );
    }

    #[test]
    fn reparent_refuses_a_move_that_pushes_a_descendant_past_the_limit() {
        let mut world = World::new();
        let mut last = spawn(&mut world, at(1.0), None);
        for _ in 0..MAX_HIERARCHY_DEPTH - 1 {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        let child = spawn(&mut world, at(1.0), None);
        let grandchild = spawn(&mut world, at(1.0), Some(child));
        let err = reparent(&mut world, child, Some(last), Keep::Local).unwrap_err();
        assert_eq!(err.code, code::HIERARCHY_REPARENT_TOO_DEEP);
        assert!(!world.has::<Parent>(child));
        let shallower = spawn(&mut world, at(1.0), None);
        reparent(&mut world, child, Some(shallower), Keep::Local).unwrap();
        assert_eq!(ancestor_count(&world, grandchild), 2);
    }

    #[test]
    fn a_sim_reparent_counts_a_presentation_descendant_against_the_depth_cap() {
        use ludion_ecs::{Schedule, SystemCtx};

        struct Move(EntityId, EntityId);
        struct Refused(Option<ludion_core::diagnostics::DiagnosticCode>);

        let mut world = World::new();
        let mut last = spawn(&mut world, at(1.0), None);
        for _ in 0..MAX_HIERARCHY_DEPTH - 1 {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        let child = spawn(&mut world, at(1.0), None);
        let ghost = world.spawn_presentation();
        world.insert(ghost, Parent(child)).unwrap();
        world.insert_resource(Move(child, last));
        world.insert_resource(Refused(None));

        let mut schedule = Schedule::new();
        schedule.add_system("reparent", |world, _| {
            let Move(child, parent) = *world.resource::<Move>().unwrap();
            let code = reparent(world, child, Some(parent), Keep::Local)
                .err()
                .map(|err| err.code);
            world.insert_resource(Refused(code));
        });
        let ctx = SystemCtx::new(
            ludion_core::time::FrameIndex::ZERO,
            ludion_core::time::TickRate::DEFAULT,
        );
        schedule.run(&mut world, &ctx);
        assert_eq!(
            world.resource::<Refused>().unwrap().0,
            Some(code::HIERARCHY_REPARENT_TOO_DEEP)
        );
        assert!(!world.has::<Parent>(child));
    }

    #[test]
    fn keep_world_under_a_rotated_non_uniform_scale_parent_keeps_the_world_translation() {
        let mut world = World::new();
        let parent = spawn(
            &mut world,
            Transform {
                translation: [3.0, -1.0, 2.0],
                rotation: [0.0, 90.0, 0.0],
                scale: [2.0, 1.0, 1.0],
            },
            None,
        );
        let child = spawn(&mut world, at(5.0), None);
        let before = world_transform(&world, child).unwrap().trs.translation;
        reparent(&mut world, child, Some(parent), Keep::World).unwrap();
        let after = world_transform(&world, child).unwrap().trs.translation;
        for axis in 0..3 {
            assert!(
                (before[axis] - after[axis]).abs() < 1e-4,
                "{before:?} {after:?}"
            );
        }
    }

    #[test]
    fn reparent_cycle_is_refused_through_a_chain_deeper_than_the_limit() {
        let mut world = World::new();
        let top = spawn(&mut world, at(1.0), None);
        let mut last = top;
        for _ in 0..MAX_HIERARCHY_DEPTH + 6 {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        let err = reparent(&mut world, top, Some(last), Keep::Local).unwrap_err();
        assert_eq!(err.code, code::HIERARCHY_REPARENT_CYCLE);
    }

    #[test]
    fn reparent_does_not_count_a_dead_link_above_the_chain() {
        let mut world = World::new();
        let gone = spawn(&mut world, at(0.0), None);
        let top = spawn(&mut world, at(1.0), None);
        let mut last = top;
        for _ in 0..MAX_HIERARCHY_DEPTH - 1 {
            last = spawn(&mut world, at(1.0), Some(last));
        }
        world.despawn(gone).unwrap();
        world.insert(top, Parent(gone)).unwrap();
        let child = spawn(&mut world, at(1.0), None);
        reparent(&mut world, child, Some(last), Keep::Local).unwrap();
    }

    #[test]
    fn reparent_under_a_parent_whose_ancestry_already_loops_is_refused() {
        let mut world = World::new();
        let a = spawn(&mut world, at(1.0), None);
        let b = spawn(&mut world, at(1.0), Some(a));
        world.insert(a, Parent(b)).unwrap();
        let c = spawn(&mut world, at(1.0), None);
        let err = reparent(&mut world, c, Some(a), Keep::Local).unwrap_err();
        assert_eq!(err.code, code::HIERARCHY_REPARENT_CYCLE);
        assert!(!world.has::<Parent>(c));
    }

    #[test]
    fn only_the_entity_with_the_dead_parent_reports_it() {
        let mut world = World::new();
        let gone = spawn(&mut world, at(0.0), None);
        let broken = spawn(&mut world, at(1.0), None);
        let below = spawn(&mut world, at(1.0), Some(broken));
        world.despawn(gone).unwrap();
        world.insert(broken, Parent(gone)).unwrap();
        for order in [[broken, below], [below, broken]] {
            let (mut scratch, mut good) = (Vec::new(), SlotTable::new());
            let faults: Vec<_> = order
                .iter()
                .map(|id| fault_of(&world, *id, &mut scratch, &mut good))
                .collect();
            let by_id = |id| faults[order.iter().position(|o| *o == id).unwrap()];
            assert_eq!(by_id(broken), Some(HierarchyFault::InvalidParent(gone)));
            assert_eq!(by_id(below), None);
        }
    }

    #[test]
    fn shear_is_flagged_and_the_affine_keeps_it() {
        let mut world = World::new();
        let parent = spawn(
            &mut world,
            Transform {
                scale: [2.0, 1.0, 1.0],
                ..Transform::default()
            },
            None,
        );
        let child = spawn(
            &mut world,
            Transform {
                rotation: [0.0, 0.0, 45.0],
                ..Transform::default()
            },
            Some(parent),
        );
        assert!(world_transform(&world, child).unwrap().sheared);
        let cols = world_affine(&world, child).unwrap().cols;
        // x axis: scale 2 then rotate 45° about z; y axis keeps x-scale-2 mixing (the shear).
        assert!(
            (cols[1][0] + 2.0 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "{cols:?}"
        );
        assert!(
            (cols[1][1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "{cols:?}"
        );
    }
}
