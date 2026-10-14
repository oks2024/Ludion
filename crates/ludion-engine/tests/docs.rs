//! Game data-doc boot tests (Phase 15 / E4, Wave 2): `EngineBuilder::doc_kind::<T>` walks the
//! content root at every world build and loads matching `*.yaml` into a typed `DocStore<T>` resource,
//! which survives `scene.load` and `restore`, is deterministic across boots, and fails the boot with
//! collected `L1119`/`L1120` diagnostics on a malformed or unregistered-kind doc.

use ludion_engine::{DocStore, Engine, EngineBuilder};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A tiny game data-doc type: `kind: weapon`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
    cooldown: f32,
}

/// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run starts
/// clean). The content root a builder is pointed at.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-docs-{}-{name}", std::process::id()));
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

/// A builder registering the `weapon` kind, rooted at `root`.
fn weapon_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .doc_kind::<Weapon>("weapon")
        .project_root(root)
}

/// The `weapon` store, collected into a sorted `(key, weapon)` list for order-sensitive assertions.
fn weapons(engine: &Engine) -> Vec<(String, Weapon)> {
    engine
        .world()
        .resource::<DocStore<Weapon>>()
        .expect("the weapon DocStore is installed once the kind is registered")
        .iter()
        .map(|(k, w)| (k.clone(), w.clone()))
        .collect()
}

/// Builds a game rooted at `root` and expects the boot to fail, returning the (collapsed) diagnostic
/// — `Engine` is not `Debug`, so `expect_err` cannot be used.
fn build_err(builder: EngineBuilder) -> ludion_core::diagnostics::Diagnostic {
    match builder.build() {
        Ok(_) => panic!("expected the boot to fail on a malformed doc"),
        Err(diagnostic) => diagnostic,
    }
}

const CROSSBOW: &str = "format: 1\nkind: weapon\ndamage: 12.0\ncooldown: 0.8\n";
const AXE: &str = "format: 1\nkind: weapon\ndamage: 30.0\ncooldown: 1.5\n";

#[test]
fn boots_a_registered_kind_into_a_docstore() {
    let dir = fresh_dir("boot");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);
    write(&dir, "weapons/axe.yaml", AXE);

    let engine = weapon_game(&dir).build().expect("valid docs boot");
    let store = engine.world().resource::<DocStore<Weapon>>().unwrap();

    assert_eq!(store.len(), 2);
    assert_eq!(store.generation(), 0);
    // Keyed by slash-normalized root-relative path, values typed.
    assert_eq!(
        store.get("weapons/crossbow.yaml"),
        Some(&Weapon {
            damage: 12.0,
            cooldown: 0.8
        })
    );
    // BTreeMap iteration order is the pinned key order: `axe` before `crossbow`.
    let keys: Vec<&String> = store.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["weapons/axe.yaml", "weapons/crossbow.yaml"]);
}

#[test]
fn dot_directories_are_not_content() {
    let dir = fresh_dir("dotdir");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);
    // `.ludion/` is the engine's scratch/output convention — a stray YAML there (an ops file, an
    // editor capture) must neither load nor fail the boot.
    write(
        &dir,
        ".ludion/scratch/ops.yaml",
        "format: 1\nkind: voxel-ops\nops: []\n",
    );
    write(&dir, ".hidden/enemy.yaml", "format: 1\nkind: enemy\n");

    let engine = weapon_game(&dir)
        .build()
        .expect("dot-directories are skipped");
    let keys: Vec<String> = weapons(&engine).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["weapons/crossbow.yaml"]);
}

#[test]
fn build_output_directories_are_not_content() {
    let dir = fresh_dir("build-output");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);
    // `target/` and `node_modules/` are build output, not dot-directories, and sit under the
    // default `project_root(".")` every `GameplayDriver::new` test used before this test existed
    // — the walk must skip them like it skips `.ludion/`, not descend into a build tree.
    write(
        &dir,
        "target/debug/build/b.yaml",
        "format: 1\nkind: weapon\n",
    );
    write(&dir, "node_modules/pkg/c.yaml", "format: 1\nkind: weapon\n");
    write(&dir, ".ludion/scratch/d.yaml", "format: 1\nkind: weapon\n");

    let engine = weapon_game(&dir)
        .build()
        .expect("target/ and node_modules/ are skipped like dot-directories");
    let keys: Vec<String> = weapons(&engine).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["weapons/crossbow.yaml"]);
}

