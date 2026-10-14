---
description: Deliberate, verified edits to a generated or imported mesh's texture maps
---

# Mesh texture editing

**Scope:** authoring a `kind: mesh-edit` document against a `kind: mesh` asset, deriving it with
`ludion mesh apply`, and picking a surface point with `ludion mesh pick` — the tools that let an
agent say what glows on a mesh it did not paint by hand. The mesh document itself and its texture
keys are `ludion docs scene-format` / `ludion devdocs scene-kind-documents`; what each map means to
the renderer is `ludion docs materials`; where a generated mesh's maps come from is
`ludion docs generation-runs`.

## Rule one: emissive is authored, never detected

No tool in this pipeline looks at a base-colour map and decides something should glow. Every
emissive texel exists because a `paint` op said so, at a named place, in a named colour.

**The trap:** emitted light is `base color × Mesh.emissive × map` (`base color` = instance colour ×
vertex colour). A correct map on an entity whose `Mesh.emissive` is `0.0` (the default) emits nothing
and looks **exactly** like a paint op that missed the surface. The map carries the glow's shape and
colour; the scene carries its intensity.

## The document

`kind: mesh-edit`, file `<name>.mesh-edit.yaml`, beside the mesh it edits. It never rewrites that
mesh: it names it as `source`, and everything the edit produces goes into a derived directory named
by `out`.

```yaml
format: 1
kind: mesh-edit
name: idol-lit
source: mesh-0.mesh.yaml        # relative to THIS document's directory (`..` fine; absolute, drive or device paths refused, `L1174`)
out: idol-lit                   # the derived directory, relative; default: this document's stem
ops:
  - op: paint
    map: emissive
    size: 1024                  # only on the op that creates a map the source does not have
    color:
      - 0.05
      - 1.0
      - 0.35
    strength: 1.0               # optional, default 1
    region:
      sphere:
        at:
          - 0.118
          - 1.342
          - 0.203
        radius: 0.035
        feather: 0.012          # optional, world units, default 0
```

- **`source`** required, a `kind: mesh` document carrying `uvs` — a map is sampled at vertex UVs, so
  an edit has nowhere to paint without them. Relative to the document: a `..` is fine, an absolute
  path or a `:` is not.
- **`out`** optional, a relative path below this document — no `..` segment, absolute path, `:`
  (drive, scheme or stream) or Windows device name; the derive owns every byte written under it.
- **`ops`** an ordered list, each reading the result of the one before. `op: paint` is the only verb.
  **`map`** is one of the five texture slots (`base_color`, `metallic_roughness`, `normal`,
  `occlusion`, `emissive`) but only `emissive` is accepted — the other four parse (so a typo reads as
  "wrong choice", not "unknown word") and are refused with `L1173`.
- **`size`** the created map's square extent in texels (1..=4096), legal only on the first op
  targeting a map the source does not already have; omitted, it takes the source's base-colour map's
  dimensions.
- **`color`** three sRGB floats in `[0, 1]`, the authored colour — not an intensity. **`strength`**
  scales the op's weight (never the colour), `[0, 1]`, default `1`.
- **`region`** one of `sphere { at, radius }`, `capsule { points, radius }`, `box { at, size }`,
  `fill { at, tolerance, max_radius }`, read in the source mesh's own coordinates, each with an optional
  `feather`: weight `1` inside the surface, falling linearly to `0` over `feather` world units
  outside it. A `box`'s `at` is its centre and `size` its full extent per axis, as a sculpt box part
  reads them. The polyline `capsule` (`from`/`to` is `L1173`), the `key: { map, color, tolerance }`
  colour filter, the seeded `fill` and their soft colour edge `color_feather` are `ludion docs
  mesh-edit-regions`.

## The derived tree

```
meshes/
  mesh-0.mesh.yaml            pristine — never rewritten
  idol-lit.mesh-edit.yaml     the edit document
  idol-lit/                   wholly generated; every byte owned by the drift gate
    mesh.mesh.yaml            generated_from: ../idol-lit.mesh-edit.yaml
    textures/8f3c….png        the derived emissive map (content-hash named, 16 hex)
textures/                     pristine maps, unchanged, referenced from the derived document
```

**Never hand-edit anything under `out`.** It is regenerated from three inputs — the edit document,
the source mesh, and the source's own committed maps — and a byte that does not match what those
three produce is drift, not content. Re-run `ludion mesh apply` after any change to the edit
document; do not touch the derived files directly.

**A scene selects the edited variant by pointing at it — nothing else changes.**
`Mesh.shape: asset: meshes/idol-lit/mesh.mesh.yaml` is an ordinary path to an ordinary mesh document;
the pristine path keeps rendering the pristine asset. There is no override table and no variant
registry: switching which one an entity uses is a one-line scene diff, and either document loads
exactly like any other `kind: mesh` asset.

## The loop: authoring blind

A region is placed in the source mesh's own coordinates, which nothing renders directly — the whole
point is to place and verify it without a human looking at the screen.

1. **Read the mesh's own bounds.** `ludion check-scene <mesh>.mesh.yaml --bounds` prints the
   model-space box, its centre and size — the numbers a region's `at`/`radius`/`size` are placed
   from. A skinned mesh also lists its joints as a tree of model-space rest heads (the read of the
   rig); `--json` adds the box, counts and joints to the summary object.
2. **Render one view as a single-cell sheet.** `ludion render --model <mesh>.mesh.yaml --views front`
   (or `left`, `iso`, ...) at the width/height you will pick against — `pick` reads a single-cell
   image, so a multi-view sheet's pixel coordinates belong to none of its cells.
