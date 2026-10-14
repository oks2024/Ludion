//! Wave 3 conformance: the frozen query API's pinned order, proven from outside the crate.
//!
//! Three layers, per the plan (`Plan/phase-12-spatial.md` Wave 3):
//!
//! - **Double-run bit-identity** — rebuilding the index twice over the same world and running every
//!   query twice yields identical results (the voxel-mesher pinned-sweep precedent).
//! - **Brute-force oracle** — for deterministic random worlds (several seeds, mixed shapes and
//!   bodies, multi-cell AABBs, cell-border positions), every query's result equals an independent
//!   O(n²) straight-line reference that never touches the grid: same set AND same order
//!   (cell-then-`EntityId` for `overlaps`, (squared distance, `EntityId`) for `k_nearest`,
//!   nearest-`t`-then-`EntityId` for `raycast`). The oracle reimplements the closed-form narrow
//!   phase inline (the formulas are the spec) but derives order by sorting, not by walking cells.
//! - **Tie-breaks** — co-located colliders, equidistant neighbors, and a genuinely bit-identical
//!   raycast `t` (mirrored geometry) each resolve by ascending `EntityId`.
//!
//! This file is an *integration* test deliberately: it consumes only the frozen public surface
//! (`#[non_exhaustive]` result structs are read, never constructed).

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_spatial::{Body, Collider, GroundPosition, Plane, Shape, spatial};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A minimal position component (tests must not depend on `ludion-engine`'s `Transform`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Pos {
    xz: [f32; 2],
}

impl GroundPosition for Pos {
    fn ground_position(&self, _plane: Plane) -> [f32; 2] {
        self.xz
    }
}

fn ctx() -> SystemCtx {
    SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
}

/// The oracle's own copy of one collider: everything a brute-force reference needs.
#[derive(Clone)]
struct Ref {
    entity: EntityId,
    center: [f32; 2],
    shape: Shape,
    body: Body,
}

fn spawn(world: &mut World, refs: &mut Vec<Ref>, xz: [f32; 2], shape: Shape, body: Body) {
    let e = world.spawn();
    world
        .insert(
            e,
            Collider {
                shape: shape.clone(),
                body,
                layer: None,
            },
        )
        .unwrap();
    world.insert(e, Pos { xz }).unwrap();
    refs.push(Ref {
        entity: e,
        center: xz,
        shape,
        body,
    });
}

/// A deterministic random world at the given seed: mixed circle/AABB shapes (including multi-cell
/// AABBs wider than [`spatial::CELL_SIZE`] and the production perimeter-wall shape), mixed
/// bodies, positions over ±70 with every 7th collider snapped to an exact cell border.
fn random_world(seed: u64, count: usize) -> (World, Vec<Ref>) {
    let mut world = World::new();
    let mut refs = Vec::with_capacity(count);
    let mut rng = DeterministicRng::new(seed);
    let coord = |rng: &mut DeterministicRng| (rng.next_f32() - 0.5) * 140.0;
    for i in 0..count {
        let mut xz = [coord(&mut rng), coord(&mut rng)];
        if i % 7 == 0 {
            // Exact cell borders — the floor() boundary case.
            xz = [
                (xz[0] / spatial::CELL_SIZE).round() * spatial::CELL_SIZE,
                (xz[1] / spatial::CELL_SIZE).round() * spatial::CELL_SIZE,
            ];
        }
        let shape = if i % 30 == 0 {
            // The production perimeter-wall shape: a map-spanning static AABB whose extreme
            // aspect ratio inflates every probe rectangle it touches — the case the
            // static/dynamic index split exists to contain.
            Shape::Aabb {
                half_extents: [2.0, 70.0],
            }
        } else {
            match i % 5 {
                0 | 3 => Shape::Circle {
                    radius: 0.2 + rng.next_f32() * 1.8,
                },
                1 => Shape::Aabb {
                    half_extents: [0.3 + rng.next_f32() * 2.0, 0.3 + rng.next_f32() * 2.0],
                },
                // Multi-cell AABBs: half-extents beyond one CELL_SIZE, spanning several cells.
                2 => Shape::Aabb {
                    half_extents: [12.0 + rng.next_f32() * 18.0, 1.0 + rng.next_f32() * 4.0],
                },
                _ => Shape::Circle {
                    radius: 0.05 + rng.next_f32() * 0.3,
                },
            }
        };
        let body = if i % 30 == 0 || i % 3 == 0 {
            Body::new_static()
        } else {
            Body::new_kinematic()
        };
        spawn(&mut world, &mut refs, xz, shape, body);
    }
    (world, refs)
}

