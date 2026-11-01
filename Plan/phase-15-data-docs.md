# Plan: Phase 15 — Game data-doc facility (E4)

> Status: **landed 2026-07-16 — all 5 waves, one commit each (Lore revs 176–180); E4 READY.**
> Exit gates green: wire surface 26 / `schema_hash` unmoved (schema-drift gate), zero golden churn,
> old `.lrp`s decode at format 1 (the `DocReload` record is a new kind byte, not a framing change),
> `check.mjs --full` green including the determinism canary. Amendments during build: none beyond
> the plan's own (the `--full` doc gate also surfaced two pre-existing Phase-14 rustdoc breaks,
> fixed in rev 178; Wave 4 found no serve/profile smoke precedent in check.mjs and added the
> `check-docs` smoke as its own step via hello-paddle's zero-kinds path).
> Amendment A1 (2026-11-01, Clockout request 0021): typed `parse_doc::<T>` added (decision 3).
> Amendment B (2026-11-01, Clockout request 0021): singleton kinds (decision 6).
> Amendment A2 (2026-11-01, Clockout request 0021): builder doc_text (decision 4).
>
> Originally: authored 2026-07-16, ready to start. This is the wave-level plan for wave **E4** of
> the first-game arc; its brief is `Plan/engine-first-game.md` §"E4 — Game data-doc facility" plus
> the "Cross-cutting rules" there, scheduled by a private roadmap (rule 5). The
> consumer is game phase **G4 — Armory** (a private plan, §G4 / roadmap M4): weapons and
> enemies become declarative docs (`kind: weapon`, `kind: enemy`), the balance tuning loop (edit a
> `.yaml` mid-run, watch it land via hot reload), and the content THE-test (a fresh agent adds a
> weapon from shipped docs alone). The plan must also keep G6's later `kind: npc` / `kind: quest`
> expressible without redesign — but builds **only what G4 consumes** (vertical slice; no
> speculative surface). The thesis: a game registers serde doc kinds and gets exactly what mesh and
> voxel content already enjoy — strict-YAML validation with L-coded pointer errors and nearest-match
> suggestions, canonical byte-idempotent emit, and hot reload — with **behavior staying in Rust**
> (docs name Rust-registered archetypes plus numbers). The E4 brief's "Settled scope" items are
> decided; its "Proposed design" items are confirmed or amended below with current-code evidence
> (amendments are called out inline). The flagged `--check-docs` decision is settled in decision 7.

## Current-code facts that shape the work

- **The `kind:`-dispatched doc pattern exists, but mesh/voxel parse and emit are *hand-rolled per
  kind* — they do not generalize to a game-defined type.** `ludion check-scene` sniffs the top-level
  `kind:` scalar (`peek_kind`, `crates/ludion-scene/src/mesh.rs:99-111`, re-exported at
  `crates/ludion-scene/src/lib.rs:296-298`) and routes to `check_mesh`/`check_voxel`
  (`crates/ludion-cli/src/check_scene.rs:37-45`); an unrecognized kind is a hard `L1110` error
  (`check_scene.rs:146-159`), and `check-scene` takes a **single file**, not a directory
  (`check_scene.rs:24-25`). But `mesh::parse` walks the YAML node tree by hand, matching a fixed
  key set field-by-field (`mesh.rs:114-200`, `MESH_KEYS` at `mesh.rs:38-49`), and `mesh::emit`
  hand-writes the canonical string (`mesh.rs:696-723`); `MeshDocument` has **no serde derives**
  (`mesh.rs:56-79`). Voxel is a structural sibling (`crates/ludion-scene/src/voxel.rs:1-16`). The
  engine cannot hand-parse a game's `WeaponDoc` — it does not know its fields.
- **The *scene-component* path, by contrast, is fully generic and schema-driven — that is the real
  template.** `validate.rs`'s `Checker` (`crates/ludion-scene/src/validate.rs:177-253`) validates an
  arbitrary `serde_json::Value` against a JSON Schema `Value`, emitting `L1014`-style diagnostics with
  a **JSON Pointer to the offending node** and, for unknown fields, a **nearest-match suggestion**
  (`validate.rs:360-381` via `suggest::nearest`). The canonical emitter has a matching generic core:
  `write_after_key`/`write_map_block`/`write_seq_block`/`ordered_keys` (`emit.rs:110-217`) emit any
  `Value` in canonical YAML, ordering object fields by **schema declaration order** through a
  `SchemaCursor` (`crates/ludion-scene/src/schema.rs:55-81`). Both are `pub(crate)` and type-agnostic
  (Values + schema Values, never a concrete type) — exactly what a game doc kind reduces to.
- **`ludion-scene` already owns *all* YAML knowledge and is deliberately type-agnostic**
  (`crates/ludion-scene/src/lib.rs:6-17`): it consumes schemas + erased serde and "never defines a
  component". A game doc kind is (a `kind` string + a `schemars` schema `Value` + a `Value`) — the
  crate's native vocabulary. So the generic doc machinery belongs here, alongside mesh/voxel, with
  no new crate.
