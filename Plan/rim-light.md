# Plan: Per-mesh rim light

> Status: **MECHANIZED** (Serious Isaac request 0010). Usage is `ludion docs materials` (`## Rim light`);
> the gallery scene is `examples/gallery/rim-light.yaml`.

## Why

A dark, low-key interior with figures close to the wall's value loses their silhouettes: the key light
is dim on purpose, so nothing separates a figure from what is behind it. Serious Isaac asked for a
per-mesh view-angle highlight (request 0010) that keeps characters readable without lighting the
room. The engine had emissive (view-independent) and wrap diffuse (light-dependent) but nothing that
depends on the angle to the eye. The alternative the request names, linking a light to a mesh, is a
per-light cost for a per-mesh effect and was not taken.

## Decisions

1. **A Fresnel term.** `Mesh.rim { color, strength, power }` adds `color · strength · (1 − n·v)^power`
   to the shaded colour. It is unlit and emissive-like. Absent, `strength: 0` or a black `color` renders
   byte-identically to a scene without it.
2. **Not scaled by ambient or haze.** Authored and dumb; the game tunes it per kit.
3. **Inherited down `Parent`.** A mesh without its own `Mesh.rim` takes its nearest ancestor's.
   Limit: a `Mesh`-less root cannot carry it — put the rim on the root's `Mesh` or on every part.
4. **One document-level `rim:`** on `kind: voxel`, `kind: sculpt` and `kind: mesh` documents.
   Precedence: the document's rim, then `Mesh.rim`, then the inherited one. Not per palette entry: the
   per-vertex attribute slots are at the cap. Assembly parts are `kind: part` documents with no `rim`
   key, so an assembly's parts take the entity's own or inherited `Mesh.rim`. A character's rim can come
   from its `.mesh.yaml`. Out-of-range values are `L1114` (voxel), `L1157` (sculpt), `L1111` (mesh);
   `Mesh.rim` colour is `L1263`.
5. **Who rims.** Viewmodel meshes take it, with the drawn camera as the eye; skinned meshes rim with
   their skinned normals. Sprites, particles and text do not.
6. **Added into the colour target exactly where emissive is**, so the medium and fog treat it as
   emissive (request 0011's per-mesh medium exemption covers it unchanged). Excluded from probe
   captures: it is view-dependent, and it would break the relight's emissive split.
7. **A real per-instance lane at `@location(20)`.** `max_vertex_attributes` rose 20 → 21 and the mesh
   instance stride 192 → 208 bytes, the cull pass included.

## Out of scope / deferred

- **Per-palette-entry rim.** Trigger: a game needs a rim on part of one model and cannot split it into
  an assembly. Needs a vertex attribute slot or a per-model table first.
- **Light linking.** Trigger: a look that must rim only against one light's side; a per-light cost.
- **Sprite and particle rim.** Trigger: a sprite kit that needs the edge and cannot bake it in the art.
- **One `Parent` walk in extract.** Extract walks `Parent` twice for a parented mesh (`inherited_rim`
  beside `inherited_fade`). Trigger: the M2 500-enemy horde — measure extract there and fuse the two
  walks into one if it shows.

## Status — request 0010

- **10a** (`4fe27e9`): the `Mesh.rim` component (`L1263`), the shader term, the instance lane,
  inheritance down `Parent`, the viewmodel and skinned paths, the probe-capture exclusion.
- **10b** (`83224e4`): the document-level `rim:` on voxel, sculpt and mesh documents, the shared
  `ludion-scene` rim module and its precedence in the extract.
- **10c**: docs (`ludion docs materials`, components, voxel, sculpt, `scene-kind-documents`), this plan,
  and the gallery scene with its strength and power sweeps. Its macOS golden was blessed on the unpinned
  M5. Owed: the pinned M4 and Windows blesses of `rim-light.png` (`Plan/owed.md`); the M5 bless is not
  authoritative.
