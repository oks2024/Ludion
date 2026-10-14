# Frozen stage fixture

A frozen tree of neutral content for the editor's tests: voxel models, a rig assembly with its parts
and two animation clips, a generation recipe, the model-stage scene and a mesh-edit tree. It
holds no game content, so the editor's suites and live smoke run on a bare checkout.

It is never regenerated and never edited in place. A test that needs a different file copies a new one.

Sources, by copy or by hand:
- `scenes/voxels/{brazier,beacon,lantern,tree,torch-post}.voxel.yaml`: byte-identical copies of the
  engine's example models and the engine test fixture.
- `scenes/voxels/rig.assembly.yaml`, `parts/rig-*.part.yaml`, `anims/rig-{walk,wave}.anim.yaml`: a
  hand-authored 11-part humanoid with a hung lamp, derived from the gallery ragdoll.
- `generation/recipes/lantern-portrait.gen.yaml`: a hand-authored recipe shaped like the CLI's
  generation fixtures.
- `scenes/meshes/plate-imported/`: the importer's tree shape around the mesh-edit plate fixtures
  (`crates/ludion-mesh-edit/tests/fixtures`).
- `scenes/model-stage.yaml`: the CLI template's stage with a voxel subject.

Readers: the vitest suites under `editor/app/src/editor/{voxel,generation,mesh}` and the ignored
live tests in `editor/app/src-tauri/src/stage.rs`.
