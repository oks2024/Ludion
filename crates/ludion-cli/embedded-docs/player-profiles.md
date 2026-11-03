---
description: Persistent save state: registering the profile type, lenient migration, and run reproducibility
---

# Player profiles

**Scope:** A game's one persistent save document — registering it, mutating it from a system, how it migrates across schema drift, and how it reaches a replay. Authored, read-only game data is data-docs.

A **player profile** is a game's persistent save state — unlocks, knowledge flags, lifetime stats:
the meta-progression that outlives a single run *and* a process restart. Where a data-doc is authored
content the engine only reads, a profile is a **singleton the sim mutates**: your systems read it to
shape a run and write it back when the run ends. You register **one** serde type; the engine loads
`save/profile.yaml` into a `Profile<P>` resource, migrates it leniently across schema drift, and — in
a live host — persists it back to disk.

The word "profile" here means *save state*, never the performance profiler (`ludion profile`, timing
budgets). They never overlap in a game's code.

## Registering the profile type, and the growable contract

A profile type carries the component bound plus **`Default`** — because a profile *load-or-defaults*: a
fresh install has no file, so it starts from `P::default()`. Every field also wants a serde default
(`#[serde(default)]` on the struct is the simplest form) — the **growable-profile contract**. Your save
type grows as the game grows, and a player's accumulated save must survive that drift:

```rust,ignore
use ludion::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The one persistent save, registered with `profile::<SaveState>()`. `#[serde(default)]` is the
/// growable contract: a field added in a later build fills from its default when an older save lacks
/// it, and a removed field is dropped — the save is never rejected for drifting.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone)]
#[serde(default)]
struct SaveState {
    unlocked_crossbow: bool,   // an unlock earned across runs
    runs: u32,                 // a lifetime stat
}

/// The scene's fighter; its starting weapon is derived from the profile at boot, so the
/// profile-at-start reaches the WorldHash through this ordinary hashed component.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone)]
#[serde(default)]
struct Fighter { weapon: String }

