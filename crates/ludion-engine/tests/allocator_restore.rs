//! Snapshot→restore preserves the full generational allocator, so a slot recycled before the snapshot
//! never comes back re-issuable at generation 0 (audit finding 1.1: stale-handle aliasing after
//! restore). The allocator state rides in an additive, opt-in `allocator` snapshot field — present
//! only for a world with recycled/gap slots, so a clean snapshot is byte-identical and a snapshot that
//! predates the field still restores.

use ludion_core::ids::EntityId;
use ludion_engine::Engine;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

/// Spawns seven bare entities and despawns slot 5, leaving it recycled (generation 1) and free —
/// exactly the shape that re-mints a stale handle across a naive restore. Returns the stale handle
/// `EntityId(5, 0)` (the despawned entity's original id).
fn recycle_slot_five(engine: &mut Engine) -> EntityId {
    let world = engine.world_mut();
    let ids: Vec<EntityId> = (0..7).map(|_| world.spawn()).collect();
    let stale = ids[5];
    world.despawn(stale).unwrap();
    stale
}

#[test]
fn restore_preserves_the_allocator_so_a_recycled_slot_does_not_alias_a_stale_handle() {
    let mut engine = Engine::new();
    let stale = recycle_slot_five(&mut engine);
    assert_eq!(stale, EntityId::new(5, 0));

    let snapshot = snapshot_of(&mut engine);
    // A recycled-slot world cannot be reproduced from the live set alone, so the allocator field rides
    // along.
    assert!(
        snapshot.get("allocator").is_some(),
        "a snapshot of a world with a recycled slot carries the allocator state"
    );

    // Restore into a *fresh* engine (the `ludion dev` recompiled-binary path).
    let mut restored = Engine::new();
    restore(&mut restored, snapshot);

    // The stale handle is a reported error, not a silent hit.
    assert!(!restored.world().is_alive(stale));
    assert!(restored.world().get::<ludion_engine::Name>(stale).is_err());

    // The next spawn reuses slot 5 at its true generation (1), never re-minting the stale handle.
    let fresh = restored.world_mut().spawn();
    assert_eq!(fresh, EntityId::new(5, 1));
    assert_ne!(
        fresh, stale,
        "the post-restore spawn must not equal a pre-restore stale handle"
    );
    // And the stale handle still does not resolve to the freshly spawned live entity.
    assert!(!restored.world().is_alive(stale));
}

#[test]
fn post_restore_spawns_match_an_uninterrupted_run() {
    // Uninterrupted: recycle slot 5, then spawn five more, recording the ids.
    let mut uninterrupted = Engine::new();
    recycle_slot_five(&mut uninterrupted);
    let direct: Vec<EntityId> = (0..5).map(|_| uninterrupted.world_mut().spawn()).collect();

    // Restored: snapshot right after the recycle, restore into a fresh engine, spawn the same count.
    let mut original = Engine::new();
    recycle_slot_five(&mut original);
    let snapshot = snapshot_of(&mut original);
    let mut restored = Engine::new();
    restore(&mut restored, snapshot);
    let after_restore: Vec<EntityId> = (0..5).map(|_| restored.world_mut().spawn()).collect();

    assert_eq!(
        after_restore, direct,
        "the post-restore spawn sequence is identical to the uninterrupted run (id determinism)"
    );
}

#[test]
fn a_clean_snapshot_omits_the_allocator_field() {
    // A never-recycled world is fully reproducible from its live set, so no allocator field is written
    // — the snapshot stays byte-identical to before the field existed.
    let mut engine = Engine::new();
    for _ in 0..4 {
        engine.world_mut().spawn();
    }
    let snapshot = snapshot_of(&mut engine);
    assert!(
        snapshot.get("allocator").is_none(),
        "a clean snapshot carries no allocator field"
    );
}

#[test]
fn a_snapshot_predating_the_allocator_field_still_restores() {
    // Old-snapshot compatibility: strip the `allocator` field (a snapshot produced before it existed)
    // and confirm restore falls back to the live-set path and still rebuilds the live world.
    let mut engine = Engine::new();
    let stale = recycle_slot_five(&mut engine);
    let mut snapshot = snapshot_of(&mut engine);
    snapshot
        .as_object_mut()
        .unwrap()
        .remove("allocator")
        .expect("the recycled-slot snapshot had an allocator field to strip");

    let mut restored = Engine::new();
    restore(&mut restored, snapshot);

    // The live set is reconstructed (six entities: slots 0-4 and 6). The old-format fallback restores
    // gap slots at generation 0, so the stale handle does resolve again here — the known, documented
    // limitation of snapshots that predate the field. New snapshots (the test above) do not.
    assert_eq!(restored.world().entity_count(), 6);
    let _ = stale;
}

/// The diagnostic a refused restore raises, as `(code, path)`.
fn restore_err(engine: &mut Engine, snapshot: Value) -> (String, Option<String>) {
    let error = engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .expect_err("the restore should be refused");
    (error.code.as_str().to_string(), error.path)
}

