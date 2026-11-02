---
description: Switching districts mid-run: the SceneSwitch request, anchors, what carries, and failure codes
---

# Scene transitions

**Scope:** Moving the player between scene files inside one run — requesting a switch, what travels with it, and what happens when it fails. Starting a fresh session on a scene is running; the save that outlives a run is player-profiles.

A game larger than one screen is many **districts** — separate scene files the player moves between
mid-run. A scene transition switches the loaded district in place: it rebuilds the world from a target
scene like a load, but where a load *starts a session* (frame 0, reseed, fresh logs) a switch
*continues* one — the run clock keeps ticking, the RNG stream keeps flowing, and the player travels to
the new district carrying its state. The rebuild is fresh only for entities the departing visit left
unmarked; an entity tagged `Residue: {}` keeps the state it had when the run left instead of resetting
to the scene's authored text (`ludion docs scene-residue`). It is `scene.load`'s twin, not its clone.

## Requesting a switch

A switch is requested through the `SceneSwitch` resource, exactly like an audio event: a system sets
it during a tick and the engine performs the switch at the frame boundary. You supply three things —
the target district's scene file (relative to the project root, the start scene's own directory), the
`name:` of an **anchor** entity in that district to drop the carried entities at, and the explicit list
of entities that travel (typically just the player):

```rust,ignore
use ludion::prelude::*;

/// The carried player. `hp` is run state that must survive a district change, so it lives on this
/// entity (which the switch carries) — not in a resource (resources do not carry).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Player { hp: u32 }

/// A district exit, authored on a scene entity: reaching it sends the player to `to`, respawned at the
/// `anchor`. Gates are data — one system drives every gate in the game.
#[derive(Serialize, Deserialize, JsonSchema, Clone)]
struct Gate { to: String, anchor: String }

/// When the player reaches a gate, request the switch carrying `[player]`. The target district has no
/// `Gate` entity, so this never re-fires after the crossing — the switch is naturally one-shot.
fn cross_gate(world: &mut World, _ctx: &SystemCtx) {
    let Some(player) = world.single::<Player>() else { return };
    let player_x = world.component::<Transform>(player).map_or(0.0, |t| t.translation[0]);

    let gates: Vec<EntityId> = world.entities()
        .filter(|&e| world.component::<Gate>(e).is_some()).collect();
    for gate in gates {
        let gate_x = world.component::<Transform>(gate).map_or(0.0, |t| t.translation[0]);
        if player_x >= gate_x {
            let g = world.component::<Gate>(gate).unwrap();
            let (to, anchor) = (g.to.clone(), g.anchor.clone());
            world.resource_mut::<SceneSwitch>().unwrap().request(to, anchor, vec![player]);
            return;
        }
    }
}
```

The last `request` in a tick wins (a game triggers at most one switch per frame; there is no queue).
The engine drains the request last in the frame-boundary trio — after the audio and profile drains —
so a `profile.save()` requested in the same frame is written before the world is rebuilt. The runnable
version of this pattern — two districts as scene files, the player walking across the gate — is
`crates/ludion/examples/scene_transition.rs` (`cargo run -p ludion --example scene_transition`).

## The anchor, and what lands where

The anchor is an ordinary `name:`d entity in the target district (invisible or decorative) whose
`Transform.translation` is the drop point. Each carried entity respawns with its `translation` set to
the anchor's; **everything else travels verbatim** — rotation, scale, `hp`, `Name`, every component,
byte-for-byte. (Anchor-driven *facing* — setting rotation from the anchor — is a non-goal.) A
carried entity brings its whole subtree, and the children keep their `Parent` links, remapped to the
respawned ids. A carried child whose parent stays behind arrives un-parented, keeping its world pose,
with warning `L3043` (`ludion docs hierarchy`). Every other entity reference a carried entity holds
(a marked `EntityId` field, such as `Volley.source`) is remapped the same way when it names a carried
entity. One that names an entity that stayed behind arrives empty when its field is optional, and its
component is left behind when the field is required; both warn `L3043`.

## What carries, and what does not

- **Components on the carried entities carry.** HP, inventory, a quest flag — model run state as
  components on the player (or whatever you carry), and it survives the switch as the exact bytes.
- **Resources do not carry.** A resource is wiped on every world rebuild (like `scene.load`), and it
  never reaches the `WorldHash` or a snapshot — so a value that must survive a switch, a snapshot, or a
  `ludion dev` reload cannot live in a resource. Put it on a carried entity instead. **Exception:
  `GameClock`** — it never reaches the `WorldHash`, but a held one *is* carried by
  `engine.snapshot`/`engine.restore`; a district switch still resets it to unheld like any other
  resource unless the game registers it below (`ludion docs game-clock`).
