//! Wave 4 conformance for [`spatial::move_and_slide`]: behavior, edge cases, pruning
//! independence, and the two-run `WorldHash`-trace determinism assertion.
//!
//! Everything here consumes the frozen public surface only. The `WorldHash` test is *the* real
//! determinism assertion of the wave: `move_and_slide` itself is a pure read, so its resolved
//! delta only matters once a game system applies it to a hashed component — the trace test drives
//! exactly that loop, twice, and compares every frame's hash.

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{Registry, Schedule, SystemCtx, World};
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

fn spawn(world: &mut World, xz: [f32; 2], shape: Shape, body: Body) -> EntityId {
    let e = world.spawn();
    world
        .insert(
            e,
            Collider {
                shape,
                body,
                layer: None,
            },
        )
        .unwrap();
    world.insert(e, Pos { xz }).unwrap();
    e
}

const MOVER: Shape = Shape::Circle { radius: 0.5 };

// --- Behavior ----------------------------------------------------------------------------------

/// A mover in exact contact with a wall (circle r=1 tangent to an AABB face) is not blocked: a
/// delta along the face passes whole.
#[test]
fn exact_contact_is_not_blocked() {
    let mut world = World::new();
    let mover = spawn(
        &mut world,
        [2.0, 0.0],
        Shape::Circle { radius: 1.0 },
        Body::new_kinematic(),
    );
    spawn(
        &mut world,
        [0.0, 0.0],
        Shape::Aabb {
            half_extents: [1.0, 1.0],
        },
        Body::new_static(),
    );
    spatial::rebuild::<Pos>(&mut world, &ctx());

    assert_eq!(
        spatial::move_and_slide::<Pos>(&world, mover, [0.0, 0.1]),
        [0.0, 0.1]
    );
}

/// Sliding diagonally into a vertical wall: the normal (x) component cancels, the tangential (z)
/// component survives.
#[test]
fn slide_along_a_wall_preserves_the_tangential_component() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    // A vertical wall occupying x ∈ [1.5, 2.5], z ∈ [-4, 4].
    spawn(
        &mut world,
        [2.0, 0.0],
        Shape::Aabb {
            half_extents: [0.5, 4.0],
        },
        Body::new_static(),
    );
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let resolved = spatial::move_and_slide::<Pos>(&world, mover, [2.0, 1.0]);
    assert_eq!(resolved, [0.0, 1.0], "x cancelled, z preserved");
}

/// A corner (both axes blocked) resolves to exactly zero.
#[test]
fn corner_blocks_both_axes() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    // Vertical wall blocking +x, horizontal wall blocking +z.
    spawn(
        &mut world,
        [2.0, 0.0],
        Shape::Aabb {
            half_extents: [0.5, 4.0],
        },
        Body::new_static(),
    );
    spawn(
        &mut world,
        [0.0, 2.0],
        Shape::Aabb {
            half_extents: [4.0, 0.5],
        },
        Body::new_static(),
    );
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let resolved = spatial::move_and_slide::<Pos>(&world, mover, [2.0, 2.0]);
    assert_eq!(resolved, [0.0, 0.0], "both axes blocked at the corner");
}

/// The x axis is the documented preference when either single axis alone would be free.
#[test]
fn x_axis_is_preferred_when_both_single_axes_are_free() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    // A small block sitting exactly on the diagonal target; both single-axis moves dodge it.
    spawn(
        &mut world,
        [2.0, 2.0],
        Shape::Aabb {
            half_extents: [0.6, 0.6],
        },
        Body::new_static(),
    );
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let resolved = spatial::move_and_slide::<Pos>(&world, mover, [2.0, 2.0]);
    assert_eq!(resolved, [2.0, 0.0], "x-only is tried before z-only");
}

// --- Edge cases --------------------------------------------------------------------------------

