//! Snapshot / restore / seek across an in-run district switch (Phase 17 / E7, Wave 4).
//!
//! Wave 2 set `engine.scene = SceneRef::Inline { target }` on a switch, so a snapshot taken *after* a
//! switch already embeds the switched district inline, its entities already include the carried player,
//! and its `profile` already carries the in-memory profile — the Wave-4 thesis (decision 7) that
//! snapshot/restore/`dev`/seek across a switch are free of any snapshot-format change. These tests
//! confirm that end to end over the **public** engine API:
//!
//! 1. a post-switch snapshot restores the switched district off-disk (the district file is deleted
//!    first — restore must not read it), with the frame/RNG/profile continuing;
//! 2. a `ludion dev`-style snapshot → fresh-engine restore rebuilds district 2 with the carried player
//!    and the unsaved in-memory profile intact, deterministic vs the uninterrupted original;
//! 3. a replay **seek** lands on a keyframe after the switch (reconstructs district 2 directly) and a
//!    keyframe before it (the deltas cross the switch, which regenerates reading disk); both post-seek
//!    traces match the linear replay at the same frames;
//! 4. a profile-free, single-district snapshot carries no new field and round-trips byte-identically
//!    (`SNAPSHOT_FORMAT` unchanged);
//! 5. the restore posture — a plain restore rebuilds `DocStore`s from disk (the E4 posture), so a
//!    switch after a restore inherits the disk value, consistent with a plain restore.
//!
//! The map-installed seek reconstruction from a *pre-switch* keyframe on a *reverted* checkout — which
//! needs `install_replay_scene_text`, a `pub(crate)` seam — is proven in-crate in
//! `src/replay.rs::seek_transition_tests`.

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AudioQueue, DocStore, Engine, EngineBuilder, Profile, SceneSwitch, replay_hashes_with,
};
use ludion_protocol::command::{AssetReloadParams, EngineRestoreParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// --- Player harness (items 1-3) ---------------------------------------------------------------

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
    coins: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct RngProbe {
    last: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    unlocked: bool,
}

const START: &str = "\
format: 1
seed: 7
entities:
  - name: player
    components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
      Player:
        hp: 100
        coins: 5
      RngProbe:
        last: 0
";

const NEXT: &str = "\
format: 1
seed: 999
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 40.0
          - 0.0
          - -10.0
  - name: beacon
    components:
      Transform:
        translation:
          - 7.0
          - 0.0
          - 0.0
";

const ANCHOR_XYZ: [f32; 3] = [40.0, 0.0, -10.0];

/// Draws one `u32` from the world RNG each tick onto every [`RngProbe`], so the RNG stream is
/// hash-visible and continues into whatever district the run is in.
fn draw_rng(world: &mut World, _ctx: &SystemCtx) {
    let value = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    for (probe,) in world.query::<(&mut RngProbe,)>() {
        probe.last = value;
    }
}

/// Frame 0 only: set + save the profile in-memory (never persisted to disk in a test) — the unsaved
/// mid-run state that must survive a snapshot→restore.
fn frame0_side_effects(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        queue.play("ding.wav", 1.0);
    }
    if let Some(profile) = world.resource_mut::<Profile<Save>>() {
        profile.get_mut().unlocked = true;
        profile.save();
    }
}

/// Requests a switch to `next.yaml` on frame 2, carrying the single `Player` entity.
fn switch_to_next(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<Player>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player],
        );
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-transition-snap-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    dir
}

/// The base game with the switch system: the `player` harness every item-1..3 test drives across a
/// gate at frame 2.
fn switch_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<RngProbe>("RngProbe")?;
            Ok(())
        })
        .system("draw_rng", draw_rng)
        .system("frame0_side_effects", frame0_side_effects)
        .system("switch_to_next", switch_to_next)
        .profile::<Save>()
        .project_root(root)
}

fn player_of(engine: &Engine) -> EntityId {
    engine
        .world()
        .entities()
        .find(|&id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == "player")
        })
        .expect("the player entity is present")
}

fn names_of(engine: &Engine) -> Vec<String> {
    engine
        .world()
        .entities()
        .filter_map(|id| {
            engine
                .world()
                .get::<ludion_engine::Name>(id)
                .ok()
                .flatten()
                .map(|n| n.0.clone())
        })
        .collect()
}

fn serialized(engine: &Engine, id: EntityId, component: &str) -> Option<Value> {
    engine
        .registry()
        .serialize(component, engine.world(), id)
        .unwrap()
}

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

/// Advances `n` live ticks, returning the per-tick `WorldHash` trace.
fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    let mut trace = Vec::new();
    for _ in 0..n {
        engine.advance_live();
        trace.push(engine.world_hash().unwrap().as_u64());
    }
    trace
}

