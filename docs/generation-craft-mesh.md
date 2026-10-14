---
description: The image-to-mesh reduction order, the textured bake graph, and the four-view loop
---

# Generation craft: mesh

**Scope:** the mesh half of `ludion docs generation-craft`, split out at 11.3 KB — the reduction
order a raw decode needs before `gen accept` can import it, what the textured bake graph produces
and how its outputs land as `Mesh` document texture maps, and the four-view loop that conditions a
run on an authored sculpt. The prompt/seed/image craft that composes with an accepted reference
stays in `ludion docs generation-craft`; the accept step itself
(the directory `--out`, the `origin:` splice, the `L1112` vertex budget) is
`ludion docs generation-runs`; what the produced maps mean to the renderer is `ludion docs materials`.

Every number below is marked **measured** (read off a run on the reference box — a 24 GB card) or
**convention**, the same rule as the parent topic.

## The density rule: remesh, then decimate

**This is the one measured craft rule of the mesh half.** A raw mesh decode is enormous and not
clean: ≈22.5 M faces with ≈219 k non-manifold edges. Feeding that straight to `DecimateMesh`
**shatters** it. Measured on one subject, all from the same run:

| chain | faces | components | boundary edges | non-manifold edges |
|---|---:|---:|---:|---:|
| raw decode | 22 545 666 | — | 225 449 (0.67 %) | 219 310 |
| `DecimateMesh` 20 000 straight off the raw decode | 12 409 | **456** | **11 717 (47.9 %)** | 19 |
| `RemeshMesh` 256 **then** `DecimateMesh` 5 000 | **4 962** | **9** | 48 (0.64 %) | **0** |

Half the edges of the decimate-only mesh are boundary edges across 456 components — loose triangle
sheets, not a solid. The remeshed one is closed but for 48 edges and has no non-manifold edge at all.
`DecimateMesh` also **undershoots badly on messy input**: 12 409 faces against a 20 000 target, where
it lands on 4 962 against 5 000 after a remesh.

So: **`RemeshMesh` (256, drop small components) before `DecimateMesh`, always.** The reduction pass
costs ≈21 s (measured), which is nothing against the decode it follows. This is the graph's job, not
Ludion's: the reduction nodes sit where the mesh already is, and no decimator is written on our side.

**The UDF remesh can leave an inverted inner shell that `drop_small_components` does not catch**
(measured: 128 faces of negative volume inside the plinth, on the same subject as the table above).
`drop_inverted_components: true` on `RemeshMesh` removes it and returns the face budget to the real
surface; the committed `trellis2-img2mesh.json` workflow sets it.

## The textured mesh graph

The committed `trellis2-img2mesh.json` (TRELLIS.2) takes the accepted reference image through decode,
remesh/decimate, UV unwrap and a texture bake, in that fixed order — **reduce → unwrap → bake**
(measured): the texture bake refuses a mesh with no UVs, so unwrap must follow reduction, and the
dense decode cannot be dropped once the mesh is remeshed, because both the texture decode and the
bake's reference mesh still need it.

**A full textured run is expensive; re-baking after it is not** (measured): the chain runs cold in
299 s at 15.7 GB peak, but once the samplers have run, resubmitting the same graph with only the
reduction or bake inputs changed costs 76 s — ComfyUI caches every upstream node whose inputs did not
change. Iterate `RemeshMesh`/`DecimateMesh` targets and bake sizes against that cheaper number, not
the full 299 s.

**Measured result at `RemeshMesh` 256 → `DecimateMesh` 5 000 (qem):** 4 977 triangles, 1 component,
55 boundary edges; 4 643 glTF vertices, which is 2 326 unique positions plus UV-seam duplicates — the
unwrap roughly doubles the vertex count the mesh's own geometry has. Budget a `DecimateMesh` target
against `L1112`'s 10 000-*vertex* line with that doubling in mind, not against the triangle count.

**The bake is three nodes packed into one glTF material by `Apply Texture`:** `Bake Texture` (base
colour, metallic and roughness), `Bake Normal Map`, and `Bake Occlusion`. **Tangents and normals are
only written into the GLB when the normal map is connected** (measured) — an untextured or
normal-less bake produces a mesh the importer reads with faceted normals and no tangent frame, same
as any other mesh without `TANGENT`. Occlusion, roughness and metallic arrive from the bake packed in
one image (R/G/B channels), so the importer lands `occlusion_texture` and `metallic_roughness_texture`
pointing at the **same** committed file.

**The texture model's metallic channel is not zero on a stone subject** (measured, the idol): 0 for
≥ 75 % of texels, but > 0.5 on 14.3 % of them. An accepted mesh is ordinary content from that moment
on — the fix is lowering the `Mesh` document's own scalar `metallic` factor (which multiplies the
map), not a regeneration.

**Baked AO at the upstream template's ray length (0.71) is bimodal on an enclosed subject** (measured:
≈25 % of texels fully black on the idol); the node's own default is 0.5, and the committed workflow's
0.71 is a template value worth revisiting per subject rather than trusting blind.

**A TRELLIS.2 subject arrives facing +Z** (measured, the idol): the model sheet's `FRONT -Z` cell
shows its **back**, and the face is in the `BACK +Z` cell. Read the sheet by what is in the cells, not
by their labels, or rotate the mesh's own `Transform` once and for all after accepting it — an
accepted mesh is ordinary content.

