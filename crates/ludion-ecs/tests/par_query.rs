//! The parallel-join contract: [`World::par_for_each`] and [`World::par_join_fold`].
//!
//! The shape of this suite mirrors `ludion_core::par`'s own: a **guard** test first, proving the
//! fixture's arithmetic is order-sensitive, then the invariance tests that would otherwise pass
//! vacuously. What is asserted throughout is `to_bits`-exactness against the *sequential* reference —
//! `World::query` — at 1, 2, and 8 threads and at several chunk sizes.

use std::any::TypeId;

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The thread counts the whole parallel surface is contracted to agree on.
const COUNTS: [usize; 3] = [1, 2, 8];

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Velocity {
    dx: f32,
    dy: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Mass(f32);

/// A component no query in this file ever touches — the control for the region's absence rule.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Label(u32);

/// A component type no entity ever carries — an absent, never-lifted read must stay `Ok(None)`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Absent(u32);

/// A resource, to prove a chunk can read one through its `ParCtx`.
struct Gravity(f32);

/// Values spanning a wide dynamic range, so regrouping additions moves the last bits. Without this
/// the invariance assertions below would hold for uninteresting reasons.
fn wide(rng: &mut DeterministicRng, i: usize) -> f32 {
    let magnitude = match i % 4 {
        0 => 1.0e6,
        1 => 1.0e-3,
        2 => 1.0e3,
        _ => 1.0,
    };
    (rng.next_f32() - 0.5) * magnitude
}

/// A world whose columns are *not* in spawn order: a scattered third of the slots are recycled, and
/// components are inserted in a shuffled order so `Storage::insert` splices instead of appending.
/// Not every entity carries every component, so the join has real work to do.
fn fixture(count: usize) -> (World, Vec<EntityId>) {
    let mut rng = DeterministicRng::new(0xC0FFEE);
    let mut world = World::new();
    world.insert_resource(Gravity(9.81));

    let mut ids: Vec<EntityId> = (0..count).map(|_| world.spawn()).collect();
    let doomed: Vec<EntityId> = ids
        .iter()
        .copied()
        .enumerate()
        .filter(|(i, _)| i % 3 == 1)
        .map(|(_, id)| id)
        .collect();
    for id in &doomed {
        world.despawn(*id).unwrap();
    }
    ids.retain(|id| world.is_alive(*id));
    for _ in 0..doomed.len() {
        ids.push(world.spawn());
    }

    let mut order: Vec<usize> = (0..ids.len()).collect();
    for i in (1..order.len()).rev() {
        let j = rng.gen_range_usize(0..i + 1);
        order.swap(i, j);
    }
    for &k in &order {
        let id = ids[k];
        world
            .insert(
                id,
                Position {
                    x: wide(&mut rng, k),
                    y: wide(&mut rng, k + 1),
                },
            )
            .unwrap();
        if k % 4 != 0 {
            world
                .insert(
                    id,
                    Velocity {
                        dx: wide(&mut rng, k + 2),
                        dy: wide(&mut rng, k + 3),
                    },
                )
                .unwrap();
        }
        if k % 7 != 0 {
            world.insert(id, Mass(wide(&mut rng, k))).unwrap();
        }
        world.insert(id, Label(k as u32)).unwrap();
    }
    ids.sort();
    (world, ids)
}

/// The per-row arithmetic under test, written once so the sequential reference and the parallel run
/// cannot drift apart. Deliberately non-linear and mass-weighted, so a mis-paired row shows up.
fn step(pos: &mut Position, vel: &Velocity, mass: f32) -> f32 {
    pos.x = pos.x * 0.5 + vel.dx * mass;
    pos.y = pos.y * 0.25 - vel.dy / mass;
    pos.x + pos.y
}

/// The sequential oracle: the same join, the same arithmetic, no threads.
fn sequential(world: &mut World) -> Vec<f32> {
    world
        .query::<(&mut Position, &Velocity, &Mass)>()
        .map(|(pos, vel, mass)| step(pos, vel, mass.0))
        .collect()
}

/// Every `Position` in the world, in ascending entity index, as raw bits — the world-state oracle.
fn position_bits(world: &mut World) -> Vec<(u32, u32)> {
    world
        .query::<(&Position,)>()
        .map(|(p,)| (p.x.to_bits(), p.y.to_bits()))
        .collect()
}

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

// --- Guard --------------------------------------------------------------------------------------

#[test]
fn the_fixture_arithmetic_is_order_sensitive() {
    let (mut world, _) = fixture(400);
    let values = sequential(&mut world);
    assert!(values.len() > 100, "the fixture must produce enough rows");

    // A flat sum and a chunked sum of the same values differ in the last bits, and different chunk
    // groupings differ from each other. Without this, `par_join_fold`'s chunk-order contract and the
    // invariance assertions below would be testing nothing.
    let flat = values.iter().sum::<f32>();
    let grouped = |size: usize| {
        values
            .chunks(size)
            .map(|c| c.iter().sum::<f32>())
            .fold(0.0f32, |a, b| a + b)
    };
    assert_ne!(flat.to_bits(), grouped(16).to_bits());
    assert_ne!(grouped(16).to_bits(), grouped(37).to_bits());
}

// --- par_for_each: the determinism contract -----------------------------------------------------

#[test]
fn par_for_each_matches_the_sequential_query_at_every_thread_count() {
    let (mut oracle_world, _) = fixture(400);
    let expected = bits(&sequential(&mut oracle_world));
    let expected_world = position_bits(&mut oracle_world);

    for count in COUNTS {
        let (mut world, _) = fixture(400);
        let produced = par::with_threads(count, || {
            world.par_for_each::<(&mut Position, &Velocity, &Mass), _>(
                32,
                |_ctx, _id, (pos, vel, mass)| step(pos, vel, mass.0),
            )
        });
        assert_eq!(
            bits(&produced),
            expected,
            "par_for_each results at {count} threads must equal the sequential query bit for bit",
        );
        assert_eq!(
            position_bits(&mut world),
            expected_world,
            "the world par_for_each leaves at {count} threads must equal the sequential one",
        );
    }
}

#[test]
fn par_for_each_is_invariant_under_chunk_rows() {
    let (mut oracle_world, _) = fixture(400);
    let expected = bits(&sequential(&mut oracle_world));
    let expected_world = position_bits(&mut oracle_world);
    let rows = expected.len();

    for chunk_rows in [1usize, 7, 64, rows + 1] {
        for count in COUNTS {
            let (mut world, _) = fixture(400);
            let produced = par::with_threads(count, || {
                world.par_for_each::<(&mut Position, &Velocity, &Mass), _>(
                    chunk_rows,
                    |_ctx, _id, (pos, vel, mass)| step(pos, vel, mass.0),
                )
            });
            assert_eq!(
                bits(&produced),
                expected,
                "chunk_rows {chunk_rows} at {count} threads changed the results",
            );
            assert_eq!(
                position_bits(&mut world),
                expected_world,
                "chunk_rows {chunk_rows} at {count} threads changed the world",
            );
        }
    }
}

#[test]
fn par_for_each_visits_exactly_the_rows_query_yields() {
    let (mut world, _) = fixture(200);
    let expected: Vec<EntityId> = world
        .query_ids::<(&mut Position, &Velocity, &Mass)>()
        .map(|(id, _)| id)
        .collect();
    // The fixture deliberately withholds Velocity from every 4th entity and Mass from every 7th, so
    // the join is a strict subset of the Position column.
    assert!(expected.len() < world.query::<(&Position,)>().count());

    for count in COUNTS {
        let (mut world, _) = fixture(200);
        let visited = par::with_threads(count, || {
            world.par_for_each::<(&mut Position, &Velocity, &Mass), _>(
                8,
                |_ctx, id, (_pos, _vel, _mass)| id,
            )
        });
        assert_eq!(visited, expected, "at {count} threads");
        assert!(
            visited.windows(2).all(|w| w[0] < w[1]),
            "rows must arrive in ascending entity index",
        );
    }
}

#[test]
fn par_for_each_over_one_column_and_two_columns() {
    let (mut world, _) = fixture(64);
    let ones = world.par_for_each::<(&mut Position,), _>(4, |_ctx, _id, (pos,)| {
        pos.x = 1.0;
        pos.x
    });
    assert!(ones.iter().all(|&v| v == 1.0));

    let twos = world.par_for_each::<(&mut Position, &mut Velocity), _>(4, |_ctx, _id, (p, v)| {
        p.y = 2.0;
        v.dx = 3.0;
        p.y + v.dx
    });
    assert!(twos.iter().all(|&v| v == 5.0));
}

#[test]
fn an_empty_join_yields_nothing_and_returns_the_columns() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Position { x: 4.0, y: 5.0 }).unwrap();
    // No Velocity anywhere, so the join is empty — but Position must survive the region.
    let out: Vec<f32> = world
        .par_for_each::<(&mut Position, &Velocity), _>(8, |_, _, _| unreachable!("no rows exist"));
    assert!(out.is_empty());
    assert_eq!(
        world.get::<Position>(e).unwrap(),
        Some(&Position { x: 4.0, y: 5.0 }),
    );
}