// --- Item 1: post-switch snapshot restores the switched district off-disk ---------------------

#[test]
fn a_post_switch_snapshot_restores_the_switched_district_off_disk() {
    let dir = fresh_dir("item1");
    let mut original = switch_game(&dir).build().expect("boots");
    original.load_inline_scene(START).unwrap();

    // Cross the gate (switch performed at frame 2), landing in district B at frame 5.
    for _ in 0..5 {
        original.advance_live();
    }
    assert!(original.last_transition_error().is_none());
    assert_eq!(original.frame().as_u64(), 5);

    let snapshot = snapshot_of(&mut original);

    // The snapshot is self-contained: `scene` is the Inline switched district (not a path), embedding
    // district B's text — so restore never needs the file on disk.
    assert_eq!(
        snapshot["scene"]["kind"].as_str(),
        Some("inline"),
        "a post-switch snapshot embeds the switched district inline, not as a path"
    );
    let scene_doc = snapshot["scene"]["document"]
        .as_str()
        .expect("the inline scene carries its document text");
    assert!(
        scene_doc.contains("beacon"),
        "the embedded scene is district B: {scene_doc}"
    );

    // The uninterrupted original continues; capture its next 5 frames as the reference.
    let original_continuation = advance_trace(&mut original, 5);

    // Delete the district file BEFORE restore — restore must reconstruct district B from the inline
    // snapshot alone, proving self-containment.
    std::fs::remove_file(dir.join("next.yaml")).unwrap();

    let mut restored = switch_game(&dir).build().expect("boots");
    restore(&mut restored, snapshot);

    // The restored world IS district B: its natives plus the carried player at the anchor.
    let names = names_of(&restored);
    assert!(names.contains(&"gate-from-start".to_string()));
    assert!(names.contains(&"beacon".to_string()));
    assert!(names.contains(&"player".to_string()));
    assert_eq!(restored.world().entity_count(), 3);
    assert_eq!(restored.frame().as_u64(), 5, "the frame counter continues");

    let player = player_of(&restored);
    let transform = serialized(&restored, player, "Transform").unwrap();
    assert_eq!(transform["translation"][0], ANCHOR_XYZ[0] as f64);

    // The frame/RNG/profile continue: post-restore ticks reproduce the original's continuation exactly.
    let restored_continuation = advance_trace(&mut restored, 5);
    assert_eq!(
        restored_continuation, original_continuation,
        "the restored district-B run continues bit-identically to the uninterrupted original"
    );
}

// --- Item 2: ludion dev-style snapshot -> fresh-engine restore in district 2 -------------------

#[test]
fn a_dev_style_restart_rebuilds_district_two_with_the_in_memory_profile() {
    let dir = fresh_dir("item2");
    let mut original = switch_game(&dir).build().expect("boots");
    original.load_inline_scene(START).unwrap();
    for _ in 0..5 {
        original.advance_live();
    }
    // The mid-run unlock was set in-memory (frame 0) and never written to disk (no `save/profile.yaml`).
    assert!(
        original
            .world()
            .resource::<Profile<Save>>()
            .unwrap()
            .get()
            .unlocked
    );
    let snapshot = snapshot_of(&mut original);
    let original_continuation = advance_trace(&mut original, 6);

    // The `ludion dev` restart: a brand-new engine from the *same builder config* restores the snapshot
    // (the recompiled-binary path). No `save/profile.yaml` exists on disk, so a disk read would default
    // `unlocked` to false — the snapshot's embedded profile is the only source of the true value.
    let mut restarted = switch_game(&dir).build().expect("boots");
    restore(&mut restarted, snapshot);

    assert_eq!(
        restarted.world().entity_count(),
        3,
        "district 2 with the player"
    );
    assert!(names_of(&restarted).contains(&"beacon".to_string()));
    assert!(
        restarted
            .world()
            .resource::<Profile<Save>>()
            .unwrap()
            .get()
            .unlocked,
        "the unsaved in-memory unlock survives the snapshot->restart->restore"
    );

    // Subsequent ticks are deterministic vs the uninterrupted original.
    let restarted_continuation = advance_trace(&mut restarted, 6);
    assert_eq!(
        restarted_continuation, original_continuation,
        "the restarted district-2 run is deterministic against the uninterrupted original"
    );
}

