---
description: Data-docs, player profiles and scene transitions: the three persistent registrations on Engine
---

# Engine: data-docs, profiles and transitions

**Scope:** The three subsystems a game registers on the builder and the engine reinstalls at every
world reset — typed `kind:` data-docs, the singleton player profile, and in-run district switches —
plus the typed system-query declarations. All four share one shape, stated under Rules. The generic
text machinery underneath is `ludion devdocs ludion-scene`; the authoring formats are
`ludion docs data-docs`, `ludion docs player-profiles` and `ludion docs scene-transitions`; what a
switch leaves behind is `ludion devdocs engine-residue`; the recorded-vs-regenerated contract is
`ludion devdocs determinism`.

## Layout

- `crates/ludion-engine/src/docs.rs` — `DocStore<T>`, `DocKind`, `load_doc_stores`, `content_root_yaml` (the one content-root walk, shared with `--check-docs`), `reload_doc`, and `is_engine_kind` (the walk's skip test over `ludion_scene::KINDS`, the one table of engine kinds — `ludion-scene`'s document formats plus `profile`, `touch-layout` and the registered `pattern`, checked against the parsers by a unit test in `kinds.rs`; a skipped kind is one no loader, component or source variant reads). **A new kind is one `KINDS` row** (plus its runner, where it wants a rich summary).
- `crates/ludion-engine/src/player_profile.rs` — `Profile<P>`, `ProfileKind`, the lenient migration (`diff_fields`), `write_atomic`, `ProfileSaveEntry`.
- `crates/ludion-engine/src/transition.rs` — the `SceneSwitch` resource (+ `SwitchTo`) and crate-private `perform`; `L3018`/`L3019`/`L3020`/`L3026`/`L3027`/`L3028` live here.
- `crates/ludion-engine/src/residue_store.rs`, and residue's half of `transition.rs` — `ludion devdocs engine-residue`.
- `crates/ludion-engine/src/query_drift.rs` — `check_query_drift_replay`/`check_query_drift_with` → `QueryDriftReport`.

## Rules

- **One persistent registration, reinstalled at three reset points.** `doc_kind::<T>` and
  `profile::<P>` capture monomorphized `fn`-pointer loaders into a list (`Vec` for docs, `Option` for
  the profile) held on `Engine` like `systems` — because `SimDriver::new`/`restore` wipe **all**
  resources. `install_docs`/`install_profile` run at construction, `scene.load` and `restore` (the
  `install_audio_queue` mold). Miss a reset point and the store silently vanishes mid-session.
- **All three are resources, never components.** No registry entry, no `WorldHash` surface, no drift
  gate. Their numbers reach the hash only through the components a game's systems derive from them —
  which is precisely why the *inputs* must travel with a recording.
- **Zero registered kinds means zero IO.** No walk, no file reads.
- **Diagnostics collect, they do not fail fast.** Discovery gathers across every file and reports at
  the end, so one bad doc does not hide four others.
- **Semantic validation is the game's rules, run by the engine.** `EngineBuilder::validate_docs`
  pushes a `DocValidator` (`Box<dyn Fn(&World) -> Vec<Diagnostic> + Send + Sync>`) onto
  `Engine::doc_validators`; the tail of `install_docs` runs them all in order over the freshly
  installed stores, returns their `Error`s — so a broken cross-reference (`L1144`) refuses a build or
  a load exactly as `L1119` does — and routes their `Warning`s into `Diagnostics` plus
  `Engine::doc_validation_warnings()`, which is what a host with no tick yet (`--check-docs`) reads.
  `reload_doc_from_disk` runs the passes before and after the swap: an `Error` the swap newly causes
  (in any kind) is undone and refused with the old doc live; warnings and pre-existing errors print
  once into `Diagnostics` and the reload applies.

## Dataflow

