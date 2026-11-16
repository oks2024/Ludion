---
description: Placing reflection probes: shapes, rank, blend, the probe store, bakes and their diagnostics
---

# Reflection probes

**Scope:** The `ReflectionProbe` component — a captured cube of the scene that glossy and metallic
surfaces inside its volume reflect, with parallax, in place of the sky. How to place probes, how they
rank and blend, and the probe store and bake commands. The sky they fall back to is
`ludion docs environment`; the optional screen-space trace blended over them is
`ludion docs post-processing`; shipping the store is `ludion docs export`.

A scene with no probe reflects the procedural sky alone. A probe adds a reflection of its
surroundings — walls, props, lamps — inside its volume, and the sky fills whatever it does not
cover. Probes are authored text; the captured pixels are derived and never committed.

## The loop

1. Place a `ReflectionProbe` entity: the `Transform` is the capture point (and a box's orientation).
2. `ludion check-scene <scene>` — `L1274`–`L1278` name a bad volume, an overlap or a duplicate.
3. `ludion render --scene <scene> --out probes.png`, then again with `--debug-view probe_index` to see
   which probe owns each pixel (coloured by rank, fading to gray as a probe's weight falls, gray where none
   covers, black only where there is no surface). `ludion render --dump-probes <dir>` writes each captured cube and `probes.json`.
   The `probes` debug-draw category (`ludion docs debug-draw`) outlines each volume live.
4. `ludion bake-probes <scene>` fills the probe store for a CLI-loadable scene (`--probe <Name>`
   recaptures one, `--json` for tooling); a game's scene is baked by the game binary's
   `--bake-probes` (with `--bake-probe <Name>`), because the CLI cannot load a game's components.
   `ludion sync` warms the store for each CLI-loadable scene that names a probe, or every scene when a prefab or other document carries one, at `--quality` (else this machine's). It also drops entries unused for 14 days, or the oldest past a 4 GiB cap; one dropped by mistake only costs a recapture.
5. `run`, `serve` and `dev` read the store; `--probes rebake` captures afresh and leaves it alone.
   A probe added, moved or removed in a live session is captured on the spot — no bake step. A
   geometry, light or sky edit shows in the probes after a reload (`scene.load`, `asset.reload`) or a
   requested refreeze (`ProbeRefreeze`, below).

## Format

```yaml
  - name: room-probe
    components:
      Transform:
        translation:
          - -6.0
          - 1.0
          - -1.0
      ReflectionProbe:
        shape:
          box:
            extent:
              - 3.0
              - 2.0
              - 3.0
        blend: 0.25
        priority: 0
```

(from `examples/gallery/probe-rooms.yaml`, which pairs it with a sphere probe over open floor.)

| Field | Meaning | Default | Valid |
| --- | --- | --- | --- |
| `shape: { box: { extent: [hx, hy, hz] } }` | an oriented box; `extent` is **half**-extents along the probe's axes | `[2, 2, 2]` | each `> 0`, else `L1274` |
| `shape: { sphere: { radius: r } }` | a sphere about the probe's position | `2` | `> 0`, else `L1274` |
| `blend` | world units of fade outside the volume (weight 1 inside it) | `0.5` | `>= 0`, else `L1274` |
| `priority` | higher wins where volumes overlap | `0` | integer |

`Transform.translation` is the capture point, and the parallax correction projects the reflection
onto the volume from there. `Transform.scale` is ignored — size the shape, never the transform.

## Rules

- **Fit a box to its room.** Set `extent` to the room's inner half-size and centre the probe in it:
  the box faces then sit on the walls and the reflection parallaxes exactly. A box wider than the
  room smears the walls; a sphere suits open areas, where no wall defines a box.
- **An exact fit is safe.** The weight is 1 inside the volume, on its surface and within a
  depth-relative tolerance of it (about 1e-4 × the view distance, at least 1 mm), so receivers
  rebuilt from depth that lie on a face never flicker at any camera pose.
- **Enclose the receivers.** A receiver meant to be covered is inside or on the volume, never just
  past it; the tolerance is no margin to author against. With `blend: 0` the weight is a hard step
  just past the tolerance: anything beyond it gets the sky fill only, so pad the box or give it a
  band where receivers stray outside.
- **The fade band reaches `blend` past the faces.** Where two fitted rooms share a wall, the
  higher-ranked room's band shows through into its neighbour, so keep `blend` at most the wall
  thickness (or 0) between rooms. Open edges, such as a doorway onto a street, want a real band.
- **Rank is static, never camera distance:** `priority` descending, then boxes before spheres, then
  the smaller volume, then entity id. Up to 4 probes cover one pixel; each adds its weight times
  what coverage is left, and the sky fills the rest. With `PostProcess.ssr > 0` the screen-space
  trace blends over the probe-and-sky result.
- **Use `priority`** for a small probe nested in a larger one (a lamp-lit alcove in a hall): the
  nested one gets the higher number. Equal priority and volume overlapping is `L1275`; a probe wholly
  inside a higher-ranked probe's volume (weight 1 there), its own band included, never shows,
  `L1276`; two probes with the same shape, size and pose are a copy-paste, `L1277`.
- **At most 32 probes per scene** (`L1278`). Merge neighbours into larger volumes.
- **A capture holds what the world looks like at load or at a requested refreeze** — meshes, the sky and the lighting. Sprites,
  particles, text and editor markers are absent, and a prop that moves later does not move in it.
  Its sky part follows the live sky, and its geometry follows the live ambient, `ibl_intensity` and
  GI, so a run clock needs no rebake; emissive surfaces (a night city's neon) keep their brightness
  as the ambient moves. Direct light on the captured geometry (sun, moon, point and
  spot lights) is frozen at capture and only scaled by that ambient ratio, so moving a
  `DirectionalLight` or switching a lamp does not follow in probe reflections until a rebake.
  The ambient ratio is exact for lit surfaces facing the probe's capture point, so a sky-gradient
  change (a zenith unlike the horizon) leaves them right; metals and surfaces facing away still
  shift in hue and brightness a little. A transparent emissive (a glass sign, particles) fades
  with the ambient like lit geometry, and an emissive seen through glass can reflect brighter
  than captured as the ambient dims.
- **`shape`, `blend` and `priority` never recapture** — they only change how the cube is projected
  and weighted. Moving the probe recaptures it. What it sees (geometry, lights, `Environment`, the
  textures) is the world at the last load or requested refreeze, so an in-session edit waits for one; the quality
  and an engine shader change give a new store key too. The grade (`PostProcess`) never does.
- **The store is per-machine, derived and ignored** — `.ludion/cooked/probes/` beside the project;
  only the text is committed. `ludion render` and `ludion test` never read it: they capture fresh,
  so a golden never depends on whether a store exists. Hosts read it and capture on a miss.
- **Export ships the store:** `ludion export` bakes every probe of every shipped scene against the
  written bundle at the target's quality; a missing entry is `L1281`. Only scenes whose text holds a
  probe are baked, so a `probe_bake_worlds` hook's worlds in a probe-less scene are not bundled.

## Probes spawned at runtime

A game that swaps a room by spawning a prefab holding probes and props asks the engine to re-freeze:
a system calls `world.resource_mut::<ProbeRefreeze>()` then `request()` on the tick it swaps.

- **Timing:** a request on tick N freezes the end-of-tick-N world at tick N+1's start; the capture or
  store lookup runs at the next render. A render before that shows the old capture.
- **Scope:** whole-world, and only on request; a `Static` change never fires it. With no probe in the
  world the request is a silent no-op that clears itself.
- **Determinism:** the store key hashes the whole frozen world (meshes, lights, sun, environment, GI
  voxel grid, quality), so a store hit needs the freeze-time world identical between bake and play.
  Anything spawned that tick counts. The key ignores spawn order and entity slots.
- **Stripped from the capture:** sprites, particles, text, ground markers, drop shadows, decals, UI,
  debug lines, nested probes and the viewmodel; light shadows and scatter, sun cascades and disc,
  and SSAO, SSR, shafts and DoF. Hidden meshes are not captured, but a hidden mesh without
  `GiDynamic` still enters the GI voxelization and so the key; hidden **and** `GiDynamic` keeps a
  warm set out of both. Visible `GiDynamic` meshes are captured.
- **Baking:** register `EngineBuilder::probe_bake_worlds(|engine, i| ..)`. After the loaded world,
  `--bake-probes` calls it for `i = 0, 1, ..` until `Ok(None)`, on one engine; it puts world `i` in
  place as the game's swap tick leaves it (despawn the last, spawn the room, step ticks if systems
  need them) and returns a label. The engine then re-freezes as a tick start does, so bake and play
  key the same world. Probes carry the label as `world` in the JSON; `--bake-probe <Name>` recaptures
  that name in every world. The hook runs after every `--bake-probes` load: return `Ok(None)` at once
  for a scene it does not drive.
- **Coverage gate:** after the swap tick, `engine.probe_coverage()?.check()` fails with `L1279` when
  a probe has no store entry, so a test catches a stale store after a room edit. Needs a GPU.

```rust
let worlds = ["hall", "vault"];
let builder = Engine::builder().probe_bake_worlds(move |engine, i| {
    let Some(room) = worlds.get(i) else { return Ok(None) };
    swap_room(engine, room)?; // despawn the last room, spawn this one
    Ok(Some(room.to_string()))
});
```

## Gotchas

- A host run with `--textures source` misses the entries a bake wrote (bakes use cooked textures) and
  warns `L1279` for every probe it captures — drop the flag, or accept the load-time cost.
- `L1279` is a warning, not a failure: the pixels are the same, the load is slower. Bake the store, or
  pass `--probes rebake` while iterating on a look.
- `ludion bake-probes` on a game's scene fails `L1013`; use the game binary's `--bake-probes`.
- `--probe`/`--bake-probe` take a `Name`; an unnamed probe cannot be forced, and an unknown name is
  `L1280`.
- A bake is keyed by the quality it ran at (`--quality`); bake at the quality the game will run.

## Next

- `ludion docs environment` — the sky probes fall back to.
- `ludion docs post-processing` — `ssr`, the trace blended over probes.
- `ludion docs export` — shipping a baked store.
- `ludion docs debug-draw` — the `probes` category and the `probe_index` view over the wire.
- `ludion docs components` — every component at a glance.
