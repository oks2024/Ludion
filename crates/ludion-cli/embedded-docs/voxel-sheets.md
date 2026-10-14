---
description: The render --model contact sheet: views, game rigs, lineups, palette sweeps, model goldens
---

# Voxel model sheets

**Scope:** `ludion render --model` — the harness that answers "what does this model look like" with
one image, and the flags that make that image honest. The loop it sits in, and the text checks that
come before it, are `ludion docs voxel-authoring`; what to look *for* in the sheet is
`ludion docs voxel-craft`; the document itself is `ludion docs voxel-format`.

## The model view harness

`render --model` is a second render mode: point it at one model document alone — `.voxel.yaml` or
`.mesh.yaml`, the same file, no wrapping scene — and it stitches a turntable around it into one
contact-sheet PNG, so checking a model costs one image no matter how many angles you need.

```
ludion render --model voxels/lantern.voxel.yaml
ludion render --model voxels/lantern.voxel.yaml --views front,iso --out lantern-check.png
ludion render --model meshes/rock.mesh.yaml --views iso --width 1080 --height 1080
```

- **Views**: `--views front,back,left,right,top,iso`, comma-separated — order and duplicates don't
  matter, the sheet always lays out the requested subset in that canonical order, row-major. Omit
  `--views` for the default six. A seventh view, **`game`**, is opt-in (`--views game`, alone or
  alongside others): a steep top-down three-quarter pitched 58° down — the angle real gameplay sees,
  where overhead geometry can hide a face the flat `front` view shows fine. It is not in the default
  set because a seventh cell would change the committed contact-sheet layout (and every pinned golden).
- **Labels**: every cell is stamped with its orientation in the corner — `FRONT -Z`, `TOP Y`,
  `GAME 58`, and so on — naming the **world** axis the camera sits on, so a cell is never ambiguous.
  The `front` cell always shows the authored front: the camera accounts for the mesher laying
  `z = 0` at world −Z, so where you write the eyes is where `FRONT` looks (the facing-yaw consequence
  of that pairing is in `ludion docs voxel-format`).
- **Grid**: 1 row for up to three views, 2×2 at four, 3 columns × 2 rows at five or six.
  `--width`/`--height` size the *whole sheet*, not each cell — cells divide it (integer division, so
  the final sheet can end up a little smaller than asked), which means **fewer views render bigger**:
  ask for `--views iso` alone when you want fine detail, not a different mode.
- **The scene is synthesized for you**: the model is normalized to a standard framing, sat on a
  ground plane, and lit by a per-view neutral key + fill rig — there is no camera or light to author.
  Same document and flags render byte-identical PNGs every time.
- **`--out`** defaults to `<name>.sheet.png` in the current directory, named from the model's own
  file stem (`lantern.voxel.yaml` → `lantern.sheet.png`, `rock.mesh.yaml` → `rock.sheet.png`).

### Rendering under a game rig

The synthesized rig is bright and neutral because it has to read *every* model. When what you need
is how the model looks in the game, `--rig <scene>` borrows a real scene's lighting for the sheet:

```
ludion render --model voxels/statue.voxel.yaml --rig examples/gallery/night-street.yaml
```

**The lift contract.** Every entity of the rig scene carrying any of `Environment`, `PostProcess`,
`DirectionalLight`, or `PointLight` is copied *whole* — its `Transform` included — into the
synthesized sheet scene. Nothing else crosses over: the rig's own models, cameras, and ground are
ignored, with one exception — an entity named `ground` carrying a `Mesh` donates its color to the
sheet's ground plane, so a palette value is judged against the floor it will sit on. A rig carrying
any light replaces the synthesized key + fill outright (mixing the two would read as neither); a
grade-only rig keeps them. A rig with none of the four components is `L1117`; a rig that is not a
valid scene reports the ordinary scene diagnostics. `--rig` is `--model`-only and conflicts with
`--scene`.

**The caveat that decides which recipe you want.** The sheet normalizes its subject to a unit
bounding sphere, and a game rig was authored around game-scale subjects. `Environment` and
`DirectionalLight` are position-independent, so a sky and a moon read true — this is why
`env-night.yaml`, which is exactly those plus the grade, is the rig worth borrowing. A `PointLight`
is *not*: its `range` and `intensity` were tuned at metres from a metres-tall subject and land
somewhere else entirely against a unit-radius one. `PostProcess` is lifted verbatim, and its
distance-coupled terms (`fog_*`, `dof`/`focus_distance`) were likewise tuned for game camera
distances, not for a sheet camera ~3 units out.

### Lining models up against each other

`--model` takes a comma-separated list, and **two or more paths render a lineup**: one cell, every
model side by side on the shared ground plane under one rig, captioned with its file stem.

```
ludion render --model voxels/house-a.voxel.yaml,voxels/house-b.voxel.yaml \
  --rig examples/gallery/night-street.yaml --views game --out houses.png
```