- **Component/registration bounds and the schema capture are the `doc_kind::<T>` template.** The
  `Component` bound is `Serialize + DeserializeOwned + JsonSchema` (+ `'static + Send + Sync`)
  (`crates/ludion-ecs/src/lib.rs:100-108`); registration captures the schema once via
  `serde_json::to_value(schemars::schema_for!(T))` (`crates/ludion-ecs/src/registry.rs:278-279`) and
  exposes it as `info.schema() -> &Value` (`registry.rs:133-135`). Erased serde is
  `serde_json::{from_value, to_value}` (`registry.rs:356-386`).
- **Byte-idempotence is proven by a proptest, not asserted.** `crates/ludion-scene/tests/
  mesh_roundtrip.rs:87-96` (`emit_is_idempotent_byte_for_byte`) does emit → parse → emit and asserts
  byte-equality twice; voxel mirrors it (`voxel_roundtrip.rs:124`). This is the mold for the doc
  round-trip test.
- **Resources are the right home for loaded docs — and they trip no drift gate.** `World` stores
  resources as `BTreeMap<TypeId, Box<dyn Any + Send>>` (`crates/ludion-ecs/src/world.rs:64`), with
  `insert_resource<T: 'static + Send>` / `resource<T>` / `resource_mut<T>` (`world.rs:277-306`).
  Resources never serialize and are **not** in the `WorldHash` (only components are). The drift gate
  `assert_components_drift_safe` iterates `registry.iter()` — **registered components only**
  (`crates/ludion-testkit/src/drift.rs:72-93`); a `DocStore<T>` resource is invisible to it. So docs
  need **no** `{}`-deserialization default, no builtin-registry append, no pinned-order/world-hash
  re-bless.
- **There is no resource seam on `EngineBuilder` today.** The full builder surface is `new`,
  `project_root`, `render_size`, `components`, `system`, `default_scene`, `build`/`build_at`,
  `load_scene_file` (`crates/ludion-engine/src/builder.rs:78-199`); games use only `.components` /
  `.system` / `.default_scene` (`games/vigil/src/lib.rs:898-913`,
  `games/hello-paddle/src/lib.rs:108-113`). `doc_kind::<T>` is a genuinely new seam.
- **Resources are wiped and reinstalled on *every* world reset — the `install_audio_queue` mold.**
  `SimDriver::new` re-inserts `DeterministicRng` + `InputState` on every build
  (`crates/ludion-sim/src/driver.rs:98-100`); `Engine::install_audio_queue` re-inserts the
  `AudioQueue` resource at construction, `scene.load`, and `restore`
  (`crates/ludion-engine/src/engine.rs:345-349`, called at `:208`, `:498`, `:1036`). Any
  `DocStore<T>` must likewise be **held on `Engine`** (like `systems`, `engine.rs:85`) and
  reinstalled at each of those points from the persistent doc-kind registrations — not inserted once
  at build and expected to survive a load.
- **The content root is the scene file's own directory, resolved lexically.**
  `EngineBuilder::load_scene_file` sets `project_root = scene_path.parent()` (`builder.rs:187-199`);
  relative asset paths resolve against `engine.project_root()` (`engine.rs:369-371`, `:656-666`).
  `ludion.toml` is **not parsed** today (`crates/ludion-cli/templates/ludion.toml:3-5`). The notebook
  records the same ("the engine's content root is the scene's directory"); vigil keeps its voxel kit
  under `games/vigil/scenes/voxels/`, so G4's weapon docs live under `games/vigil/scenes/…`.
- **The `--profile`/`--profile-check` short-circuit is the `--check-docs` template.** `run()` resolves
  the scene, then `if let Some(frames) = args.profile { return run_profile(...) }`
  (`crates/ludion/src/lib.rs:182-184`) — diverting entirely out of hosting **before** `Server::bind`
  (`lib.rs:231`). `run_profile` builds the engine via `builder.load_scene_file` (`lib.rs:318`),
  validates and drives it headless, prints to stdout/stderr, returns an `ExitCode` — **no server, no
  GPU, no "listening" line** (`lib.rs:312-359`). `RunArgs` uses `clap` (`lib.rs:94-124`).