#[test]
#[should_panic(expected = "chunk_rows must be non-zero")]
fn par_for_each_rejects_a_zero_chunk_size() {
    let (mut world, _) = fixture(8);
    let _: Vec<()> = world.par_for_each::<(&mut Position,), _>(0, |_, _, _| ());
}

#[test]
#[should_panic(expected = "same component type twice")]
fn par_for_each_rejects_a_duplicated_component_type() {
    let (mut world, _) = fixture(8);
    let _: Vec<()> = world.par_for_each::<(&mut Position, &Position), _>(4, |_, _, _| ());
}

// --- par_for_each: the region model -------------------------------------------------------------

#[test]
fn inside_the_region_the_exclusive_columns_are_absent_but_everything_else_reads() {
    let (mut world, ids) = fixture(64);
    let other = ids[3];

    let observations =
        world.par_for_each::<(&mut Position, &Velocity), _>(8, |ctx, _id, (_pos, _vel)| {
            (
                // The exclusive column was lifted out of the world for the region: a loud error,
                // never `Ok(None)`.
                ctx.world
                    .get::<Position>(other)
                    .err()
                    .map(|diagnostic| diagnostic.code),
                // A shared column of the same query stays in place.
                ctx.world.get::<Velocity>(other).unwrap().is_some(),
                // So does a component the query never named.
                ctx.world.get::<Label>(other).unwrap().is_some(),
                // A type that is merely absent (never lifted) is still `Ok(None)`.
                ctx.world.get::<Absent>(other).unwrap().is_none(),
                // And so do resources.
                ctx.world.resource::<Gravity>().map(|g| g.0),
            )
        });

    assert!(!observations.is_empty());
    for (lifted, velocity, label, absent, gravity) in observations {
        assert_eq!(lifted, Some(code::LIFTED_COMPONENT_READ));
        assert!(velocity, "a & column of the same query still reads");
        assert!(label, "an unrelated component still reads");
        assert!(absent, "an absent, unlifted type is still Ok(None)");
        assert_eq!(gravity, Some(9.81));
    }

    // The region closed: the column is back and the writes landed.
    assert!(world.get::<Position>(other).unwrap().is_some());
    assert!(world.has::<Position>(other));
}

