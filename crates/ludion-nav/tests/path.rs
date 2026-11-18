//! Point-to-point A*: agreement with the flow field's Dijkstra, determinism, partial paths, string pulling.

mod common;

use common::*;
use ludion_nav::{
    ColumnSource, FlowField, NavGrid, NodeId, PathOptions, PathStatus, find_path, nearest_node,
    string_pull, walkable_line,
};

/// 32x32 ground with two walls, an unreachable ledge, and a deck at y=1.5 reached by a ramp.
fn world() -> impl ColumnSource {
    FnSource(|i, j, out| match (i, j) {
        (10, 0..=23) => {}
        (12..=27, 20) => {}
        (2..=5, 26..=29) => out.push(flat(3.0)),
        (22, 2..=5) => out.push(tilted(0.345, 30.0)),
        (23, 2..=5) => out.push(tilted(0.9226, 30.0)),
        (24..=27, 2..=5) => out.push(flat(1.5)),
        _ => out.push(flat(0.0)),
    })
}

fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

#[test]
fn a_star_matches_the_flow_field_on_a_thousand_pairs() {
    let grid = NavGrid::build(params(32, 32), &world());
    let n = grid.node_count() as u32;
    let mut state = 7u64;
    let mut field = FlowField::default();
    let (mut complete, mut unreachable) = (0, 0);
    for _ in 0..1000 {
        let from = NodeId(lcg(&mut state) % n);
        let to = NodeId(lcg(&mut state) % n);
        field.compute(&grid, to, f32::INFINITY);
        let want = field.distance(from);
        let got = find_path(
            &grid,
            from,
            to,
            &PathOptions {
                partial: false,
                ..PathOptions::default()
            },
        );
        if want.is_finite() {
            assert_eq!(got.status, PathStatus::Complete, "{from:?} -> {to:?}");
            assert!(
                (got.cost - want).abs() <= 1e-4 * want.max(1.0),
                "{from:?} -> {to:?}: {} vs {want}",
                got.cost
            );
            assert_eq!(got.nodes.first(), Some(&from));
            assert_eq!(got.nodes.last(), Some(&to));
            complete += 1;
        } else {
            assert_eq!(got.status, PathStatus::Unreachable, "{from:?} -> {to:?}");
            assert!(got.nodes.is_empty() && got.cost.is_infinite());
            unreachable += 1;
        }
    }
    assert!(
        complete > 500 && unreachable > 10,
        "{complete} / {unreachable}"
    );
}

#[test]
fn the_same_request_gives_the_same_result() {
    let grid = NavGrid::build(params(32, 32), &world());
    let a = grid.node_at(at(1, 1, 0.0)).unwrap();
    let b = grid.node_at(at(30, 30, 0.0)).unwrap();
    let opts = PathOptions::default();
    assert_eq!(find_path(&grid, a, b, &opts), find_path(&grid, a, b, &opts));
    let same = find_path(&grid, a, a, &opts);
    assert_eq!(
        (same.status, same.cost, same.nodes.len()),
        (PathStatus::Complete, 0.0, 1)
    );
}

#[test]
fn the_deck_is_reached_only_by_its_ramp() {
    let grid = NavGrid::build(params(32, 32), &world());
    let a = grid.node_at(at(18, 3, 0.0)).unwrap();
    let deck = grid.node_at(at(26, 3, 1.5)).unwrap();
    let path = find_path(&grid, a, deck, &PathOptions::default());
    assert_eq!(path.status, PathStatus::Complete);
    assert!(
        path.nodes
            .iter()
            .any(|n| (22..=23).contains(&((grid.node_pos(*n)[0]) as u32)))
    );
}

#[test]
fn a_partial_path_ends_at_the_node_nearest_the_goal() {
    let grid = NavGrid::build(params(32, 32), &world());
    let ledge = grid.node_at(at(3, 27, 3.0)).unwrap();
    let from = grid.node_at(at(8, 27, 0.0)).unwrap();
    let path = find_path(&grid, from, ledge, &PathOptions::default());
    assert_eq!(path.status, PathStatus::Partial);
    let end = grid.node_pos(*path.nodes.last().unwrap());
    // The ledge block is ringed by ground at i=6 / j=25 / j=30; the nearest ground to (3.5, 27.5) is adjacent.
    let (dx, dz) = (end[0] - 3.5, end[2] - 27.5);
    let d = ludion_core::math::sqrt(dx * dx + dz * dz);
    assert!(d < 3.0, "ended at {end:?}");
    assert!(end[1] == 0.0);
    let strict = find_path(
        &grid,
        from,
        ledge,
        &PathOptions {
            partial: false,
            ..PathOptions::default()
        },
    );
    assert_eq!(strict.status, PathStatus::Unreachable);
}

