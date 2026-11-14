//! The opt-in write journal: exactly the writes to watched entities, the drain contract (alive or
//! dead, never a reset signal), ancestor expansion, restores, the presentation range, and
//! `par_for_each` agreeing with the sequential query at every thread count.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_ecs::{Parent, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Marker;
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Other(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct A(u32);
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct B(u32);

/// A world watching `Marker`, with `n` entities carrying `Marker` and `A`, drained once.
fn watched_world(n: usize) -> (World, Vec<EntityId>) {
    let mut world = World::new();
    world.watch_with::<Marker>();
    let ids: Vec<_> = (0..n)
        .map(|i| {
            let e = world.spawn();
            world.insert(e, Marker).unwrap();
            world.insert(e, A(i as u32)).unwrap();
            e
        })
        .collect();
    let _ = world.drain_watched::<Marker>();
    (world, ids)
}

#[test]
fn read_paths_journal_nothing() {
    let (mut world, ids) = watched_world(4);
    let _ = world.get::<A>(ids[0]).unwrap();
    let _ = world.component::<A>(ids[1]);
    assert_eq!(world.query::<(&A,)>().count(), 4);
    let sum = world.par_join_fold::<(&A,), u32, u32>(
        2,
        |rows| rows.map(|(_, (a,))| a.0).sum(),
        0,
        |t, p| t + p,
    );
    assert_eq!(sum, 6);
    assert!(world.drain_watched::<Marker>().is_empty());
}

#[test]
fn absent_component_and_stale_handles_journal_nothing() {
    let (mut world, ids) = watched_world(2);
    assert!(world.get_mut::<Other>(ids[0]).unwrap().is_none());
    assert!(world.remove::<Other>(ids[0]).unwrap().is_none());
    world.despawn(ids[1]).unwrap();
    let _ = world.drain_watched::<Marker>();
    assert!(world.get_mut::<A>(ids[1]).is_err());
    assert!(world.insert(ids[1], A(0)).is_err());
    assert!(world.remove::<A>(ids[1]).is_err());
    assert!(world.drain_watched::<Marker>().is_empty());
}

#[test]
fn unwatched_entities_and_second_drains_are_empty() {
    let (mut world, ids) = watched_world(1);
    let plain = world.spawn();
    world.insert(plain, A(0)).unwrap();
    world.get_mut::<A>(plain).unwrap().unwrap().0 = 5;
    world.component_mut::<A>(ids[0]).unwrap().0 = 7;
    assert_eq!(world.drain_watched::<Marker>(), vec![ids[0]]);
    assert!(world.drain_watched::<Marker>().is_empty());
    assert!(
        world.drain_watched::<Other>().is_empty(),
        "an unwatched marker drains empty"
    );
}

#[test]
fn queries_journal_only_what_they_yield() {
    let (mut world, ids) = watched_world(3);
    let other = world.spawn();
    world.insert(other, Other(1)).unwrap();
    // The join excludes every watched entity (none has `Other`).
    for (o,) in world.query::<(&mut Other,)>() {
        o.0 += 1;
    }
    assert!(world.drain_watched::<Marker>().is_empty());

    // A partially consumed query journals only the first row.
    let mut query = world.query::<(&mut A,)>();
    let (a,) = query.next().unwrap();
    a.0 += 1;
    assert_eq!(world.drain_watched::<Marker>(), vec![ids[0]]);

    // `query_ids` journals like `query`.
    for (_, (a,)) in world.query_ids::<(&mut A,)>() {
        a.0 += 1;
    }
    assert_eq!(world.drain_watched::<Marker>(), ids);
    let mut query = world.query_ids::<(&mut A,)>();
    let (id, _) = query.next().unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![id]);
}

#[test]
fn marker_membership_is_journaled() {
    let mut world = World::new();
    world.watch_with::<Marker>();
    let e = world.spawn();
    world.insert(e, A(0)).unwrap();
    assert!(world.drain_watched::<Marker>().is_empty());

    world.insert(e, Marker).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![e]);

    world.remove::<Marker>(e).unwrap();
    assert_eq!(
        world.drain_watched::<Marker>(),
        vec![e],
        "removal reports the entity, alive without the marker"
    );
    world.get_mut::<A>(e).unwrap().unwrap().0 = 1;
    assert!(world.drain_watched::<Marker>().is_empty());
}

#[test]
fn watch_with_seeds_existing_carriers_and_is_idempotent() {
    let mut world = World::new();
    let ids: Vec<_> = (0..3).map(|_| world.spawn()).collect();
    world.insert(ids[0], Marker).unwrap();
    world.insert(ids[2], Marker).unwrap();
    world.watch_with::<Marker>();
    world.watch_with::<Marker>();
    assert_eq!(world.drain_watched::<Marker>(), vec![ids[0], ids[2]]);
    world.watch_with::<Marker>();
    assert!(world.drain_watched::<Marker>().is_empty());
}