- **The asset-reload path already reaches the engine at a frame boundary — and is deliberately
  render-only.** The watcher forwards every changed path (300 ms debounce,
  `crates/ludion/src/lib.rs:85`; `ludion-assets/src/watch.rs`) as `Command::AssetReload` into the one
  intake queue (`crates/ludion-server/src/lib.rs:260-271`); `run_live` drains every pending command
  at the top of each frame **before the tick** (`crates/ludion-engine/src/live.rs:184-197`). Dispatch
  routes to `asset::reload` (`crates/ludion-engine/src/dispatch.rs:72`), which resolves the path
  against the **CPU asset cache** (images/meshes/voxels only, `crates/ludion-engine/src/asset_cache.rs:43-47`,
  BTreeMap-keyed by normalized relative path, `asset_cache.rs:214-216`), reloads it, and pushes an
  `asset.reloaded` event (`crates/ludion-engine/src/commands/asset.rs:37-49`). **A path the cache
  does not hold is silently ignored** (`asset.rs:37-40`) — the extension seam a doc reload rides.
- **Crucially: `asset.reload` is NOT recorded in LRP1, on purpose.** The recording predicate groups
  `Command::AssetReload(_)` with the not-recorded commands (`crates/ludion-engine/src/recording.rs:64`),
  with the rationale that "asset bytes never enter the WorldHash … recording it would only make a
  replay depend on assets it does not carry" (`recording.rs:50-52`; echoed `asset_cache.rs:22-24`).
  LRP1 records exactly three frame-stamped kinds — `Command`, `Input`, `Keyframe`
  (`crates/ludion-protocol/src/replay.rs:51-53,110-137`). **This is the design gap:** the E4 brief's
  "sim-deterministic because the reload lands as a frame-stamped event, replay-recorded like input"
  describes a path that today records nothing, over state that today is WorldHash-invisible. Game
  docs invert both halves (decision 9).
- **The `AudioEvent` frame-stamped log is *regenerated from the sim*, not recorded — so it is not a
  usable mold for an external reload.** A system pushes into the `AudioQueue` resource
  (`crates/ludion-engine/src/audio.rs:26-50`); the engine drains it into a frame-stamped `audio_log`
  each tick (`engine.rs:324-340`); the log is **not written to LRP1** — a replay re-drives the same
  ticks and *rebuilds the identical log from sim state* (`audio.rs:14-16`). That works only because
  audio is a pure function of the sim. A doc reload is the opposite — an out-of-band edit the sim
  cannot regenerate — so it must be *recorded*, not regenerated (decision 9).
- **Free L-codes: L1105–L1109, L1119, L1120 in the L111x band; L1204+ in the L120x band**
  (`crates/ludion-core/src/codes.rs:183-275`: L1101–L1104 assets, L1110–L1117 mesh/voxel/collider/
  model-sheet, L1118 = E3's UI, L1201–L1203 import). E4 takes **L1119** and **L1120** (adjacent to
  the doc-format codes). Registration is three-part: a `const` in `mod code`, a `CodeEntry` in
  `CODES`, and a `DECLARED_CODES` entry (`codes.rs:2009-2101`) — a consistency test fails otherwise.
- **Docs flow through the guide + embedded-docs machinery; no new crate → no new topic.** The 24
  topics are the `docs/*.md` stems (`crates/ludion-cli/src/docs.rs`); relevant here are
  `ludion-scene`, `scene-format`, `ludion-engine`, `ludion`, `determinism`. The guide
  (`docs/engine-guide.md`) mirrors into `crates/ludion-cli/embedded-docs/` and every project's
  `LUDION-GUIDE.md` via `build.rs` + `new.rs`; the drift test `embedded_topics_match_the_docs_directory`
  (`crates/ludion-cli/tests/docs.rs:209`) gates it. Guide chapters are full `##` sections (the
  "Voxel models" / "Materials" / "Screen-space UI" shape, `docs/engine-guide.md`).
- **Non-visual waves ship examples + THE-test, not a gallery golden.** Phase 9 (also non-visual)
  produced "no goldens/hashes" and made "every public item documented *with a runnable example*"
  plus the THE-test its proof (`Plan/phase-9-hardening.md:80,89,135`). E4 follows suit: the
  cross-cutting "gallery example with a blessed golden where the feature is visible" is satisfied by
  a runnable facade example + G4's content THE-test — docs do not render, so **there is no golden and
  zero golden churn by construction** (amendment, decision 10).

## Design decisions (settled by this plan)

1. **No new crate: generic doc machinery in `ludion-scene`, typed registration + `DocStore<T>` in
   `ludion-engine`, surface in `ludion`.** `ludion-scene` already owns all YAML/schema/canonical-emit
   and is type-agnostic (schema `Value`s + `Value`s, `lib.rs:6-17`); a doc kind is exactly that
   vocabulary. `ludion-engine` owns the typed side (`doc_kind::<T>`, the `DocStore<T>` resource, the
   content-root walk) because it is where `T`, the builder, and the world live.
   *Rejected:* a new `ludion-docs` crate — a new clippy denylist, a new crate-graph edge, and a new
   docs topic, all for machinery that is a *reuse* of `ludion-scene`'s private `Checker`/emit/`SchemaCursor`;
   mesh and voxel docs live in `ludion-scene` for precisely this reason.