/// No static in range → the delta comes back unchanged (bit for bit).
#[test]
fn no_static_in_range_returns_the_delta_unchanged() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    spawn(&mut world, [50.0, 50.0], MOVER, Body::new_static()); // far away
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let delta = [0.123, -4.567];
    assert_eq!(spatial::move_and_slide::<Pos>(&world, mover, delta), delta);
}

/// An entity without a `Collider` is a legal non-colliding mover: delta unchanged.
#[test]
fn entity_without_a_collider_returns_the_delta_unchanged() {
    let mut world = World::new();
    let bare = world.spawn();
    world.insert(bare, Pos { xz: [0.0, 0.0] }).unwrap();
    spawn(&mut world, [0.5, 0.0], MOVER, Body::new_static()); // would block if tested
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let delta = [1.0, 1.0];
    assert_eq!(spatial::move_and_slide::<Pos>(&world, bare, delta), delta);
}

/// Before the first rebuild there is no index: delta unchanged.
#[test]
fn pre_rebuild_returns_the_delta_unchanged() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    let delta = [1.0, 0.0];
    assert_eq!(spatial::move_and_slide::<Pos>(&world, mover, delta), delta);
}

/// A collider-carrying entity with no position component: delta unchanged (nowhere to sweep from).
#[test]
fn entity_without_a_position_returns_the_delta_unchanged() {
    let mut world = World::new();
    let e = world.spawn();
    world
        .insert(
            e,
            Collider {
                shape: MOVER,
                body: Body::new_kinematic(),
                layer: None,
            },
        )
        .unwrap();
    spawn(&mut world, [0.5, 0.0], MOVER, Body::new_static());
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let delta = [1.0, 0.0];
    assert_eq!(spatial::move_and_slide::<Pos>(&world, e, delta), delta);
}

/// The mover's own index entry never blocks it — excluded by `EntityId`, even when the mover is
/// itself `Static` (the index holds a stale snapshot of the mover at its pre-move position).
#[test]
fn mover_never_collides_with_its_own_entry() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_static());
    spatial::rebuild::<Pos>(&mut world, &ctx());

    // A small step: the end position still overlaps the mover's own stale entry at the origin.
    let delta = [0.1, 0.0];
    assert_eq!(
        spatial::move_and_slide::<Pos>(&world, mover, delta),
        delta,
        "an entity must not be blocked by its own index entry"
    );
}

/// `Dynamic` colliders never block a slide — mover-vs-mover response is game logic.
#[test]
fn dynamic_colliders_never_block() {
    let mut world = World::new();
    let mover = spawn(&mut world, [0.0, 0.0], MOVER, Body::new_kinematic());
    spawn(&mut world, [1.0, 0.0], MOVER, Body::new_kinematic()); // dead ahead, but dynamic
    spatial::rebuild::<Pos>(&mut world, &ctx());

    let delta = [1.0, 0.0];
    assert_eq!(spatial::move_and_slide::<Pos>(&world, mover, delta), delta);
}

// --- Determinism and pruning independence ------------------------------------------------------

/// Same world + same delta → the same resolved delta, bit for bit, across repeated calls and
/// across a second rebuild.
#[test]
fn same_world_and_delta_resolve_identically() {
    fn resolve_all(world: &World, movers: &[EntityId]) -> Vec<[f32; 2]> {
        let deltas = [[0.3, -0.2], [1.5, 1.5], [-2.0, 0.7]];
        let mut out = Vec::new();
        for &m in movers {
            for &d in &deltas {
                out.push(spatial::move_and_slide::<Pos>(world, m, d));
            }
        }
        out
    }

    let (mut world, movers, _) = random_world(11);
    spatial::rebuild::<Pos>(&mut world, &ctx());
    let first = resolve_all(&world, &movers);
    let second = resolve_all(&world, &movers);
    assert_eq!(first, second);
    spatial::rebuild::<Pos>(&mut world, &ctx());
    let third = resolve_all(&world, &movers);
    assert_eq!(first, third);
}

/// The oracle's copy of one collider.
#[derive(Clone)]
struct Ref {
    entity: EntityId,
    center: [f32; 2],
    shape: Shape,
    body: Body,
}