#[test]
fn two_markers_are_independent() {
    let mut world = World::new();
    world.watch_with::<Marker>();
    world.watch_with::<Other>();
    let (a, b) = (world.spawn(), world.spawn());
    world.insert(a, Marker).unwrap();
    world.insert(b, Other(0)).unwrap();
    world.insert(a, A(0)).unwrap();
    world.insert(b, A(0)).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![a]);
    assert_eq!(world.drain_watched::<Other>(), vec![b]);
    world.insert(b, Marker).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![b]);
    assert_eq!(world.drain_watched::<Other>(), vec![b]);
}

/// `n` watched entities carrying `A` and `B` (every third lacks `B`), drained once.
fn par_fixture(n: usize) -> (World, Vec<EntityId>) {
    let (mut world, ids) = watched_world(n);
    for (i, &e) in ids.iter().enumerate() {
        if i % 3 != 0 {
            world.insert(e, B(i as u32)).unwrap();
        }
    }
    // An unwatched bystander with the same columns.
    let plain = world.spawn();
    world.insert(plain, A(0)).unwrap();
    world.insert(plain, B(0)).unwrap();
    let _ = world.drain_watched::<Marker>();
    (world, ids)
}

#[test]
fn par_for_each_journals_like_the_sequential_query() {
    let (mut oracle, _) = par_fixture(200);
    for (a, b) in oracle.query::<(&mut A, &B)>() {
        a.0 += b.0;
    }
    let expected = oracle.drain_watched::<Marker>();
    assert!(!expected.is_empty());

    for count in [1, 2, 8] {
        for chunk in [1, 7, 64] {
            let (mut world, _) = par_fixture(200);
            par::with_threads(count, || {
                world.par_for_each::<(&mut A, &B), ()>(chunk, |_, _, (a, b)| a.0 += b.0);
            });
            assert_eq!(
                world.drain_watched::<Marker>(),
                expected,
                "{count} threads, chunk {chunk}"
            );
        }
    }
}

#[test]
fn par_for_each_read_only_journals_nothing_and_a_panic_journals_the_join() {
    let (mut world, ids) = par_fixture(40);
    world.par_for_each::<(&A, &B), ()>(4, |_, _, _| {});
    assert!(world.drain_watched::<Marker>().is_empty());

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        world.par_for_each::<(&mut A, &B), ()>(4, |_, id, _| {
            assert!(id.index != 20, "boom");
        });
    }));
    assert!(outcome.is_err());
    let drained = world.drain_watched::<Marker>();
    let joined: Vec<_> = (0..ids.len())
        .filter(|i| i % 3 != 0)
        .map(|i| ids[i])
        .collect();
    assert_eq!(drained, joined);
}

#[test]
fn despawn_reports_the_dead_id_and_slot_reuse_reports_both() {
    let (mut world, ids) = watched_world(2);
    world.despawn(ids[0]).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![ids[0]]);

    world.despawn(ids[1]).unwrap();
    let reborn = world.spawn();
    assert_eq!(reborn.index, ids[1].index, "LIFO reuse");
    assert_ne!(reborn, ids[1]);
    world.insert(reborn, Marker).unwrap();
    let mut expected = vec![ids[1], reborn];
    expected.sort();
    assert_eq!(world.drain_watched::<Marker>(), expected);
}

#[test]
fn cascade_and_despawn_many_report_watched_descendants() {
    let mut world = World::new();
    world.watch_with::<Marker>();
    let root = world.spawn();
    let kids: Vec<_> = (0..2).map(|_| world.spawn()).collect();
    for &k in &kids {
        world.insert(k, Parent(root)).unwrap();
        world.insert(k, Marker).unwrap();
    }
    let _ = world.drain_watched::<Marker>();
    world.despawn(root).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), kids);

    let root = world.spawn();
    let kid = world.spawn();
    let lone = world.spawn();
    world.insert(kid, Parent(root)).unwrap();
    world.insert(kid, Marker).unwrap();
    world.insert(lone, Marker).unwrap();
    let _ = world.drain_watched::<Marker>();
    assert!(world.despawn_many(&[root, lone]).is_empty());
    let mut expected = vec![kid, lone];
    expected.sort();
    assert_eq!(world.drain_watched::<Marker>(), expected);
}

#[test]
fn unwatched_then_despawned_entity_reports_once() {
    let (mut world, ids) = watched_world(1);
    world.get_mut::<A>(ids[0]).unwrap().unwrap().0 = 2;
    world.remove::<Marker>(ids[0]).unwrap();
    world.despawn(ids[0]).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![ids[0]]);
}

