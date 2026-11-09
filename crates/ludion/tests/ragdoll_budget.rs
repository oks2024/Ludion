//! Behavioral proof for the copyable ragdoll cap (`crates/ludion/examples/ragdoll_budget.rs`): 300
//! enemies switch to ragdolls at 25 a second under a 120-group cap, settled and evicted groups retire
//! to drawn poses, and the pool — registered components — hashes, restores and runs identically at
//! every thread count. Every comparison is in-process: no golden, no committed trace.
//!
//! The test compiles the example itself, so it proves the code a game copies.

#[path = "../examples/ragdoll_budget.rs"]
#[allow(dead_code)]
mod example;

use example::{ADMISSIONS_PER_FRAME, GroupState, KILLS, RUN_TICKS, groups};
use ludion::prelude::*;
use ludion_core::par;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use std::collections::BTreeSet;
use std::path::PathBuf;

const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
const BONES: usize = 11;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn build(scene: &str) -> Engine {
    let mut engine = example::game()
        .project_root(gallery_root())
        .build()
        .expect("the engine builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

/// The scene with the budget replaced.
fn scene_with(max_simulated: u32, max_retired: u32) -> String {
    example::SCENE
        .replace(
            "max_simulated: 120",
            &format!("max_simulated: {max_simulated}"),
        )
        .replace("max_retired: 300", &format!("max_retired: {max_retired}"))
}

/// One `<tick:04> <hash:016x>` line per tick, from tick `from`.
fn trace_from(engine: &mut Engine, from: usize, to: usize) -> Vec<String> {
    (from..to)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn run_trace() -> Vec<String> {
    trace_from(&mut build(example::SCENE), 0, RUN_TICKS)
}

/// Skins with at least one part still carrying a `Collider`.
fn skins_in_the_solver(world: &mut World) -> usize {
    world
        .query_ids::<(&SkinBone, &Collider)>()
        .map(|(_, (bone, _))| bone.skin)
        .collect::<BTreeSet<_>>()
        .len()
}

/// A group as the tick left it: admission order, pin and state.
type Row = (u64, bool, GroupState);

fn table(engine: &mut Engine) -> Vec<Row> {
    groups(engine.world_mut())
        .into_iter()
        .map(|(_, group)| (group.admitted, group.pinned, group.state))
        .collect()
}

#[test]
fn simulated_groups_never_exceed_the_cap() {
    let mut engine = build(example::SCENE);
    let mut admitted = BTreeSet::new();
    for tick in 0..RUN_TICKS {
        engine.advance_live();
        let rows = table(&mut engine);
        let simulated = rows
            .iter()
            .filter(|row| row.2 == GroupState::Simulated)
            .count();
        assert!(
            simulated <= 120,
            "tick {tick}: {simulated} groups simulated"
        );
        let frame = tick as u64;
        assert!(
            rows.iter()
                .all(|row| row.2 != GroupState::Simulated
                    || frame - row.0 / ADMISSIONS_PER_FRAME <= 300),
            "tick {tick}: a group has been simulated past max_sim_ticks"
        );
        let in_solver = skins_in_the_solver(engine.world_mut());
        assert!(
            in_solver <= 120,
            "tick {tick}: {in_solver} skins hold colliders"
        );
        admitted.extend(rows.iter().map(|row| row.0));
    }
    assert_eq!(admitted.len() as u64, KILLS, "every enemy was admitted");

    let world = engine.world_mut();
    let mut parts = std::collections::BTreeMap::<EntityId, Vec<EntityId>>::new();
    for (id, (bone,)) in world.query_ids::<(&SkinBone,)>() {
        parts.entry(bone.skin).or_default().push(id);
    }
    let all = groups(world);
    assert!(all.iter().any(|(_, g)| g.state != GroupState::Simulated));
    for (skin, group) in &all {
        let parts = &parts[skin];
        assert_eq!(parts.len(), BONES, "a group keeps all its parts");
        if group.state == GroupState::Simulated {
            continue;
        }
        for &part in parts {
            assert!(world.has::<Transform>(part));
            assert!(!world.has::<Collider>(part) && !world.has::<Joint>(part));
            assert!(!world.has::<Velocity>(part) && !world.has::<JointMotor>(part));
        }
    }
    let missing = world
        .resource_mut::<PhysicsWorld>()
        .expect("physics")
        .take_missing_joints();
    assert!(missing.is_empty(), "no inert pin: {missing:?}");
}

#[test]
fn the_run_hashes_identically_twice() {
    assert_eq!(run_trace(), run_trace());
}

#[test]
fn a_mid_run_restore_reproduces_the_trace() {
    let at = 600;
    let expected = run_trace();
    let mut warm = build(example::SCENE);
    assert_eq!(trace_from(&mut warm, 0, at), expected[..at]);
    let snapshot = match warm.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = build(example::SCENE);
    let response = resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert!(
        matches!(response.body, ResponseBody::Restored { .. }),
        "expected a restore, got {:?}",
        response.body
    );
    assert_eq!(trace_from(&mut resumed, at, RUN_TICKS), expected[at..]);
}

#[test]
fn the_run_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| scope.spawn(move || par::with_threads(n, run_trace)))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

/// Every tick's group table under a tight budget.
fn tight_run() -> Vec<Vec<Row>> {
    let mut engine = build(&scene_with(8, 12));
    (0..RUN_TICKS)
        .map(|_| {
            engine.advance_live();
            table(&mut engine)
        })
        .collect()
}

#[test]
fn eviction_takes_the_oldest_unpinned_group_first() {
    let tables = tight_run();
    let mut evictions = 0;
    for pair in tables.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        let state_after = |admitted: u64| after.iter().find(|row| row.0 == admitted).map(|r| r.2);
        for &(admitted, pinned, state) in after {
            let was_simulated = before
                .iter()
                .any(|row| row.0 == admitted && row.2 == GroupState::Simulated);
            if state != GroupState::Evicted || !was_simulated {
                continue;
            }
            evictions += 1;
            let still_simulated = |row: &&Row| {
                row.2 == GroupState::Simulated && state_after(row.0) == Some(GroupState::Simulated)
            };
            if pinned {
                assert!(
                    !before
                        .iter()
                        .filter(|row| !row.1)
                        .any(|row| still_simulated(&row)),
                    "a pinned group {admitted} was evicted while an unpinned one stayed simulated"
                );
            } else {
                assert!(
                    !before
                        .iter()
                        .filter(|row| !row.1 && row.0 < admitted)
                        .any(|row| still_simulated(&row)),
                    "group {admitted} was evicted before an older unpinned one"
                );
            }
        }
    }
    assert!(evictions >= 50, "only {evictions} evictions");
}

#[test]
fn a_pinned_group_outlives_loose_ones() {
    let tables = tight_run();
    let admitted: BTreeSet<u64> = tables.iter().flatten().map(|row| row.0).collect();
    let (pinned, loose) = {
        let mut order = admitted.iter().copied();
        (order.next().unwrap(), order.next().unwrap())
    };
    let find = |table: &[Row], key: u64| table.iter().find(|row| row.0 == key).copied();
    let flat: Vec<Row> = tables.iter().flatten().copied().collect();
    assert!(
        find(&flat, pinned).is_some_and(|row| row.1),
        "the first kill is pinned"
    );
    assert!(
        find(&flat, loose).is_some_and(|row| !row.1),
        "the second is loose"
    );

    let mut checked = false;
    for pair in tables.windows(2) {
        let (before, after) = (&pair[0], &pair[1]);
        if find(before, loose).is_some() && find(after, loose).is_none() {
            checked = true;
            assert!(
                find(after, pinned).is_some(),
                "the pinned group outlives the loose one"
            );
        }
        if find(before, pinned).is_some_and(|row| row.2 == GroupState::Simulated)
            && find(after, pinned).is_some_and(|row| row.2 == GroupState::Evicted)
        {
            assert!(
                !before
                    .iter()
                    .any(|row| !row.1 && row.2 == GroupState::Simulated),
                "the pinned group was evicted while an unpinned one was simulated"
            );
        }
    }
    assert!(checked, "the loose group was never despawned");
}
