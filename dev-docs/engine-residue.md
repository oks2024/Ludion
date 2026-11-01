---
description: Scene residue internals: capture and re-apply inside perform, the session map, and ResidueStore
---

# Engine: scene residue

**Scope:** The session-side memory a district keeps between visits — where capture and re-apply sit
inside the transactional switch, the migration and identity policy, and the read-only mirror. The
switch itself, the doc stores and the profile are `ludion devdocs engine-content-kinds`; the
recorded-vs-regenerated contract is `ludion devdocs determinism`; the authoring format is
`ludion docs scene-residue`.

## Layout

- `crates/ludion-engine/src/transition.rs` — capture and re-apply inside `perform`, plus
  `RESIDUE_MAX_SCENES`/`RESIDUE_MAX_RECORDS_PER_SCENE` and `L3026`/`L3028`/`L3030`/`L3031`.
- `crates/ludion-engine/src/residue_store.rs` — the `ResidueStore` resource, the per-world read-only
  mirror of `SessionState.residue`.

## Dataflow

`SessionState.residue` is a `StableMap<String, SceneResidue>` keyed by scene `name:`, engine-side
session state the same shape as `doc_reloads`: never hashed, never recorded, snapshotted additively
(`SNAPSHOT_FORMAT` 1's `residue` field). `perform`'s own step order is residue's authority: step (b′)
captures every `Residue`-marked entity's bag (all but `Name`) out of the departing world *before* the
target loads, held locally so a rejected switch (`L3018`/`L3019`) leaves the session map untouched;
step (c′) re-applies the arriving scene's records — named ones matched onto the freshly instantiated
entity of the same `Name` in ascending-`Name` order, then anonymous ones spawned in capture order —
before the anchor lookup, so carried entities still land on the highest `EntityId`s (a contract, since
neither step despawns; `transition_residue.rs` pins it). A record is migrated before it touches that
world, `snapshot::migrate_component`'s policy at record granularity: a bag this build can no longer
read is dropped whole (`L3030` — residue outlives a component change through a snapshot, and the
in-place branch strips its entity first), and a name two entities of the arriving scene share lands on
the first and reports both ids (`L3026`). A named record with **no** counterpart splits on
`ResidueRecord.authored`, stamped at capture from `SessionState.authored_names` (the live document's
`Name` set — after a `restore` derived lazily from the restored inline scene text, `#[serde(default)]`
false in a snapshot): authored means renamed,
so it is dropped and reported (`L3031`); code-spawned means the document never named it, so it respawns
with its `Name` — vigil's per-sector census depends on that, and the default keeps every older snapshot
on the respawn side. Step (f) commits the departing capture into the session map only after every
fallible check has passed, holding both `RESIDUE_MAX_SCENES` (LRU eviction by `last_visit` frame) and
`RESIDUE_MAX_RECORDS_PER_SCENE` (capture-order truncation), each reported `L3028`.

Entity references inside a bag are recorded by identity (`ResidueRecord.refs`: a record index or a
`Name`; additive in the snapshot) through the same `remap_refs` walker the carry uses, and the
components holding them are inserted only after both spawn passes, once every landed id is known. An
unresolvable one is cleared (optional) or its component dropped (required), reported `L3053`.

## Rules

- **The triad mirrors `doc_reloads`.** `SessionState::reset` (`scene.load`) clears the map, `carry` (a
  switch) is a no-op because `perform` itself is what writes it, and `adopt` (`restore`) replaces it
  wholesale with the snapshot's map, never merges.
- **`ResidueStore` is a read-only per-world mirror**, reinstalled by `install_residue_store` at every
  world (re)build (construction, `scene.load`, a switch, `restore`) so a district reads back what
  another scene kept without a resource that world resets would wipe. Its one write, `clear()`, is a
  request `drain_residue_clear` honours at the frame boundary **before** `drain_scene_switch`, so a
  clear and a switch requested in the same frame compose — the run forgets every prior district and
  then immediately re-captures the one it is leaving now.
- **The engine stores facts** — component bags, captured verbatim — and never interprets them; a game
  recomputes what they mean (a door open, a spawner destroyed) the same way it always did.

## Verify

```
cargo test -p ludion-engine --test transition_residue  # capture/apply, bounds, ResidueStore mirror
cargo test -p ludion-engine --test transition_snapshot
cargo run -p ludion --example scene_residue
```
