# import-node-clips fixture

A hand-made three-part hinge glTF (`make-fixture.mjs`) for the importer's node clips (Clockout
request 0016): a glTF animation whose channels sit on nodes that become prefab parts imports as a
`kind: animation` clip whose tracks are named by the part, and the prefab lists it in `clips:`.

- `base` (slab, root) → `lid` (slab whose node origin is its hinge edge, so a rotation about X swings
  it about that edge) → `latch` (small cube, the lid's child).
- `open` — LINEAR: the lid rotates 0 → −100° about X in four keys, the latch slides back.
- `snap` — STEP: the lid and latch keyed on twos at 12 fps; every key after the first keeps
  `ease: step`.

The fixture imports in default mode with no warning; its committed `expected/` tree (the prefab with
its `clips:` map, three meshes, `clips/open.anim.yaml`, `clips/snap.anim.yaml`) is byte-gated by
`node_clips_import_reproduces_committed_tree_in_default_mode` (`crates/ludion-cli/tests/import.rs`).
Regenerate with:

```
node examples/import-node-clips/make-fixture.mjs
```

Then re-import it beside a copy named `scene.gltf` so `generated_from` reads `../scene.gltf`, and copy
the output over `expected/`. The geometry is generated in the script and carries no third-party
licence.