/// A recycled-slot snapshot: slot 5 is free at generation 1, six entities are live.
fn recycled_snapshot() -> Value {
    let mut engine = Engine::new();
    recycle_slot_five(&mut engine);
    snapshot_of(&mut engine)
}

#[test]
fn a_duplicate_free_entry_is_refused() {
    let mut snapshot = recycled_snapshot();
    let free = snapshot["allocator"]["free"].as_array_mut().unwrap();
    free.push(free[0].clone());

    let (code, path) = restore_err(&mut Engine::new(), snapshot);
    assert_eq!(code, "L0025");
    assert!(path.unwrap().starts_with("/allocator/free/"));
}

#[test]
fn an_out_of_range_free_entry_is_refused() {
    let mut snapshot = recycled_snapshot();
    snapshot["allocator"]["free"][0] = Value::from(999);

    let (code, path) = restore_err(&mut Engine::new(), snapshot);
    assert_eq!(code, "L0025");
    assert!(path.unwrap().starts_with("/allocator/free/"));
}

#[test]
fn an_entity_list_that_disagrees_with_the_allocator_is_refused_and_changes_nothing() {
    let mut snapshot = recycled_snapshot();
    snapshot["entities"].as_array_mut().unwrap().remove(1);

    let mut target = Engine::new();
    let marker = target.world_mut().spawn();
    let before: Vec<EntityId> = target.world().entities().collect();
    let frame = target.frame();

    let (code, path) = restore_err(&mut target, snapshot);
    assert_eq!(code, "L3041");
    assert_eq!(path.as_deref(), Some("/entities"));
    assert!(target.world().is_alive(marker));
    assert_eq!(target.world().entities().collect::<Vec<_>>(), before);
    assert_eq!(target.frame(), frame);
}

#[test]
fn an_unsorted_but_correct_entity_list_still_restores() {
    let mut snapshot = recycled_snapshot();
    snapshot["entities"].as_array_mut().unwrap().reverse();

    let mut restored = Engine::new();
    restore(&mut restored, snapshot);
    assert_eq!(restored.world().entity_count(), 6);
}

#[test]
fn a_duplicate_entity_row_without_an_allocator_is_refused() {
    let mut snapshot = recycled_snapshot();
    snapshot.as_object_mut().unwrap().remove("allocator");
    let entities = snapshot["entities"].as_array_mut().unwrap();
    entities.push(entities[0].clone());

    let (code, path) = restore_err(&mut Engine::new(), snapshot);
    assert_eq!(code, "L0025");
    assert!(path.unwrap().starts_with("/entities/"));
}

#[test]
fn a_huge_entity_index_without_an_allocator_is_refused_and_changes_nothing() {
    let mut snapshot = recycled_snapshot();
    snapshot.as_object_mut().unwrap().remove("allocator");
    snapshot["entities"][0]["index"] = Value::from(4_000_000_000_u32);

    let mut target = Engine::new();
    let marker = target.world_mut().spawn();
    let frame = target.frame();

    let (code, path) = restore_err(&mut target, snapshot);
    assert_eq!(code, "L0025");
    assert!(path.unwrap().starts_with("/entities/"));
    assert!(target.world().is_alive(marker));
    assert_eq!(target.world().entity_count(), 1);
    assert_eq!(target.frame(), frame);
}

/// Spawns six, despawns slots 2 and 4, spawns two more: they reuse slots 4 then 2, so spawn order
/// differs from slot order.
fn churned_order(engine: &mut Engine) -> Vec<EntityId> {
    let world = engine.world_mut();
    let ids: Vec<EntityId> = (0..6).map(|_| world.spawn()).collect();
    world.despawn(ids[2]).unwrap();
    world.despawn(ids[4]).unwrap();
    world.spawn();
    world.spawn();
    world.entities().collect()
}

#[test]
fn a_churned_spawn_order_survives_a_restore() {
    let mut engine = Engine::new();
    let live = churned_order(&mut engine);
    let order = engine.world().spawn_order().expect("slots were reused");
    let snapshot = snapshot_of(&mut engine);
    assert_eq!(snapshot["spawn_order"], serde_json::json!(order));

    let mut restored = Engine::new();
    restore(&mut restored, snapshot);
    assert_eq!(restored.world().spawn_order(), Some(order));
    for pair in live.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        assert_eq!(
            engine.world().spawn_rank(a) < engine.world().spawn_rank(b),
            restored.world().spawn_rank(a) < restored.world().spawn_rank(b),
        );
    }
}

#[test]
fn a_clean_snapshot_omits_the_spawn_order() {
    let mut engine = Engine::new();
    for _ in 0..5 {
        engine.world_mut().spawn();
    }
    assert!(snapshot_of(&mut engine).get("spawn_order").is_none());
}

#[test]
fn a_spawn_order_that_is_not_a_permutation_is_refused() {
    let mut engine = Engine::new();
    churned_order(&mut engine);
    let mut snapshot = snapshot_of(&mut engine);
    snapshot["spawn_order"][0] = Value::from(999);

    let (code, path) = restore_err(&mut Engine::new(), snapshot);
    assert_eq!(code, "L0025");
    assert!(path.unwrap().starts_with("/spawn_order"));
}
