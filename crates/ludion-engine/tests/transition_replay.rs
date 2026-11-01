//! Cross-transition replay tests (Phase 17 / E7, Wave 3): a recorded session that crosses a district
//! gate reproduces its `WorldHash` trace from the `.lrp` — off-disk, on a checkout whose target
//! district (and any doc it reloaded) has since changed or vanished.
//!
//! The switch itself is never a drive action: it **regenerates** from the sim's re-set request under
//! replay (same seed, inputs, and profile → the switch-triggering system re-fires at the identical
//! frame). The additive `SceneSwitch` record supplies only the target district's canonical text, which
//! the replay pre-scans into an off-disk source so the regenerated drain resolves it without touching
//! disk (decision 6). The doc-reload×switch interaction (item 4) rides the same off-disk posture: a
//! doc reloaded before a switch is re-applied on the rebuilt district from the session's reload log,
//! which a replay fills from the `DocReload` records — so `install_docs`'s disk re-walk cannot revert
//! it.

use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{DocStore, Engine, EngineBuilder, replay_hashes_with};
use ludion_protocol::Command;
use ludion_protocol::command::{AssetReloadParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// --- Components -------------------------------------------------------------------------------

/// The carried player's game state — a hash-visible surface that travels across the gate.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Player {
    hp: u32,
}

/// A per-tick RNG probe so the RNG stream reaches the `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct RngProbe {
    last: u32,
}

// --- Scenes -----------------------------------------------------------------------------------

/// District A (the start): seed 7, one named player at a distinctive translation.
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
      RngProbe:
        last: 0
";

/// District B (the target): a `gate-from-start` anchor and one distinctive native prop — its presence
/// is what makes a switch hash-visible against a run that stays in A.
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

/// A **different** valid district B written to disk after recording (a reverted checkout): the beacon
/// sits elsewhere, so a replay that (wrongly) read disk would produce a different, valid world — the
/// mismatch the embedded-text-wins guarantee prevents.
const NEXT_REVERTED: &str = "\
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
          - 999.0
          - 0.0
          - 0.0
";

// --- Systems ----------------------------------------------------------------------------------

/// Draws one `u32` from the world RNG each tick into every [`RngProbe`], making the RNG stream
/// hash-visible.
fn draw_rng(world: &mut World, _ctx: &SystemCtx) {
    let value = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    for (probe,) in world.query::<(&mut RngProbe,)>() {
        probe.last = value;
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
    if let Some(switch) = world.resource_mut::<ludion_engine::SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player],
        );
    }
}

/// Requests a switch to a real district that lies outside the project root.
fn switch_outside_root(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    if let Some(switch) = world.resource_mut::<ludion_engine::SceneSwitch>() {
        switch.request(
            format!("../ludion-outside-{}.yaml", std::process::id()),
            "gate-from-start".to_string(),
            Vec::new(),
        );
    }
}

// --- Harness ----------------------------------------------------------------------------------

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-transition-replay-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    dir
}

/// The base game rooted at `root`: the RNG probe + the components; a caller adds the switch system.
fn base_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| {
            reg.register::<Player>("Player")?;
            reg.register::<RngProbe>("RngProbe")?;
            Ok(())
        })
        .system("draw_rng", draw_rng)
        .project_root(root)
}

/// A recording sink shared with the test so the recorded bytes can be read back.
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Drives `steps` single-tick `sim.step`s through `engine.execute` (so a switch records when armed),
/// returning the per-frame `(frame, hash)` trace — the exact shape `drive_hashes`/`replay_hashes_with`
/// produce, for an exact round-trip comparison.
fn run_recorded(engine: &mut Engine, steps: usize) -> Vec<(FrameIndex, WorldHash)> {
    let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    for _ in 0..steps {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
        trace.push((engine.frame(), engine.world_hash().unwrap()));
    }
    trace
}

/// The `SceneSwitch` records in a recording, as `(scene_path, document)`.
fn scene_switch_records(bytes: &[u8]) -> Vec<(String, String)> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::SceneSwitch {
                scene_path,
                document,
                ..
            } => Some((scene_path, document)),
            _ => None,
        })
        .collect()
}

// --- Tests ------------------------------------------------------------------------------------

#[test]
fn a_switch_target_outside_the_project_root_is_rejected_and_never_recorded() {
    let dir = fresh_dir("outside-root");
    let outside = dir
        .parent()
        .unwrap()
        .join(format!("ludion-outside-{}.yaml", std::process::id()));
    std::fs::write(&outside, NEXT).unwrap();
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("switch_outside_root", switch_outside_root)
        .build()
        .unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let _ = std::fs::remove_file(&outside);

    let error = engine
        .last_transition_error()
        .expect("an escaping target is reported");
    assert_eq!(error.code.as_str(), "L1321");
    assert_eq!(engine.world().entity_count(), 1, "the world is unchanged");
    assert!(scene_switch_records(&sink.bytes()).is_empty());
}