/// A deterministic random world: dynamic circle movers among a field of static circles and AABBs
/// (including multi-cell AABBs and the production perimeter-wall shape), so pruning has
/// something to prune.
fn random_world(seed: u64) -> (World, Vec<EntityId>, Vec<Ref>) {
    let mut world = World::new();
    let mut movers = Vec::new();
    let mut refs = Vec::new();
    let mut rng = DeterministicRng::new(seed);
    let push = |world: &mut World, refs: &mut Vec<Ref>, xz: [f32; 2], shape: Shape, body| {
        let e = spawn(world, xz, shape.clone(), body);
        refs.push(Ref {
            entity: e,
            center: xz,
            shape,
            body,
        });
        e
    };
    for i in 0..120 {
        let xz = [(rng.next_f32() - 0.5) * 80.0, (rng.next_f32() - 0.5) * 80.0];
        if i % 41 == 0 {
            // The production perimeter-wall shape: a map-spanning static AABB whose extreme
            // aspect ratio inflates every probe rectangle it touches — the case the
            // static/dynamic index split exists to contain.
            push(
                &mut world,
                &mut refs,
                xz,
                Shape::Aabb {
                    half_extents: [2.0, 70.0],
                },
                Body::new_static(),
            );
            continue;
        }
        match i % 4 {
            0 => {
                let e = push(
                    &mut world,
                    &mut refs,
                    xz,
                    Shape::Circle {
                        radius: 0.2 + rng.next_f32() * 0.6,
                    },
                    Body::new_kinematic(),
                );
                movers.push(e);
            }
            1 => {
                push(
                    &mut world,
                    &mut refs,
                    xz,
                    Shape::Aabb {
                        half_extents: [0.4 + rng.next_f32() * 2.0, 0.4 + rng.next_f32() * 2.0],
                    },
                    Body::new_static(),
                );
            }
            2 => {
                // Multi-cell statics (wider than CELL_SIZE).
                push(
                    &mut world,
                    &mut refs,
                    xz,
                    Shape::Aabb {
                        half_extents: [12.0 + rng.next_f32() * 10.0, 1.0 + rng.next_f32() * 2.0],
                    },
                    Body::new_static(),
                );
            }
            _ => {
                push(
                    &mut world,
                    &mut refs,
                    xz,
                    Shape::Circle {
                        radius: 0.3 + rng.next_f32() * 1.2,
                    },
                    Body::new_static(),
                );
            }
        }
    }
    (world, movers, refs)
}

/// Straight-line reimplementation of the three closed-form overlap cases (the oracle's own copy).
/// `random_world` never generates a 3D shape; the wildcard arm is defensive-only.
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

/// Brute-force `move_and_slide`: the same four-step test-and-cancel, with the blocked test a full
/// scan over every collider — no grid, no pruning.
fn oracle_slide(
    refs: &[Ref],
    mover: EntityId,
    shape: &Shape,
    center: [f32; 2],
    delta: [f32; 2],
) -> [f32; 2] {
    let blocked = |end: [f32; 2]| {
        refs.iter().any(|r| {
            matches!(r.body, Body::Static(_))
                && r.entity != mover
                && oracle_overlap(shape, end, &r.shape, r.center)
        })
    };
    if !blocked([center[0] + delta[0], center[1] + delta[1]]) {
        return delta;
    }
    if !blocked([center[0] + delta[0], center[1]]) {
        return [delta[0], 0.0];
    }
    if !blocked([center[0], center[1] + delta[1]]) {
        return [0.0, delta[1]];
    }
    [0.0, 0.0]
}

