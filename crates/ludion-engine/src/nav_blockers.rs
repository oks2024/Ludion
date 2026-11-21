//! The derived blocker overlay: `NavBlocker` entities marked onto each class grid by `nav_update`.

use ludion_components::{NavBlocker, NavBlockerMode, NavBlockerShape};
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_nav::{NavGrid, NavOverlay, NodeId};

use crate::hierarchy::world_transform;
use crate::nav::NavClass;
use ludion_core::math::Trs;

/// The enabled blockers one class applies, in ascending entity order; an entry's index is the stop tag.
#[derive(Default)]
pub(crate) struct BlockerTable {
    pub(crate) entries: Vec<EntityId>,
}

/// A `NavBlocker` `classes` entry no class has: the blocker entity, the entry's index and its text.
pub(crate) type UnknownBlockerClass = (EntityId, usize, String);

/// One overlay and blocker table per class grid from the enabled `NavBlocker`s, plus the `classes` entries
/// naming no class. An overlay is `None` where no blocker applies, so a world with no blocker keeps the
/// grid untouched.
///
/// A node is covered when its xz, taken in the box's frame at the box's centre height, lies within the half
/// extents inflated by the class radius (divided by the axis scale, so a non-uniform scale is approximated
/// per axis) and the agent's body `[y, y + height]` overlaps the box's world vertical extent.
pub(crate) fn build_blockers(
    world: &mut World,
    grids: &[NavGrid],
    classes: &[NavClass],
) -> (
    Vec<Option<NavOverlay>>,
    Vec<BlockerTable>,
    Vec<UnknownBlockerClass>,
) {
    let mut found: Vec<(EntityId, NavBlocker)> = world
        .query_ids::<(&NavBlocker,)>()
        .map(|(id, (blocker,))| (id, blocker.clone()))
        .collect();
    found.sort_by_key(|(id, _)| *id);
    let mut unknown = Vec::new();
    for (entity, blocker) in &found {
        for (i, name) in blocker.classes.iter().enumerate() {
            if !classes.iter().any(|c| c.name == *name) {
                unknown.push((*entity, i, name.clone()));
            }
        }
    }
    let mut overlays = Vec::with_capacity(grids.len());
    let mut tables = Vec::with_capacity(grids.len());
    for (grid, class) in grids.iter().zip(classes) {
        let mut overlay = NavOverlay::new(grid.node_count());
        let mut table = BlockerTable::default();
        for (entity, blocker) in found
            .iter()
            .filter(|(_, b)| b.enabled && (b.classes.is_empty() || b.classes.contains(&class.name)))
        {
            let tag = table.entries.len() as u32;
            table.entries.push(*entity);
            let pose = world_transform(world, *entity).map_or(Trs::IDENTITY, |p| p.trs);
            mark_box(grid, class, &mut overlay, blocker, &pose, tag);
        }
        overlays.push((!table.entries.is_empty()).then_some(overlay));
        tables.push(table);
    }
    (overlays, tables, unknown)
}

fn mark_box(
    grid: &NavGrid,
    class: &NavClass,
    overlay: &mut NavOverlay,
    blocker: &NavBlocker,
    pose: &Trs,
    tag: u32,
) {
    let NavBlockerShape::Box { half_extents: h } = blocker.shape;
    let affine = pose.to_affine();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for corner in 0..8u32 {
        let local = [
            if corner & 1 == 0 { -h[0] } else { h[0] },
            if corner & 2 == 0 { -h[1] } else { h[1] },
            if corner & 4 == 0 { -h[2] } else { h[2] },
        ];
        let p = affine.transform_point(local);
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if !lo.iter().chain(hi.iter()).all(|c| c.is_finite()) {
        return;
    }
    let scale = pose.scale.map(f32::abs);
    if scale.iter().any(|s| *s <= f32::EPSILON) {
        return;
    }
    let to_local = pose.inverse().to_affine();
    let centre_y = 0.5 * (lo[1] + hi[1]);
    let r = class.radius;
    let (params, cs) = (grid.params(), grid.params().cell_size);
    let cell = |v: f32, origin: f32, n: u32| {
        (((v - origin) / cs).floor().max(0.0) as u32).min(n.saturating_sub(1))
    };
    let (i0, i1) = (
        cell(lo[0] - r, params.origin[0], params.nx),
        cell(hi[0] + r, params.origin[0], params.nx),
    );
    let (j0, j1) = (
        cell(lo[2] - r, params.origin[1], params.nz),
        cell(hi[2] + r, params.origin[1], params.nz),
    );
    for j in j0..=j1 {
        for i in i0..=i1 {
            for n in grid.column_nodes(i, j) {
                let p = grid.node_pos(NodeId(n));
                if !(p[1] < hi[1] && p[1] + class.height > lo[1]) {
                    continue;
                }
                let local = to_local.transform_point([p[0], centre_y, p[2]]);
                if local[0].abs() > h[0] + r / scale[0] || local[2].abs() > h[2] + r / scale[2] {
                    continue;
                }
                match blocker.mode {
                    NavBlockerMode::Block => overlay.block(n),
                    NavBlockerMode::Cost => overlay.cost(n, blocker.cost),
                    NavBlockerMode::Interact => overlay.stop(n, tag),
                }
            }
        }
    }
}