#[test]
fn a_recorded_switch_session_reproduces_its_worldhash_trace_twice() {
    let dir = fresh_dir("cross-transition");
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let live = run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // The gate crossing wrote exactly one `SceneSwitch` record carrying district B's text inline.
    let switches = scene_switch_records(&bytes);
    assert_eq!(switches.len(), 1, "exactly one switch was recorded");
    assert_eq!(switches[0].0, "next.yaml");
    assert!(
        switches[0].1.contains("beacon"),
        "the record carries district B's canonical text inline: {}",
        switches[0].1
    );

    // The recorded session reproduces its trace from the `.lrp`, run twice (regeneration is stable).
    let replay1 = replay_hashes_with(
        base_game(&dir).system("switch_to_next", switch_to_next),
        &bytes,
        &dir,
    )
    .unwrap();
    let replay2 = replay_hashes_with(
        base_game(&dir).system("switch_to_next", switch_to_next),
        &bytes,
        &dir,
    )
    .unwrap();
    assert_eq!(
        live, replay1,
        "the recorded switch session reproduces its live WorldHash trace"
    );
    assert_eq!(
        replay1, replay2,
        "regeneration is deterministic across runs"
    );
}

#[test]
fn the_switch_reproduces_off_disk_when_the_target_district_is_reverted() {
    let dir = fresh_dir("reverted");
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let live = run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // A reverted checkout: district B on disk now differs (a valid but different world). If replay read
    // disk it would place the beacon at 999 and diverge; the embedded record text (beacon at 7) wins.
    std::fs::write(dir.join("next.yaml"), NEXT_REVERTED).unwrap();

    let replayed = replay_hashes_with(
        base_game(&dir).system("switch_to_next", switch_to_next),
        &bytes,
        &dir,
    )
    .unwrap();
    assert_eq!(
        live, replayed,
        "the switch reproduces off-disk from the embedded record text, not the reverted disk file"
    );
}

#[test]
fn the_switch_reproduces_off_disk_when_the_target_district_is_absent() {
    let dir = fresh_dir("absent");
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let live = run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // The target district is gone entirely. A disk read would reject the switch (L3018) and the run
    // would continue in district A — a wholly different trace; the embedded text reproduces it.
    std::fs::remove_file(dir.join("next.yaml")).unwrap();

    let replayed = replay_hashes_with(
        base_game(&dir).system("switch_to_next", switch_to_next),
        &bytes,
        &dir,
    )
    .unwrap();
    assert_eq!(
        live, replayed,
        "the switch reproduces off-disk even when the target district file is absent"
    );
}

#[test]
fn switching_to_b_diverges_from_staying_in_a() {
    let dir_switch = fresh_dir("div-switch");
    let mut switching = base_game(&dir_switch)
        .system("switch_to_next", switch_to_next)
        .build()
        .unwrap();
    switching.load_inline_scene(START).unwrap();
    let switched = run_recorded(&mut switching, 6);

    let dir_stay = fresh_dir("div-stay");
    let mut staying = base_game(&dir_stay).build().unwrap(); // no switch system
    staying.load_inline_scene(START).unwrap();
    let stayed = run_recorded(&mut staying, 6);

    assert_ne!(
        switched, stayed,
        "a run that crosses the gate is hash-visibly different from one that stays in district A"
    );
}

#[test]
fn a_no_switch_recording_reproduces_unchanged() {
    // A recording that never crosses a gate carries no `SceneSwitch` record; replay installs an
    // empty scene-text map all the same and reproduces it.
    let dir = fresh_dir("no-switch");
    let sink = SharedSink::default();
    let mut engine = base_game(&dir).build().unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let live = run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    assert!(
        scene_switch_records(&bytes).is_empty(),
        "a run that never switches records no SceneSwitch"
    );
    let replayed = replay_hashes_with(base_game(&dir), &bytes, &dir).unwrap();
    assert_eq!(live, replayed, "a no-switch recording reproduces unchanged");
}

#[test]
fn a_switch_rejected_live_stays_rejected_when_the_target_appears() {
    let dir = fresh_dir("rejected");
    std::fs::remove_file(dir.join("next.yaml")).unwrap();
    let sink = SharedSink::default();
    let mut engine = base_game(&dir)
        .system("switch_to_next", switch_to_next)
        .build()
        .unwrap();
    engine.load_inline_scene(START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    // The target is absent, so the live switch is rejected (L3018) and records nothing.
    let live = run_recorded(&mut engine, 6);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();
    assert!(
        scene_switch_records(&bytes).is_empty(),
        "a rejected switch records no SceneSwitch"
    );

    // The target now exists on this checkout; the replay must still not read it off disk.
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    let replayed = replay_hashes_with(
        base_game(&dir).system("switch_to_next", switch_to_next),
        &bytes,
        &dir,
    )
    .unwrap();
    assert_eq!(
        live, replayed,
        "a switch rejected live stays rejected on replay"
    );
}

// --- Item 4: the doc-reload × switch interaction ----------------------------------------------

/// The `weapon` doc: one tunable number a system copies into a hashed component.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

/// A hashed component a system derives from the `weapon` `DocStore` — the doc's sim-visible surface.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct DamageReadout {
    value: f32,
}