#[test]
fn two_broken_docs_both_appear_in_the_diagnostics() {
    let dir = fresh_dir("broken");
    // One envelope error (L1119: wrong kind), one schema error (L1120: damage is a string).
    write(&dir, "weapons/wrong_kind.yaml", "format: 1\nkind: enemy\n");
    write(
        &dir,
        "weapons/bad_field.yaml",
        "format: 1\nkind: weapon\ndamage: fast\ncooldown: 0.8\n",
    );

    let err = build_err(weapon_game(&dir));

    // The collapse puts the first diagnostic primary and the rest in `related` — collect all codes
    // and messages to prove both broken files surfaced in one pass (collect-across-files).
    let mut all = vec![err.clone()];
    all.extend(err.related.iter().cloned());
    let rendered: String = all
        .iter()
        .map(|d| d.message.clone())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("weapons/bad_field.yaml"),
        "the schema-broken file must be named: {rendered}"
    );
    let codes: Vec<&str> = all.iter().map(|d| d.code.as_str()).collect();
    // `wrong_kind` is an unregistered `kind: enemy` (peeked, not registered) → L1119; `bad_field`
    // is a schema mismatch → L1120.
    assert!(codes.contains(&"L1119"), "codes: {codes:?}");
    assert!(codes.contains(&"L1120"), "codes: {codes:?}");
}

#[test]
fn unregistered_kind_fails_boot_with_a_nearest_match() {
    let dir = fresh_dir("typo");
    // A typo'd kind that IS a near-miss of the registered `weapon`.
    write(&dir, "weapons/oops.yaml", "format: 1\nkind: wepon\n");

    let err = build_err(weapon_game(&dir));
    assert_eq!(err.code.as_str(), "L1119");
    assert!(err.message.contains("weapons/oops.yaml"), "{}", err.message);
    assert!(
        err.help.as_deref().unwrap_or("").contains("weapon"),
        "the nearest-match help must suggest `weapon`: {:?}",
        err.help
    );
}

#[test]
fn a_mixed_content_root_routes_correctly() {
    let dir = fresh_dir("mixed");
    // A scene (no `kind`) — skipped.
    write(&dir, "scene.yaml", "format: 1\nentities: []\n");
    // An engine mesh doc — skipped.
    write(
        &dir,
        "meshes/tri.mesh.yaml",
        "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\n  - 0.0\n  \
         - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n",
    );
    // An engine voxel doc — skipped.
    write(
        &dir,
        "voxels/v.voxel.yaml",
        "format: 1\nkind: voxel\nsize:\n  - 1\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - \
         1.0\n      - 0.5\n      - 0.2\n      - 1.0\nslices:\n  -\n    - \"A\"\n",
    );
    // A registered weapon doc — loaded.
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = weapon_game(&dir).build().expect("mixed root boots");
    let loaded = weapons(&engine);
    assert_eq!(
        loaded,
        vec![(
            "weapons/crossbow.yaml".to_string(),
            Weapon {
                damage: 12.0,
                cooldown: 0.8
            }
        )],
        "only the registered `weapon` doc lands in the store — scene/mesh/voxel are skipped"
    );
}

#[test]
fn the_store_survives_scene_load() {
    let dir = fresh_dir("scene-load");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let mut engine = weapon_game(&dir).build().expect("boots");
    assert_eq!(weapons(&engine).len(), 1);

    // A `scene.load` resets the world (wiping resources); the store must be rebuilt from the root.
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");

    let store = engine.world().resource::<DocStore<Weapon>>().unwrap();
    assert_eq!(store.generation(), 0, "a fresh boot resets generation to 0");
    assert_eq!(
        store.get("weapons/crossbow.yaml"),
        Some(&Weapon {
            damage: 12.0,
            cooldown: 0.8
        }),
        "the store is rebuilt after scene.load, not lost to the resource wipe"
    );
}

