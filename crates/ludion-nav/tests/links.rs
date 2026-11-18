//! Off-mesh links: reachability, direction, cost choice, equality with `find_path` when empty, link marks.

mod common;

use common::*;
use ludion_nav::{
    ColumnSource, NavGrid, NavLinks, NodeId, OffMeshLink, PathOptions, PathStatus, find_path,
    find_path_linked, string_pull, string_pull_linked, walkable_line,
};

/// 32x8: ground for `i < 16`, a deck at y = 3 for `i >= 16` with no ramp between them.
fn two_floors() -> impl ColumnSource {
    FnSource(|i, _j, out| out.push(flat(if i < 16 { 0.0 } else { 3.0 })))
}

/// 32x8 ground split by a wall at column 16 with one gap in row 7.
fn walled() -> impl ColumnSource {
    FnSource(|i, j, out| {
        if i != 16 || j == 7 {
            out.push(flat(0.0))
        }
    })
}

fn strict() -> PathOptions {
    PathOptions {
        partial: false,
        ..PathOptions::default()
    }
}

fn link(from: NodeId, to: NodeId, cost: f32, bidirectional: bool, tag: u32) -> OffMeshLink {
    OffMeshLink {
        from,
        to,
        cost,
        bidirectional,
        tag,
    }
}

#[test]
fn a_one_way_link_joins_the_floors() {
    let grid = NavGrid::build(params(32, 8), &two_floors());
    let low = grid.node_at(at(14, 3, 0.0)).unwrap();
    let high = grid.node_at(at(18, 3, 3.0)).unwrap();
    let none = find_path(&grid, low, high, &strict());
    assert_eq!(none.status, PathStatus::Unreachable);

    let (a, b) = (
        grid.node_at(at(15, 3, 0.0)).unwrap(),
        grid.node_at(at(16, 3, 3.0)).unwrap(),
    );
    let links = NavLinks::new(vec![link(a, b, 5.0, false, 9)]);
    let path = find_path_linked(&grid, &links, low, high, &strict());
    assert_eq!(path.status, PathStatus::Complete);
    // 1 step to the link start, the link, 2 steps on the deck.
    assert!((path.cost - 8.0).abs() < 1e-5, "{}", path.cost);
    assert_eq!(path.links, vec![(1, 9)]);

    let back = find_path_linked(&grid, &links, high, low, &strict());
    assert_eq!(
        back.status,
        PathStatus::Unreachable,
        "a one-way link is not crossed backwards"
    );
    let both = NavLinks::new(vec![link(a, b, 5.0, true, 9)]);
    let back = find_path_linked(&grid, &both, high, low, &strict());
    assert_eq!(back.status, PathStatus::Complete);
    assert_eq!(back.links.len(), 1);
}

#[test]
fn the_cheaper_way_wins() {
    let grid = NavGrid::build(params(32, 8), &walled());
    let from = grid.node_at(at(10, 0, 0.0)).unwrap();
    let to = grid.node_at(at(22, 0, 0.0)).unwrap();
    let (a, b) = (
        grid.node_at(at(15, 0, 0.0)).unwrap(),
        grid.node_at(at(17, 0, 0.0)).unwrap(),
    );
    let walk = find_path(&grid, from, to, &strict());
    assert_eq!(walk.status, PathStatus::Complete);
    assert!(walk.links.is_empty());

    let cheap = find_path_linked(
        &grid,
        &NavLinks::new(vec![link(a, b, 2.0, true, 1)]),
        from,
        to,
        &strict(),
    );
    assert_eq!(cheap.links.len(), 1);
    assert!(cheap.cost < walk.cost, "{} vs {}", cheap.cost, walk.cost);

    let dear = find_path_linked(
        &grid,
        &NavLinks::new(vec![link(a, b, 100.0, true, 1)]),
        from,
        to,
        &strict(),
    );
    assert!(dear.links.is_empty(), "a costly link loses to the detour");
    // Ties between equal-cost detours may break differently under the linked heuristic.
    assert!(
        (dear.cost - walk.cost).abs() < 1e-4,
        "{} vs {}",
        dear.cost,
        walk.cost
    );
}

fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

#[test]
fn empty_links_equal_find_path() {
    let grid = NavGrid::build(params(32, 8), &walled());
    let n = grid.node_count() as u32;
    let mut state = 11u64;
    let empty = NavLinks::default();
    for _ in 0..200 {
        let from = NodeId(lcg(&mut state) % n);
        let to = NodeId(lcg(&mut state) % n);
        let want = find_path(&grid, from, to, &PathOptions::default());
        let got = find_path_linked(&grid, &empty, from, to, &PathOptions::default());
        assert_eq!(got, want, "{from:?} -> {to:?}");
    }
}

#[test]
fn link_marks_and_waypoints() {
    let grid = NavGrid::build(params(32, 8), &two_floors());
    let low = grid.node_at(at(2, 1, 0.0)).unwrap();
    let high = grid.node_at(at(28, 6, 3.0)).unwrap();
    let (a, b) = (
        grid.node_at(at(15, 3, 0.0)).unwrap(),
        grid.node_at(at(16, 3, 3.0)).unwrap(),
    );
    let links = NavLinks::new(vec![link(a, b, 3.0, false, 4)]);
    let path = find_path_linked(&grid, &links, low, high, &strict());
    assert_eq!(path.status, PathStatus::Complete);
    let (points, marks) = string_pull_linked(&grid, &path);
    assert_eq!(marks.len(), 1);
    let (i, tag) = marks[0];
    assert_eq!(tag, 4);
    assert_eq!(points[i as usize], grid.node_pos(a));
    assert_eq!(points[i as usize + 1], grid.node_pos(b));
    for k in 0..points.len() - 1 {
        if k as u32 != i {
            assert!(walkable_line(&grid, points[k], points[k + 1]), "leg {k}");
        }
    }
    assert_eq!(points.first(), Some(&grid.node_pos(low)));
    assert_eq!(points.last(), Some(&grid.node_pos(high)));

    let plain = find_path(&grid, low, a, &strict());
    let flat_path = ludion_nav::PathResult {
        links: Vec::new(),
        ..plain.clone()
    };
    let (pts, marks) = string_pull_linked(&grid, &flat_path);
    assert!(marks.is_empty());
    assert_eq!(pts, string_pull(&grid, &plain.nodes));
}

#[test]
fn the_same_request_twice_is_identical() {
    let grid = NavGrid::build(params(32, 8), &two_floors());
    let low = grid.node_at(at(2, 1, 0.0)).unwrap();
    let high = grid.node_at(at(28, 6, 3.0)).unwrap();
    let (a, b) = (
        grid.node_at(at(15, 3, 0.0)).unwrap(),
        grid.node_at(at(16, 3, 3.0)).unwrap(),
    );
    let (c, d) = (
        grid.node_at(at(15, 5, 0.0)).unwrap(),
        grid.node_at(at(16, 5, 3.0)).unwrap(),
    );
    let links = NavLinks::new(vec![link(a, b, 3.0, false, 4), link(c, d, 3.0, true, 5)]);
    let one = find_path_linked(&grid, &links, low, high, &strict());
    let two = find_path_linked(&grid, &links, low, high, &strict());
    assert_eq!(one, two);
}