#[test]
fn has_on_a_lifted_column_panics_with_l0026_and_the_world_recovers() {
    let (mut world, ids) = fixture(64);
    let other = ids[3];

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.par_for_each::<(&mut Position, &Velocity), _>(8, |ctx, _id, _row| {
            ctx.world.has::<Position>(other)
        })
    }));
    let payload = caught.expect_err("`has` on a lifted column cannot answer a bool");
    let message = payload
        .downcast_ref::<String>()
        .expect("the panic carries the rendered diagnostic");
    assert!(message.contains("L0026"), "{message}");
    assert!(message.contains("Position"), "{message}");

    // Every lifted box was put back before the unwind resumed, and the lifted set with it.
    assert!(world.get::<Position>(other).unwrap().is_some());
    assert!(world.has::<Position>(other));
}

/// Runs `read` inside a `par_for_each::<(&mut Position, &Velocity)>` region and returns the panic
/// message it must raise; asserts the world recovered afterwards.
fn lifted_read_panic_message(read: impl Fn(&World) + Sync) -> String {
    let (mut world, ids) = fixture(64);
    let positions = world.query::<(&Position,)>().count();

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.par_for_each::<(&mut Position, &Velocity), _>(8, |ctx, _id, _row| read(ctx.world))
    }));
    let payload = caught.expect_err("a read of a lifted column must panic");
    let message = payload
        .downcast_ref::<String>()
        .expect("the panic carries the rendered diagnostic")
        .clone();

    assert!(world.get::<Position>(ids[3]).unwrap().is_some());
    assert_eq!(world.query::<(&Position,)>().count(), positions);
    message
}