#[test]
fn the_store_survives_restore() {
    let dir = fresh_dir("restore");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let mut e1 = weapon_game(&dir).build().expect("boots");
    e1.load_inline_scene("format: 1\nentities: []\n").unwrap();
    let snapshot = match e1.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot body, got {other:?}"),
    };

    // Restore into a second engine built from the same game config + root.
    let mut e2 = weapon_game(&dir).build().expect("boots");
    match e2
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { report } => assert!(report.is_clean(), "{report:?}"),
        other => panic!("expected a restored body, got {other:?}"),
    }

    let store = e2.world().resource::<DocStore<Weapon>>().unwrap();
    assert_eq!(store.generation(), 0, "restore rebuilds the store at 0");
    assert_eq!(
        store.get("weapons/crossbow.yaml"),
        Some(&Weapon {
            damage: 12.0,
            cooldown: 0.8
        }),
        "the store is rebuilt after restore, not lost to the world rebuild"
    );
}

#[test]
fn two_boots_are_deterministic() {
    let dir = fresh_dir("determinism");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);
    write(&dir, "weapons/axe.yaml", AXE);

    let mut a = weapon_game(&dir).build().expect("boots");
    let mut b = weapon_game(&dir).build().expect("boots");

    assert_eq!(
        weapons(&a),
        weapons(&b),
        "two boots from the same content root produce identical stores"
    );
    assert_eq!(
        a.world_hash().unwrap(),
        b.world_hash().unwrap(),
        "the store is resource-only — WorldHash is identical across boots"
    );
}

/// The voxel-animation multi-file format (`Plan/voxel-animation.md`) — `kind: assembly` /
/// `kind: part` / `kind: animation` — is engine-owned like `mesh`/`voxel`: a content root holding
/// one of each loads through `Mesh.shape: assembly`/`ludion_engine::load_assembly`, never through a
/// `DocStore`, so the walk must skip all three without raising `L1119`, exactly like the mixed-root
/// mesh/voxel case above.
#[test]
fn assembly_part_animation_files_are_skipped_not_data_docs() {
    let dir = fresh_dir("voxel-animation");
    write(
        &dir,
        "models/player.assembly.yaml",
        "format: 1\n\
         kind: assembly\n\
         name: player\n\
         size:\n  - 4\n  - 4\n  - 4\n\
         palette:\n  A:\n    color:\n      - 1.0\n      - 1.0\n      - 1.0\n      - 1.0\n\
         parts:\n  - name: body\n    file: parts/body.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n\
         clips:\n  walk: anims/walk.anim.yaml\n",
    );
    write(
        &dir,
        "models/parts/body.part.yaml",
        "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - A\n",
    );
    write(
        &dir,
        "models/anims/walk.anim.yaml",
        "format: 1\n\
         kind: animation\n\
         duration: 1.0\n\
         loop: true\n\
         tracks:\n  body:\n    translation:\n      - t: 0.0\n        v:\n          - 0.0\n          \
         - 0.0\n          - 0.0\n",
    );
    // A registered weapon doc alongside them, so the walk demonstrably still loads real content.
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = weapon_game(&dir)
        .build()
        .expect("assembly/part/animation files are engine-owned and skipped, not L1119");
    let loaded = weapons(&engine);
    assert_eq!(
        loaded,
        vec![(
            "weapons/crossbow.yaml".to_string(),
            Weapon {
                damage: 12.0,
                cooldown: 0.8
            }
        )],
        "only the registered `weapon` doc lands in the store — assembly/part/animation are skipped"
    );
}