#[test]
fn the_expansion_cap_bounds_the_search() {
    let grid = NavGrid::build(params(32, 32), &world());
    let a = grid.node_at(at(1, 1, 0.0)).unwrap();
    let b = grid.node_at(at(30, 30, 0.0)).unwrap();
    let path = find_path(
        &grid,
        a,
        b,
        &PathOptions {
            max_expanded: 10,
            ..PathOptions::default()
        },
    );
    assert_eq!(path.status, PathStatus::Partial);
    assert_eq!(path.expanded, 10);
}

#[test]
fn a_cost_cap_below_the_true_cost_is_partial_or_unreachable() {
    let grid = NavGrid::build(params(32, 32), &world());
    let a = grid.node_at(at(1, 1, 0.0)).unwrap();
    let b = grid.node_at(at(30, 30, 0.0)).unwrap();
    let full = find_path(&grid, a, b, &PathOptions::default());
    let cap = full.cost * 0.5;
    let partial = find_path(
        &grid,
        a,
        b,
        &PathOptions {
            max_cost: cap,
            ..PathOptions::default()
        },
    );
    assert_eq!(partial.status, PathStatus::Partial);
    assert!(partial.cost <= cap);
    let strict = find_path(
        &grid,
        a,
        b,
        &PathOptions {
            max_cost: cap,
            partial: false,
            ..PathOptions::default()
        },
    );
    assert_eq!(strict.status, PathStatus::Unreachable);
}

#[test]
fn string_pulling_keeps_walkable_legs_and_one_corner_of_an_l() {
    // An L: row j=0 from i=0..=9, then column i=9 down j=0..=9.
    let l = FnSource(|i, j, out| {
        if j == 0 || i == 9 {
            out.push(flat(0.0));
        }
    });
    let grid = NavGrid::build(params(10, 10), &l);
    let a = grid.node_at(at(0, 0, 0.0)).unwrap();
    let b = grid.node_at(at(9, 9, 0.0)).unwrap();
    let path = find_path(&grid, a, b, &PathOptions::default());
    let pulled = string_pull(&grid, &path.nodes);
    assert_eq!(pulled.len(), 3, "{pulled:?}");
    assert_eq!(pulled[1], at(9, 0, 0.0));
    for w in pulled.windows(2) {
        assert!(walkable_line(&grid, w[0], w[1]));
    }

    let big = NavGrid::build(params(32, 32), &world());
    let a = big.node_at(at(1, 1, 0.0)).unwrap();
    let b = big.node_at(at(30, 30, 0.0)).unwrap();
    let path = find_path(&big, a, b, &PathOptions::default());
    let pulled = string_pull(&big, &path.nodes);
    assert!(pulled.len() >= 2 && pulled.len() < path.nodes.len());
    for w in pulled.windows(2) {
        assert!(walkable_line(&big, w[0], w[1]), "{w:?}");
    }
}

#[test]
fn a_line_through_a_wall_is_not_walkable() {
    let grid = NavGrid::build(params(32, 32), &world());
    assert!(walkable_line(&grid, at(1, 1, 0.0), at(8, 5, 0.0)));
    assert!(!walkable_line(&grid, at(1, 1, 0.0), at(14, 1, 0.0)));
    assert!(
        !walkable_line(&grid, at(10, 1, 0.0), at(14, 1, 0.0)),
        "starts in a wall"
    );
}

#[test]
fn the_nearest_node_comes_from_inside_a_wall() {
    let grid = NavGrid::build(params(32, 32), &world());
    let wall = at(10, 5, 0.0);
    assert!(grid.node_at(wall).is_none());
    let n = nearest_node(&grid, wall, 2.0).unwrap();
    let p = grid.node_pos(n);
    assert!((p[0] - wall[0]).abs() <= 1.0 + 1e-3 && p[0] != wall[0]);
    assert_eq!(nearest_node(&grid, wall, 0.4), None);
    assert_eq!(nearest_node(&grid, at(10, 5, 0.0), 2.0), Some(n));
}