2. **The mechanism is the schema-driven *component* path, not the hand-rolled mesh/voxel parse —
   amendment to the brief's "mesh/voxel is the template".** Parse = strict-YAML → `Value`
   (`yaml::parse_document`) + the existing `Checker` against `schema_for!(T)` (`validate.rs:177`) +
   `serde_json::from_value::<T>()`; emit = `serde_json::to_value(&doc)` + the generic schema-ordered
   value emitter (`emit.rs:110-217`). The `kind:` dispatch and byte-idempotence **goals** come from
   mesh/voxel; the validation + emit **machinery** comes from the scene-component path. The `Checker`
   and emit core (today `pub(crate)`) are exposed through two new `ludion-scene` entry points
   (decision 3).
   *Rejected:* hand-rolling parse/emit per kind (impossible — the engine does not know `T`'s fields;
   only the game does, via `schemars`) — this is the whole reason the generic path exists.

3. **Two `ludion-scene` entry points, one doc envelope.** `check_doc(source, kind, schema: &Value)
   -> Result<Value, Vec<Diagnostic>>` and `emit_doc(kind, value: &Value, schema: &Value) -> String`.
   The envelope is `format:` + `kind:` at the top, then `T`'s fields flattened at top level — the
   mesh/voxel shape:
   ```yaml
   format: 1
   kind: weapon
   name: crossbow      # ← T's own fields, validated against schema_for::<WeaponDoc>()
   damage: 12.0
   cooldown: 0.8
   ```
   `check_doc` parses to a `Value`, validates the envelope (`format == 1`, `kind` matches the
   argument, no stray top-level envelope keys → **L1119**), strips `format`/`kind`, validates the
   remaining map against `schema` with the existing `Checker` (**L1120**, JSON Pointer + nearest-match),
   and returns the field map for the caller to `from_value`. `emit_doc` writes the `format`/`kind`
   envelope then the field map in schema-declaration order via the generic emitter. Strict-YAML
   syntax errors reuse the existing `L1001`–`L1006` codes (the shared `yaml` layer).
   *Rejected:* nesting `T` under a `data:` key (extra indentation; diverges from the mesh/voxel
   top-level convention); folding `format`/`kind` into `T` (forces every game struct to carry engine
   envelope fields — leaky, and `schemars` would then validate them).

   **Amendment A1 (2026-11-01, Clockout request 0021):** a third, typed entry point
   `parse_doc::<T>(source, kind) -> Result<T, Vec<Diagnostic>>` runs the boot loader's steps (the
   L1257 reserved-field check, `check_doc`, then the deserialize) so a test or tool does not copy
   them. The engine's install/reload share its `deserialize_doc` step, so the boot and the test
   parse give the same value and the same L1120 help. Re-exported from the `ludion` prelude with
   `check_doc`, `emit_doc` and `code`.

4. **Discovery: eager load at boot, routed by the in-doc `kind:`.** At each world build the engine
   walks the content root (`engine.project_root()`), and for every `*.yaml` peeks `kind:`
   (`peek_kind`); a value matching a registered doc kind loads into that kind's `DocStore<T>`, a
   scene (no `kind`) or engine kind (`mesh`/`voxel`) is skipped. Layout is free — G4's
   `weapons/crossbow.yaml` works because the in-doc `kind: weapon` is authoritative (the mesh/voxel
   dispatch rule). A malformed doc fails boot with L1119/L1120, exactly as a bad scene fails today
   (fail-fast); discovery collects diagnostics across all files rather than stopping at the first.
   *Rejected:* lazy per-system load (non-deterministic load order; a bad doc would not fail fast);
   mandating `*.<kind>.yaml` filenames or a per-kind directory (G4 uses `weapons/crossbow.yaml`; the
   in-doc kind already disambiguates, as it does for mesh/voxel).

   **Amendment A2 (2026-11-01, Clockout request 0021):** `EngineBuilder::doc_text(key, text)` hands a
   doc in as text, merged over the walk by key (it replaces the file at that key or adds one; a
   shadowed file is not read). It takes the same kind dispatch, schema check, `validate_docs` and
   `L1315` count as a file doc, lives for the engine's life (reinstalled on `scene.load`, restore and
   district rebuild) and is builder-only. A recording carries each as a `DocReload` record ahead of
   the arm keyframe, which replay folds back into the builder (no format change); a live
   `asset.reload` of that key is ignored.

