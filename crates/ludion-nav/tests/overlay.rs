//! The dynamic overlay: an empty one changes nothing, blocks split reachability, costs are charged once per
//! crossing, stop regions are passable for A* only and split string pulls.

mod common;

use common::*;
use ludion_nav::{
    FlowField, NavGrid, NavOverlay, NodeId, NodeMark, PathOptions, PathStatus, find_path,
    string_pull, string_pull_linked, string_pull_marked, walkable_line,
};

fn open() -> impl ludion_nav::ColumnSource {
    FnSource(|_i, _j, out| out.push(flat(0.0)))
}

fn strict() -> PathOptions {
    PathOptions {
        partial: false,
        ..PathOptions::default()
    }
}

fn node(grid: &NavGrid, i: u32, j: u32) -> NodeId {
    grid.node_at(at(i, j, 0.0)).unwrap()
}

/// An overlay marking every node of the columns `cols` (all rows) through `mark`.
fn strip(
    grid: &NavGrid,
    cols: std::ops::RangeInclusive<u32>,
    mark: impl Fn(&mut NavOverlay, u32),
) -> NavOverlay {
    let mut overlay = NavOverlay::new(grid.node_count());
    for i in cols {
        for j in 0..grid.params().nz {
            if let Some(n) = grid.node_at(at(i, j, 0.0)) {
                mark(&mut overlay, n.0);
            }
        }
    }
    overlay
}

fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as u32
}

#[test]
fn an_empty_overlay_changes_nothing() {
    let mut grid = NavGrid::build(params(16, 8), &open());
    let reference = NavGrid::build(params(16, 8), &open());
    let generation = grid.generation();
    assert!(!grid.set_overlay(None));
    assert!(!grid.set_overlay(Some(NavOverlay::new(grid.node_count()))));
    assert_eq!(grid.generation(), generation);

    let (a, b) = (node(&grid, 0, 0), node(&grid, 15, 7));
    assert_eq!(
        find_path(&grid, a, b, &strict()),
        find_path(&reference, a, b, &strict())
    );
    let (mut f, mut g) = (FlowField::default(), FlowField::default());
    f.compute(&grid, b, f32::INFINITY);
    g.compute(&reference, b, f32::INFINITY);
    for n in 0..grid.node_count() as u32 {
        assert_eq!(
            f.distance(NodeId(n)).to_bits(),
            g.distance(NodeId(n)).to_bits()
        );
        assert_eq!(f.flow(NodeId(n)), g.flow(NodeId(n)));
    }
    let path = find_path(&grid, a, b, &strict());
    assert_eq!(
        string_pull(&grid, &path.nodes),
        string_pull(&reference, &path.nodes)
    );
    let marked = string_pull_marked(&grid, &path);
    assert!(marked.links.is_empty() && marked.stops.is_empty());
    assert_eq!(marked.points, string_pull_linked(&reference, &path).0);
}

#[test]
fn a_block_wall_splits_reachability_and_clearing_restores_it() {
    let mut grid = NavGrid::build(params(16, 8), &open());
    let (a, b) = (node(&grid, 2, 3), node(&grid, 13, 3));
    assert_eq!(
        find_path(&grid, a, b, &strict()).status,
        PathStatus::Complete
    );
    let before = grid.generation();

    let wall = strip(&grid, 8..=8, |o, n| o.block(n));
    assert!(grid.set_overlay(Some(wall.clone())));
    assert_eq!(grid.generation(), before + 1);
    assert_eq!(
        find_path(&grid, a, b, &strict()).status,
        PathStatus::Unreachable
    );
    assert!(!walkable_line(&grid, at(2, 3, 0.0), at(13, 3, 0.0)));
    let mut field = FlowField::default();
    field.compute(&grid, b, f32::INFINITY);
    assert!(field.distance(a).is_infinite());
    assert!(grid.is_blocked(node(&grid, 8, 3)));

    assert!(!grid.set_overlay(Some(wall)), "the same overlay is a no-op");
    assert!(grid.set_overlay(None));
    assert_eq!(grid.generation(), before + 2);
    assert_eq!(
        find_path(&grid, a, b, &strict()).status,
        PathStatus::Complete
    );
    assert!(walkable_line(&grid, at(2, 3, 0.0), at(13, 3, 0.0)));
}

#[test]
fn a_block_does_not_let_a_diagonal_cut_its_corner() {
    let mut grid = NavGrid::build(params(4, 4), &open());
    let mut overlay = NavOverlay::new(grid.node_count());
    overlay.block(node(&grid, 2, 1).0);
    grid.set_overlay(Some(overlay));
    // (1,1) -> (2,2) would cut the corner of the blocked (2,1).
    let path = find_path(&grid, node(&grid, 1, 1), node(&grid, 2, 2), &strict());
    assert!(path.cost > 1.5, "{}", path.cost);
}

#[test]
fn a_cost_strip_is_charged_once_per_crossing() {
    // A 1-row corridor 10 long: the strip is 3 cells wide, crossed once.
    let mut grid = NavGrid::build(params(10, 1), &open());
    let (a, b) = (node(&grid, 0, 0), node(&grid, 9, 0));
    let plain = find_path(&grid, a, b, &strict()).cost;
    let strip = strip(&grid, 3..=5, |o, n| o.cost(n, 4.0));
    grid.set_overlay(Some(strip));
    let priced = find_path(&grid, a, b, &strict());
    assert_eq!(priced.status, PathStatus::Complete);
    assert!(
        (priced.cost - (plain + 4.0)).abs() < 1e-5,
        "{}",
        priced.cost
    );
}