/// The sprite engine's authored/generated pair (`Plan/sprite-engine.md`) — `kind: sprite` /
/// `kind: atlas-source` / `kind: atlas` — is engine-owned like `mesh`/`voxel`: an atlas reference
/// (`atlas`/`frame`) loads them directly off disk, never through a `DocStore`, so a content root
/// holding one of each must be skipped by the walk without raising `L1119`, exactly like the
/// mixed-root mesh/voxel case and the assembly/part/animation case above.
#[test]
fn sprite_atlas_atlas_source_files_are_skipped_not_data_docs() {
    let dir = fresh_dir("sprite-atlas");
    write(
        &dir,
        "sprites/ship.sprite.yaml",
        "format: 1\nkind: sprite\nsize:\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n  \
         \x20   - 1.0\n      - 1.0\n      - 1.0\nframes:\n  idle:\n    rows:\n      - \"A\"\n",
    );
    write(
        &dir,
        "atlases/ship.atlas-source.yaml",
        "format: 1\n\
         kind: atlas-source\n\
         name: ship\n\
         pixels_per_unit: 16.0\n\
         filter: linear\n\
         inputs:\n  - sprites/ship.sprite.yaml\n",
    );
    write(
        &dir,
        "atlases/ship.atlas.yaml",
        "format: 1\n\
         kind: atlas\n\
         generated_from: ship.atlas-source.yaml\n\
         pixels_per_unit: 16.0\n\
         maps:\n  color: ship.png\n\
         regions: {}\n",
    );
    // A registered weapon doc alongside them, so the walk demonstrably still loads real content.
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = weapon_game(&dir)
        .build()
        .expect("sprite/atlas/atlas-source files are engine-owned and skipped, not L1119");
    let loaded = weapons(&engine);
    assert_eq!(
        loaded,
        vec![(
            "weapons/crossbow.yaml".to_string(),
            Weapon {
                damage: 12.0,
                cooldown: 0.8
            }
        )],
        "only the registered `weapon` doc lands in the store — sprite/atlas/atlas-source are skipped"
    );
}

/// `kind: voxel-ops` (the voxel edit-op list `ludion voxel` applies) is engine-owned like `voxel`:
/// a content root holding one under `ops/` must be skipped, not raise `L1119`.
#[test]
fn voxel_ops_files_are_skipped_not_data_docs() {
    let dir = fresh_dir("voxel-ops");
    write(
        &dir,
        "ops/skull.yaml",
        "format: 1
kind: voxel-ops
ops: []
",
    );
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = weapon_game(&dir)
        .build()
        .expect("a voxel-ops file is engine-owned and skipped, not L1119");
    let loaded = weapons(&engine);
    assert_eq!(
        loaded
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec!["weapons/crossbow.yaml"],
        "only the registered `weapon` doc lands in the store"
    );
}

/// A doc the strict-YAML subset rejects (a non-empty flow mapping) must fail the boot with the
/// reader's own `L100x` diagnostic naming the file — never load as an empty store the way a silent
/// `peek_kind` skip used to (the defect this test guards against). A `kind:`-less scene beside it,
/// by contrast, is still skipped without a diagnostic — flow style is a parse failure, no `kind` key
/// at all is not.
#[test]
fn a_flow_style_doc_fails_boot_instead_of_vanishing() {
    let dir = fresh_dir("flow-style");
    write(
        &dir,
        "weapons/flow.yaml",
        "format: 1\nkind: weapon\ndamage: { a: 1 }\ncooldown: 0.8\n",
    );
    // A scene beside it (no `kind` key) — still silently skipped, not a parse failure.
    write(&dir, "scene.yaml", "format: 1\nentities: []\n");

    let err = build_err(weapon_game(&dir));
    assert!(
        err.message.contains("weapons/flow.yaml"),
        "the offending file must be named: {}",
        err.message
    );
    assert!(
        err.code.as_str().starts_with("L100"),
        "a strict-YAML reader rejection, not L1119/L1120: {}",
        err.code
    );
}

/// The walk parses only files that could declare a top-level `kind`: a kind-less file the strict
/// subset would reject is treated as a scene and left to the scene loader, so it neither fails the
/// boot nor lands in a store, while a real doc beside it still loads.
#[test]
fn a_kind_less_unparseable_file_is_left_to_the_scene_loader() {
    let dir = fresh_dir("kind-less-unparseable");
    write(&dir, "broken-scene.yaml", "format: 1\nentities: { a: 1 }\n");
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = weapon_game(&dir)
        .build()
        .expect("a kind-less file is a scene to the doc walk, parsed or not");
    assert_eq!(
        weapons(&engine),
        vec![(
            "weapons/crossbow.yaml".to_string(),
            Weapon {
                damage: 12.0,
                cooldown: 0.8
            }
        )],
    );
}