**Game data-docs** (`docs.rs`) are the typed side of `ludion-scene`'s generic `check_doc`/`emit_doc`.
`load_doc_stores` walks `project_root`, routes each `*.yaml` by its `kind:`, skips scenes (no `kind`) and
engine kinds — `ludion_scene::sniff_kind` reads the key textually and those files are never parsed, so a
malformed engine-kind or kind-less file is reported by its loader / `check-scene` (or when its scene
loads), not by the boot walk; anything the sniff cannot decode is full-parsed as before — validates a registered kind, and inserts each kind's `DocStore<T>` resource — empty if
it matched nothing. An unregistered `kind:` is an `L1119` with a nearest match when a game registered any kind of its own; on an engine carrying only engine-owned kinds (`patterns()` — the built-in engine behind `serve`/`sim-hash`) the file is skipped with an `L1155` warning, read back from `Engine::doc_skip_warnings()` (one per file) or `Engine::doc_skip_summary()` (one for the walk, naming the kinds and their counts). A host prints the summary: a game's scene directory holds tens of its own docs, and a block per file buries whatever the host says next. The store is keyed by
slash-normalized content-root-relative path in `BTreeMap` order, with a `generation` counter a system
compares to react to reloads. Every registered kind's captured `schema_for!(T)` is published on
`engine.describe` under `documents` (`{"<kind>": <schema>}`, sorted by kind; `{}` on an engine that
registered none) — the *same* document `check_doc` validates against, so a client that builds an
authoring form from it cannot disagree with the validator.

A kind registered with `doc_singleton` must hold exactly one doc (`L1315`, checked wherever the doc
validators run; `DocStore::single`). `EngineBuilder::doc_text(key, text)` supplies a doc inline, through
the same dispatch and validators as a file; it is reinstalled on every scene load, restore and district
rebuild, and a recording writes it as a `DocReload` record ahead of the first keyframe.

**The doc-reload recording seam** is the one place `asset.reload` is recorded. `commands/asset.rs`'s
`resolve_doc_path` routes an incoming path: a `*.yaml` under the root whose `kind:` is registered
takes the doc branch — `reload_doc` re-validates and upserts, bumps `generation`, pushes
`asset.reloaded`, and returns the new **canonical** text; a path the CPU asset cache holds
(image/mesh/voxel) stays the render-only path; anything else is a silent no-op. Because a doc is
sim-visible state a replay cannot regenerate, `record_kind` records *this* case and no other
`AssetReload`: a frame-stamped `DocReload` record carrying the canonical text inline, which replay
reapplies at the identical frame, off disk.

**Player profiles** (`player_profile.rs`) are the singleton twin of the doc machinery, differing in
exactly one way that matters: the load is **lenient** where `check_doc` hard-rejects. Serde defaults
fill missing fields, `diff_fields` reports drift, and an unhonorable file (bad envelope, retyped
field, newer `format`) is backed up to a content-hashed `.bak.<hash>` sibling and defaulted — an
`L1121` warning on `Engine::profile_migration()`, never a boot failure. A game's `profile.save()` sets
a dirty flag that `drain_profile` clears at the frame boundary beside `drain_audio`, pushing a
frame-stamped `ProfileSaveEntry` into `profile_save_log` — **regenerated by replay, never recorded**
(the audio contract) — and writing to disk **only** when a host called
`EngineBuilder::persist_profile` (which also gates the boot backup-rename; a failure is a non-fatal
`L1122` on `profile_save_error`). `restore` installs the snapshot's *embedded* profile via
`install_profile_from_text`, not disk.

