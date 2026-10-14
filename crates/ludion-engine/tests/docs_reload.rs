//! Game data-doc hot-reload + determinism/replay tests (Phase 15 / E4, Wave 3).
//!
//! A doc reload rides the existing `asset.reload` command: a `*.yaml` under the content root whose
//! `kind:` is registered is re-validated and upserted into its `DocStore`, the store's generation is
//! bumped, and — when recording is armed — the swap is written to LRP1 as a frame-stamped `DocReload`
//! record carrying the new canonical text inline, so a replay reproduces the identical `WorldHash`
//! trace off-disk. A malformed edit is a diagnosed `L1119`/`L1120` that leaves the store untouched and
//! records nothing; a non-doc reload (a scene, a voxel asset) never takes the doc path.

use ludion_core::time::FrameIndex;
use ludion_engine::{DocStore, Engine, EngineBuilder, replay_hashes_with};
use ludion_protocol::command::{AssetReloadParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The `weapon` doc: one tunable number.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
}

/// A component a game system derives from the `DocStore` — the doc's sim-visible surface, so a reload
/// moves the `WorldHash` exactly when a tick next reads the store.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct DamageReadout {
    value: f32,
}

/// The one doc key every test tunes.
const KEY: &str = "weapons/crossbow.yaml";
const DOC_V1: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
const DOC_V2: &str = "format: 1\nkind: weapon\ndamage: 99.0\n";
/// A schema-invalid edit (`damage` is not a number) → `L1120`.
const DOC_BAD: &str = "format: 1\nkind: weapon\ndamage: fast\n";
/// One entity carrying the readout the system writes into.
const SCENE: &str =
    "format: 1\nseed: 1\nentities:\n  - components:\n      DamageReadout:\n        value: 0.0\n";

/// Copies the current `weapon` damage into every `DamageReadout` each tick, making the store
/// sim-visible (the hashed component tracks the doc's number).
fn apply_weapon(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let damage = world
        .resource::<DocStore<Weapon>>()
        .and_then(|store| store.get(KEY))
        .map(|weapon| weapon.damage);
    if let Some(damage) = damage {
        for (readout,) in world.query::<(&mut DamageReadout,)>() {
            readout.value = damage;
        }
    }
}

/// A builder for the weapon game rooted at `root`: the `DamageReadout` component, the `weapon` doc
/// kind, and the system that couples them.
fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<DamageReadout>("DamageReadout"))
        .system("apply_weapon", apply_weapon)
        .doc_kind::<Weapon>("weapon")
        .project_root(root)
}

