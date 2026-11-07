//! Flow fields: determinism, the radius cap, layered routing.

mod common;

use common::*;
use ludion_nav::{ColumnSource, FlowField, NavGrid, NodeId};

fn layered() -> impl ColumnSource {
    // 10x3, ground everywhere except row j=1: a bridge at y=3 over i=3..6 (with the ground under it) and a
    // 30° ramp over i=6..9 descending east. The bridge top is reachable from the ground only via the ramp.
    FnSource(|i, j, out| match (i, j) {
        (3..=5, 1) => {
            let mut g = flat(0.0);
            g.headroom = 3.0;
            out.push(g);
            out.push(flat(3.0));
        }
        (6..=8, 1) => out.push(tilted(2.25 - 0.75 * (i - 6) as f32, 30.0)),
        _ => out.push(flat(0.0)),
    })
}

fn bits(f: &FlowField, n: usize) -> Vec<(u32, [u32; 2])> {
    (0..n as u32)
        .map(|k| {
            let k = NodeId(k);
            let fl = f.flow(k);
            (f.distance(k).to_bits(), [fl[0].to_bits(), fl[1].to_bits()])
        })
        .collect()
}

#[test]
fn a_field_is_bit_identical_across_runs_and_buffer_reuse() {
    let grid = NavGrid::build(params(10, 3), &layered());
    let n = grid.node_count();
    let target = grid.node_at(at(0, 1, 0.0)).unwrap();
    let mut a = FlowField::default();
    a.compute(&grid, target, f32::INFINITY);
    let mut b = FlowField::default();
    b.compute(&grid, target, f32::INFINITY);
    assert_eq!(bits(&a, n), bits(&b, n));
    assert_eq!(a.reached(), b.reached());
    // Reuse: compute elsewhere, then back, against a fresh field.
    let other = grid.node_at(at(9, 1, 3.0)).unwrap();
    a.compute(&grid, other, 4.0);
    a.compute(&grid, target, f32::INFINITY);
    assert_eq!(bits(&a, n), bits(&b, n));
    assert_eq!(a.reached(), b.reached());
}

#[test]
fn the_radius_bounds_the_search() {
    let grid = NavGrid::build(params(20, 1), &FnSource(|_, _, o| o.push(flat(0.0))));
    let target = grid.node_at(at(0, 0, 0.0)).unwrap();
    let mut f = FlowField::default();
    f.compute(&grid, target, 5.0);
    // Nodes at distance <= 5 expand; their neighbours at <= 6 are reached.
    assert_eq!(f.reached().len(), 7);
    assert!(
        !f.distance(grid.node_at(at(10, 0, 0.0)).unwrap())
            .is_finite()
    );
}

#[test]
fn routing_under_a_bridge_goes_through_the_ramp() {
    let grid = NavGrid::build(params(10, 3), &layered());
    let bridge_top = grid.node_at(at(4, 1, 3.0)).unwrap();
    let mut f = FlowField::default();
    f.compute(&grid, bridge_top, f32::INFINITY);
    let under = grid.node_at(at(4, 1, 0.0)).unwrap();
    assert_ne!(under, bridge_top);
    // Straight up is 3 m; the only way is out to the ramp base at i=8 and back along the bridge.
    assert!(f.distance(under) > 8.0, "got {}", f.distance(under));
    assert!(f.distance(under).is_finite());
}

#[test]
fn a_wall_with_a_gap_bends_the_flow() {
    // 9x9, wall at i=4 except a gap at j=7; target at (0, 0).
    let src = FnSource(|i, j, out| {
        if i != 4 || j == 7 {
            out.push(flat(0.0));
        }
    });
    let grid = NavGrid::build(params(9, 9), &src);
    let target = grid.node_at(at(0, 0, 0.0)).unwrap();
    let mut f = FlowField::default();
    f.compute(&grid, target, f32::INFINITY);
    let flow = f.flow_at(&grid, at(6, 1, 0.0)).unwrap();
    assert!(flow[1] > 0.0, "flows toward the gap (+z), got {flow:?}");
    // A blocked cell reads its lowest neighbour.
    assert!(f.flow_at(&grid, at(4, 3, 0.0)).is_some());
}

#[test]
fn distances_follow_the_links_an_agent_walks() {
    // a(0,0) y0, P(1,0) y0.5, Q(0,1) y0.1, b(1,1) y0.3. a and P differ by more than a step, so the
    // corner-cut rule drops a -> b while b -> a stays.
    let grid = NavGrid::build(
        params(2, 2),
        &FnSource(|i, j, out| {
            out.push(flat(match (i, j) {
                (0, 0) => 0.0,
                (1, 0) => 0.5,
                (0, 1) => 0.1,
                _ => 0.3,
            }))
        }),
    );
    let a = grid.node_at(at(0, 0, 0.0)).unwrap();
    let b = grid.node_at(at(1, 1, 0.3)).unwrap();
    assert_eq!(grid.link(a, 4), None, "a cannot cut the corner to b");
    assert_eq!(grid.link(b, 7), Some(a), "b can step down to a");

    let mut field = FlowField::default();
    field.compute(&grid, b, f32::INFINITY);
    let walked = (1.01f32).sqrt() + (1.04f32).sqrt();
    assert!(
        (field.distance(a) - walked).abs() < 1e-5,
        "{} vs {walked}",
        field.distance(a)
    );
    assert!(field.distance(a) > 2.09f32.sqrt() + 0.1);
}