**Scene transitions** (`transition.rs`) are `scene.load`'s continuity twin. `drain_scene_switch` —
the third frame-boundary drain, after `drain_audio`/`drain_profile` — takes the request and calls
`perform`, which is **transactional**: it captures the run's continuity (`*engine.rng()`, `frame`,
`run_state`, `speed`, `emit_profile_text`) and each carried entity's component bag from the *live*
world, then builds the target district into a **local** world. A load failure (`L3018`), a missing
anchor (`L3019`) or an anchor carrying no `Transform` (`L3029` — refused rather than defaulting to the
origin) discards that local value and the live driver is never touched — the sim runs on in
the current district. Every outcome goes through `Engine::report_transition`: it stores the diagnostic
on `last_transition_error` **and** prints it on `drain_diagnostics`' two sinks (an `Event::Log` at the
code's registered severity, plus stderr), latched on the `code: message` line so a game re-requesting a
rejected switch every frame reports once. `drain_profile`'s `L1122` prints the same pair. On success `install_switched` runs
`engine.resource_carriers` (each `EngineBuilder::carry_resource::<T>()` mover, while both worlds are
still alive — the switch is the one rebuild a game resource may cross; `scene.load` and `restore` run
none), swaps the driver in, reinstalls the engine-owned
resources, re-applies this session's `session_doc_reloads` off-disk on top of the freshly-walked
content root, carries the in-memory profile, sets `engine.scene = SceneRef::Inline{target}`, and —
unlike `scene.load`/`restore` — **leaves the session logs intact**, because a switch is mid-session.
Carried entities respawn at the anchor's `Transform.translation` (translation only; all else
verbatim); a carry id not alive at the switch frame is skipped and reported (`L3020`) rather than
aborting. Under recording, `recording::write_scene_switch` stores the district text just read;
under replay, `install_replay_scene_text` pre-loads a `path→text` map and the regenerated switch
resolves from it, never disk.

**Scene residue** (`transition.rs` + `residue_store.rs`) is `SessionState.residue`, captured and
re-applied inside `perform`'s own step order — `ludion devdocs engine-residue`.

**Typed system query declarations.** A system may declare the component queries it runs, chained on
the builder with the same tuple syntax the body uses:
`.system("fire", fire).query::<(&Loadout, &Transform)>().query::<(&mut Health,)>()`. Each `.query`
declares one shape for the most-recently-registered system; `.query` before any `.system` panics.
Declarations carry `TypeId`s until build, where they resolve against the registry — a declared type
that is not a registered component fails the build with `L1125`. `engine.describe`'s `systems[]`
entries then carry `fn_path` (the Rust module path, for jump-to-source — a module path, so no
filesystem leak) and `queries` (`{components: [{name, access}]}`, `access` = `read`/`write`); an
undeclared system simply has `queries: []`.

`query_drift.rs` is the *measured* half. It runs an instrumented replay with
`Engine::begin_query_recording` armed and compares each system's observed accesses — typed queries
**and** the per-entity `get`/`get_mut`/`insert`/`remove` shapes the recorder folds in — against its
declared shapes: `L1126` observed-but-undeclared is an error, `L1127` declared-but-never-observed a
warning. It shares `replay.rs`'s `prepare_replay`/`drive_actions`, so it cannot drift from a plain
replay, and recording accumulates across scene loads and district switches, so a cross-district
replay verifies every district's systems.

## Gotchas

- A doc store is a **resource**, so a system that forgets `world.resource::<DocStore<T>>()` returns
  `None` sees "kind not registered" and silently does nothing. That `None` is also the honest answer
  for a game that never registered the kind — the two cases are indistinguishable by design.
- A malformed *edit* mid-tune leaves the store **untouched** (`L1119`/`L1120` diagnosed), so a bad
  save never wipes live state. A malformed doc at *boot* fails the build.
- The `profile` kind is a `ludion_scene::KINDS` row, so the walk and `--check-docs` (both through
  `is_engine_kind`) skip a saved `save/profile.yaml`. A kind with no row is how a valid save starts
  failing doc checks.
- The switch request resource is excluded from `WorldHash` like `AudioQueue` — the switch is
  hash-visible only through the world it rebuilds.

## Verify

```
cargo nextest run -p ludion-engine --test it -E 'test(/^docs::/)'             # boot load, survive-reset, determinism
cargo nextest run -p ludion-engine --test it -E 'test(/^docs_reload::/)'      # hot-reload determinism + record→replay off-disk
cargo nextest run -p ludion-engine --test it -E 'test(/^transition::/)'       # carry, continuity, failure paths
cargo nextest run -p ludion-engine --test it -E 'test(/^(transition_replay|transition_snapshot)::/)'
cargo run -p ludion --example data_docs
cargo run -p ludion --example player_profile
```
