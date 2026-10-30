# import-cutout fixture

A glTF exported from Blender 5.2 by `make-fixture.py`, used to exercise the importer's cutout and
double-sided path (Clockout request 0010): four objects, each one material.

- `grating` — a plane whose material carries a packed bar-and-hole image through Blender's
  mask node chain (Image Alpha, Less Than 0.25, Subtract from 1, Principled Alpha) with backface
  culling on, so it exports `alphaMode: MASK`, `alphaCutoff: 0.25` and a single-sided face.
  The mesh document gains `alpha_mode: mask` and `alpha_cutoff: 0.25`.
- `flap` — a tilted plane, culling off: `doubleSided`, so `double_sided: true`.
- `tape` — a long thin strip, culling off: `double_sided: true`.
- `shade` — an open cylinder (no end caps) as a lamp shade, culling off: `double_sided: true`.

The fixture imports in default mode, no `--allow-lossy` and no warning, and its committed
`expected/` tree is byte-gated by `cutout_import_reproduces_committed_tree_in_default_mode`
(`crates/ludion-cli/tests/import.rs`). Regenerate the glTF with:

```
blender --background --factory-startup --python examples/import-cutout/make-fixture.py
```

Then re-import it beside a copy named `scene.gltf` so `generated_from` reads `../scene.gltf`, and
copy the output over `expected/`. Blender's exporter output is not byte-stable across versions, so
the glTF is committed and only the import of it is gated. The image is generated in the script and
carries no third-party licence.