// --- Item 3: replay seek across a switch ------------------------------------------------------

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Drives a seek's delta records over the public API, returning the reconstructed `(frame, hash)`
/// trace. `SceneSwitch`/`Keyframe` records are not drive actions — the switch regenerates from the
/// re-driven sim (the `replay_hashes_with` posture); the recording carries only `SimStep` commands.
fn drive_deltas(engine: &mut Engine, deltas: Vec<ReplayRecord>) -> Vec<(u64, u64)> {
    let mut trace = vec![(
        engine.frame().as_u64(),
        engine.world_hash().unwrap().as_u64(),
    )];
    for record in deltas {
        if let ReplayRecord::Command { command, .. } = record {
            engine.execute(command).unwrap();
            trace.push((
                engine.frame().as_u64(),
                engine.world_hash().unwrap().as_u64(),
            ));
        }
    }
    trace
}

#[test]
fn a_replay_seek_reconstructs_the_district_on_both_sides_of_a_switch() {
    let dir = fresh_dir("item3");
    let sink = SharedSink::default();
    let mut engine = switch_game(&dir).build().expect("boots");
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    // Drive past the default keyframe cadence (300) so a keyframe lands AFTER the frame-2 switch.
    for _ in 0..305 {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    // The reference: the linear replay's per-frame hash (frame -> hash).
    let linear: BTreeMap<u64, u64> = replay_hashes_with(switch_game(&dir), &bytes, &dir)
        .unwrap()
        .into_iter()
        .map(|(f, h)| (f.as_u64(), h.as_u64()))
        .collect();

    let reader = ReplayReader::new(&bytes).unwrap();

    // (a) A keyframe AFTER the switch: seek lands on the frame-300 keyframe, which already embeds
    //     district 2 — restore reconstructs it directly, no delta-replay of the switch needed.
    let after = reader
        .seek(ludion_core::time::FrameIndex::new(302))
        .unwrap();
    let (kf_frame, kf_snapshot) = after.keyframe.clone().expect("a keyframe at or before 302");
    assert!(
        kf_frame.as_u64() > 2,
        "the chosen keyframe (frame {}) is after the switch",
        kf_frame.as_u64()
    );
    let mut recon_a = switch_game(&dir).build().expect("boots");
    restore(&mut recon_a, kf_snapshot);
    assert!(
        names_of(&recon_a).contains(&"beacon".to_string()),
        "restoring the post-switch keyframe reconstructs district 2 directly"
    );
    assert_eq!(
        recon_a.world_hash().unwrap().as_u64(),
        linear[&kf_frame.as_u64()],
        "the reconstructed keyframe matches the linear replay at that frame"
    );
    for (frame, hash) in drive_deltas(&mut recon_a, after.deltas) {
        assert_eq!(
            hash, linear[&frame],
            "post-seek frame {frame} matches linear"
        );
    }

    // (b) A keyframe BEFORE the switch: seek(6) lands on the boot keyframe (frame 0); the deltas cross
    //     the switch, which regenerates reading disk (present, same-machine). The reconstruction ends in
    //     district 2 and every frame it produces matches the linear replay.
    let before = reader.seek(ludion_core::time::FrameIndex::new(6)).unwrap();
    let (boot_frame, boot_snapshot) = before.keyframe.clone().expect("a boot keyframe");
    assert_eq!(
        boot_frame.as_u64(),
        0,
        "the boot keyframe precedes the switch"
    );
    let mut recon_b = switch_game(&dir).build().expect("boots");
    restore(&mut recon_b, boot_snapshot);
    assert_eq!(
        recon_b.world().entity_count(),
        1,
        "restored into district A"
    );
    let trace_b = drive_deltas(&mut recon_b, before.deltas);
    assert!(
        names_of(&recon_b).contains(&"beacon".to_string()),
        "the re-driven deltas regenerated the switch — the reconstruction ends in district 2"
    );
    assert!(
        trace_b.last().unwrap().0 > 2,
        "the reconstruction ran past the switch frame"
    );
    for (frame, hash) in trace_b {
        assert_eq!(
            hash, linear[&frame],
            "post-seek frame {frame} matches linear"
        );
    }
}

// --- Item 4: a profile-free, single-district snapshot round-trips byte-identically ------------

#[test]
fn a_single_district_snapshot_carries_no_new_field_and_round_trips_byte_identically() {
    let mut engine = Engine::new();
    engine
        .load_inline_scene(
            "\
format: 1
seed: 3
entities:
  - components:
      Transform:
        translation:
          - 1.0
          - 0.0
          - 0.0
",
        )
        .unwrap();
    engine.advance_live();
    engine.advance_live();

    let snapshot = snapshot_of(&mut engine);

    // `SNAPSHOT_FORMAT` is unchanged and E7 added no snapshot field: the key set is exactly the pre-E7
    // set. A profile-free, unnamed, single-district engine carries no `profile`, no `scene_name`, and
    // certainly no switch/transition field.
    assert_eq!(
        snapshot["format"].as_u64(),
        Some(1),
        "SNAPSHOT_FORMAT stays 1"
    );
    let keys: Vec<&str> = snapshot
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        vec![
            "format",
            "engine_version",
            "frame",
            "run_state",
            "speed",
            "seed",
            "scene",
            "rng",
            "entities",
        ],
        "no field was perturbed or added by E7 (no `profile`, `scene_name`, or transition key)"
    );
    // The additive `doc_reloads` field (finding 2.1) obeys the same presence rule as `allocator` and
    // `profile`: this engine registers no doc kind and performs no reload, so an empty log is skipped
    // and the field is absent — the snapshot is byte-identical to before the field existed.
    assert!(
        !keys.contains(&"doc_reloads"),
        "a reload-free snapshot omits `doc_reloads` (additive, skip-if-empty)"
    );

    // A round-trip is byte-identical: restoring and re-capturing yields the exact same document.
    let mut restored = Engine::new();
    restore(&mut restored, snapshot.clone());
    let recaptured = snapshot_of(&mut restored);
    assert_eq!(
        serde_json::to_string(&snapshot).unwrap(),
        serde_json::to_string(&recaptured).unwrap(),
        "a single-district snapshot round-trips byte-identically"
    );
}