3. **Pick the surface point under a pixel of that sheet.** `ludion mesh pick <mesh>.mesh.yaml --view
   front --pixel X,Y` prints the model-space position, the triangle, the UV, and the texel each
   referenced map samples there — plus a ready `at:` block, a copy-paste starting point for a
   region's `at`.
4. **Write the op**, a `sphere`/`capsule`/`box` region around that point with a starting radius and a
   small `feather` — or, to light a patch of one colour (an eye, a rune), a `fill` seeded at the point.
5. **Apply it.** `ludion mesh apply <edit>.mesh-edit.yaml` derives and writes the tree, printing a
   per-op report.
6. **Read the report before looking at anything.** Zero texels painted is `L1177`, not a quiet
   no-op — the likeliest blind mistake is a region near the surface but not on it, and the report's
   world-space box confirms or refutes that without an image. `apply --view front` adds the box in
   that cell's pixels, to compare against the pixel you picked. It is an axis-aligned hull, so a fill
   or capsule on a curved patch reports a box larger than the patch.
7. **Render the emissive channel.** `ludion render --model <derived>/mesh.mesh.yaml --views front
   --channel emissive` should show one lit blob where you meant it; re-render `left`/`back`/`top`
   too — a region can paint through to a far-side surface, and nothing else checks that.
8. **Only then set the scene's intensity.** Point the entity's `Mesh.shape` at the derived document
   and raise `Mesh.emissive` above `0` (Rule one). The plain `ludion render --model <derived>/mesh.mesh.yaml` sheet already draws a mesh with an emissive map at unit intensity.

## The verbs

Both take `--json`.

| verb | does |
|---|---|
| `ludion mesh pick <mesh.yaml> --view <name> --pixel X,Y [--width W] [--height H] [--json]` | Rebuilds the exact `render --model` camera for that view and cell size (default `1920×1080`) and casts one CPU ray through the pixel's **centre**. Prints the model-space hit, the triangle, barycentrics, UV, the texel each referenced map samples there and a copyable `at:` block. `--view` is `front`, `back`, `left`, `right`, `top`, `iso` or `game`. A miss, an unknown view, or a pixel outside the extent is `L1175`. |
| `ludion mesh apply <edit.mesh-edit.yaml> [--view <name> [--width W] [--height H]] [--check] [--json]` | Re-derives the whole `out` tree. Without `--check`, writes it and prints the per-op report: texels painted, their share of the map, the world-space box painted, triangles spanned, and per written map its size and non-black texel count. With `--view`, each op also reports that box as an inclusive pixel span in that view's single cell (`clipped` when it leaves the cell). With `--check`, writes nothing and byte-compares against the committed tree, printing `DRIFT <path>: <reason>` — the `mesh-edit-drift` gate's own form. |

There is no `mesh new`, `mesh undo` (delete the op) or preview verb — the report and the emissive
channel are the preview.

## Gotchas

- **A pick's `--pixel` must come from a single-cell sheet at the same width and height.** Render with
  `--views <one view>` and pass matching `--width`/`--height` to `pick`; a pixel read off a multi-view
  grid belongs to none of `pick`'s camera.
- **A view is named by the world axis its camera sits on.** The `front` cell sits on `-Z` (labelled
  `FRONT -Z`); a mesh facing `+Z` (a generated asset commonly does) shows its **back** there.
- **A source mesh with no `uvs` cannot be edited** (`L1176`) — a map is sampled at vertex UVs; a
  primitive, voxel model or sculpt model carries none.
- **A derived mesh document duplicates the source's full float lists.** A generated idol's derived
  copy can be over 1 MB even though not one vertex changed; if the size gate's 1 MiB line trips,
  the allowlist entry (`scripts/size-allowlist.txt`, with its reason spelled out) lands in the same
  commit as the derived file, never a follow-up.
- **`out` is refused before anything is written** if it holds a file the derive does not name, or a
  tree whose `generated_from` names another edit — one edit per `out`. Maps this edit wrote earlier
  and no longer produces are removed once the new tree is in place; a failed write leaves the
  previous tree whole.

## Codes

| code | for |
|---|---|
| `L1173` | Mesh-edit document structure — missing/wrong-typed key, unknown key with its nearest match, unrecognized `op`/region shape, a capsule's retired `from`/`to`, or a `map` this engine does not paint. Also carries the drift report. |
| `L1174` | Mesh-edit document data — non-finite number, non-positive radius/`max_radius`/extent, a capsule with fewer than two `points`, colour or strength outside `[0, 1]`, negative `feather`, `tolerance` or `color_feather`, an `out` that escapes the document, a `source` that is absolute, a `source` inside the edit's own `out`, `size` on a map that already exists. |
| `L1175` | `pick` resolved to no surface — no triangle under the pixel, an unknown view, or a pixel outside the assumed extent. |
| `L1176` | The edit cannot apply to its source — no `uvs`, a target map with no `size` to create it, a `key`/`fill` reading a map nothing provides, or `out` holding files the derive did not write or a tree another edit owns. |
| `L1177` | An op painted nothing — its region covers no texel of the surface (names the op index and the region's box against the mesh's own bounds), or reaches texels at zero weight (a `strength` of 0). |
| `L1178` | A colour match failed — a `key` rejected every texel its region reaches (names the nearest distance and that texel's colour), or a `fill`'s seed is off the surface or outside `max_radius`. |

## Next

- `ludion docs mesh-edit-regions` — the polyline capsule, `key` and seeded `fill`.
- `ludion docs materials` — what `emissive`/`metallic`/`roughness` and their maps mean to the renderer.
- `ludion docs generation-runs` — where a generated mesh's maps and provenance come from.
- `ludion docs sculpt-craft` — the region-primitive vocabulary this format reuses, read as art direction.
- `ludion docs scene-format` — the strict-YAML document family a mesh document belongs to.