#[test]
fn a_cost_strip_makes_a_star_detour_only_when_the_detour_is_cheaper() {
    let mut grid = NavGrid::build(params(20, 12), &open());
    let (a, b) = (node(&grid, 2, 5), node(&grid, 17, 5));
    // A strip across columns 9..=10 on rows 2..=8: the way round through row 0 or 11 costs extra.
    let mut overlay = NavOverlay::new(grid.node_count());
    for i in 9..=10 {
        for j in 2..=8 {
            overlay.cost(node(&grid, i, j).0, 50.0);
        }
    }
    grid.set_overlay(Some(overlay));
    let detour = find_path(&grid, a, b, &strict());
    assert!(
        detour.nodes.iter().all(|&n| grid.mark(n) == NodeMark::None),
        "an expensive strip is walked round"
    );

    let mut overlay = NavOverlay::new(grid.node_count());
    for i in 9..=10 {
        for j in 2..=8 {
            overlay.cost(node(&grid, i, j).0, 0.5);
        }
    }
    grid.set_overlay(Some(overlay));
    let cross = find_path(&grid, a, b, &strict());
    assert!(
        cross
            .nodes
            .iter()
            .any(|&n| matches!(grid.mark(n), NodeMark::Cost(_))),
        "a cheap strip is crossed"
    );
    assert!((cross.cost - 15.5).abs() < 1e-4, "{}", cross.cost);
}

#[test]
fn a_star_equals_the_dijkstra_under_a_cost_overlay() {
    let mut grid = NavGrid::build(params(24, 24), &open());
    let mut overlay = NavOverlay::new(grid.node_count());
    for i in 4..20u32 {
        for j in 0..24u32 {
            if (i * 7 + j * 3) % 5 < 2 {
                overlay.cost(node(&grid, i, j).0, (1 + (i + j) % 4) as f32);
            } else if (i + j) % 11 == 0 {
                overlay.block(node(&grid, i, j).0);
            }
        }
    }
    grid.set_overlay(Some(overlay));
    let n = grid.node_count() as u32;
    let mut state = 11u64;
    let mut field = FlowField::default();
    let mut complete = 0;
    for _ in 0..1000 {
        let (from, to) = (NodeId(lcg(&mut state) % n), NodeId(lcg(&mut state) % n));
        field.compute(&grid, to, f32::INFINITY);
        let want = field.distance(from);
        let got = find_path(&grid, from, to, &strict());
        if want.is_finite() {
            assert_eq!(got.status, PathStatus::Complete, "{from:?} -> {to:?}");
            assert!(
                (got.cost - want).abs() <= 1e-4 * want.max(1.0),
                "{} vs {want}",
                got.cost
            );
            complete += 1;
        } else {
            assert_eq!(got.status, PathStatus::Unreachable, "{from:?} -> {to:?}");
        }
    }
    assert!(complete > 400, "{complete}");
}

#[test]
fn a_stop_strip_is_crossed_by_a_star_but_not_by_fields_or_lines() {
    let mut grid = NavGrid::build(params(12, 1), &open());
    let (a, b) = (node(&grid, 1, 0), node(&grid, 10, 0));
    let door = strip(&grid, 5..=6, |o, n| o.stop(n, 3));
    grid.set_overlay(Some(door));
    assert_eq!(grid.stop_tag(node(&grid, 5, 0)), Some(3));

    let path = find_path(&grid, a, b, &strict());
    assert_eq!(path.status, PathStatus::Complete);
    assert!(!walkable_line(&grid, at(1, 0, 0.0), at(10, 0, 0.0)));

    let mut field = FlowField::default();
    field.compute(&grid, b, f32::INFINITY);
    assert!(
        field.distance(a).is_infinite(),
        "a chaser does not open doors"
    );
    assert!(field.distance(node(&grid, 8, 0)).is_finite());

    let pulled = string_pull_marked(&grid, &path);
    // The node before the region is column 4, the fourth waypoint of the pre-pull path `1..=4`.
    assert_eq!(pulled.stops.len(), 1);
    let (at_index, tag) = pulled.stops[0];
    assert_eq!(tag, 3);
    let before = grid.node_pos(node(&grid, 4, 0));
    assert_eq!(pulled.points[at_index as usize], before);
    let inside = grid.node_pos(node(&grid, 5, 0));
    assert_eq!(pulled.points[at_index as usize + 1], inside);
    assert!(pulled.links.is_empty());
}

#[test]
fn block_beats_stop_beats_cost() {
    let mut overlay = NavOverlay::new(4);
    overlay.cost(0, 1.0);
    overlay.stop(0, 5);
    overlay.stop(0, 2);
    overlay.cost(0, 9.0);
    assert_eq!(overlay.mark(0), NodeMark::Stop(2));
    overlay.block(0);
    overlay.stop(0, 1);
    assert_eq!(overlay.mark(0), NodeMark::Block);
}