/// A fresh, empty temp directory unique to this process + `name`.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-docreload-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `contents` to `root`/`rel`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// One step of a hand-driven scenario.
enum Act {
    /// Advance one tick, sampling the per-frame hash.
    Step,
    /// Write `new_doc` to the doc on disk, then drive `asset.reload` (the live doc-reload path).
    Reload(&'static str),
}

/// Drives `script` on `engine`, returning the per-frame `(frame, WorldHash)` trace — the same
/// stepping-and-hashing shape `drive_hashes` produces (a step appends a hash; a reload refines the
/// current frame's hash in place, applying between ticks). Uses `engine.execute`, so a reload records
/// when recording is armed.
fn run(engine: &mut Engine, root: &Path, script: &[Act]) -> Vec<(FrameIndex, WorldHash)> {
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
            Act::Reload(new_doc) => {
                write(root, KEY, new_doc);
                engine
                    .execute(Command::AssetReload(AssetReloadParams {
                        path: Some(KEY.to_string()),
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

/// The `weapon` store's generation.
fn generation(engine: &Engine) -> u64 {
    engine
        .world()
        .resource::<DocStore<Weapon>>()
        .unwrap()
        .generation()
}

/// The loaded `crossbow` doc's damage.
fn damage(engine: &Engine) -> f32 {
    engine
        .world()
        .resource::<DocStore<Weapon>>()
        .unwrap()
        .get(KEY)
        .unwrap()
        .damage
}

/// A recording sink shared with the test so the recorded bytes can be read back.
#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

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

#[test]
fn live_reload_is_deterministic_and_diverges_at_the_reload_frame() {
    let dir = fresh_dir("live");
    write(&dir, KEY, DOC_V1);

    // A no-reload baseline (disk stays v1 — this run writes nothing).
    let mut base = game(&dir).build().unwrap();
    base.load_inline_scene(SCENE).unwrap();
    let baseline = run(
        &mut base,
        &dir,
        &[
            Act::Step,
            Act::Step,
            Act::Step,
            Act::Step,
            Act::Step,
            Act::Step,
        ],
    );

    // A run that reloads to v2 after three steps.
    write(&dir, KEY, DOC_V1); // ensure the reload run boots on v1
    let mut reloaded_engine = game(&dir).build().unwrap();
    reloaded_engine.load_inline_scene(SCENE).unwrap();
    let script = [
        Act::Step,
        Act::Step,
        Act::Step,
        Act::Reload(DOC_V2),
        Act::Step,
        Act::Step,
        Act::Step,
    ];
    let reloaded = run(&mut reloaded_engine, &dir, &script);

    assert_eq!(generation(&reloaded_engine), 1, "one applied reload");
    assert_eq!(
        damage(&reloaded_engine),
        99.0,
        "the store holds the new value"
    );

    // The trace is identical through the reload frame (frame 3: the reload applies between ticks, so
    // no tick has yet read the new value) and diverges at the first tick after it (frame 4).
    assert_eq!(
        &reloaded[0..=3],
        &baseline[0..=3],
        "unchanged until the reload frame"
    );
    assert_eq!(reloaded[4].0, FrameIndex::new(4));
    assert_ne!(
        reloaded[4].1, baseline[4].1,
        "the first tick after the reload diverges"
    );

    // Two identical reload runs produce identical traces.
    write(&dir, KEY, DOC_V1);
    let mut again = game(&dir).build().unwrap();
    again.load_inline_scene(SCENE).unwrap();
    let reloaded_again = run(&mut again, &dir, &script);
    assert_eq!(reloaded, reloaded_again, "the reload is deterministic");
}

#[test]
fn record_then_replay_reproduces_from_inline_content_off_disk() {
    let dir = fresh_dir("record");
    write(&dir, KEY, DOC_V1);

    let sink = SharedSink::default();
    let mut engine = game(&dir).build().unwrap();
    // An inline scene makes the replay self-contained (the scene text travels in the header).
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let script = [
        Act::Step,
        Act::Step,
        Act::Step,
        Act::Reload(DOC_V2),
        Act::Step,
        Act::Step,
        Act::Step,
    ];
    let original = run(&mut engine, &dir, &script);
    engine.stop_recording().unwrap();
    let bytes = sink.bytes();

    // The reload wrote v2 to disk; revert the on-disk doc to its committed v1 baseline. The replay's
    // BOOT re-reads v1 (so frames 0..3 reproduce), and the mid-run tune to v2 lives ONLY in the .lrp's
    // inline `DocReload` record — never on disk. If replay re-read the file at the reload frame it
    // would apply v1 (12.0) and the post-reload frames would read 12.0, not the original's 99.0, so
    // the exact-equality below is the inline-content proof.
    write(&dir, KEY, DOC_V1);

    let replayed = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(
        original, replayed,
        "the recorded session reproduces off-disk from its inline doc-reload record"
    );

    // The record is present and carries the canonical v2 text inline (not the on-disk v1).
    let reader = ReplayReader::new(&bytes).unwrap();
    let reloads: Vec<_> = reader
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|record| match record {
            ReplayRecord::DocReload {
                doc_kind,
                path,
                document,
                ..
            } => Some((doc_kind, path, document)),
            _ => None,
        })
        .collect();
    assert_eq!(reloads.len(), 1, "exactly one doc reload was recorded");
    assert_eq!(reloads[0].0, "weapon");
    assert_eq!(reloads[0].1, KEY);
    assert!(
        reloads[0].2.contains("99"),
        "the inline text carries the new value: {}",
        reloads[0].2
    );
}

#[test]
fn a_malformed_reload_is_rejected_and_preserves_the_store() {
    let dir = fresh_dir("malformed");
    write(&dir, KEY, DOC_V1);

    let sink = SharedSink::default();
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    run(&mut engine, &dir, &[Act::Step, Act::Step]);

    // Edit the doc to a schema-invalid value, then reload.
    write(&dir, KEY, DOC_BAD);
    let err = engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(KEY.to_string()),
        }))
        .expect_err("a malformed edited doc is a diagnosed error, not a crash or a silent skip");
    assert_eq!(err.code.as_str(), "L1120", "a schema mismatch");

    // The store kept its previous contents and generation — a bad save mid-tune must not wipe state.
    assert_eq!(generation(&engine), 0, "generation unchanged");
    assert_eq!(damage(&engine), 12.0, "the previous value survives");

    engine.stop_recording().unwrap();
    assert!(
        !has_doc_reload(&sink.bytes()),
        "a rejected reload records nothing"
    );
}

#[test]
fn a_flow_style_reload_is_rejected_and_preserves_the_store() {
    let dir = fresh_dir("flow-style");
    write(&dir, KEY, DOC_V1);

    let sink = SharedSink::default();
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    run(&mut engine, &dir, &[Act::Step, Act::Step]);

    // Edit the doc to a non-empty flow mapping — outside the strict-YAML subset entirely, not merely
    // schema-invalid.
    write(&dir, KEY, "format: 1\nkind: weapon\ndamage: { a: 1 }\n");
    let err = engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(KEY.to_string()),
        }))
        .expect_err(
            "a doc the strict-YAML reader rejects is a diagnosed error, never a silent skip",
        );
    assert!(
        err.code.as_str().starts_with("L100"),
        "a strict-YAML reader rejection, not L1119/L1120: {}",
        err.code
    );
    assert!(
        err.message.contains(KEY),
        "the offending file must be named: {}",
        err.message
    );

    // The store kept its previous contents and generation — a bad save mid-tune must not wipe state.
    assert_eq!(generation(&engine), 0, "generation unchanged");
    assert_eq!(damage(&engine), 12.0, "the previous value survives");

    engine.stop_recording().unwrap();
    assert!(
        !has_doc_reload(&sink.bytes()),
        "a rejected reload records nothing"
    );
}