const WEAPON_KEY: &str = "weapons/crossbow.yaml";
const WEAPON_V1: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
const WEAPON_V2: &str = "format: 1\nkind: weapon\ndamage: 99.0\n";

/// District A for the doc test: a carried player holding the readout the `weapon` doc drives.
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

/// District B for the doc test: just the anchor — the carried player brings the readout, so the
/// post-switch readout reflects district B's `DocStore` (the item-4 surface).
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

/// Copies the current `weapon` damage into every `DamageReadout` each tick.
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

/// Requests a switch to `next.yaml` on frame 2, carrying the `DamageReadout` entity (the player).
fn switch_carrying_readout(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    let ids: Vec<EntityId> = world.entities().collect();
    let Some(player) = ids
        .into_iter()
        .find(|&id| matches!(world.get::<DamageReadout>(id), Ok(Some(_))))
    else {
        return;
    };
    if let Some(switch) = world.resource_mut::<ludion_engine::SceneSwitch>() {
        switch.request(
            "next.yaml".to_string(),
            "gate-from-start".to_string(),
            vec![player],
        );
    }
}

fn doc_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<DamageReadout>("DamageReadout"))
        .system("apply_weapon", apply_weapon)
        .system("switch_carrying_readout", switch_carrying_readout)
        .doc_kind::<Weapon>("weapon")
        .project_root(root)
}

/// One step of the doc-test scenario.
enum Act {
    Step,
    ReloadWeapon(&'static str),
}

/// Drives a mixed step/reload script through `engine.execute`, returning the per-frame trace.
fn run_doc(engine: &mut Engine, root: &Path, script: &[Act]) -> Vec<(FrameIndex, WorldHash)> {
    let mut trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    for act in script {
        match act {
            Act::Step => {
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                trace.push((engine.frame(), engine.world_hash().unwrap()));
            }
            Act::ReloadWeapon(new_doc) => {
                std::fs::write(root.join(WEAPON_KEY), new_doc).unwrap();
                engine
                    .execute(Command::AssetReload(AssetReloadParams {
                        path: Some(WEAPON_KEY.to_string()),
                    }))
                    .unwrap();
                if let Some(last) = trace.last_mut() {
                    *last = (engine.frame(), engine.world_hash().unwrap());
                }
            }
        }
    }
    trace
}

fn write_weapon(root: &Path, contents: &str) {
    let path = root.join(WEAPON_KEY);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn a_doc_reloaded_before_a_switch_survives_the_rebuild_off_disk() {
    let dir = fresh_dir("doc-switch");
    // District B for this game (overwrite the base harness's `next.yaml` with the doc-test anchor).
    std::fs::write(dir.join("next.yaml"), DOC_NEXT).unwrap();
    write_weapon(&dir, WEAPON_V1);

    let sink = SharedSink::default();
    let mut engine = doc_game(&dir).build().unwrap();
    engine.load_inline_scene(DOC_START).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    // Reload the weapon to v2 BEFORE the frame-2 switch, then tick on in district B so `apply_weapon`
    // re-derives the readout there from the (reloaded) store.
    let script = [
        Act::Step,
        Act::ReloadWeapon(WEAPON_V2),
        Act::Step,
        Act::Step,
        Act::Step,
        Act::Step,
    ];
    let live = run_doc(&mut engine, &dir, &script);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // The readout in district B holds the RELOADED value (99), not the boot value — the run really
    // does depend on the reloaded doc after the switch.
    let readout = engine
        .world_mut()
        .query::<(&DamageReadout,)>()
        .next()
        .map(|(r,)| r.value);
    assert_eq!(
        readout,
        Some(99.0),
        "district B derives from the reloaded doc"
    );

    // The `.lrp` carries both a DocReload and a SceneSwitch record.
    let reader = ReplayReader::new(&bytes).unwrap();
    let records = reader.records().unwrap();
    assert!(
        records
            .iter()
            .any(|r| matches!(r, ReplayRecord::DocReload { .. })),
        "the pre-switch reload was recorded"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r, ReplayRecord::SceneSwitch { .. })),
        "the switch was recorded"
    );

    // A reverted checkout: the weapon doc on disk is back to v1. `install_docs` at the switch would
    // re-read v1 (12) and revert the district-B readout to 12 — unless the session's reload log (fed
    // from the DocReload record) is re-applied off-disk (item 4). The exact-equality below is that
    // proof: it holds only because district B ends on the reloaded v2 (99) both live and under replay.
    write_weapon(&dir, WEAPON_V1);

    let replayed = replay_hashes_with(doc_game(&dir), &bytes, &dir).unwrap();
    assert_eq!(
        live, replayed,
        "a doc reloaded before the switch is re-applied on the rebuilt district off-disk"
    );
}