/// The whole pattern: read the profile to shape the run, then write it back when the run ends.
fn play_run(world: &mut World, ctx: &SystemCtx) {
    // (a) On the first tick, stamp each fighter's starting weapon from the profile-at-start.
    if ctx.frame.as_u64() == 0 {
        let unlocked = world.resource::<Profile<SaveState>>()
            .is_some_and(|p| p.get().unlocked_crossbow);
        let weapon = if unlocked { "crossbow" } else { "fists" };
        let entities: Vec<EntityId> = world.entities().collect();
        for e in entities {
            if let Some(fighter) = world.component_mut::<Fighter>(e) {
                fighter.weapon = weapon.to_string();
            }
        }
    }
    // (b) At a run-end condition, record the run and request a save — once (idempotent within a frame).
    if ctx.frame.as_u64() == 90
        && let Some(profile) = world.resource_mut::<Profile<SaveState>>()
    {
        let save = profile.get_mut();
        save.runs += 1;
        save.unlocked_crossbow = true;
        profile.save();
    }
}

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Fighter>("Fighter"))
        .profile::<SaveState>()
        .system("play_run", play_run)
}
```

Read the profile with `world.resource::<Profile<SaveState>>()` and its `.get()`, mutate it with
`world.resource_mut::<…>()` and `.get_mut()`. Mutating alone does **not** persist — call
`profile.save()` to request a write; the engine drains the request at the frame boundary (like an audio
event). Several `save()` calls in one frame collapse into one write. A save is crash-durable, so a
power loss leaves a whole save; it costs a disk flush on that frame, so save at milestones rather
than every frame. A failed save (`L1122`) keeps the value in memory until the next `save()`; a
`scene.load` discards it, a scene switch does not. The runnable version of this
pattern — two runs on one save directory, the second booting on what the first wrote — is
`crates/ludion/examples/player_profile.rs` (`cargo run -p ludion --example player_profile`).

## Lenient migration: never rejects, never destroys

Loading a profile always succeeds and never destroys a player's bytes. The migration report
(`Engine::profile_migration()`, printed to stderr by `ludion::run` at boot) tells you what it did:

- **Fields the type gained** (the file lacked them) are filled from their defaults — reported, not an
  error.
- **Fields the type lost** (the file still had them) are dropped — reported, not an error.
- Both lists name a nested field by its dotted path (`inner.b`).
- **A file that cannot be honored at all** — a malformed envelope, a field whose *type* changed so it no
  longer deserializes, or a `format:` newer than this build reads — is **backed up** to a content-hashed
  sibling `save/profile.yaml.bak.<hash>` and a fresh `P::default()` starts, with an `L1121` warning. The
  player's bytes are preserved in the backup; the game always boots. (`ludion explain L1121`.) The
  backup happens in a persisting host (`ludion::run`); a test or replay reports the same `L1121` with an
  empty `backup` and leaves the file in place.

This is the one place a profile deliberately diverges from a data-doc: an authored doc that drifts is a
hard `L1119`/`L1120` an agent must fix, but a *player's* save must migrate, not brick the game.

## Location and the save envelope

The profile lives at `<save_root>/save/profile.yaml`. The save root defaults to the project root —
the scene's own directory, the same root every asset resolves against — but
`EngineBuilder::save_root(path)` moves it, so a read-only content directory and a writable save
directory can be separate. `profile_path` stays relative to the save root; override it with
`EngineBuilder::profile_path("saves/slot.yaml")`. The file is the canonical `kind:` envelope every
data-doc uses, with `kind: profile`. A profile type with a top-level `format` or `kind` field is
refused when the engine is built (`L1257`) — rename it:

```yaml
format: 1
kind: profile
unlocked_crossbow: true
runs: 7
```

`profile` is an **engine-owned** kind: the content-root doc walk and `--check-docs` skip it (the profile
loader owns it, exactly as the asset cache owns `mesh`/`voxel`), so a game with a saved profile beside
its data-docs still validates clean.

## Reproducibility: a run is `(seed, profile-at-start, inputs)`

A profile reaches the `WorldHash` only through the components your systems derive from it — but that is
enough to change the whole run, so a replay must reproduce the *exact* profile the recording started on.
It does: the **profile-at-start travels inside the recording** (the `.lrp` header) and inside every
snapshot keyframe, so a committed session reproduces its trace bit-for-bit on a machine whose on-disk
`save/profile.yaml` differs or is absent — the embedded profile wins over disk. `save()` requests are a
pure function of the sim, so a replay *regenerates* the identical save log and it is **never recorded**
(the audio contract), and a `ludion dev` snapshot→restore preserves unsaved in-memory profile changes
exactly. Disk writes happen in **exactly one mode** — a live host run (`ludion::run`, including
`--record`); a replay, `--check-docs`, `--profile`, or a test leaves persistence off, so none ever
clobbers your real save.

## Non-goals (and their triggers)

- **One slot.** There is a single `save/profile.yaml`, not save slots. Trigger to revisit: a second
  person plays the same install.
- **No cloud/Steam sync, no encryption/anti-cheat.** Text-is-truth applies to saves — a profile is
  plain canonical YAML you can read and diff. Trigger: none foreseen for the prototype (anti-cheat is a
  whole-engine non-goal).
- **No preserved-unknown-fields across a re-save.** Canonical emit writes only the current type's
  fields, so an older build that re-saves a newer file would drop the newer fields — which is why a
  newer `format:` is backed up rather than silently downgraded. Trigger: a real need to round-trip
  forward-compatible fields.

## Next

- `ludion docs data-docs` — game data as validated text.
- `ludion docs scene-transitions` — moving between districts mid-run.
- `ludion docs verify-loop` — the `cargo test` verification loop.
- `ludion docs scene-format` — the strict-YAML scene document.