// --- The independent O(n²) oracle -----------------------------------------------------------

fn oracle_cell(point: [f32; 2]) -> (i32, i32) {
    (
        (point[0] / spatial::CELL_SIZE).floor() as i32,
        (point[1] / spatial::CELL_SIZE).floor() as i32,
    )
}

fn oracle_admits(filter: Option<Body>, body: Body) -> bool {
    filter.is_none_or(|wanted| wanted == body)
}

/// Straight-line reimplementation of the three closed-form overlap cases. `random_world` never
/// generates a 3D shape, so the wildcard arm is defensive-only, mirroring the production
/// `shapes_overlap`'s own conservative `false`.
fn oracle_overlap(a: &Shape, ac: [f32; 2], b: &Shape, bc: [f32; 2]) -> bool {
    fn circle_vs_aabb(cc: [f32; 2], r: f32, bc: [f32; 2], he: [f32; 2]) -> bool {
        let nx = cc[0].clamp(bc[0] - he[0], bc[0] + he[0]);
        let nz = cc[1].clamp(bc[1] - he[1], bc[1] + he[1]);
        let dx = cc[0] - nx;
        let dz = cc[1] - nz;
        dx * dx + dz * dz <= r * r
    }
    match (a, b) {
        (Shape::Circle { radius: ra }, Shape::Circle { radius: rb }) => {
            let dx = ac[0] - bc[0];
            let dz = ac[1] - bc[1];
            dx * dx + dz * dz <= (ra + rb) * (ra + rb)
        }
        (Shape::Aabb { half_extents: ha }, Shape::Aabb { half_extents: hb }) => {
            (ac[0] - bc[0]).abs() <= ha[0] + hb[0] && (ac[1] - bc[1]).abs() <= ha[1] + hb[1]
        }
        (Shape::Circle { radius }, Shape::Aabb { half_extents }) => {
            circle_vs_aabb(ac, *radius, bc, *half_extents)
        }
        (Shape::Aabb { half_extents }, Shape::Circle { radius }) => {
            circle_vs_aabb(bc, *radius, ac, *half_extents)
        }
        _ => false,
    }
}

/// Brute-force `overlaps`: test every collider, then sort into the pinned order — candidate cells
/// in lexicographic `(cx, cz)` order, then `EntityId` — by *sorting*, never by walking a grid.
fn oracle_overlaps(
    refs: &[Ref],
    probe: &Shape,
    center: [f32; 2],
    filter: Option<Body>,
) -> Vec<EntityId> {
    let mut matched: Vec<((i32, i32), EntityId)> = refs
        .iter()
        .filter(|r| oracle_admits(filter, r.body))
        .filter(|r| oracle_overlap(probe, center, &r.shape, r.center))
        .map(|r| (oracle_cell(r.center), r.entity))
        .collect();
    matched.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    matched.into_iter().map(|(_, e)| e).collect()
}