The row is normalized as a **whole** — the union of every model's AABB scaled by its declared
`scale:` — so relative sizes are honest, which single-model sheets never are. It reads left to right
in the order given, broadside to the chosen view so neighbours never hide one another. A lineup takes
**one** view (`game` by default), caps at 8 models, requires `--out`, composes with `--rig` and
`--true-size` (the tallest model earns the pixels), and conflicts with `--set`/`--sweep` — calibrate
one palette first, write the value into its document, then line it up.

### Calibrating palette values

`--set` and `--sweep` work on a model document too, addressing its **palette** — the loop that
otherwise costs a re-edit and a re-render per candidate. Nothing on disk changes.

```
ludion render --model voxels/house-c.voxel.yaml --rig examples/gallery/night-street.yaml \
  --views game --true-size --sweep /palette/G/emissive=1.6,2.2,2.8
```

Addressable: `/palette/<glyph>/color/<0..3>` (0 red, 1 green, 2 blue, 3 alpha) and
`/palette/<glyph>/emissive|metallic|roughness`. Anything else is `L4008`, and so is a `.mesh.yaml`
model, which has per-vertex colors and no palette. A scalar the entry left unset is created and
echoed `unset -> <value>`. `--set` composes with any views; `--sweep` spends its cells on values, so
it takes **exactly one** view. An out-of-range value is still the document's own `L1114`, exactly as
writing it into the file would be.

**When you need absolute fidelity**, don't use the sheet at all — render the model *in* a stage scene
built at game scale, swapping the subject in memory with `--set` (nothing on disk changes):

```
ludion render --scene scenes/model-stage.yaml \
  --set /entities/3/components/Mesh/shape/voxel=voxels/lantern.voxel.yaml \
  --set /entities/3/components/Transform/scale=[0.24,0.24,0.24] \
  --out lantern-stage.png
```

Entity `3` is that scene's `subject` — the index is the entity's position in the file, counting from
zero, and every applied override is echoed above the summary line so a wrong one is visible
immediately. The stage carries the game's real torch and moon-fill point lights at their real
distances, which is the read `--rig` cannot reproduce; what it costs is one camera angle instead of
six.

### Isolating one contribution

`--channel emissive` swaps the finished picture for a diagnostic one: every light, the ambient term
and the whole sky/IBL environment are zeroed, the caption is dropped and the post stack is a pass
through, so a non-black pixel means emission and nothing else.

```
ludion render --model meshes/idol-lit/mesh.mesh.yaml --views front --channel emissive
```

Writes `<name>-emissive.sheet.png` by default, never over the ordinary sheet; conflicts with `--rig`,
whose lights are exactly what the channel removes. A `kind: mesh` document referencing an
`emissive_texture` is given a unit `Mesh.emissive` for this render alone — a standalone document
carries no scene to author that intensity in, and the map alone emits nothing (`ludion docs
materials`) — so the channel answers "does this map glow, and only where I meant it to" without
needing a scene at all. Verifying a painted map this way, against a mesh edited with `ludion mesh
apply`, is `ludion docs mesh-editing`'s own loop.

### Golden per model

The kind-dispatch lives in the golden test itself, so it applies everywhere a
golden test runs: `ludion test`'s directory suite discovers a top-level `*.voxel.yaml`/`*.mesh.yaml`
file exactly like a scene, pairing it with `goldens/<os>/<stem>.png` — the stem keeps the `.voxel`/`.mesh`
suffix, so `lantern.voxel.yaml` pairs with `goldens/<os>/lantern.voxel.png` — and `--bless` blesses the
contact sheet exactly as it blesses a scene render. A regression still localizes — a damaged tile in
the diff report points at one turntable view, not just "the model changed."

## Gotchas

- **In Git Bash, prefix a `--set` render with `MSYS_NO_PATHCONV=1`** — MSYS otherwise rewrites the
  leading-slash JSON Pointer into a Windows path and the override silently stops matching anything.
- **`--model` and `--scene` are mutually exclusive**, and `--model` also conflicts with
  `--dump-timings` (a model sheet is many renders, not the one instrumented render `--dump-timings`
  times) — pick one render mode per invocation.
- **`--true-size` reads `--width`/`--height` as an aspect ratio**, since it computes the cell from
  the model's declared `scale:`; passing either explicitly alongside it is a conflict, not a
  silently-ignored flag. See `ludion docs voxel-craft` for what it corrects.
- **Pointing `--model` at a scene**, or at any document with no `kind:` or an unrecognized one, is
  `L1117` (`ludion explain L1117`); a `--model` document that *is* recognized but malformed still
  fails with its usual `L1110`/`L1111` (mesh) or `L1113`/`L1114` (voxel) diagnostics.

## Next

- `ludion docs voxel-assemblies` — the multi-part model this harness renders at its rest pose.
- `ludion docs voxel-animation` — `--clip`/`--time`/`--strip`, the posed forms of this sheet.
- `ludion docs voxel-authoring` — the loop this harness is step 3 of, and the text checks.
- `ludion docs voxel-craft` — what to look for in the sheet you just rendered.
- `ludion docs voxel-format` — the document schema, orientation, and palette.