- **…unless you register it.** `EngineBuilder::carry_resource::<T>()` moves the resource of type `T`
  into the district a **switch** builds. It is for session state that deliberately is not world state
  (which input device the player last touched, a debug toggle) — putting that on an entity would drag
  it into the `WorldHash`. Only a switch carries: a `scene.load` and an `engine.restore` do not.
- **Held input carries.** The keys, buttons, cursor and sticks held at the switch, and injected events
  addressed past the switch frame, arrive in the new district; `scene.load` resets them and
  `engine.restore` restores the snapshot's held input. Edges never re-fire (a carried key is not a
  new press), so a game that reads a *held level as intent* — a hold-to-open — should demand a fresh
  press on arrival (a `GateRearm`-style latch).
- **The run clock is frame arithmetic.** The `FrameIndex` is continuous across a switch (no reset to
  0), so a run timer is `frame - run_start_frame` — derived, never stored as a carried value.
- **The RNG stream continues.** The one seeded RNG is carried into the new district uninterrupted; the
  **target scene's `seed:` is ignored** on a switch (honoring it would fork the run's one stream). A
  district's `seed:` matters only when that scene is loaded as a fresh session's start.
- **The player profile carries in-memory.** An unlock earned this run survives the district change
  (the switch carries the live `Profile<P>`, never re-reads disk).
- **Residue stays with the district.** An entity marked `Residue: {}` keeps its state for the
  session while the run is elsewhere, the mirror image of carry (`ludion docs scene-residue`).

## When a switch fails

A switch is **transactional**: the target district is built in a local world and swapped in only on
full success. Any failure rejects the switch, leaves the live world untouched, and the sim runs on in
the current district — a bad gate never crashes or half-rebuilds the world:

- **`L3018`** — the target district could not be loaded or validated (missing file, malformed scene).
- **`L3019`** — the named anchor was not found in the (otherwise valid) target district.
- **`L3029`** — the anchor was found but carries no `Transform`, so it names no drop point. Refused
  rather than dropping the run at the world origin, where it would read as a physics bug.
- **`L3020`** — the switch *happened*, but a carried `EntityId` was not alive at the switch frame and
  was skipped (a game bug: build the carry list from live queries in the same frame you request the
  switch). The switch still carried whichever ids were live.

Each is a non-fatal warning on `Engine::last_transition_error()` (cleared only by a fully-clean
switch), never a panic — and each is **printed**: one `log` event plus the same `L####: message` line
on stderr, so a district that fails to load mid-run says so, exactly as a failed save (`L1122`) does.
The print is latched on the line, so a gate the player is standing in — re-requesting the same rejected
switch every frame — reports once. Query the accessor for the diagnostic itself (a test asserts on it;
`ludion explain L3018`/`L3019`/`L3020`/`L3029`).

## Reproducibility: the switch regenerates

A replay drives the sim through the same ticks, so the switch-triggering system re-sets the request at
the identical frame and the switch **regenerates** — it is not replayed as a recorded action. The one
thing a replay cannot do is read the target district off disk, so a live switch records a `SceneSwitch`
replay record carrying the **target district's canonical text inline** (the same trick the boot scene
and a `DocReload` use). Under replay the switch resolves its text from those embedded records, never
disk — so a committed `.lrp` that crosses a gate reproduces its `WorldHash` trace bit-for-bit on
another machine even if the district file has since changed or is gone. The record is additive: old
`.lrp` still decode, the wire surface is unmoved. (See the `determinism` topic for the full contract.)

Snapshots and `ludion dev` cross a switch for free: the switch sets the engine's scene to the target's
inline text, so a snapshot taken *after* a switch already embeds the switched district, its carried
player, and the carried profile — `engine.restore`, a keyframe seek, and `dev`'s
snapshot→restart→restore all reconstruct the post-switch district through the existing restore path,
with no snapshot-format change.

## Non-goals (and their triggers)

- **No async / streamed / preloaded loading.** A switch is a synchronous frame-boundary rebuild (a
  district load is milliseconds of instantiation). Trigger: a proven need for seamless streaming (whose
  entry point is this same carry seam).
- **No cross-scene queries or references.** One district is loaded at a time — an entity in district A
  cannot name one in district B. Trigger: seamless streaming.
- **Carry is translation-only.** No anchor-driven facing. Triggers recorded per feature.

## Next

- `ludion docs scene-residue` — what a district keeps between visits, and reading it back.
- `ludion docs player-profiles` — persistent save state.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs running` — hosting a scene and observing it, and the `Shutdown` resource that ends it.
- `ludion docs scene-format` — the strict-YAML scene document.
- `ludion docs game-clock` — `GameClock`, the one resource a snapshot carries.