#[test]
fn a_watched_ancestor_expands_to_its_watched_descendants() {
    let mut world = World::new();
    world.watch_with::<Marker>();
    let root = world.spawn();
    let child = world.spawn();
    let plain_child = world.spawn();
    let grandchild = world.spawn();
    world.insert(root, Marker).unwrap();
    world.insert(root, A(0)).unwrap();
    world.insert(child, Parent(root)).unwrap();
    world.insert(plain_child, Parent(root)).unwrap();
    world.insert(grandchild, Parent(child)).unwrap();
    for e in [child, grandchild] {
        world.insert(e, Marker).unwrap();
        world.insert(e, A(0)).unwrap();
    }
    let _ = world.drain_watched::<Marker>();

    world.get_mut::<A>(root).unwrap().unwrap().0 = 1;
    assert_eq!(
        world.drain_watched::<Marker>(),
        vec![root, child, grandchild]
    );

    world.get_mut::<A>(child).unwrap().unwrap().0 = 1;
    assert_eq!(world.drain_watched::<Marker>(), vec![child, grandchild]);
}

#[test]
fn restore_reports_old_ids_and_keeps_the_registration() {
    let (mut world, ids) = watched_world(3);
    world.despawn(ids[1]).unwrap();
    let _ = world.drain_watched::<Marker>();
    world.get_mut::<A>(ids[2]).unwrap().unwrap().0 = 9;

    let live = [ids[0], ids[2]];
    world.restore_entities(&live).unwrap();
    let drained = world.drain_watched::<Marker>();
    assert_eq!(
        drained,
        live.to_vec(),
        "dirty and watched slots are forgotten"
    );
    assert!(world.drain_watched::<Marker>().is_empty());

    for &e in &live {
        world.insert(e, Marker).unwrap();
    }
    assert_eq!(world.drain_watched::<Marker>(), live.to_vec());

    let state = world.allocator_state();
    if let Some(state) = state {
        world.restore_allocator(&state).unwrap();
        assert_eq!(world.drain_watched::<Marker>(), live.to_vec());
    }
}

#[test]
fn presentation_entities_are_journaled() {
    let mut world = World::new();
    world.watch_with::<Marker>();
    let sim = world.spawn();
    let pres = world.spawn_presentation();
    world.insert(sim, Marker).unwrap();
    world.insert(pres, Marker).unwrap();
    world.insert(pres, A(0)).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![sim, pres]);
    world.get_mut::<A>(pres).unwrap().unwrap().0 = 1;
    assert_eq!(world.drain_watched::<Marker>(), vec![pres]);
    world.despawn(pres).unwrap();
    assert_eq!(world.drain_watched::<Marker>(), vec![pres]);
}

/// The contract as a naive model, driven by random ops against the real journal.
#[test]
fn randomized_ops_match_the_naive_model() {
    #[derive(Clone, Default)]
    struct Model {
        marker: bool,
        other: bool,
        a: bool,
    }

    let mut rng = DeterministicRng::new(0x30_39);
    let mut world = World::new();
    world.watch_with::<Marker>();
    let mut live: Vec<(EntityId, Model)> = Vec::new();
    let mut expected: BTreeSet<EntityId> = BTreeSet::new();

    for step in 0..2000 {
        let pick = if live.is_empty() {
            0
        } else {
            rng.gen_range_usize(0..live.len())
        };
        match rng.gen_range_u32(0..9) {
            0 | 1 => {
                let e = world.spawn();
                live.push((e, Model::default()));
            }
            2 if !live.is_empty() => {
                let (e, m) = &mut live[pick];
                world.insert(*e, Marker).unwrap();
                m.marker = true;
                expected.insert(*e);
            }
            3 if !live.is_empty() => {
                let (e, m) = &mut live[pick];
                let had = world.remove::<Marker>(*e).unwrap().is_some();
                assert_eq!(had, m.marker);
                if had {
                    expected.insert(*e);
                }
                m.marker = false;
            }
            4 if !live.is_empty() => {
                let (e, m) = &mut live[pick];
                world.insert(*e, Other(0)).unwrap();
                m.other = true;
                if m.marker {
                    expected.insert(*e);
                }
            }
            5 if !live.is_empty() => {
                let (e, m) = &mut live[pick];
                let some = world.get_mut::<Other>(*e).unwrap().is_some();
                assert_eq!(some, m.other);
                if some && m.marker {
                    expected.insert(*e);
                }
            }
            6 if !live.is_empty() => {
                let (e, m) = &mut live[pick];
                world.insert(*e, A(0)).unwrap();
                m.a = true;
                if m.marker {
                    expected.insert(*e);
                }
            }
            7 => {
                for (e, _) in world.query_ids::<(&mut Other, &A)>() {
                    let m = &live.iter().find(|(id, _)| *id == e).unwrap().1;
                    assert!(m.other && m.a);
                }
                for (e, m) in &live {
                    if m.other && m.a && m.marker {
                        expected.insert(*e);
                    }
                }
            }
            8 if !live.is_empty() => {
                let (e, m) = live.swap_remove(pick);
                world.despawn(e).unwrap();
                if m.marker || expected.contains(&e) {
                    expected.insert(e);
                }
            }
            _ => {}
        }
        if step % 37 == 0 {
            let got: BTreeSet<_> = world.drain_watched::<Marker>().into_iter().collect();
            assert_eq!(got, expected, "step {step}");
            expected.clear();
        }
    }
}