#[test]
fn read_column_of_a_lifted_column_panics_with_l0026_and_the_world_recovers() {
    let message = lifted_read_panic_message(|world| {
        let _ = world.read_column::<Position>();
    });
    assert!(message.contains("L0026"), "{message}");
    assert!(message.contains("Position"), "{message}");
}

#[test]
fn par_join_fold_over_a_lifted_column_panics_with_l0026_and_the_world_recovers() {
    let message = lifted_read_panic_message(|world| {
        let _ = world.par_join_fold::<(&Position,), usize, usize>(
            4,
            |rows| rows.count(),
            0,
            |total, part| total + part,
        );
    });
    assert!(message.contains("L0026"), "{message}");
    assert!(message.contains("Position"), "{message}");
}

#[test]
fn entity_indices_with_a_lifted_column_panics_with_l0026_and_the_world_recovers() {
    let message = lifted_read_panic_message(|world| {
        let _ = world.entity_indices_with(TypeId::of::<Position>()).count();
    });
    assert!(message.contains("L0026"), "{message}");
    assert!(message.contains("Position"), "{message}");
}

#[test]
fn region_readers_still_answer_for_unlifted_and_absent_types() {
    let (mut world, _) = fixture(64);
    let velocities = world.query::<(&Velocity,)>().count();
    let labelled: Vec<u32> = world
        .entity_indices_with(TypeId::of::<Velocity>())
        .collect();
    assert!(velocities > 0);

    let observations = world.par_for_each::<(&mut Position, &Velocity), _>(8, |ctx, _id, _row| {
        (
            ctx.world.par_join_fold::<(&Velocity,), usize, usize>(
                4,
                |rows| rows.count(),
                0,
                |total, part| total + part,
            ),
            ctx.world.par_join_fold::<(&Absent,), usize, usize>(
                4,
                |rows| rows.count(),
                0,
                |total, part| total + part,
            ),
            ctx.world
                .entity_indices_with(TypeId::of::<Velocity>())
                .collect::<Vec<_>>(),
            ctx.world
                .entity_indices_with(TypeId::of::<Absent>())
                .count(),
        )
    });

    assert!(!observations.is_empty());
    for (folded, absent_folded, indices, absent_indices) in observations {
        assert_eq!(folded, velocities);
        assert_eq!(absent_folded, 0);
        assert_eq!(indices, labelled);
        assert_eq!(absent_indices, 0);
    }
}

#[test]
fn in_join_answers_membership_for_live_stale_and_absent_handles() {
    let (mut world, ids) = fixture(64);
    let member = world
        .query_ids::<(&mut Position, &Velocity)>()
        .map(|(id, _)| id)
        .next()
        .expect("the fixture joins at least one row");
    let outsider = *ids
        .iter()
        .find(|&&id| !world.has::<Velocity>(id))
        .expect("the fixture withholds Velocity from some entities");
    let dead = world.spawn();
    world.despawn(dead).unwrap();
    let stale = EntityId::new(member.index(), member.generation().wrapping_add(1));

    let answers = world.par_for_each::<(&mut Position, &Velocity), _>(8, |ctx, _id, _row| {
        (
            ctx.in_join(member),
            ctx.in_join(outsider),
            ctx.in_join(dead),
            ctx.in_join(stale),
        )
    });
    for (is_member, is_outsider, is_dead, is_stale) in answers {
        assert!(is_member, "a joined entity is in the join");
        assert!(!is_outsider, "an entity missing a column is not");
        assert!(!is_dead, "a despawned handle is not");
        assert!(!is_stale, "a stale generation on a joined slot is not");
    }
}

