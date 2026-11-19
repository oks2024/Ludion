---
description: Multi-part voxel models: kind: assembly and kind: part, pivots, joint plugs, the humanoid rig
---

# Voxel assemblies

**Scope:** authoring a voxel model out of several parts that can move independently — the
`kind: assembly` and `kind: part` documents and the craft of a rig that survives being posed. Making
one move is `ludion docs voxel-animation`; a single-piece model is `ludion docs voxel-format`; the
contact sheet both share is `ludion docs voxel-sheets`.

An assembly is one file that owns **one palette**, places lean part files into one assembly-global
grid, and declares the part hierarchy. Its **rest pose is the assembled grid**: what a sheet shows,
every `check-scene` view analyses, and renders when no clip plays.

## The loop

1. **Author** `models/x.assembly.yaml` and one `parts/*.part.yaml` per part. Part and clip paths are
   resolved relative to the assembly file, so the model moves as a directory; `..` is fine, an
   absolute, drive or device path is refused (`L1138`).
2. **Check** — cross-file, no GPU, and it reads every part:
   ```
   ludion check-scene models/x.assembly.yaml
   ```
   ```text
   voxels/figure.assembly.yaml: OK (assembly figure, 8x8x2, 4 parts, 1 clip)
     torso: file parts/torso.part.yaml, place (2,0,0), parent none (root), mirror none, meshed 6 quads / 12 triangles / 24 vertices
     arm-r: file parts/arm.part.yaml, place (6,4,0), parent torso, mirror x, meshed 6 quads / 12 triangles / 24 vertices
     clip wave: anims/figure-wave.anim.yaml
   ```
   Two checks always run over the composed grid, behind no flag: the `L1129` connectivity warning
   (with a `connectivity owners:` addendum naming the parts owning each stray piece and the nearest
   main-body cell) and the `L1143` overlap lint.
3. **Read the composed grid as text.** `--mirror-x`, `--slice`, `--column`, `--exposure` and
   `--from game` take a `kind: assembly` and run over the *assembled* grid — one implementation, the
   one `ludion docs voxel-authoring` documents — with part attribution added:
   ```
   ludion check-scene models/x.assembly.yaml --exposure --mirror-x
   ```
   Mirror-x regions gain `parts <names>`, enclosed exposure samples gain `[<names>]`, and `--exposure`
   prints an `overlap:` block listing every cell more than one part filled.
4. **Render the rest pose** — `ludion render --model models/x.assembly.yaml` writes
   `x.sheet.png`. `--views`, `--rig`, lineups and `--true-size` all compose (`ludion docs voxel-sheets`).
5. **Reference it** from a scene, and check that too:
   ```yaml
   Mesh:
     shape:
       assembly: models/x.assembly.yaml
   ```
   Draws the whole model; `shape: {assembly_part: {assembly: models/x.assembly.yaml, part:
   lower-arm-l}}` draws one named part alone, on its own entity — what `switch_to_ragdoll` spawns
   (below), never hand-authored for a rest-pose model.

## Format

Both documents are strict-subset YAML: **block style only**, one entry per line. A flow collection
(`place: [4, 10, 2]`, `S: { color: [...] }`) is `L1005` and does not parse.

`kind: assembly` — the model of record:

| key | required | meaning |
| --- | --- | --- |
| `format` | yes | `1` |
| `kind` | yes | `assembly` |
| `name` | no | human label, echoed by `check-scene` |
| `size` | yes | `[x, y, z]` cells of the assembled grid — the rest pose's extent |
| `palette` | yes | glyph → entry, exactly a voxel model's palette (`ludion docs voxel-format`) |
| `parts` | yes | the placed parts, in authored order — the order `/parts/<i>` pointers index |
| `clips` | no | short name → clip document path (`ludion docs voxel-animation`) |
| `joints` | no | child part name → joint spec, for `switch_to_ragdoll` — see Ragdoll joints below |
| `masks` | no | mask name → `joints` (part names), `descendants`, `weights`: which parts an `AnimationLayers` layer touches (`ludion docs animation-layers`); an unknown part is `L1138` |
| `ik` | no | name → a two-part chain (`parts`, `effector`, optional `hinge`) or a look-at (`aim` mask, `forward`, `limits_deg`), aimed by `IkTargets` (`ludion docs voxel-animation`); structure `L1137`, an unknown part or mask or a missing hinge `L1138` |
| `blends` | no | blend name → `points` (`clip` + `at`) and optional `sync`: clips mixed by `Animation.params` (`ludion docs voxel-animation`); structure `L1137`, an unbound, non-looping or unsynced point clip `L1138` |

One `parts` entry:

| key | required | meaning |
| --- | --- | --- |
| `name` | yes | unique within the assembly; what a clip's tracks address |
| `file` | yes | the `kind: part` document, relative to the assembly file (`..` fine; absolute, drive or device refused, `L1138`) |
| `parent` | no | another part's name; **exactly one** entry omits it — the root |
| `place` | yes | integer `[x, y, z]`: where this part's own cell `(0,0,0)` lands in the assembled grid |
| `pivot` | no | part-local **float** voxel coordinates, default `[0, 0, 0]`; every rotation is about it |
| `mirror` | no | `x`, the only legal value: reverses the part's rows so one limb file serves both sides |

`kind: part` — a lean slices-only document: `format`, `kind: part`, optional `name`, `size`, and
`slices` (one row-string layer per y level, bottom-up, exactly a voxel document's grid). **No
palette:** a part's glyphs resolve against the owning assembly's, so any printable ASCII glyph other
than `.` is structurally legal here and an undefined one is the assembly's `L1138`.

Canonical form: `parts` keeps its authored order (the hierarchy reads as written), `palette` sorts by
glyph and `clips` by short name, and a `[0, 0, 0]` pivot, an empty `clips`, and `mirror` absent are
each written as the key's absence — one spelling per meaning.

## Rules

- **The assembly owns the one palette.** Parts carry none, so whole-model value structure is enforced
  by construction and a re-skin is a new assembly over the same part files. A glyph no palette entry
  defines is `L1138`, pointed at `/parts/<i>`.
- **`parts` is a tree with exactly one root.** Zero roots, two roots, an unknown `parent`, a cycle, a
  duplicate `name`, or a part that does not fit inside `size` at its `place` are all `L1138`.
- **The rest pose is the assembled grid.** 1 voxel = 1 unit, voxel bottom-center origin: a one-part
  assembly placed at the origin renders exactly where the equivalent `.voxel.yaml` would (pinned by
  a byte-identity test).
- **Overlap is legal and expected** (see the joint plug below). An overlapped cell counts as filled
  **once** in every grid analysis, with both parts kept as attribution.
- **Each part is meshed on its own** — nothing is welded, because each part must move. The renderer
  batches per part mesh, so a horde of five-part models costs five draws.
- **Diagnostics carry the code of the document that is wrong.** `L1137` (assembly structure) /
  `L1138` (assembly data) / `L1139` (part structure) / `L1140` (part data) / `L1101` (a part or clip
  file that cannot be read) / `L1143` (the overlap lint, a warning) / `L1184` (a `joints:` entry —
  unknown part, the root, a bad axis/limit/`break_impulse`) / `L1185` (an `assembly_part` naming a
  part the assembly lacks). Raised while checking a *scene*, each is re-pointed onto
  `/entities/<i>/components/Mesh/shape/assembly` (or `.../assembly_part/...`). `ludion explain L1143` carries the full lint contract.

## The rig

### The humanoid convention

Part names are a contract between a model and a clip: one clip file drives any assembly that uses
these names.

```text
torso                              the one root
|- head
|- upper-arm-l  -> lower-arm-l     `-l` is the CHARACTER's left
|- upper-arm-r  -> lower-arm-r
|- upper-leg-l  -> lower-leg-l
`- upper-leg-r  -> lower-leg-r
```

Two segments per limb, never one: at 32 cells tall a knee and an elbow are each about 7 cells of
travel — a walk versus a pair of scissors. Anything past this list (a lantern, a scabbard) is named
by no shared clip and simply rides its parent.

Axes follow `ludion docs voxel-format`: forward **−Z**, up **+Y**, the character's left **−X**, so
every `-l` part sits at low `x` and its `-r` twin is the same file with `mirror: x`. Rotations are Euler XYZ degrees about the part's pivot:

- **+X swings a limb forward** (toward −Z) — the walk axis. The two hinges then fold *opposite* ways:
  a knee flexes −X (the heel goes back), an elbow flexes +X (the hand comes forward). A backwards
  sign passes every check and reads as a broken puppet; only a look catches it.
- **+Y** turns to the character's right — a torso twist, a head glance.
- **+Z** raises the character-left limb outward and lowers the right one. Both sides take the same
  sign, so a symmetric shrug needs opposite signs on the two tracks.

### Pivots

A pivot is part-local, **unmirrored**, float voxel coordinates — a `.5` reaches the centre of an
even-width limb. The loader carries it into the mirrored frame itself, so a `-l` part and its
`mirror: x` twin quote the **same** pivot numbers.

Put each pivot on the anatomical joint — the shoulder, not the arm's centre. A pivot's `x` is
irrelevant to an X-axis swing, so quote the *inboard* joint (nearest the torso): it keeps the plug
cluster tight, which keeps `L1143` quiet.

### Joint plugs

Author a child's last two courses **inside** its parent's volume. Rotating about the pivot then
slides geometry through geometry instead of opening a seam — the joint-plug technique, and the reason
overlap is legal.

`L1143` is the lint that separates a plug from a mistake. A cluster of shared cells is **silent**
when it is a part overlapping its own `parent` and its centre lies within the **plug radius** — half
the child part's largest grid extent, never below 1 cell — of that child's pivot. Everything else
warns, with the pair, the cluster's cell count and bounding box, and (for a parent/child pair) the
distance measured against the radius it failed:

```text
warning[L1143]: part `limb` overlaps its parent `trunk` in a cluster of 1 cell spanning (2,5,0)-(2,5,0),
centered 5.00 cells from `limb`'s pivot at (2.5,10.5,0.5) — past this joint's plug radius 3.00
  --> /parts/1
```

The valuable catch is the second form: the plug is at the shoulder but the pivot was authored at
the wrist, so the limb swings about the wrong end once a clip plays. It is a warning —
`check-scene` still exits success.

**The acceptance test for a new limb or a moved pivot is one line:** `check-scene` reports one
connected component and zero `L1143`. Run it after every rig edit.

## Ragdoll joints

`joints:` (top-level, optional) binds a non-root part — keyed by its own `name` — to a
hinge/ball/slider/fixed constraint on its `parent`, for `switch_to_ragdoll`: same kind/axis/limits/
`break_impulse` shape as the live `Joint` component (`ludion docs physics-joints`). A part missing
from `joints:` switches **fixed**. The anchor is always the pivot — `child.pivot` in the parent's
frame — so a joint-plug's overlap doubles as the constraint's own geometry. Only a **parent/child**
pair skips contacts (each `Joint`'s own `a`/`b`); two sibling limbs sharing cells still collide once
switched, so keep siblings apart even where a plug-into-parent overlap is fine.

### Ragdolls

`switch_to_ragdoll` (Rust, `ludion-engine`) turns a posed `kind: assembly` entity into one rigid
body per part, at its **current** pose: it reads gameplay time, so a mid-clip switch (a death blow)
freezes the pose it died in — layers included, so a layered assembly spawns at the pose that was drawn —
and every joint's limits count from that pose, not the rest one. The root entity survives — its id and every component but `Mesh`/`Animation`/`Collider`/`Velocity` stay.
A root `SurfaceTag` is inherited: each part takes its `parts` entry's surface by part name, else the
root's (`ludion docs collision`).

`switch_to_ragdoll_with(world, entity, assembly, game_time, opts)` takes a `RagdollSwitch`; build it
from `RagdollSwitch::default()` and set `limits_from`. `LimitsFrom::Rest` stores a `JointZero` (the
identity: parts have no rest rotation) on each joint carrier, so limits, `JointMotor` targets and
`JointPoses` count from the **rig rest pose** however the body fell, and survive a rebuild, a
snapshot restore and a save and reload. A switch pose outside a limit measured from rest is clamped
by the solver on the first step — a bounded snap, not an error. The default, `LimitsFrom::SwitchPose`,
is `switch_to_ragdoll` exactly. `ludion docs physics-joints` §Rules has `JointZero`.

`part_velocity: true` adds the drawn pose's own motion to each part's `Velocity`, on top of the root's
rigid velocity: a finite difference of the part's posed transform at the switch tick and one tick
earlier, per gameplay second, so clips, blends, crossfades and layers all count (a staggering body
keeps its stumble; a choked one goes limp). It defaults to `false`, which is `switch_to_ragdoll`
exactly. A clip cut at the switch tick contributes only its own motion, and a rest figure adds none.

## Gotchas

- **`--set`/`--sweep` reach an assembly's `palette:`** exactly as they do a voxel model's; the staged
  copy carries its parts and clips along. `--sweep` cannot combine with `--strip` (`L4008`, both
  multiply cells) but composes with `--clip`/`--time`.
- **An assembly declares no `scale:`**, so `render --model --true-size` frames it at `1.0`. Size the
  model in its own grid, or scale the entity's `Transform` in the scene.
- **`--exposure` measures direction, not depth.** A flame recessed one cell behind a lantern's frame
  scores four exposed faces and is invisible in game. `--from game` ray-tests against the model's own
  geometry and is the check that catches it.
- **A part file is shared, a placement is not.** Two entries naming the same `file` are two parts with
  their own `place`, `pivot`, `parent` and `mirror` — editing the file moves both.

## Next

- `ludion docs voxel-animation` — `kind: animation` clips, the `Animation` component, posed sheets.
- `ludion docs voxel-format` — the palette entry keys, orientation, and the single-piece document.
- `ludion docs voxel-sheets` — every `render --model` flag an assembly inherits.
- `ludion docs voxel-craft` — what makes a model read at game scale once it is valid.