#[test]
fn a_non_doc_yaml_reload_is_ignored_and_not_recorded() {
    let dir = fresh_dir("nondoc");
    write(&dir, KEY, DOC_V1);
    // A scene (no `kind`) and an engine voxel doc — both on disk under the content root.
    write(&dir, "level2.yaml", "format: 1\nentities: []\n");
    write(
        &dir,
        "voxels/v.voxel.yaml",
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - \
         1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"A\"\n",
    );

    let sink = SharedSink::default();
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();

    // Neither a cached asset nor a registered doc → ignored: no event, no error, an empty reply.
    for path in ["level2.yaml", "voxels/v.voxel.yaml"] {
        let body = engine
            .execute(Command::AssetReload(AssetReloadParams {
                path: Some(path.to_string()),
            }))
            .unwrap()
            .body;
        assert!(
            matches!(body, ResponseBody::AssetsReloaded { paths } if paths.is_empty()),
            "reloading `{path}` (a non-doc yaml) is a silent no-op"
        );
    }

    assert_eq!(generation(&engine), 0, "no doc store changed");
    engine.stop_recording().unwrap();
    assert!(
        !has_doc_reload(&sink.bytes()),
        "a non-doc reload records nothing"
    );
}

/// Whether a recorded session contains any `DocReload` record.
fn has_doc_reload(bytes: &[u8]) -> bool {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .any(|record| matches!(record, ReplayRecord::DocReload { .. }))
}