#[test]
fn a_panicking_chunk_propagates_and_leaves_the_world_whole() {
    for count in COUNTS {
        let (mut world, _) = fixture(64);
        let before = position_bits(&mut world);

        // The schedule's boundary shape: catch at the system edge, then keep using the world.
        let caught = par::with_threads(count, || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                world.par_for_each::<(&mut Position, &Velocity), _>(1, |_ctx, id, (pos, _vel)| {
                    assert!(id.index() < 4, "row {} refuses", id.index());
                    pos.x
                })
            }))
        });
        let payload = caught.expect_err("a panicking chunk must reach the caller");
        let message = payload
            .downcast_ref::<String>()
            .expect("the payload is the panicking row's own message");
        assert!(
            message.starts_with("row 4 refuses"),
            "at {count} threads the lowest-index chunk's panic must win, got {message:?}",
        );

        // The lifted column went back: the world still answers queries and per-entity reads.
        let after = position_bits(&mut world);
        assert_eq!(after.len(), before.len(), "the Position column came back");
        assert!(
            world.query::<(&mut Position, &Velocity)>().next().is_some(),
            "the world is usable after the caught panic",
        );
    }
}

// --- Recording ----------------------------------------------------------------------------------

/// The distinct shapes a system was observed running, as sorted `(TypeId, write)` vectors.
fn shapes(world: &mut World, system: &str) -> Vec<Vec<(TypeId, bool)>> {
    let observed = world.take_query_recording().expect("recording was armed");
    let mut out: Vec<Vec<(TypeId, bool)>> = observed.shapes(system).map(<[_]>::to_vec).collect();
    out.sort();
    out
}

#[test]
fn par_for_each_records_what_the_sequential_idiom_would() {
    let (mut world, ids) = fixture(32);
    let other = ids[1];

    // The sequential idiom: a query, plus a per-entity read mid-loop.
    world.begin_query_recording();
    world.set_recording_system("integrate");
    let touched: Vec<EntityId> = world
        .query_ids::<(&mut Position, &Velocity)>()
        .map(|(id, _)| id)
        .collect();
    for _ in &touched {
        let _ = world.get::<Label>(other).unwrap();
    }
    let expected = shapes(&mut world, "integrate");

    // The parallel form, with the same mid-loop read happening on worker threads.
    world.begin_query_recording();
    world.set_recording_system("integrate");
    par::with_threads(8, || {
        world.par_for_each::<(&mut Position, &Velocity), _>(4, |ctx, _id, _row| {
            ctx.world.get::<Label>(other).unwrap().is_some()
        })
    });
    let produced = shapes(&mut world, "integrate");

    assert_eq!(
        produced, expected,
        "a region declares the join shape and its chunks' reads exactly as the sequential idiom does",
    );
    assert_eq!(produced.len(), 2, "the join shape plus the Label read");
}

#[test]
fn a_region_records_nothing_outside_a_system() {
    let (mut world, _) = fixture(16);
    world.begin_query_recording();
    world.set_recording_system("mover");
    world.clear_recording_system();
    let _ = world.par_for_each::<(&mut Position,), _>(4, |_, _, (p,)| p.x);
    assert!(shapes(&mut world, "mover").is_empty());
}

// --- par_join_fold ------------------------------------------------------------------------------

/// The sequential oracle for the fold: the same rows, the same chunk grouping, no threads.
fn sequential_fold(world: &mut World, chunk_rows: usize) -> f32 {
    let values: Vec<f32> = world
        .query::<(&Position, &Mass)>()
        .map(|(p, m)| p.x * m.0)
        .collect();
    values
        .chunks(chunk_rows)
        .map(|c| c.iter().sum::<f32>())
        .fold(0.0f32, |total, part| total + part)
}