#[test]
fn zero_registered_kinds_does_not_walk() {
    let dir = fresh_dir("zero-kinds");
    // A doc that WOULD be malformed if walked — but a game that registers no kinds never walks, so
    // the boot is byte-identical to a bare engine (no error, no store).
    write(&dir, "junk.yaml", "format: 1\nkind: wepon\n");

    let engine = Engine::builder()
        .project_root(&dir)
        .build()
        .expect("a game with no doc kinds ignores every doc — no walk, no IO");
    assert!(
        engine.world().resource::<DocStore<Weapon>>().is_none(),
        "no kind registered means no store"
    );
}

/// A minimal valid `kind: pattern` document — the one engine-owned data-doc kind a builder can
/// register without being a game (`EngineBuilder::patterns`).
const PATTERN: &str = "format: 1\nkind: pattern\nname: probe\nbullets:\n  pellet:\n    life: 60\n\
                       timeline:\n  - at: 0\n    bullet: pellet\n    speed: 1.0\n";

#[test]
fn an_engine_owned_kind_skips_a_foreign_kind_instead_of_failing() {
    let dir = fresh_dir("engine-owned");
    write(&dir, "patterns/probe.yaml", PATTERN);
    // A game's own kind, which this engine has no way of knowing: the built-in engine behind
    // `ludion serve`/`ludion sim-hash` hosts anyone's scene.
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);

    let engine = Engine::builder()
        .patterns()
        .project_root(&dir)
        .build()
        .expect("a foreign kind is skipped, not a boot failure, when no game kind is registered");

    let skipped = engine.doc_skip_warnings();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].code.as_str(), "L1155");
    assert!(
        skipped[0].message.contains("weapons/crossbow.yaml"),
        "{}",
        skipped[0].message
    );
    assert_eq!(
        skipped[0].severity,
        ludion_core::diagnostics::Severity::Warning,
    );

    // The host's form of the same fact: one diagnostic for the whole walk, naming the kinds rather
    // than the files, because a host prints this before it binds and a block per file buries
    // whatever it says next.
    let summary = engine
        .doc_skip_summary()
        .expect("a walk that skipped something has a summary");
    assert_eq!(summary.code.as_str(), "L1155");
    assert_eq!(
        summary.severity,
        ludion_core::diagnostics::Severity::Warning,
    );
    assert!(
        summary.message.contains("skipped 1 document"),
        "{}",
        summary.message
    );
    assert!(
        summary.message.contains("weapon (1)"),
        "{}",
        summary.message
    );
    assert!(
        !summary.message.contains("crossbow"),
        "the file list is the precise answer's job, not the summary's: {}",
        summary.message
    );

    // The skip is per-file: the engine-owned kind beside it still loaded.
    let patterns = engine
        .world()
        .resource::<DocStore<ludion_engine::PatternDoc>>()
        .expect("the pattern store is installed by `patterns()`");
    assert_eq!(patterns.len(), 1);
    assert!(patterns.get("patterns/probe.yaml").is_some());
}

#[test]
fn a_game_registered_kind_restores_the_l1119_failure() {
    let dir = fresh_dir("game-kind");
    write(&dir, "patterns/probe.yaml", PATTERN);
    write(&dir, "weapons/crossbow.yaml", CROSSBOW);
    // Once the game owns kinds in this root, an unknown one is a typo again — and fatal.
    write(&dir, "weapons/oops.yaml", "format: 1\nkind: nonsense\n");

    let err = build_err(
        Engine::builder()
            .patterns()
            .doc_kind::<Weapon>("weapon")
            .project_root(&dir),
    );
    assert_eq!(err.code.as_str(), "L1119");
    assert!(err.message.contains("weapons/oops.yaml"), "{}", err.message);
}

/// A type whose own `kind` field collides with the doc envelope.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
struct Tagged {
    kind: String,
    damage: f32,
}

#[test]
fn a_doc_type_with_a_top_level_kind_field_is_refused_at_build() {
    let dir = fresh_dir("tagged_kind");
    let diagnostic = build_err(
        Engine::builder()
            .doc_kind::<Tagged>("weapon")
            .project_root(&dir),
    );
    assert_eq!(diagnostic.code.as_str(), "L1257");
    assert!(
        diagnostic.message.contains("`kind`"),
        "{}",
        diagnostic.message
    );
}
