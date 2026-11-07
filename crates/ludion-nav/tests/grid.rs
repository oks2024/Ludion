//! Grid derivation: nodes, links, layers, region rebuilds.

mod common;

use common::*;
use ludion_nav::{FlowField, NEIGHBORS, NavGrid, NodeId};

/// 8x1 strip: flat at 0 for i<2, a 30° ramp over i=2..6 rising 2.3 m total, flat at 2.3 for i>=6.
fn ramp_source() -> impl ludion_nav::ColumnSource {
    FnSource(|i, _j, out| {
        let rise = 2.3 / 4.0;
        match i {
            0 | 1 => out.push(flat(0.0)),
            2..=5 => out.push(tilted(0.0 + rise * (i - 1) as f32, 30.0)),
            _ => out.push(flat(2.3)),
        }
    })
}

#[test]
fn a_ramp_links_end_to_end() {
    let grid = NavGrid::build(params(8, 1), &ramp_source());
    let top = grid.node_at(at(7, 0, 2.3)).unwrap();
    let mut f = FlowField::default();
    f.compute(&grid, top, f32::INFINITY);
    let bottom = grid.node_at(at(0, 0, 0.0)).unwrap();
    assert!(f.distance(bottom).is_finite());
}

#[test]
fn a_ledge_does_not_link() {
    let src = FnSource(|i, _j, out| out.push(flat(if i < 2 { 0.0 } else { 1.0 })));
    let grid = NavGrid::build(params(4, 1), &src);
    let upper = grid.node_at(at(3, 0, 1.0)).unwrap();
    let mut f = FlowField::default();
    f.compute(&grid, upper, f32::INFINITY);
    assert!(!f.distance(grid.node_at(at(0, 0, 0.0)).unwrap()).is_finite());
    assert!(f.distance(grid.node_at(at(2, 0, 1.0)).unwrap()).is_finite());
}

fn overhang(headroom: f32) -> impl ludion_nav::ColumnSource {
    FnSource(move |i, _j, out| {
        let mut ground = flat(0.0);
        if i >= 2 {
            ground.headroom = headroom;
            out.push(flat(3.0));
        }
        out.push(ground);
    })
}

#[test]
fn an_overhang_keeps_both_layers() {
    let grid = NavGrid::build(params(4, 1), &overhang(3.0));
    assert_eq!(grid.node_count(), 6);
    let under = grid.node_at(at(2, 0, 0.0)).unwrap();
    let over = grid.node_at(at(2, 0, 3.0)).unwrap();
    assert_ne!(under, over);
    assert_eq!(grid.node_pos(under)[1], 0.0);
    assert_eq!(grid.node_pos(over)[1], 3.0);
}

#[test]
fn low_headroom_removes_the_ground_node() {
    let grid = NavGrid::build(params(4, 1), &overhang(1.0));
    assert_eq!(grid.node_count(), 4, "columns 2 and 3 keep only the bridge");
    assert_eq!(grid.node_at(at(2, 0, 0.0)), None);
}

#[test]
fn too_steep_a_slope_makes_no_node() {
    let src = FnSource(|_i, _j, out| out.push(tilted(0.0, 60.0)));
    assert_eq!(NavGrid::build(params(3, 3), &src).node_count(), 0);
}

#[test]
fn sample_order_does_not_change_the_grid() {
    let fwd = FnSource(|_i, _j, out| {
        out.push(flat(0.0));
        out.push(flat(3.0));
    });
    let rev = FnSource(|_i, _j, out| {
        out.push(flat(3.0));
        out.push(flat(0.0));
    });
    assert_same(
        &NavGrid::build(params(3, 3), &fwd),
        &NavGrid::build(params(3, 3), &rev),
    );
}

fn assert_same(a: &NavGrid, b: &NavGrid) {
    assert_eq!(a.node_count(), b.node_count());
    for n in 0..a.node_count() as u32 {
        let n = NodeId(n);
        assert_eq!(a.node_pos(n), b.node_pos(n));
        for dir in 0..NEIGHBORS.len() {
            assert_eq!(a.link(n, dir), b.link(n, dir));
            assert_eq!(
                a.link_cost(n, dir).map(f32::to_bits),
                b.link_cost(n, dir).map(f32::to_bits)
            );
        }
    }
}

#[test]
fn rebuild_region_equals_a_fresh_build() {
    use std::cell::Cell;
    let wall = Cell::new(false);
    let src = FnSource(|i, j, out| {
        if !(wall.get() && i == 3 && j == 2) {
            out.push(flat(0.0));
        }
    });
    let mut grid = NavGrid::build(params(6, 6), &src);
    let before = grid.generation();
    wall.set(true);
    grid.rebuild_region(&src, [2, 1], [4, 3]);
    assert!(grid.generation() > before);
    assert_same(&grid, &NavGrid::build(params(6, 6), &src));
    assert_eq!(grid.node_count(), 35);
}
