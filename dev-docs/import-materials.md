---
description: ludion-import material, texture and geometry mapping — emissive proportionality, tangents
---
# import-materials

**Scope:** how `ludion_import` maps a primitive's material, textures and geometry. Hub: `ludion devdocs ludion-import`.

## Dataflow

A primitive's material maps through `mesh_reference`: `baseColorFactor` → `Mesh.color`,
`roughnessFactor`/`metallicFactor` → `Mesh.roughness`/`Mesh.metallic` directly, and
`emissiveFactor` through `proportional_emissive` — Ludion's `Mesh.emissive` is a scalar HDR
intensity on the surface's *own* color, never an independent tint, so a non-zero `emissiveFactor`
only maps when it equals `k × baseColorFactor.rgb` exactly on the f32 values (within an 8-ULP
tolerance that absorbs the rounding chain, documented at `EMISSIVE_REL_TOLERANCE`); otherwise it's
an `L1202` naming the material, and the emissive is dropped (not the whole material) under
`--allow-lossy`. `KHR_materials_emissive_strength` multiplies `emissiveFactor` first, so the product is
what must be proportional and what lands in `Mesh.emissive`. Its five metallic-roughness maps map through `material_textures`: each referenced
image re-encodes through `ludion_assets::decode_png`/`encode_png` and is written as
`textures/<content hash of the canonical PNG bytes>.png` — identical pixels are **one** committed
file whatever the exporter named them, and an image no supported slot references is not written at
all (a referenced JPEG is `L1202`). The mesh document's reference carries the path *relative to its
own directory* (`../textures/<hash>.png`), the sampler's wrap mode, and `normalTexture.scale` /
`occlusionTexture.strength` as its factor.

Geometry maps through `build_mesh_document`: triangle-list, non-sparse, non-morph primitives only;
positions are required, normals/UVs carried when present (normals omitted means faceted normals
computed at load, per `ludion-scene`). Tangents are imported verbatim from `TANGENT`, generated
(`tangents.rs`, glTF handedness: `cross(n, t) * w` points toward decreasing `v`, image-up, as
Blender's exporter writes it) when a normal map needs a frame and the document carries none, and
absent otherwise; a generated frame on a primitive without `NORMAL` writes the faceted normals out beside it, so the
committed frame is orthogonal to the committed normals.
