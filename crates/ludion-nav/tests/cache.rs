//! The flow cache: sharing, cell-keyed recompute, eviction.

mod common;

use common::*;
use ludion_nav::{FlowCache, NavGrid, NodeId};

fn open() -> impl ludion_nav::ColumnSource {
    FnSource(|_, _, o| o.push(flat(0.0)))
}

#[test]
fn one_target_is_computed_once() {
    let grid = NavGrid::build(params(8, 8), &open());
    let mut cache = FlowCache::new(f32::INFINITY);
    let t = grid.node_at(at(2, 2, 0.0)).unwrap();
    cache.field(&grid, t);
    cache.field(&grid, t);
    assert_eq!(cache.computes(), 1);
}

#[test]
fn recomputes_only_when_the_target_changes_cell() {
    let grid = NavGrid::build(params(8, 8), &open());
    let mut cache = FlowCache::new(f32::INFINITY);
    cache.field_at(&grid, [2.1, 0.0, 2.1]).unwrap();
    cache.field_at(&grid, [2.9, 0.0, 2.8]).unwrap();
    assert_eq!(cache.computes(), 1);
    cache.field_at(&grid, [3.1, 0.0, 2.1]).unwrap();
    assert_eq!(cache.computes(), 2);
}

#[test]
fn a_region_rebuild_recomputes() {
    let mut grid = NavGrid::build(params(8, 8), &open());
    let mut cache = FlowCache::new(f32::INFINITY);
    let t = grid.node_at(at(2, 2, 0.0)).unwrap();
    cache.field(&grid, t);
    grid.rebuild_region(&open(), [0, 0], [1, 1]);
    cache.field(&grid, t);
    assert_eq!(cache.computes(), 2);
}

#[test]
fn end_frame_evicts_unused_targets_in_stable_order() {
    let grid = NavGrid::build(params(8, 8), &open());
    let mut cache = FlowCache::new(f32::INFINITY);
    let ids: Vec<NodeId> = [(1, 1), (2, 2), (3, 3)]
        .iter()
        .map(|&(i, j)| grid.node_at(at(i, j, 0.0)).unwrap())
        .collect();
    for &t in &ids {
        cache.field(&grid, t);
    }
    cache.end_frame();
    cache.field(&grid, ids[2]);
    cache.field(&grid, ids[0]);
    cache.end_frame();
    assert_eq!(cache.targets().collect::<Vec<_>>(), vec![ids[0], ids[2]]);
    assert_eq!(cache.computes(), 3);
}