5. **`DocStore<T>` = `{ docs: BTreeMap<String, T>, generation: u64 }`, keyed by slash-normalized
   root-relative path.** `BTreeMap` satisfies the determinism denylist and pins iteration order (the
   `asset_cache.rs:40-42` precedent). The store is **not** hashed into `WorldHash` (resources never
   are); its numbers reach the hash only through the components game systems derive from it, so a
   reload's effect surfaces exactly at the frame those systems next read the store. Systems read
   `world.resource::<DocStore<T>>()` (`world.rs:288`); the store needs only `'static + Send` (the
   `Serialize + DeserializeOwned + JsonSchema` bound lives at registration, decision 6). `generation`
   is the "something changed" signal a system can compare against a cached value; it starts at 0 and
   increments on each applied reload.
   *Rejected:* storing raw `Value`s (loses the typed ergonomics G4 wants — systems read `&WeaponDoc`,
   not `Value`); hashing the store into `WorldHash` (redundant — the mediated component changes
   already move the hash — and it would force `T: Hash`).

6. **Registration: `EngineBuilder::doc_kind::<T>("weapon")`, held on `Engine`, reinstalled at every
   reset.** `T: Serialize + DeserializeOwned + JsonSchema` (the component bound, `ecs/src/lib.rs:100`).
   The builder captures `(kind, schema_for!(T), erased load/emit/insert closures)` into a persistent
   list on `Engine` (like `systems`, `engine.rs:85`); an `install_docs` step re-walks the content
   root and re-inserts+reloads each `DocStore<T>` at construction, `scene.load`, and `restore` — the
   `install_audio_queue` mold (`engine.rs:345-349`), because `SimDriver::new`/`restore` wipe all
   resources (`driver.rs:98-100`).
   *Rejected:* a generic `.resource::<R>(value)` builder seam (broader than E4 needs — speculative;
   `doc_kind` is the one seam G4 consumes, and it must own the content-root walk, which a bare
   resource insert cannot express); inserting the store once at `build` (it would vanish on the first
   `scene.load` or `restore`).

   **Amendment B (2026-11-01, Clockout request 0021):** `doc_singleton::<T>(kind)` registers a kind
   that must hold exactly one doc. The engine counts at every validator point (build, `scene.load`/
   `restore`/district switch, `--check-docs`, live `asset.reload`), chained ahead of the game's
   passes: zero or two-or-more is `L1315`, and a reload that adds a second doc is refused with the
   store and generation untouched. `DocStore::single()` reads the one doc on any store. No wire
   command, no hash surface.

7. **`--check-docs` on `ludion::run`, short-circuiting hosting — settling the flagged decision.**
   `run()` resolves `--scene`, then `if args.check_docs { return run_check_docs(builder, &scene) }`
   beside `lib.rs:182`. `run_check_docs` builds the engine via `builder.load_scene_file` (which
   carries the game's registered kinds and roots at the scene's directory), walks the content root,
   validates every doc of every registered kind, prints L1119/L1120 diagnostics to stderr, and exits
   non-zero on any — **no server bound** (the `run_profile` precedent, `lib.rs:312-359`). CI-able as a
   plain `cargo run -p <game> -- --check-docs`. `ludion check-scene` keeps owning only engine kinds
   (single-file, hard-rejects unknown kinds, `check_scene.rs:37-45`).
   *Rejected:* **games emit JSON Schema for external checking** — (a) an external JSON-Schema tool
   cannot produce L-coded, JSON-Pointer, nearest-match diagnostics (`validate.rs` `Checker` +
   `suggest::nearest`), and "errors are the UX budget"; (b) strict-YAML rejections
   (anchors/tags/block scalars) are inexpressible in JSON Schema, so the engine's strict-YAML pass is
   needed regardless; (c) it forks the validator — CI would check with tool A while the running
   engine validates with tool B, and they would drift; (d) `--check-docs` reuses the **exact
   in-process validation the live engine runs**, so CI and runtime agree by construction, and the
   `--profile`/`--profile-check` short-circuit (`lib.rs:96-124,182`) means it is one more mode with
   zero new infrastructure. Also rejected: extending `check-scene` to game kinds — it is
   engine-owned and cannot know a game's types without loading the game binary, which is the whole
   reason the check lives in the facade.

8. **Hot reload rides the existing `asset.reload` — no wire change (26 methods, `schema_hash`
   unmoved).** The watcher already forwards every changed path as `Command::AssetReload`
   (`server/src/lib.rs:267-271`); the handler ignores paths the CPU asset cache does not hold
   (`asset.rs:37-40`). It is extended with a sibling branch: a path matching a registered doc kind
   reloads that doc into its `DocStore<T>`, bumps the store's `generation`, and pushes the existing
   `asset.reloaded` event (`asset.rs:45`). Docs are not in the CPU `AssetCache` (images/meshes/voxels
   only, `asset_cache.rs:43`) — the doc reload is its own path in the same handler.
   *Rejected:* a new `doc.reload` wire method — the brief expects no wire change, and the
   path-carrying `asset.reload` already lands at a frame boundary; additive-only holds.

