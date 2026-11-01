---
description: kind:-tagged game data files: the envelope, registering a kind, the tuning loop, checking in CI
---

# Game data-docs

**Scope:** Authoring a game's own declarative data (weapons, enemies, quests) as validated strict-YAML documents, and reading them from systems. Persistent save state is player-profiles; the shared YAML subset is scene-format.

A **game data-doc** is a `kind:`-tagged strict-YAML file holding a game's *own* declarative data —
weapons, enemies, items, quests — with L-coded pointer errors and nearest-match suggestions, a typed
in-world store, canonical byte-idempotent emit, and hot reload. You register a serde type as a
*kind*; every matching file under the content root loads, schema-validated, into a store your systems
read. **Numbers and archetype names live in text; behavior stays in Rust**, so retuning the armory
needs no recompile.

It is not a scene (no `kind:`, spawns entities) and not an engine asset (an engine kind such as `mesh`, `voxel`, `prefab`, `atlas`): it is
game data the *engine* has no built-in type for, so you teach it the type.

## The doc envelope

Every doc opens with a two-key envelope, then the registered type's own fields flattened at the top
level — the same top-level shape mesh/voxel use:

```yaml
format: 1
kind: weapon
name: crossbow      # from here down, the registered type's own fields
damage: 12.0
cooldown: 0.8
```

- `format: 1` — the document format version (the only version this engine reads; else `L1119`).
- `kind: weapon` — marks the file and routes it. The engine peeks this on every `*.yaml` under the
  content root: no `kind` is a scene (skipped), an engine kind (any row of `ludion_scene::KINDS`: `mesh`, `voxel`, `prefab`, `atlas`, …) is skipped, a
  registered kind loads, and an **unregistered** `kind` fails the boot with `L1119` and a nearest-match
  suggestion — a typo'd `kind:` is loud, never silently ignored. The exception is an engine with no
  game kind registered, such as the built-in one (`ludion serve`, `ludion sim-hash`): it hosts anyone's
  scene, so a foreign kind is skipped with an `L1155` warning — host through your game's binary to load it.
- `format` and `kind` are reserved at the top level: a registered type that declares a top-level field
  of either name is refused when the engine is built (`L1257`) — rename it (nested fields are fine).
- Everything after the envelope is your type's fields. They validate against the type's JSON Schema: a
  wrong type, an out-of-range value, a missing required field, or a typo'd field name is `L1120` with
  a JSON Pointer into the offending field and a nearest-match suggestion — like a scene component's
  `L1014`. Strict-YAML syntax violations (anchors, tags, flow style) surface `L100x`, as in a scene —
  reported, not skipped.

Layout under the content root is free — `weapons/crossbow.yaml`, `enemies/goblin.yaml`, one file per
doc — because the in-file `kind:` is what routes each file, not its path or name. The content root is
the scene file's own directory (where `ludion::run` roots a hosted game; `Engine::builder()` roots at
`.` until `project_root(...)`), the same root every asset resolves against.

## Registering a kind and reading the store

A doc type carries the same bound as a component — `Serialize + Deserialize + JsonSchema` — because it
round-trips through canonical text and publishes a schema for validation. You register it on the
builder with `doc_kind::<T>("weapon")`; the engine walks the content root at every world build and
loads each matching file into a `DocStore<T>` resource. Unlike a component, a doc kind adds **no**
registry entry and no `WorldHash` surface: the store is a resource your systems read, and its numbers
reach the hash only through the components those systems derive from it.