/// Pruning independence: the pruned slide equals the brute-force full-scan reference for every
/// mover and a spread of deltas, over several seeds.
#[test]
fn move_and_slide_matches_the_brute_force_oracle() {
    for seed in [1u64, 2, 42, 0xC0FFEE] {
        let (mut world, movers, refs) = random_world(seed);
        spatial::rebuild::<Pos>(&mut world, &ctx());
        let deltas = [
            [0.5, 0.0],
            [0.0, -0.5],
            [1.5, 1.5],
            [-3.0, 2.0],
            [20.0, -20.0], // a long teleport-ish delta, crossing many cells
            [0.0, 0.0],
        ];
        for &m in &movers {
            let (shape, center) = {
                let collider = world.get::<Collider>(m).unwrap().unwrap();
                let pos = world.get::<Pos>(m).unwrap().unwrap();
                (collider.shape.clone(), pos.xz)
            };
            for &delta in &deltas {
                let got = spatial::move_and_slide::<Pos>(&world, m, delta);
                let expected = oracle_slide(&refs, m, &shape, center, delta);
                assert_eq!(
                    got, expected,
                    "seed {seed}: slide diverged for {m} delta {delta:?}"
                );
            }
        }
    }
}

// --- The WorldHash trace ------------------------------------------------------------------------

/// The real determinism assertion. `move_and_slide` is a pure read, so its output only
/// becomes observable when a game system applies the resolved delta to a hashed component — this
/// drives that exact loop (rebuild system, then a driver system applying slides to `Pos`) for 30
/// frames, twice from identical worlds, and asserts the per-frame `WorldHash` traces match.
///
/// Lives in `ludion-spatial`'s integration tests via a **dev-dependency on `ludion-sim`**
/// (`WorldHash`'s home): dev-deps are outside the boundary graph — the recorded
/// `ludion-server → client, testkit` precedent — and `ludion-sim` never depends on this crate, so
/// there is no cycle in either direction.
#[test]
fn two_run_world_hash_trace_is_identical() {
    use ludion_sim::WorldHash;

    /// The driver: every dynamic collider slides by a deterministic per-entity, per-frame delta,
    /// and the resolved delta is applied to `Pos` — the hashed write, in "game" code.
    fn slide_movers(world: &mut World, ctx: &SystemCtx) {
        let movers: Vec<EntityId> = world
            .entities()
            .filter(|&e| {
                world
                    .get::<Collider>(e)
                    .is_ok_and(|c| c.is_some_and(|c| matches!(c.body, Body::Kinematic(_))))
                    && world.has::<Pos>(e)
            })
            .collect();
        let frame = ctx.frame.as_u64();
        for e in movers {
            // A fixed heading per entity (from its index), stepped every frame; plain arithmetic,
            // deterministic in f32 (not exact).
            let phase = (e.index() % 8) as f32;
            let delta = [
                0.15 * (phase - 3.5) * if frame.is_multiple_of(2) { 1.0 } else { -0.5 },
                0.10 * (3.5 - phase),
            ];
            let resolved = spatial::move_and_slide::<Pos>(world, e, delta);
            let pos = world.get_mut::<Pos>(e).unwrap().unwrap();
            pos.xz[0] += resolved[0];
            pos.xz[1] += resolved[1];
        }
    }

    fn run() -> Vec<String> {
        let (mut world, _, _) = random_world(99);
        let mut registry = Registry::new();
        registry.register::<Pos>("Pos").unwrap();
        registry.register::<Collider>("Collider").unwrap();

        let mut schedule = Schedule::new();
        schedule.add_system("spatial_rebuild", spatial::rebuild::<Pos>);
        schedule.add_system("slide_movers", slide_movers);

        let mut trace = Vec::new();
        for frame in 0..30u64 {
            let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
            schedule.run(&mut world, &ctx);
            trace.push(WorldHash::compute(&world, &registry).unwrap().to_string());
        }
        trace
    }

    let first = run();
    let second = run();
    assert_eq!(first, second, "two identical runs must hash identically");
    // The trace must actually evolve (movers move), or the assertion above proves nothing.
    assert!(
        first.windows(2).any(|w| w[0] != w[1]),
        "the world must change across frames for the trace to be meaningful"
    );
}
