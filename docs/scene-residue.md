---
description: What a district keeps while you are away: the Residue marking, its identity rules, and ResidueStore
---

# Scene residue

**Scope:** The state a district holds onto between visits inside one session — the `Residue` marking, how a record finds its entity again, and reading another district's residue back. What travels *with* the player across a switch is scene-transitions; the save that outlives a run is player-profiles.

Carry answers "what travels with you"; **residue** answers "what the district keeps while you are
away". Mark an entity `Residue: {}` and its full component bag (everything but `Name`) is captured out
of the world when a switch leaves that scene, and re-applied when the run returns later in the same
session — so a door opened on the first visit is still open on the second. An unmarked entity gets no
such memory: it is rebuilt from the scene's authored text on every visit, exactly as before residue
existed.

## Format

```yaml
entities:
  - name: door
    components:
      Residue: {}
      Openable:
        open: false
```

## Rules

- **Identity is the entity's `name:`.** A named `Residue` entity replaces its counterpart in place on
  return (the `EntityId` does not move); an entity with no `name:` is *anonymous* — never matched,
  always respawned fresh (the dropped-pickup case). A named record with no counterpart on return
  depends on where the entity came from: one a **system** spawned respawns with its `name:` (that is
  how per-district state a game creates itself survives a visit), while one the **scene authored** was
  renamed, so it is dropped and reported (`L3031`) — spawning it would duplicate the renamed entity.
- **A record is migrated, not trusted.** A snapshot carries residue, so a bag can outlive the shape of
  its components (`ludion dev`'s restart, a replay seek); one this build cannot read is dropped whole
  and reported (`L3030`), so a component change never makes a district unreachable.
- **A reference follows its target's identity.** An entity reference in a record (`Parent` and every
  marked field) follows another residue record to its respawned entity, or an entity with a `name:`
  to the first entity of that name in the scene. Any other reference arrives empty, or its component
  is left behind when the field is required (`L3053`) — an anonymous entity that is not itself
  residue, a carried one and a despawned one have no identity to find again.
- **State must stay alive to be remembered.** Capture reads the departing world as it is, so an entity
  despawned before the switch leaves nothing behind and the scene's authored version comes back. A
  thing used up keeps its entity and flips a component to say so, rather than being deleted.
- **Residue is keyed by the scene's `name:`.** A scene that declares none keeps nothing. `scene.load`
  starts a session and clears every district's residue; a switch carries it forward; `engine.restore`
  adopts whatever the snapshot embeds.
- **It never enters the `WorldHash` and is never recorded.** Residue regenerates from the ticks that
  produced it, the same way a district switch itself does.
- **An entity is either carried or marked, never both** (`L3027`) — one travels with the player, the
  other stays with the scene. Two `Residue` entities sharing a `name:` cannot both be captured, so the
  first wins and the rest are skipped (`L3026`) — and the same code fires on the way back in when two
  entities of the arriving scene answer to one record's name.
- **Both bounds are loud, never silent.** A session remembers at most 32 scenes (the oldest visited is
  dropped) and one scene holds at most 4096 records (the tail of an oversized capture is dropped) —
  either is `L3028`, a warning; the switch itself still happens.
- `ludion map-check` raises `L1032` on an unnamed map marker whose `components:` bag authors `Residue`:
  its `Name` is index-derived, so its identity would shift whenever a marker is inserted above it.

## Reading it back

Read residue back from anywhere — a map screen previewing a district the run has not walked back into
yet — through the `ResidueStore` resource, a read-only mirror rebuilt at every world (re)build:

```rust,ignore
fn preview_home(world: &mut World, _ctx: &SystemCtx) {
    match world
        .resource::<ResidueStore>()
        .and_then(|store| store.get::<Openable>("district-a", "door"))
    {
        Some(door) if door.open => println!("district-a kept its door open"),
        Some(_) => println!("district-a kept its door shut"),
        None => println!("district-a has kept nothing"),
    }
}
```

The runnable version — one door that stays open across a round trip, read back through the mirror
while away — is `crates/ludion/examples/scene_residue.rs` (`cargo run -p ludion --example
scene_residue`). `ludion explain L3026`/`L3027`/`L3028`/`L3030`/`L3031`/`L1032` cover the codes above.

## Next

- `ludion docs scene-transitions` — the switch that captures and re-applies residue.
- `ludion docs player-profiles` — the save state that outlives the session residue does not.
- `ludion docs map-authoring` — authoring a `Residue` marker on a district map.
- `ludion docs components` — declaring the components a captured bag holds.