9. **Doc reload determinism: docs are sim-visible state; reload is recorded *like input*, not
   regenerated *like audio* — the design gap, fully resolved.** The brief's claim is checked against
   a path (`asset.reload`) that today records nothing (`recording.rs:64`) over state that today is
   WorldHash-invisible (`asset_cache.rs:22-24`); game docs invert both, so this wave *makes the claim
   true* rather than inheriting it. The resolution has three parts:
   - **Boot is deterministic and reproducible within the repo.** The `DocStore` is seeded at world
     build by walking the content root (decision 4). A replay re-reads docs from the content root at
     boot exactly as it re-reads the scene file and its mesh/voxel/texture assets — none of which are
     inlined — so `WorldHash` from frame 0 reproduces because docs are committed, version-controlled
     content.
   - **Live reload is deterministic for free.** A doc `asset.reload` drains at a frame boundary
     before that frame's tick (`live.rs:184-197`); the `DocStore` swap + `generation` bump apply
     there, so two runs applying the same reload at the same frame produce identical `WorldHash`
     traces, diverging from a no-reload run **exactly at the reload frame** (the brief's "unchanged
     until the event frame"). No LRP1 change is needed for this property.
   - **Recorded reload is recorded, not regenerated.** Because a doc reload is an *external* mutation
     the sim cannot regenerate (unlike `AudioEvent`s, which replay rebuilds from the sim,
     `audio.rs:14-16`), when recording is armed the drained doc reload is written to LRP1
     frame-stamped **with its new canonical doc text inline**, and replay reapplies the identical
     swap at the identical frame. This flips `recording.rs:64`'s "AssetReload → not recorded" for the
     **doc case only** (texture/mesh/voxel reloads stay render-only and unrecorded — still
     WorldHash-invisible). The LRP1 record is additive; old `.lrp` keep decoding (the E2
     format-stability precedent, notebook rev 152–159). The wire `asset.reload` stays path-only —
     recording is an LRP1-format concern, not a wire method.
   *Rejected:* (a) keeping the reload render-only/unrecorded (today's behavior) — a recorded session
   that reloads a doc mid-run would diverge from its own replay, a P0 determinism hole; (b)
   regenerating the reload from the sim like audio — impossible, the sim cannot know a file changed;
   (c) forbidding reloads while recording — a bizarre "can't tune while recording" restriction.
   *Deferred with trigger:* inlining the **boot** doc set into the `.lrp` (the Phase-5 scene-text
   precedent) for standalone portability — trigger: sharing a game `.lrp` outside its repo, the same
   gap committed assets already have.

10. **Additive-only and non-visual, confirmed.** No wire change (26 methods, `schema_hash` unmoved —
    reload rides `asset.reload`, decision 8); **no new component** (the `DocStore` is a resource — no
    drift gate, `drift.rs:74` iterates components only; no builtin-registry append; no
    pinned-order/world-hash re-bless); **zero golden churn** (docs do not render — the non-visual
    Phase-9 posture, `phase-9-hardening.md:80,89`: a runnable facade example + G4's THE-test are the
    proof, no gallery scene, no golden); two new L-codes (**L1119**, **L1120**); one additive LRP1
    record (doc reload, old `.lrp` decode unchanged). **If any wave discovers otherwise, that is a
    flagged decision, not a drive-by.**

## Waves

Ordering rationale: the pure, engine-free machinery first (Wave 1 — parse/validate/emit against a
provided schema) → typed registration + boot load, the data model everything else names (Wave 2) →
hot reload + the full determinism/replay story on top (Wave 3) → the CI-able `--check-docs` mode
(Wave 4) → the runnable example, guide, docs, and exit (Wave 5). Each wave is independently
landable, one commit, ending in `node scripts/check.mjs` green.

### Wave 1 — generic doc machinery in `ludion-scene` (`check_doc`, `emit_doc`, L1119/L1120)

- A `doc` module in `ludion-scene`: `check_doc(source, kind, schema: &Value) -> Result<Value, Vec<Diagnostic>>`
  (strict-YAML → `Value` via `yaml::parse_document`; envelope validation → **L1119**; strip
  `format`/`kind`; run the existing `Checker` on the remainder against `schema` → **L1120** with JSON
  Pointer + `suggest::nearest`) and `emit_doc(kind, value: &Value, schema: &Value) -> String` (the
  `format`/`kind` envelope + the generic schema-ordered value emitter). Expose the minimum of
  `validate.rs`'s `Checker` and `emit.rs`'s value core needed to reuse them (crate-internal
  visibility widened, no behavior change to scene validate/emit).
- L1119 ("game data-doc structure is invalid" — envelope: missing/wrong `format`/`kind`, unknown
  envelope key, unregistered kind) and L1120 ("game data-doc does not match its `<kind>` schema" —
  the `Checker` output) registered three-part in `crates/ludion-core/src/codes.rs`, each message
  carrying the kind name.
- **Exit gate (mechanized):** unit tests — envelope missing/bad `format`/`kind` → L1119 with a
  pointer; unknown envelope key → L1119 nearest-match; a schema mismatch (wrong type, out-of-range,
  typo'd field) → L1120 with `/field` pointer + nearest-match; a strict-YAML syntax error surfaces
  the existing `L100x`. A **byte-idempotence proptest** over a hand-built test schema + conforming
  generated `Value`s (`emit_doc → check_doc → emit_doc` byte-equal, the `mesh_roundtrip.rs:87` mold).
  `ludion explain L1119`/`L1120` work. `node scripts/check.mjs` green (existing scene emit/validate
  proptests stay green — the exposed core is unchanged for components).

### Wave 2 — `doc_kind::<T>` registration + `DocStore<T>` resource + boot load

- `EngineBuilder::doc_kind::<T>("weapon")` (the new seam, decision 6): captures
  `(kind, schema_for!(T), erased loader/inserter)` into a persistent list on `Engine`. `DocStore<T>`
  type (decision 5) in `ludion-engine`. `install_docs` walks `project_root`, peeks `kind`, calls
  `ludion_scene::check_doc` against the captured schema + `from_value::<T>()`, and inserts each
  `DocStore<T>`; reinstalled at construction, `scene.load`, and `restore` (the `install_audio_queue`
  mold). Prelude re-exports `DocStore`.
- **Exit gate (mechanized):** a test fixture (in `ludion-engine` tests, or a tiny example crate)
  registers a doc kind and boots from a content root of valid docs → `DocStore` populated in BTreeMap
  order; a malformed doc fails boot with L1119/L1120; the store **survives a `scene.load` and a
  `restore`** (the reinstall proof); `world.resource::<DocStore<T>>()` reads the typed docs; two
  boots produce identical `DocStore` contents and identical `WorldHash` (boot determinism). `node
  scripts/check.mjs` green.

### Wave 3 — hot reload + the determinism/replay story

- Extend the `asset::reload` handler (`commands/asset.rs`) with the doc branch (decision 8): a path
  matching a registered doc kind reloads via `check_doc`, replaces the `DocStore` entry, bumps
  `generation`, pushes `asset.reloaded`. Recording (decision 9): when recording is armed, the drained
  doc reload writes a frame-stamped LRP1 record carrying the new canonical doc text inline; replay
  reapplies it at the same frame. Update `recording.rs`'s policy for the doc case; texture/mesh/voxel
  reloads stay unrecorded.
- **Exit gate (mechanized):** the **live** hot-reload determinism test — boot with doc v1, run to
  frame N, apply the doc reload, assert `generation` bumped and the `WorldHash` trace identical to a
  no-reload run through frame N-1 and divergent at frame N (the brief's property); two reload runs
  match. The **record → replay** test — a recorded session with a mid-run doc reload reproduces its
  `WorldHash` trace from the committed `.lrp` **on a checkout whose on-disk doc differs** (the inline
  content proof, the portable-replay mold). Every pre-E4 committed `.lrp` still decodes; wire surface
  still **26**, `schema_hash` unmoved. `node scripts/check.mjs --full` green (the determinism canary).

### Wave 4 — `--check-docs` facade mode

- `--check-docs` flag on `RunArgs`; `run_check_docs` short-circuit beside `lib.rs:182` (the
  `run_profile` mold, decision 7): build the engine via `load_scene_file`, walk the content root,
  validate every registered kind, print diagnostics to stderr, return the exit code — no `Server::bind`,
  no "listening" line.
- **Exit gate (mechanized):** `--check-docs` on a valid content root exits 0 with a one-line summary;
  a malformed doc → L1119/L1120 on stderr and a non-zero exit; no server is bound (no "listening on"
  on stdout — asserted); a CLI smoke in `check.mjs`'s smoke set (the `serve`/`profile` smoke
  precedent). `node scripts/check.mjs` green.

### Wave 5 — facade example, guide chapter, docs topics, exit

- A runnable facade **example** (`crates/ludion/examples/data_docs.rs`): register a `WeaponDoc` kind,
  load it under a content root, a system that drives a component from `DocStore<WeaponDoc>` and
  responds to `generation`, with a behavioral **two-run WorldHash test** (the `hud_bar.rs` /
  `cursor_aimed_turret.rs` mold) — the copyable G4 pattern. (Vigil's weapon/enemy docs are G4's work,
  not E4's — E4 ships the pattern.)
- Engine-guide chapter **"Game data-docs"** (placed with the content-authoring chapters, after "Voxel
  models"): the doc envelope + `doc_kind::<T>` registration, `DocStore<T>` access + the `generation`
  signal, the hot-reload tuning loop, `--check-docs`, canonical emit, and the non-goals
  (no cross-references/ID resolution — games resolve names themselves; no inheritance/templating).
  Docs topics updated via the `ludion-docs` skill: `ludion-scene` (generic `check_doc`/`emit_doc`),
  `ludion-engine` (`doc_kind` + `DocStore` + reload recording), `ludion` (facade `doc_kind` +
  `--check-docs`), `scene-format` (the doc envelope + kind dispatch), `determinism` (doc reload
  recorded, texture/mesh reload not). No new crate → no new topic; embedded-docs drift satisfied by
  rebuild.
- **Exit gate (roadmap "ready"):** wire surface still **26** + `schema_hash` unmoved (decision 10);
  no golden churn; example + behavioral test green; guide chapter + docs topics shipped
  (`embedded_topics_match_the_docs_directory` green); `--check-docs` documented; `node scripts/check.mjs`
  green. **Handoff to G4:** `kind: weapon`/`kind: enemy` are registrable, the tuning loop lands via
  hot reload, and the content THE-test can run against shipped docs. **G6 forward-compat:** `kind: npc`
  / `kind: quest` are additional `doc_kind::<T>(…)` calls with no engine change — the generic design
  precludes no later kind. **No human gate is owned by E4** — M4's G4 gate ("two runs with visibly
  different builds") judges the facility in anger.

## Explicitly out of scope (triggers recorded)

- **Per-process routing cache** (Clockout 0029 point 2: `(path, len, mtime)` → routing decision, game
  docs always re-read) — deferred; trigger: the walk's own time (`docs_ms`) still a measurable share of
  boot after the textual sniff on a content root of Clockout's size. Measured on vigil, `--check-docs`
  wall time median of 3: 1183 ms before the sniff, 418 ms after.
- **Doc cross-references / ID resolution** (a doc naming another doc by id) — games resolve names
  themselves; trigger: content volume that makes hand-resolution hurt (the brief's non-goal).
- **Doc inheritance / templating** (a base weapon others extend) — trigger: the same content-volume
  pressure.
- **Editor doc UI** (a panel to browse/edit docs) — the inspector edits components, not docs;
  trigger: hand-editing docs in a text editor becoming a recurring friction (watch G6).
- **Standalone `.lrp` portability via boot-doc inlining** — a committed `.lrp` reproduces within its
  repo (docs re-read from the content root, like every other asset); trigger: sharing a game `.lrp`
  outside its repo (the same gap committed assets already have).
- **A generic `.resource::<R>(value)` builder seam** — `doc_kind` is the one seam G4 consumes;
  trigger: a second, non-doc resource a game needs to register at build.
- **Recording texture/mesh/voxel reloads** — they are render-only and WorldHash-invisible
  (`recording.rs:50-52`); trigger: none foreseen.
- **`--check-docs` across multiple content roots / a parsed `ludion.toml`** — the root is the
  scene's directory today; trigger: a game with more than one content root, or a real need to parse
  the manifest.

## Risks

- **The LRP1 doc-reload record is a replay-format change.** Contained by making it additive and
  gating on both an old-`.lrp`-still-decodes test and the record→replay reproducibility test (Wave 3),
  the exact posture E2 used to grow `InputEvent` at format 1 (notebook rev 152–159). Whether the new
  record needs a `REPLAY_FORMAT_VERSION` bump is settled at build against `replay.rs`'s byte layout;
  either way old replays must keep decoding.
- **Exposing `ludion-scene`'s `Checker`/emit core must not perturb scene bytes.** The generic
  validate/emit are shared with the scene-component path; widening their visibility must be
  behavior-neutral. Contained by the existing scene emit/validate proptests and every committed scene
  golden staying byte-identical (Wave 1 gate).
- **`DocStore` reinstall on every reset must preserve semantics.** A `restore`/`scene.load` rebuilds
  the store from the content root and resets `generation` to 0 — correct (a restore is a fresh boot
  of that world), but the reinstall must not silently drop a kind or a doc. Contained by the Wave 2
  survive-a-reset test.
- **Boot fail-fast surfaces every bad doc, not just the first.** A game with many docs must get all
  diagnostics in one pass (the scene-validation posture), or an agent tuning content iterates blind.
  Contained by having discovery collect diagnostics across files (decision 4) and a Wave 2 test with
  two independently-broken docs.
- **Content-root walk cost / misfire.** Peeking `kind:` on every `*.yaml` at boot must skip scenes
  (no `kind`) and engine kinds (`mesh`/`voxel`) cleanly and stay boot-time-only. Contained by the
  peek-then-route rule (decision 4) and the Wave 2 discovery test over a mixed content root. Engine kinds
  are sniffed textually, never parsed (Clockout 0029).
- **Sequencing vs open gates.** E4 adds no wire method, no component, and no golden; it shares only
  the guide/docs files and one additive LRP1 record with the open Phase 8/9/M0/M1 human gates, so it
  is safe to run with those queued. Per roadmap rule 2, E4 is the engine wave in flight during M3
  (its consumer is G4/M4); the user directed its start on 2026-07-16 with the M1/M2 human gates and
  G3 still queued — the E3 precedent (rule-2 throttle overridden by the user, gates unaffected).
