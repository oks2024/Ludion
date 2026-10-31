# import-tinted-emissive fixture

A hand-made, three-quad glTF (`make-fixture.mjs`) for the importer's tinted-emissive bake (Clockout
request 0014). `Mesh.emissive` is a scalar on the surface's own colour, so a glTF `emissiveFactor`
that is not proportional to `baseColorFactor` is baked into an emissive map instead of refused:
`Mesh.emissive` takes the largest `emissiveFactor / baseColorFactor` ratio and the map carries the
rest as a hue.

- `card-textured` — white base, `emissiveFactor [1, 0.5, 0.25]` × `KHR_materials_emissive_strength` 3,
  an `emissiveTexture` ramp: the map is the ramp times the hue, `emissive: 3.0`.
- `card-constant` — base `[0.5, 0.5, 1]`, `emissiveFactor [0.25, 0.5, 2]`, no emissive texture: a 1×1
  map of the hue `[0.25, 0.5, 1]`, `emissive: 2.0`.
- `card-proportional` — control, `emissiveFactor` exactly 2 × the base: the scalar path, `emissive:
  2.0`, no baked map.

The fixture imports in default mode with no warning; its committed `expected/` tree is byte-gated by
`tinted_emissive_import_reproduces_committed_tree_in_default_mode`
(`crates/ludion-cli/tests/import.rs`), and `needs_gpu_tinted_emissive_renders_texture_times_factor`
renders `card-constant` against the same card with the factor spelled directly. Regenerate with:

```
node examples/import-tinted-emissive/make-fixture.mjs
```

Then re-import it beside a copy named `scene.gltf` so `generated_from` reads `../scene.gltf`, and copy
the output over `expected/` (`scene.yaml` there is hand-authored). The images are generated in the
script and carry no third-party licence.

The Windows render of `expected/scene.yaml` is the golden `goldens/windows/scene.png`, pinned by `needs_gpu_tinted_emissive_exit_gate` in `crates/ludion-cli/tests/import.rs`.
