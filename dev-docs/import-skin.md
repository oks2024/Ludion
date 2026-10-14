---
description: ludion-import skin mapping — joint folding, rest poses, weights and their L1202/L1205 cases
---
# import-skin

**Scope:** glTF skins in `ludion_import` (`skin.rs`, `bind_skins`). Hub: `ludion devdocs ludion-import`; clips: `ludion devdocs import-animation`.

## Dataflow

Skins (`skin.rs`, `bind_skins`/`emitted_nodes` in `lib.rs`): a skinned node that wears a mesh binds
that mesh's document to the skin, and the skin's joint nodes fold into the document's `joints:`
instead of becoming entities. A joint's parent is its nearest ancestor in the same skin; joints
reorder parent-first (a stable no-op on an ordered export); a non-root rest is the node's TRS
verbatim, a root's is relative to the skinned mesh's frame (verbatim when the mesh node is an
identity sibling — the Blender rig layout — else `inverse(world(mesh)) × world(root)` in `f64` glam).
A non-joint node between two joints of the skin (`SkinBinding.between`) is dropped as an entity
(`L1202`, strict mode still refuses) but its transform is folded into the rest of the joint below
(`world(helper chain) × local(joint)`, `reframed`); `rest_from` is the one decomposition for a folded
joint and a re-expressed root, and a matrix that shears (a non-uniform scale above a rotation, which a
TRS rest cannot hold) is an `L1202` naming the joint and its consequence. A singular rest (zero scale
on an axis, including a zero-scale mesh frame on the root path) collapses to scale 0 with an identity
rotation and an `L1202`, never NaN. A node `matrix` is used as authored (`local`, a plain joint's rest),
so its shear reaches the same check, and an emitted node whose `matrix` shears is an `L1202` of its
own. The shear, collapse and chain-node `L1202`s each carry their own help.
`build_mesh_document` then maps `JOINTS_0`/`WEIGHTS_0` (slots remapped to joint indices, weights
renormalized and sorted heaviest first, zero-weight slots bound to joint 0); a vertex whose four
weights sum to zero binds fully to the root and is one `L1205` per primitive. An unskinned mesh
ignores `JOINTS_*`/`WEIGHTS_*`. `L1202`: over 256 joints, a second joint/weight set, a mesh shared by
skinned and other nodes, a node under a joint (dropped with its subtree), a mesh/camera/light on a
joint. `MeshSkin::rest_model_matrices` (`ludion-scene`) composes the rest pose in model space (`None` unless the joints are parent-first).