#[test]
fn par_join_fold_matches_the_sequential_fold_at_every_thread_count() {
    let (mut world, _) = fixture(400);
    let expected = sequential_fold(&mut world, 32);

    for count in COUNTS {
        let total = par::with_threads(count, || {
            world.par_join_fold::<(&Position, &Mass), f32, f32>(
                32,
                |rows| rows.map(|(_id, (p, m))| p.x * m.0).sum(),
                0.0,
                |total, part| total + part,
            )
        });
        assert_eq!(
            total.to_bits(),
            expected.to_bits(),
            "the fold at {count} threads must equal the sequential one bit for bit",
        );
    }
}

#[test]
fn par_join_fold_joins_three_and_four_columns_like_the_sequential_query() {
    let (mut world, _) = fixture(400);
    let expected3: Vec<u32> = world
        .query::<(&Position, &Velocity, &Mass)>()
        .map(|(p, v, m)| (p.x * v.dx + m.0).to_bits())
        .collect();
    let expected4: Vec<u32> = world
        .query::<(&Position, &Velocity, &Mass, &Label)>()
        .map(|(p, v, m, l)| (p.x * v.dx + m.0 + l.0 as f32).to_bits())
        .collect();
    assert!(expected3.len() > 32 && expected4.len() > 32);

    let join = |mut acc: Vec<u32>, part: Vec<u32>| {
        acc.extend(part);
        acc
    };
    for count in COUNTS {
        let (three, four) = par::with_threads(count, || {
            (
                world.par_join_fold::<(&Position, &Velocity, &Mass), Vec<u32>, Vec<u32>>(
                    32,
                    |rows| {
                        rows.map(|(_id, (p, v, m))| (p.x * v.dx + m.0).to_bits())
                            .collect()
                    },
                    Vec::new(),
                    join,
                ),
                world.par_join_fold::<(&Position, &Velocity, &Mass, &Label), Vec<u32>, Vec<u32>>(
                    32,
                    |rows| {
                        rows.map(|(_id, (p, v, m, l))| (p.x * v.dx + m.0 + l.0 as f32).to_bits())
                            .collect()
                    },
                    Vec::new(),
                    join,
                ),
            )
        });
        assert_eq!(three, expected3, "3-tuple at {count} threads");
        assert_eq!(four, expected4, "4-tuple at {count} threads");
    }
}

#[test]
fn par_join_fold_combines_chunks_in_ascending_chunk_order() {
    let (world, _) = fixture(200);
    // A non-commutative combiner: the answer is the chunk order itself.
    let order = world.par_join_fold::<(&Position, &Mass), Vec<u32>, Vec<u32>>(
        16,
        |rows| rows.map(|(id, _)| id.index()).collect(),
        Vec::new(),
        |mut acc, part| {
            acc.extend(part);
            acc
        },
    );
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "chunks must fold in ascending chunk order, so rows come back ascending",
    );

    // And the guard: folding those same chunks in any other order would be observable.
    let mut reversed = order.clone();
    reversed.reverse();
    assert_ne!(order, reversed);
}

#[test]
fn par_join_fold_depends_on_chunk_rows_but_never_on_threads() {
    let (mut world, _) = fixture(400);
    let coarse = sequential_fold(&mut world, 128);
    let fine = sequential_fold(&mut world, 3);
    assert_ne!(
        coarse.to_bits(),
        fine.to_bits(),
        "chunk size is part of the contract: a different grouping must be a different sum",
    );

    for count in COUNTS {
        for (chunk_rows, expected) in [(128usize, coarse), (3, fine)] {
            let total = par::with_threads(count, || {
                world.par_join_fold::<(&Position, &Mass), f32, f32>(
                    chunk_rows,
                    |rows| rows.map(|(_id, (p, m))| p.x * m.0).sum(),
                    0.0,
                    |total, part| total + part,
                )
            });
            assert_eq!(total.to_bits(), expected.to_bits(), "at {count} threads");
        }
    }
}