**An accepted mesh is megabytes, and every one needs its own size-allowlist pair** (measured, the
idol at `DecimateMesh` 5 000): ≈1.2 MB of canonical vertex text for the `.mesh.yaml`, plus three
1024² maps at ≈0.8–1.3 MB each. Both lines go into `scripts/size-allowlist.txt` with the reason
spelled out — the text is the decimate target's cost, the maps are the bake extent's — because the
size gate's 1 MiB line exists to catch an asset nobody meant to commit, and a generated mesh is
exactly the kind of asset that has to be meant.

**Texture extent is a recipe override, not a workflow edit:** `Unwrap UVs`, `Bake Texture` and
`Bake Normal Map` each take a size. Three 2048² maps are ≈ 9 MB of PNG once content-hashed and
committed, which is why the idol recipe bakes at 1024.

## The multi-view loop: conditioning on an authored form

`Pixal3DMultiViewConditioning` takes four optional square views — `front`, `left`, `back`, `right`,
a 90° orbit at elevation 0 — plus one `fov` in degrees, and the committed
`pixal3d-multiview.json` binds them to four `LoadImage` nodes a recipe fills from `images:`. This is
the path that makes a generated mesh **directable**: the silhouette is the brief, not a prompt.

**`render --model` is the rig, once three things are undone.** Read off the sheet code: every view
shares one camera distance and one target and sits at elevation 0 on a horizontal axis, at
`fov_y` 45° — a true orbit at one scale, which is exactly what the conditioning assumes. There is no
`--yaw`; **one `--views <single>` render per view** is how you get four images, and each is a
one-cell sheet carrying a label banner, a ground plane and a sky, with the subject fitted to its
*bounding sphere* so it fills only about two thirds of the cell.

**The fix is one fixed crop plus a mask, both in the graph** (measured, and the committed workflow's
shape):

1. **One `ImageCrop` rect, identical for all four views, centred on the image centre.** The sheet's
   principal point *is* the orbit centre, and cropping a pinhole image about its principal point
   yields a valid pinhole image with a narrower field — which is why one fixed rect keeps the four
   views a consistent rig. A per-view `ImageCropToMask` does **not**: it re-crops to each view's own
   mask bounding box, normalizing the scale per view, and the conditioning's own tooltip asks for
   "the same scale in every view".
2. **Size the rect so the widest silhouette spans `rect ÷ 1.1`** — the rig frames a unit cube at
   1/1.1. Derive it per subject from one probe render: render one view, read the subject's pixel
   extents, take the largest across the four. The committed workflow is 1024² renders, a 760 rect at
   (132, 132).
3. **Then `fov = 2·atan(tan(fov_y/2) · rect / width)`** — 34.16° for that rect, which is what the
   committed recipe's `Conditioning` node carries. The crop and the `fov` are **one fact in two
   inputs**: change either and change both.
4. **Mask onto black.** BiRefNet `RemoveBackground` → `ImageCompositeMasked` over a black
   `EmptyImage` removes ground, sky and label in one step (the label is outside the crop anyway).
   **Measured:** the alpha route (`InvertMask` — `JoinImageWithAlpha` inverts its mask — feeding a
   4-channel image the conditioning premultiplies) produced a mesh within 2 vertices of the composite
   route. Prefer the composite: fewer nodes, and its output is an image you can look at.

**Measured, and the finding that matters: four-view conditioning reconstructs a silhouette, it does
not invent surface.** What comes back has no more detail than what went in, and usually less. On the
gallery's 22-part authored creature the reconstruction is recognisable — four legs, tail, horned
head, every axis within 17 % of the authored box — but lumpy and softened. On a five-primitive smooth
head blank it failed: the first run collapsed to a solid of revolution of the side silhouette, and
sharpening the blank until every feature cleared its neighbours in profile recovered the horns and
the brow ridge and still lost the muzzle. **So the blank has to state the form in profile as well as
head-on, and buying a form this way buys geometry, not detail.**

**A multi-view graph for a sculpt part stops at `SaveGLB` — no unwrap, no bake.** A `shape: mesh`
sculpt part samples the triangles as a distance field on a 64-cell grid over the mesh's box
(`ludion docs sculpt-format`), so it reads form and never a texture; baking three maps a part
discards costs the bake stage and two size-allowlist entries for nothing. The reduction order above
still applies: `RemeshMesh` then `DecimateMesh`, and with no UV seams to duplicate vertices a
4 000-face target lands under 2 200 vertices and a `.mesh.yaml` of ≈0.4 MB — under the size gate,
where the textured idol needed two allowlist lines.

**`VaeDecodeStructureTrellis2` at `resolution: "64"` is not a tuning knob, it is a different budget**
(measured, same graph and subject): the upsample sampler goes from ≈1 s to **≈85 s per step**, so a
12-step stage is 17 minutes instead of 12 seconds, and the whole run goes from ≈80 s to over 20 min.
Reach for it only when a feature is provably being lost to the structure grid, never as a default.

## Rules

- **A generated mesh is credited "Built with DINOv3".** Both mesh generators condition on Meta's
  DINOv3 encoder. In the Ludion repository an accepted mesh gets a line in `THIRD-PARTY-NOTICES.md`,
  beside the licence copy; a game that ships one carries the phrase in its credits or about screen.

## Next

- `ludion docs generation-craft` — the image half: prompts, seeds, image-to-image.
- `ludion docs generation-runs` — `gen accept`'s mesh path, the vertex budget, provenance.
- `ludion docs materials` — what the baked maps mean to the renderer.
- `ludion docs sculpt-craft` — authoring the rough form the four-view loop conditions on.