```rust,ignore
use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One weapon, authored as a `kind: weapon` doc under `weapons/*.yaml`.
#[derive(Serialize, Deserialize, JsonSchema, Clone)]
struct WeaponDoc { name: String, damage: f32, cooldown: f32 }

/// Names the weapon an entity wields by the doc's `name`; the game resolves it against the store.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Default)]
#[serde(default)]
struct Loadout { weapon: String }

/// Stats derived from the equipped weapon — an ordinary hashed component, so the doc's numbers reach
/// the WorldHash here. `applied_generation` caches the store generation they were derived at.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Default)]
#[serde(default)]
struct WeaponStats { damage: f32, cooldown: f32, applied_generation: Option<u64> }

/// Re-derives WeaponStats only when the store's `generation` differs from the cached one, so a hot
/// reload is noticed without re-deriving every tick.
fn equip_weapons(world: &mut World, _ctx: &SystemCtx) {
    let Some(generation) = world.resource::<DocStore<WeaponDoc>>().map(|s| s.generation()) else {
        return; // the `weapon` kind isn't registered — nothing to equip from
    };
    // Resolve stale entities into owned numbers first, so the store + read borrows end before writing.
    let entities: Vec<EntityId> = world.entities().collect();
    let mut updates: Vec<(EntityId, f32, f32)> = Vec::new();
    for e in entities {
        let Some(loadout) = world.get::<Loadout>(e).unwrap() else { continue };
        let current = world.get::<WeaponStats>(e).unwrap()
            .is_some_and(|s| s.applied_generation == Some(generation));
        if current { continue; }
        let weapon = loadout.weapon.clone();
        let store = world.resource::<DocStore<WeaponDoc>>().unwrap();
        if let Some(doc) = store.iter().map(|(_, d)| d).find(|d| d.name == weapon) {
            updates.push((e, doc.damage, doc.cooldown));
        }
    }
    for (e, damage, cooldown) in updates {
        if let Some(s) = world.get_mut::<WeaponStats>(e).unwrap() {
            s.damage = damage; s.cooldown = cooldown; s.applied_generation = Some(generation);
        }
    }
}

/// Register the kind alongside the game's components and systems.
fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| { reg.register::<Loadout>("Loadout")?; reg.register::<WeaponStats>("WeaponStats") })
        .doc_kind::<WeaponDoc>("weapon")
        .system("equip_weapons", equip_weapons)
}
```

`DocStore<T>` is keyed by slash-normalized, content-root-relative path (`weapons/crossbow.yaml`) and
iterates in `BTreeMap` order — pinned and reproducible, so a system walking it sees the same sequence
every run. Register several kinds with repeated `doc_kind` calls (`doc_singleton` for a kind that must hold exactly one doc). The complete example — a headless
demo with a two-run `WorldHash` test — is `crates/ludion/examples/data_docs.rs` (`cargo run -p ludion
--example data_docs`).

## The tuning loop, and its determinism

Data-docs hot-reload through the same watcher as every other asset. Under `ludion dev` (or any
attached viewer), saving a `weapons/*.yaml` forwards an `asset.reload` for that path; the engine
re-validates the file, upserts it into the `DocStore` (a malformed edit is a diagnosed `L1119`/`L1120`
that leaves the store **untouched** — a bad save mid-tune never wipes your state), and bumps the
store's `generation`. The reload applies at a frame boundary, so the next tick's systems see the new
numbers. The loop is: edit a number → save → watch it land, no restart, no recompile.

Because a data-doc is *sim-visible* state (its numbers flow into hashed components), a doc reload is
**deterministic and replay-recorded** — unlike a texture or mesh reload, which is render-only and
never recorded. When recording is armed, an applied doc reload is written to the replay with its new
canonical text **inline**, and replay reapplies the identical swap at the identical frame. So a
recorded session that retuned a weapon mid-run reproduces its exact `WorldHash` trace on replay —
even from a checkout whose on-disk doc has since changed, because the reloaded text travels in the
`.lrp`, not a file reference. Boot docs are re-read from the content root (like every committed
asset), so a replay reproduces within its repo; sharing a `.lrp` outside its repo is the same gap
committed assets already have.

## Checking docs in CI

`--check-docs`, cross-doc rules (`validate_docs`) and one-doc kinds (`doc_singleton`): `ludion docs data-docs-checks`.

## Reading a doc in a test or tool

`parse_doc::<T>(source, kind)` (in the prelude) reads one doc into its type with the boot loader's
steps and codes (L1257, L1119, L1120, strict-YAML L100x):

```rust
let weapon = parse_doc::<Weapon>(&std::fs::read_to_string(path)?, "weapon")
    .unwrap_or_else(|errors| panic!("{path}: {errors:?}"));
```

`validate_docs` does not run here — it needs a `World` and runs at boot, `scene.load` and live
reload; a cross-doc rule needs the engine. No count rule applies either.

## A variant doc in a test

```rust
vigil::game().project_root(&empty_root)
    .doc_text("weather/rain.yaml", &text)
    .build()?
```

`doc_text(key, text)` replaces the file doc at that content-root-relative key, or adds one. It takes
the same kind dispatch, schema check and `validate_docs` as a file, and fails `build()` the same way;
a `doc_singleton` kind stays at one doc if the key matches, and a new key gives `L1315`. It lives for
the engine (surviving `scene.load` and restore) and is recorded, so replays reproduce it. An
`asset.reload` of that key is ignored. The root is still walked: use an empty scratch root when the
real tree must not load.

## Writing docs back canonically

A tool that *generates or rewrites* docs (an importer, a balance script) emits canonical text with
`ludion_scene::emit_doc(kind, &value, &schema)`: the `format`/`kind` envelope, then fields in
schema-declaration order (an enum variant's fields by the variant the value takes; map keys in byte
order), ryu-shortest floats (an `f32` field as its shortest f32 decimal), LF endings — byte-idempotent,
so a git diff stays minimal. The matching reader is `parse_doc` (see above). Hand-authored docs load
without being canonical, but the next tool-write normalizes the file.

## Non-goals (and their triggers)

- **No engine-side ID resolution.** A doc naming another doc is just a string: the game resolves it
  (as `equip_weapons` does) and states its own rules through `validate_docs`. There is no link type
  the engine follows for you.
- **No inheritance / templating.** There is no "base weapon others extend" — repeat the fields, the
  same discipline scenes use (repetition keeps diffs local). Trigger: the same content-volume
  pressure.
- **Behavior stays in Rust.** A doc carries numbers and archetype *names*, never logic — there is no
  embedded scripting layer, by design. New behavior is a new Rust archetype the doc can then name.

## Next

- `ludion docs data-docs-checks` — `--check-docs`, cross-doc rules, one-of-a-kind kinds.
- `ludion docs player-profiles` — persistent save state.
- `ludion docs scene-format` — the strict-YAML scene document.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs dev-loop` — the live reload loop.