#[test]
fn par_join_fold_reads_the_whole_world_and_records_its_shape() {
    let (mut world, ids) = fixture(32);
    world.begin_query_recording();
    world.set_recording_system("census");
    let labelled = world.par_join_fold::<(&Position,), usize, usize>(
        8,
        |rows| rows.filter(|(id, _)| id.index() == ids[0].index()).count(),
        0usize,
        |total, part| total + part,
    );
    assert_eq!(labelled, 1);

    let recorded = shapes(&mut world, "census");
    assert_eq!(recorded, vec![vec![(TypeId::of::<Position>(), false)]]);
}

#[test]
fn par_join_fold_over_an_empty_world_is_the_initial_value() {
    let world = World::new();
    let total = world.par_join_fold::<(&Position,), f32, f32>(
        8,
        |rows| rows.map(|(_, (p,))| p.x).sum(),
        7.5,
        |total, part| total + part,
    );
    assert_eq!(total, 7.5);
}

#[test]
#[should_panic(expected = "chunk_rows must be non-zero")]
fn par_join_fold_rejects_a_zero_chunk_size() {
    let (world, _) = fixture(8);
    let _ =
        world.par_join_fold::<(&Position,), usize, usize>(0, |rows| rows.count(), 0, |a, b| a + b);
}

// --- The threading contract itself --------------------------------------------------------------

#[test]
fn the_world_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<World>();
}

// --- Randomized join equivalence ------------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GA(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GB(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GC(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct GD(u32);

type Mirror = [std::collections::BTreeMap<u32, u32>; 4];
type Rows = Vec<(u32, Vec<u32>)>;

/// `n` spawns, a quarter despawned so slot indices have holes, then component `t` (value = the
/// entity's index) on every live entity `keep(t)` accepts, inserted in shuffled order. Every insert
/// is mirrored in a `BTreeMap` for the naive reference join.
fn join_world(n: usize, seed: u64, density: [u32; 4]) -> (World, Mirror) {
    let mut rng = DeterministicRng::new(0x009A_110B ^ seed);
    let mut world = World::new();
    let ids: Vec<EntityId> = (0..n).map(|_| world.spawn()).collect();
    for &id in &ids {
        if rng.gen_range_u32(0..4) == 0 {
            world.despawn(id).unwrap();
        }
    }
    let mut live: Vec<EntityId> = world.entities().collect();
    for i in (1..live.len()).rev() {
        let j = rng.gen_range_usize(0..i + 1);
        live.swap(i, j);
    }
    let mut mirror = Mirror::default();
    for &id in &live {
        let i = id.index();
        for (t, map) in mirror.iter_mut().enumerate() {
            if rng.gen_range_u32(0..1000) >= density[t] {
                continue;
            }
            match t {
                0 => drop(world.insert(id, GA(i)).unwrap()),
                1 => drop(world.insert(id, GB(i)).unwrap()),
                2 => drop(world.insert(id, GC(i)).unwrap()),
                _ => drop(world.insert(id, GD(i)).unwrap()),
            }
            map.insert(i, i);
        }
    }
    (world, mirror)
}

/// The naive join: the first column's keys ascending, kept when every other column has the key.
fn join_reference(mirror: &Mirror, cols: &[usize]) -> Rows {
    mirror[cols[0]]
        .keys()
        .filter(|k| cols[1..].iter().all(|&c| mirror[c].contains_key(k)))
        .map(|&k| (k, cols.iter().map(|&c| mirror[c][&k]).collect()))
        .collect()
}

/// Adds `delta` to column `written` of every reference row, as a parallel run's closure did.
fn apply_delta(mirror: &mut Mirror, rows: &Rows, written: usize, delta: u32) {
    for (k, _) in rows {
        *mirror[written].get_mut(k).unwrap() += delta;
    }
}

