# import-textured fixture

A hand-made, two-cube glTF used to exercise the importer's full five-map metallic-roughness path
(`Plan/textured-pbr.md`, G7 fixtures): `baseColorTexture`, `metallicRoughnessTexture`,
`normalTexture`, `occlusionTexture`, `emissiveTexture`, on both cubes. `tangent-cube` carries an
authored `TANGENT` accessor (exporter convention: `cross(normal, tangent) * w` points image-up,
so `w = -1` here); `computed-cube` carries none, so the importer's compute-tangent path runs on
it. `tangent-cube`'s base-color texture and `computed-cube`'s base-color texture (`base`/`base-dup`)
are pixel-identical under different names, to exercise content-hash dedup.

Regenerate with:

```
node examples/import-textured/make-fixture.mjs
```

The script is deterministic (no timestamps, no randomness, stable key order) — running it twice
produces a byte-identical `scene.gltf`.

The six PNG maps are synthetic, generated in the script from flat colour math (checkers,
gradients, a hand-rolled normal-map ridge pattern whose ridges tilt in both tangent x and y, a
radial falloff, a solid-plus-square emissive mask). They are self-made and carry no third-party licence.