/// Brute-force `k_nearest`: every distance, sorted by (squared distance, `EntityId`), first k.
fn oracle_k_nearest(
    refs: &[Ref],
    center: [f32; 2],
    k: usize,
    max_radius: f32,
    filter: Option<Body>,
) -> Vec<EntityId> {
    let max_sq = max_radius * max_radius;
    let mut candidates: Vec<(f32, EntityId)> = refs
        .iter()
        .filter(|r| oracle_admits(filter, r.body))
        .filter_map(|r| {
            let dx = center[0] - r.center[0];
            let dz = center[1] - r.center[1];
            let sq = dx * dx + dz * dz;
            (sq <= max_sq).then_some((sq, r.entity))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    candidates.into_iter().take(k).map(|(_, e)| e).collect()
}

/// Straight-line smallest non-negative ray parameter against one shape.
///
/// Mirrors the library's arithmetic exactly (quadratic for circles; slab with `* (1/d)` for AABBs)
/// so an asserted bit-equal `t` is meaningful — a re-derived formula could differ in the last bit
/// and turn an exact tie-break test into noise.
fn oracle_ray_t(origin: [f32; 2], dir: [f32; 2], shape: &Shape, center: [f32; 2]) -> Option<f32> {
    match shape {
        Shape::Circle { radius } => {
            let radius = *radius;
            let ox = origin[0] - center[0];
            let oz = origin[1] - center[1];
            let a = dir[0] * dir[0] + dir[1] * dir[1];
            if a == 0.0 {
                return (ox * ox + oz * oz <= radius * radius).then_some(0.0);
            }
            let b = 2.0 * (ox * dir[0] + oz * dir[1]);
            let c = ox * ox + oz * oz - radius * radius;
            let disc = b * b - 4.0 * a * c;
            if disc < 0.0 {
                return None;
            }
            let sq = ludion_core::math::sqrt(disc);
            let t0 = (-b - sq) / (2.0 * a);
            let t1 = (-b + sq) / (2.0 * a);
            if t0 >= 0.0 {
                Some(t0)
            } else if t1 >= 0.0 {
                Some(0.0)
            } else {
                None
            }
        }
        Shape::Aabb { half_extents: he } => {
            let he = *he;
            let min = [center[0] - he[0], center[1] - he[1]];
            let max = [center[0] + he[0], center[1] + he[1]];
            let mut t_enter = f32::NEG_INFINITY;
            let mut t_exit = f32::INFINITY;
            for axis in 0..2 {
                if dir[axis] == 0.0 {
                    if origin[axis] < min[axis] || origin[axis] > max[axis] {
                        return None;
                    }
                } else {
                    let inv = 1.0 / dir[axis];
                    let mut t_near = (min[axis] - origin[axis]) * inv;
                    let mut t_far = (max[axis] - origin[axis]) * inv;
                    if t_near > t_far {
                        std::mem::swap(&mut t_near, &mut t_far);
                    }
                    t_enter = t_enter.max(t_near);
                    t_exit = t_exit.min(t_far);
                    if t_enter > t_exit {
                        return None;
                    }
                }
            }
            if t_exit < 0.0 {
                return None;
            }
            Some(t_enter.max(0.0))
        }
        // `random_world` never generates a 3D shape; a miss is the defensive answer.
        Shape::Sphere { .. }
        | Shape::Box { .. }
        | Shape::Capsule { .. }
        | Shape::Cylinder { .. }
        | Shape::Convex { .. }
        | Shape::Heightfield { .. }
        | Shape::Compound { .. }
        | Shape::Mesh { .. } => None,
    }
}

/// Brute-force `raycast`: every collider's `t`, minimum by (`t`, `EntityId`).
fn oracle_raycast(
    refs: &[Ref],
    origin: [f32; 2],
    dir: [f32; 2],
    filter: Option<Body>,
) -> Option<(EntityId, f32)> {
    let mut best: Option<(f32, EntityId)> = None;
    for r in refs.iter().filter(|r| oracle_admits(filter, r.body)) {
        let Some(t) = oracle_ray_t(origin, dir, &r.shape, r.center) else {
            continue;
        };
        let candidate = (t, r.entity);
        let better = match best {
            None => true,
            Some(current) => {
                candidate
                    .0
                    .total_cmp(&current.0)
                    .then(candidate.1.cmp(&current.1))
                    == std::cmp::Ordering::Less
            }
        };
        if better {
            best = Some(candidate);
        }
    }
    best.map(|(t, e)| (e, t))
}

// --- Oracle property tests -------------------------------------------------------------------

const SEEDS: [u64; 5] = [1, 2, 3, 42, 0xC0FFEE];
const WORLD_SIZE: usize = 150;
fn filters() -> [Option<Body>; 3] {
    [None, Some(Body::new_static()), Some(Body::new_kinematic())]
}

/// Deterministic probe centers per seed: random points plus exact cell corners.
fn probe_centers(rng: &mut DeterministicRng) -> Vec<[f32; 2]> {
    let mut centers: Vec<[f32; 2]> = (0..8)
        .map(|_| {
            [
                (rng.next_f32() - 0.5) * 160.0,
                (rng.next_f32() - 0.5) * 160.0,
            ]
        })
        .collect();
    centers.push([0.0, 0.0]);
    centers.push([spatial::CELL_SIZE, -spatial::CELL_SIZE]);
    centers.push([-3.0 * spatial::CELL_SIZE, 2.0 * spatial::CELL_SIZE]);
    centers
}

#[test]
fn overlaps_matches_the_brute_force_oracle() {
    for seed in SEEDS {
        let (mut world, refs) = random_world(seed, WORLD_SIZE);
        spatial::rebuild::<Pos>(&mut world, &ctx());
        let mut rng = DeterministicRng::new_with_stream(seed, 1);
        let probes = [
            Shape::Circle { radius: 0.5 },
            Shape::Circle { radius: 6.0 },
            Shape::Circle {
                radius: 2.5 * spatial::CELL_SIZE,
            },
            Shape::Aabb {
                half_extents: [3.0, 0.5],
            },
            Shape::Aabb {
                half_extents: [spatial::CELL_SIZE, spatial::CELL_SIZE],
            },
        ];
        for center in probe_centers(&mut rng) {
            for probe in &probes {
                for filter in filters() {
                    let got = spatial::overlaps(&world, probe.clone(), center, filter);
                    let expected = oracle_overlaps(&refs, probe, center, filter);
                    assert_eq!(
                        got, expected,
                        "seed {seed}: overlaps diverged at {center:?} probe {probe:?} \
                         filter {filter:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn k_nearest_matches_the_brute_force_oracle() {
    for seed in SEEDS {
        let (mut world, refs) = random_world(seed, WORLD_SIZE);
        spatial::rebuild::<Pos>(&mut world, &ctx());
        let mut rng = DeterministicRng::new_with_stream(seed, 2);
        for center in probe_centers(&mut rng) {
            for &(k, radius) in &[(1usize, 4.0f32), (8, 6.0), (20, 40.0), (WORLD_SIZE, 500.0)] {
                for filter in filters() {
                    let got = spatial::k_nearest(&world, center, k, radius, filter);
                    let expected = oracle_k_nearest(&refs, center, k, radius, filter);
                    assert_eq!(
                        got, expected,
                        "seed {seed}: k_nearest diverged at {center:?} k {k} radius {radius} \
                         filter {filter:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn raycast_matches_the_brute_force_oracle() {
    for seed in SEEDS {
        let (mut world, refs) = random_world(seed, WORLD_SIZE);
        spatial::rebuild::<Pos>(&mut world, &ctx());
        let mut rng = DeterministicRng::new_with_stream(seed, 3);
        let dirs = [
            [1.0, 0.0],
            [0.0, -1.0],
            [0.7, -0.7],
            [3.0, 1.5], // deliberately non-normalized: t scales, order does not
            [0.0, 0.0], // degenerate zero ray: hits only what the origin is inside
        ];
        for origin in probe_centers(&mut rng) {
            for &dir in &dirs {
                for filter in filters() {
                    let got = spatial::raycast(&world, origin, dir, filter);
                    let expected = oracle_raycast(&refs, origin, dir, filter);
                    match (got, expected) {
                        (None, None) => {}
                        (Some(hit), Some((entity, t))) => {
                            assert_eq!(
                                hit.entity, entity,
                                "seed {seed}: raycast entity diverged at {origin:?} dir {dir:?} \
                                 filter {filter:?}"
                            );
                            assert_eq!(
                                hit.t.to_bits(),
                                t.to_bits(),
                                "seed {seed}: raycast t diverged at {origin:?} dir {dir:?}"
                            );
                            assert_eq!(
                                [hit.point[0].to_bits(), hit.point[1].to_bits()],
                                [
                                    (origin[0] + t * dir[0]).to_bits(),
                                    (origin[1] + t * dir[1]).to_bits()
                                ],
                                "seed {seed}: raycast point diverged at {origin:?} dir {dir:?}"
                            );
                        }
                        (got, expected) => panic!(
                            "seed {seed}: raycast hit/miss diverged at {origin:?} dir {dir:?} \
                             filter {filter:?}: got {got:?}, expected {expected:?}"
                        ),
                    }
                }
            }
        }
    }
}

// --- Double-run bit-identity -----------------------------------------------------------------

/// Rebuilding twice over the same world and running every query twice yields bit-identical
/// results — determinism by construction, asserted (the voxel-mesher pinned-sweep precedent).
#[test]
fn double_run_results_are_bit_identical() {
    for seed in SEEDS {
        let (mut world, _) = random_world(seed, WORLD_SIZE);
        let probe = Shape::Circle { radius: 5.0 };
        let center = [7.0, -13.0];
        let dir = [0.6, 0.8];

        spatial::rebuild::<Pos>(&mut world, &ctx());
        let overlaps_1 = spatial::overlaps(&world, probe.clone(), center, None);
        let nearest_1 = spatial::k_nearest(&world, center, 10, 30.0, None);
        let ray_1 = spatial::raycast(&world, center, dir, None);
        // Query again without rebuilding …
        assert_eq!(
            overlaps_1,
            spatial::overlaps(&world, probe.clone(), center, None)
        );
        assert_eq!(
            nearest_1,
            spatial::k_nearest(&world, center, 10, 30.0, None)
        );
        assert_eq!(ray_1, spatial::raycast(&world, center, dir, None));

        // … and again after a second rebuild of the same world content.
        spatial::rebuild::<Pos>(&mut world, &ctx());
        assert_eq!(overlaps_1, spatial::overlaps(&world, probe, center, None));
        assert_eq!(
            nearest_1,
            spatial::k_nearest(&world, center, 10, 30.0, None)
        );
        assert_eq!(ray_1, spatial::raycast(&world, center, dir, None));
    }
}

// --- Tie-breaks -------------------------------------------------------------------------------

/// Colliders at the *same position with the same shape* return in ascending `EntityId` order from
/// `overlaps` and `k_nearest` (identical cell, identical distance — only the id can order them).
#[test]
fn co_located_colliders_return_in_entity_id_order() {
    let mut world = World::new();
    let mut refs = Vec::new();
    let shape = Shape::Circle { radius: 1.0 };
    for _ in 0..3 {
        spawn(
            &mut world,
            &mut refs,
            [5.0, 5.0],
            shape.clone(),
            Body::new_kinematic(),
        );
    }
    spatial::rebuild::<Pos>(&mut world, &ctx());
    let ids: Vec<EntityId> = refs.iter().map(|r| r.entity).collect();
    assert_eq!(
        spatial::overlaps(&world, shape, [5.0, 5.0], None),
        ids,
        "co-located overlaps must come back in ascending EntityId order"
    );
    assert_eq!(
        spatial::k_nearest(&world, [5.0, 5.0], 3, 10.0, None),
        ids,
        "co-located k_nearest must come back in ascending EntityId order"
    );
}

/// Equidistant `k_nearest` neighbors (bit-identical squared distance by mirrored geometry) order by
/// ascending `EntityId`.
#[test]
fn equidistant_k_nearest_orders_by_entity_id() {
    let mut world = World::new();
    let mut refs = Vec::new();
    let shape = Shape::Circle { radius: 0.5 };
    // Four colliders mirrored around the probe: (±3, 0), (0, ±3) — dx²+dz² is exactly 9.0 for all.
    for xz in [[3.0, 0.0], [-3.0, 0.0], [0.0, 3.0], [0.0, -3.0]] {
        spawn(&mut world, &mut refs, xz, shape.clone(), Body::new_static());
    }
    spatial::rebuild::<Pos>(&mut world, &ctx());
    let ids: Vec<EntityId> = refs.iter().map(|r| r.entity).collect();
    assert_eq!(spatial::k_nearest(&world, [0.0, 0.0], 4, 10.0, None), ids);
    // A k smaller than the tie group still takes the lowest ids.
    assert_eq!(
        spatial::k_nearest(&world, [0.0, 0.0], 2, 10.0, None),
        ids[..2]
    );
}

/// A genuinely bit-identical raycast `t` (two circles mirrored across the ray axis: `(±0.5)²` is
/// exact in f32, so both quadratics produce the same bits) resolves to the lower `EntityId` — and
/// the iteration meets the higher-sorting cell first, so this exercises the tie-break, not
/// iteration luck.
#[test]
fn raycast_equal_t_tie_breaks_by_entity_id() {
    let mut world = World::new();
    let mut refs = Vec::new();
    let shape = Shape::Circle { radius: 1.0 };
    // Ray along +x at z = 0. Mirrored circles at [5, ±0.5] both straddle the ray; their quadratics
    // share ox, a, b bit for bit and oz² = 0.25 exactly, hence identical t. Cell keys differ
    // ((0, 0) vs (0, -1) at CELL_SIZE 16): the (0, -1) cell — the *second-spawned, higher* id —
    // iterates first, so the assertion below fails unless the equal-t branch actively prefers the
    // lower EntityId.
    spawn(
        &mut world,
        &mut refs,
        [5.0, 0.5],
        shape.clone(),
        Body::new_static(),
    );
    spawn(
        &mut world,
        &mut refs,
        [5.0, -0.5],
        shape.clone(),
        Body::new_static(),
    );
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let first = refs[0].entity;
    let second = refs[1].entity;
    assert!(first < second);
    let hit = spatial::raycast(&world, [0.0, 0.0], [1.0, 0.0], None).unwrap();
    assert_eq!(
        hit.entity, first,
        "equal-t raycast must prefer the lower EntityId"
    );

    // Prove the tie is real: both circles alone yield the same t bits.
    let t_alone = |xz: [f32; 2]| {
        let mut w = World::new();
        let mut r = Vec::new();
        spawn(&mut w, &mut r, xz, shape.clone(), Body::new_static());
        spatial::rebuild::<Pos>(&mut w, &ctx());
        spatial::raycast(&w, [0.0, 0.0], [1.0, 0.0], None)
            .unwrap()
            .t
    };
    assert_eq!(
        t_alone([5.0, 0.5]).to_bits(),
        t_alone([5.0, -0.5]).to_bits()
    );
}

// --- k_nearest_entries (J1) ---------------------------------------------------------------------

/// `k_nearest_entries` returns entries whose ids exactly match `k_nearest`'s output — same
/// candidates, same sort, same `take(k)` — and whose `center` is the rebuild-time position, not a
/// live re-read (the whole point: a J1 caller in a parallel region can consume a neighbor's
/// position without touching its potentially-checked-out column).
#[test]
fn k_nearest_entries_ids_match_k_nearest_and_centers_are_rebuild_time() {
    for seed in SEEDS {
        let (mut world, refs) = random_world(seed, WORLD_SIZE);
        spatial::rebuild::<Pos>(&mut world, &ctx());
        let mut rng = DeterministicRng::new_with_stream(seed, 4);
        for center in probe_centers(&mut rng) {
            for &(k, radius) in &[(1usize, 4.0f32), (8, 6.0), (20, 40.0), (WORLD_SIZE, 500.0)] {
                for filter in filters() {
                    let ids = spatial::k_nearest(&world, center, k, radius, filter);
                    let entries = spatial::k_nearest_entries(&world, center, k, radius, filter);
                    let entry_ids: Vec<EntityId> = entries.iter().map(|e| e.entity).collect();
                    assert_eq!(
                        entry_ids, ids,
                        "seed {seed}: k_nearest_entries ids diverged from k_nearest at \
                         {center:?} k {k} radius {radius} filter {filter:?}"
                    );
                    for entry in &entries {
                        let expected_center = refs
                            .iter()
                            .find(|r| r.entity == entry.entity)
                            .expect("every returned entry must be one of the spawned refs")
                            .center;
                        assert_eq!(
                            entry.center, expected_center,
                            "seed {seed}: k_nearest_entries center for {:?} was not the \
                             rebuild-time position",
                            entry.entity
                        );
                    }
                }
            }
        }
    }
}

// --- move_and_slide_at (J1) ----------------------------------------------------------------------

/// `move_and_slide_at`, given the mover's own live-read center, resolves identically to
/// `move_and_slide::<Pos>` for the same world and delta — free, single-axis-blocked (slide), and
/// corner-blocked (both axes) cases, the same wall placements `tests/slide.rs` uses to pin the
/// frozen fn's own behavior.
#[test]
fn move_and_slide_at_matches_move_and_slide_across_blocked_and_unblocked_cases() {
    fn check(statics: &[([f32; 2], Shape)], mover_start: [f32; 2], delta: [f32; 2]) {
        let mover_shape = Shape::Circle { radius: 0.5 };
        let mut world = World::new();
        let mut refs = Vec::new();
        let mover = world.spawn();
        world
            .insert(
                mover,
                Collider {
                    shape: mover_shape,
                    body: Body::new_kinematic(),
                    layer: None,
                },
            )
            .unwrap();
        world.insert(mover, Pos { xz: mover_start }).unwrap();
        for (xz, shape) in statics {
            spawn(
                &mut world,
                &mut refs,
                *xz,
                shape.clone(),
                Body::new_static(),
            );
        }
        spatial::rebuild::<Pos>(&mut world, &ctx());

        let center = world.get::<Pos>(mover).unwrap().unwrap().xz;
        assert_eq!(center, mover_start);
        let via_p = spatial::move_and_slide::<Pos>(&world, mover, delta);
        let via_at = spatial::move_and_slide_at(&world, mover, center, delta);
        assert_eq!(
            via_at, via_p,
            "move_and_slide_at diverged from move_and_slide for start {mover_start:?} \
             delta {delta:?}"
        );
    }

    // Free: nothing within reach of the full delta.
    check(
        &[([50.0, 50.0], Shape::Circle { radius: 0.5 })],
        [0.0, 0.0],
        [0.123, -4.567],
    );
    // Single-axis blocked: a vertical wall dead ahead in x, the slide preserves z.
    check(
        &[(
            [2.0, 0.0],
            Shape::Aabb {
                half_extents: [0.5, 4.0],
            },
        )],
        [0.0, 0.0],
        [2.0, 1.0],
    );
    // Corner: a vertical wall blocks +x, a horizontal wall blocks +z — both axes cancel.
    check(
        &[
            (
                [2.0, 0.0],
                Shape::Aabb {
                    half_extents: [0.5, 4.0],
                },
            ),
            (
                [0.0, 2.0],
                Shape::Aabb {
                    half_extents: [4.0, 0.5],
                },
            ),
        ],
        [0.0, 0.0],
        [2.0, 2.0],
    );
}

// --- Empty index ------------------------------------------------------------------------------

/// An index rebuilt over a collider-free world behaves exactly like the pre-rebuild state: every
/// query returns empty/None. (The pre-rebuild case itself is covered by the Wave 1 unit test.)
#[test]
fn rebuilt_empty_world_returns_empty_results() {
    let mut world = World::new();
    world.spawn(); // an entity with no Collider must not disturb anything
    spatial::rebuild::<Pos>(&mut world, &ctx());
    assert!(spatial::overlaps(&world, Shape::default(), [0.0, 0.0], None).is_empty());
    assert!(spatial::k_nearest(&world, [0.0, 0.0], 5, 100.0, None).is_empty());
    assert!(spatial::raycast(&world, [0.0, 0.0], [1.0, 0.0], None).is_none());
}