// --- Item 5: a snapshot carries the doc-reload log; a restore re-applies it off-disk (finding 2.1) --

/// The fixed posture. A snapshot carries the session's data-doc reload log (`SnapshotDoc.doc_reloads`,
/// additive), and `install_restored` **adopts and re-applies** it off-disk over the freshly-walked
/// `DocStore`s — the way a scene switch does — instead of the old clear-then-rebuild-from-disk. So a
/// restore onto a checkout whose on-disk doc has since reverted still holds the *reloaded* value (not
/// the disk value), and a scene switch performed after a restore inherits it too. This is what a
/// replay-seek keyframe landing after a live reload needs, and what keeps a post-restore switch from
/// rebuilding from possibly-changed disk (the cross-machine divergence the switch path was built to
/// prevent). This test is the regression guard against reverting to the disk-authoritative restore.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct DamageReadout {
    value: f32,
}

const WEAPON_KEY: &str = "weapons/crossbow.yaml";
const WEAPON_V1: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
const WEAPON_V2: &str = "format: 1\nkind: weapon\ndamage: 99.0\n";

const DOC_START: &str = "\
format: 1
seed: 1
entities:
  - name: player
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      DamageReadout:
        value: 0.0
";

const DOC_NEXT: &str = "\
format: 1
seed: 2
entities:
  - name: gate-from-start
    components:
      Transform:
        translation:
          - 5.0
          - 0.0
          - 0.0
";

fn apply_weapon(world: &mut World, _ctx: &SystemCtx) {
    let damage = world
        .resource::<DocStore<Weapon>>()
        .and_then(|store| store.get(WEAPON_KEY))
        .map(|weapon| weapon.damage);
    if let Some(damage) = damage {
        for (readout,) in world.query::<(&mut DamageReadout,)>() {
            readout.value = damage;
        }
    }
}

/// Requests a switch on frame 5 (after the restore point), carrying the `DamageReadout` entity.
fn switch_at_5(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 5 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<DamageReadout>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player],
        );
    }
}

fn write_weapon(root: &Path, contents: &str) {
    let path = root.join(WEAPON_KEY);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn doc_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<DamageReadout>("DamageReadout"))
        .system("apply_weapon", apply_weapon)
        .system("switch_at_5", switch_at_5)
        .doc_kind::<Weapon>("weapon")
        .project_root(root)
}

fn store_damage(engine: &Engine) -> f32 {
    engine
        .world()
        .resource::<DocStore<Weapon>>()
        .unwrap()
        .get(WEAPON_KEY)
        .unwrap()
        .damage
}

fn readout_of(engine: &mut Engine) -> f32 {
    engine
        .world_mut()
        .query::<(&DamageReadout,)>()
        .next()
        .map(|(r,)| r.value)
        .unwrap()
}