#[test]
fn par_join_matches_reference_under_randomized_columns() {
    // Per-type density in permille: absent, rare, sparse, half, everything.
    const DENSITIES: [u32; 5] = [0, 1, 50, 500, 1000];

    for seed in 0..32u64 {
        let mut rng = DeterministicRng::new(0x7A2 ^ seed);
        let n = match rng.gen_range_u32(0..4) {
            0 => 0,
            1 => rng.gen_range_usize(1..8),
            2 => rng.gen_range_usize(8..200),
            _ => rng.gen_range_usize(1000..4000),
        };
        let density: [u32; 4] =
            std::array::from_fn(|_| DENSITIES[rng.gen_range_usize(0..DENSITIES.len())]);
        let (mut world, mut mirror) = join_world(n, seed, density);
        let delta = seed as u32 + 1;

        for count in COUNTS {
            for chunk_rows in [1usize, 3, 64] {
                let label =
                    format!("seed {seed} n {n} {density:?} threads {count} chunk {chunk_rows}");

                let fold1 = par::with_threads(count, || {
                    world.par_join_fold::<(&GA,), Rows, Rows>(
                        chunk_rows,
                        |rows| rows.map(|(id, (a,))| (id.index(), vec![a.0])).collect(),
                        Vec::new(),
                        |mut all, part| {
                            all.extend(part);
                            all
                        },
                    )
                });
                assert_eq!(fold1, join_reference(&mirror, &[0]), "{label}: fold (&GA,)");

                let fold2 = par::with_threads(count, || {
                    world.par_join_fold::<(&GA, &GB), Rows, Rows>(
                        chunk_rows,
                        |rows| {
                            rows.map(|(id, (a, b))| (id.index(), vec![a.0, b.0]))
                                .collect()
                        },
                        Vec::new(),
                        |mut all, part| {
                            all.extend(part);
                            all
                        },
                    )
                });
                assert_eq!(
                    fold2,
                    join_reference(&mirror, &[0, 1]),
                    "{label}: fold (&GA, &GB)"
                );

                let rows2 = par::with_threads(count, || {
                    world.par_for_each::<(&GA, &mut GB), _>(chunk_rows, |_, id, (a, b)| {
                        let row = (id.index(), vec![a.0, b.0]);
                        b.0 += delta;
                        row
                    })
                });
                let expected = join_reference(&mirror, &[0, 1]);
                assert_eq!(rows2, expected, "{label}: par_for_each (&GA, &mut GB)");
                apply_delta(&mut mirror, &expected, 1, delta);

                let rows3 = par::with_threads(count, || {
                    world.par_for_each::<(&GA, &mut GB, &GC), _>(chunk_rows, |_, id, (a, b, c)| {
                        let row = (id.index(), vec![a.0, b.0, c.0]);
                        b.0 += delta;
                        row
                    })
                });
                let expected = join_reference(&mirror, &[0, 1, 2]);
                assert_eq!(rows3, expected, "{label}: par_for_each (&GA, &mut GB, &GC)");
                apply_delta(&mut mirror, &expected, 1, delta);

                let rows4 = par::with_threads(count, || {
                    world.par_for_each::<(&GA, &GB, &GC, &mut GD), _>(
                        chunk_rows,
                        |_, id, (a, b, c, d)| {
                            let row = (id.index(), vec![a.0, b.0, c.0, d.0]);
                            d.0 += delta;
                            row
                        },
                    )
                });
                let expected = join_reference(&mirror, &[0, 1, 2, 3]);
                assert_eq!(
                    rows4, expected,
                    "{label}: par_for_each (&GA, &GB, &GC, &mut GD)"
                );
                apply_delta(&mut mirror, &expected, 3, delta);
            }
        }

        // Every write landed on exactly the joined rows: the world's columns equal the mirror's.
        for id in world.entities().collect::<Vec<_>>() {
            let i = id.index();
            assert_eq!(
                world.get::<GB>(id).unwrap().map(|b| b.0),
                mirror[1].get(&i).copied()
            );
            assert_eq!(
                world.get::<GD>(id).unwrap().map(|d| d.0),
                mirror[3].get(&i).copied()
            );
        }
    }
}