#[test]
fn a_restore_re_applies_a_reloaded_doc_off_disk_and_a_later_switch_stays_consistent() {
    let dir = fresh_dir("item5");
    std::fs::write(dir.join("next.yaml"), DOC_NEXT).unwrap();
    write_weapon(&dir, WEAPON_V1);

    let mut engine = doc_game(&dir).build().expect("boots");
    engine.load_inline_scene(DOC_START).unwrap();

    engine.advance_live(); // frame 1: readout <- store v1 (12)
    assert_eq!(readout_of(&mut engine), 12.0);

    // Live-reload the doc to v2 (99) BEFORE the snapshot (the off-disk reload path); the store now
    // holds 99 and the snapshot logs the reload.
    write_weapon(&dir, WEAPON_V2);
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(WEAPON_KEY.to_string()),
        }))
        .unwrap();
    engine.advance_live(); // frame 2: readout <- store v2 (99)
    assert_eq!(readout_of(&mut engine), 99.0);
    assert_eq!(store_damage(&engine), 99.0);

    let snapshot = snapshot_of(&mut engine);
    // The snapshot carries the reload log additively — one entry, the canonical v2 text.
    let reloads = snapshot["doc_reloads"]
        .as_array()
        .expect("a live reload is carried in `doc_reloads`");
    assert_eq!(reloads.len(), 1, "exactly the one live reload");
    assert_eq!(reloads[0]["kind"].as_str(), Some("weapon"));
    assert_eq!(reloads[0]["key"].as_str(), Some(WEAPON_KEY));
    assert!(
        reloads[0]["document"].as_str().unwrap().contains("99"),
        "the inline document is the reloaded v2 text"
    );

    // A reverted checkout: modify the doc on disk back to v1 (12). The reloaded value now lives ONLY in
    // the snapshot's reload log — never on disk.
    write_weapon(&dir, WEAPON_V1);

    let mut restored = doc_game(&dir).build().expect("boots");
    restore(&mut restored, snapshot);

    // The snapshot's derived component holds the reloaded value...
    assert_eq!(
        readout_of(&mut restored),
        99.0,
        "the snapshot carried the post-reload component value"
    );
    // ...and so does the doc STORE: the restore re-applied the logged reload off-disk over the
    // freshly-walked (disk = v1) store, so it holds v2 (99), NOT the reverted disk value (finding 2.1).
    assert_eq!(
        store_damage(&restored),
        99.0,
        "the restore re-applies the logged reload off-disk, not the reverted disk doc"
    );

    // The next tick re-derives the readout from the (re-applied) store: the reload survives the restore.
    restored.advance_live(); // frame 3
    assert_eq!(
        readout_of(&mut restored),
        99.0,
        "post-restore the readout follows the re-applied reload, not the reverted disk doc"
    );

    // Now cross a gate (switch at frame 5). The switch re-walks disk (12) then re-applies the carried
    // reload log, so district 2's store is the reloaded value (99) too — the post-restore switch is
    // consistent with the restore, both off-disk.
    restored.advance_live(); // frame 4
    restored.advance_live(); // frame 5 -> switch performed
    restored.advance_live(); // frame 6 (district 2)
    assert!(restored.last_transition_error().is_none());
    assert!(names_of(&restored).contains(&"gate-from-start".to_string()));
    assert_eq!(
        store_damage(&restored),
        99.0,
        "the switch after a restore re-applies the carried reload log, staying off-disk"
    );
    assert_eq!(readout_of(&mut restored), 99.0);
}

/// An older snapshot — one produced before `doc_reloads` existed — has no such key. `#[serde(default)]`
/// deserializes it as an empty log, so the restore adopts nothing and rebuilds the `DocStore`s purely
/// from disk: the pre-fix clean-disk behavior, unchanged. Simulated by stripping the field.
#[test]
fn an_old_snapshot_without_doc_reloads_falls_back_to_the_disk_rebuild() {
    let dir = fresh_dir("item5-old");
    write_weapon(&dir, WEAPON_V1);

    let mut engine = doc_game(&dir).build().expect("boots");
    engine.load_inline_scene(DOC_START).unwrap();
    engine.advance_live();

    // Reload to v2 (99), then take a snapshot that carries the reload...
    write_weapon(&dir, WEAPON_V2);
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(WEAPON_KEY.to_string()),
        }))
        .unwrap();
    engine.advance_live();
    let mut snapshot = snapshot_of(&mut engine);

    // ...then strip the field, so the document looks like one from before the field existed.
    assert!(
        snapshot
            .as_object_mut()
            .unwrap()
            .remove("doc_reloads")
            .is_some(),
        "the live-reload snapshot carried the field to strip"
    );

    // On a disk still at v2 (99), the fallback rebuild reads disk → 99. (Using the current disk proves
    // the value comes from disk, not a carried log.)
    let mut restored = doc_game(&dir).build().expect("boots");
    restore(&mut restored, snapshot);
    assert_eq!(
        store_damage(&restored),
        99.0,
        "an old snapshot (no `doc_reloads`) rebuilds the store from disk — the pre-fix fallback"
    );
}
