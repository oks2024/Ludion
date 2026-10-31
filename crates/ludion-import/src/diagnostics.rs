//! The importer's diagnostics: `L1201` (unreadable/invalid glTF), `L1202` (a feature with no
//! Ludion mapping), `L1203` (output write failure), `L1204` (an external source refused), `L1206`
//! (a clip over its error bound), `L1207` (invalid animation extras) and `L1208` (a clip that never
//! leaves the rest pose), and `L1239` (an unmapped optional extension, a warning). Each aims at the rustc bar — what is wrong, where, and what to do — and `L1202` always names the offending
//! node/mesh/accessor.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// `L1201` — the glTF input could not be read, parsed, or resolved.
pub(crate) fn invalid_gltf(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_INVALID_GLTF, detail.into()).with_help(
        "check the path and that the file is a valid glTF export; external buffers/images must sit \
         in the `.gltf`'s directory or below it, at the URIs it names (a `.glb` or `data:` URIs \
         have no external deps)",
    )
}

/// `L1201` — the file parsed but its data is inconsistent (a non-finite number, an accessor that
/// does not fit its buffer view, attributes of different lengths, an index past the vertex count).
/// `detail` names the element.
pub(crate) fn malformed(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_INVALID_GLTF, detail.into()).with_help(
        "the file parsed but its data is inconsistent — re-export it from the authoring tool (glTF \
         requires finite numbers, unit rotation quaternions, accessors that fit their buffer views \
         with a stride at least one element wide, attributes of one length, and indices below the \
         vertex count)",
    )
}

/// `L1204` — an external buffer or image URI is outside what the import may read. `kind`/`index`
/// name the source, `reason` says why the URI is refused.
pub(crate) fn uri_refused(kind: &str, index: usize, uri: &str, reason: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_URI_REFUSED,
        format!("{kind} {index} references `{uri}`, which {reason}"),
    )
    .with_help(
        "keep external buffers and images in the `.gltf`'s directory or below it, referenced by \
         relative URIs such as `textures/albedo.png`, or export a `.glb` with everything embedded",
    )
}

/// `L1202` — a glTF feature has no Ludion mapping. `what` names the feature; `offender` names the
/// node/mesh/accessor it appears on (the rustc-bar "where").
pub(crate) fn unsupported(what: &str, offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!("{what} on {offender} has no Ludion mapping"),
    )
    .with_help(
        "re-export without it (put every texture on UV set 0 with one wrap mode per texture, drop \
         morphs), or pass `--allow-lossy` to import the \
         supported subset and leave this out",
    )
}

/// `L1202` — a `BLEND` alpha mode has no mapping; `MASK` is the mapped alternative. `offender`
/// names the primitive it appears on.
pub(crate) fn blend_unsupported(offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "a `BLEND` alpha mode on {offender} has no Ludion mapping (meshes draw opaque or \
             alpha-cutout)"
        ),
    )
    .with_help(
        "switch the material to alpha clip (`MASK`), which maps onto `alpha_mode: mask`, or pass \
         `--allow-lossy` to import it opaque",
    )
}

/// `L1202` — a glTF camera cannot ride a prefab (`Camera` is a singleton, `L1229`).
pub(crate) fn camera_unsupported(offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "a camera on {offender} has no Ludion mapping (a prefab carries no Camera; place the \
             camera in the scene that instances it)"
        ),
    )
    .with_help(
        "place the camera in the scene that instances it and re-export without it, or pass \
         `--allow-lossy` to import the model and leave the camera out",
    )
}

/// `L1202` — a tinted emissive is baked into a map scaled by `baseColorFactor`, so a channel whose
/// base is (near-)zero cannot carry a non-zero `emissiveFactor`. Names the material and the channel.
pub(crate) fn emissive_zero_base_channel(
    material: &str,
    channel: &str,
    emissive: f32,
    base: f32,
) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "{material}'s emissiveFactor {channel} {emissive} sits on a baseColorFactor {channel} of \
             {base}, which is too small to carry it (emission is baked relative to the base colour)"
        ),
    )
    .with_help(
        "give `baseColorFactor` a larger value in that channel (at least 1/255), or zero the \
         emissive there, or pass `--allow-lossy` to import the rest and leave the emissive out",
    )
}

/// `L1202` — a tinted emissive is baked into an emissive map, which needs `TEXCOORD_0` to be
/// sampled. `material` names the offending material.
pub(crate) fn emissive_without_uvs(material: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "{material}'s `emissiveFactor` is not a scalar multiple of `baseColorFactor`, so it needs \
             a baked emissive map, but the primitive has no TEXCOORD_0 to sample it with"
        ),
    )
    .with_help(
        "export the mesh with UVs (TEXCOORD_0), or make `emissiveFactor` a scalar multiple of \
         `baseColorFactor`, or pass `--allow-lossy` to import the rest and leave the emissive out",
    )
}

/// `L1202` — a joint's rest (relative to the skinned mesh's frame, or folded through non-joint
/// nodes) shears, which a translation/rotation/scale rest cannot hold. `frame` says what the rest
/// is relative to.
pub(crate) fn rest_shears(offender: &str, frame: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "the rest of {offender} {frame} shears (a non-uniform scale above a rotation); a joint \
             rest holds only translation, rotation and scale, so the joints from here down deform \
             off the source"
        ),
    )
    .with_help(
        "apply the scale in the DCC tool (Blender: select the object or armature, Ctrl+A, Scale) or \
         keep non-uniform scale off nodes above a rotated joint, then re-export; or pass \
         `--allow-lossy` to import the rest without its shear",
    )
}

/// `L1202` — a node's authored `matrix` shears, which a `Transform` cannot hold.
pub(crate) fn node_matrix_shears(offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "the `matrix` of {offender} shears (a skew, or a non-uniform scale above a rotation); a \
             `Transform` holds only translation, rotation and scale, so it imports without the shear"
        ),
    )
    .with_help(
        "apply the scale and clear the skew in the DCC tool (Blender: select the object, Ctrl+A, \
         Scale), then re-export; or pass `--allow-lossy` to import without the shear",
    )
}

/// `L1202` — a joint's rest is singular (a zero scale on an axis, or near-parallel axes), which a
/// translation/rotation/scale rest cannot hold; the subtree imports collapsed to a point.
pub(crate) fn rest_collapses(offender: &str, frame: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "the rest of {offender} {frame} is singular (a zero scale on an axis, or axes that \
             are near-parallel), which a translation/rotation/scale rest cannot hold; it imports with scale 0 and no \
             rotation, so the joints from here down collapse to a point at its origin, even if the \
             source only flattens them"
        ),
    )
    .with_help(
        "if a node is scaled to 0 on every axis to hide part of the rig, pass `--allow-lossy`: the \
         subtree imports collapsed to a point, which is how it displays. If the source flattens \
         the subtree (one axis at zero), apply the scale in the DCC tool (Blender: Ctrl+A, Scale) \
         or avoid a zero axis, then re-export",
    )
}

/// `L1202` — an animation channel on a joint whose rest was folded through a non-joint node or
/// re-expressed in the mesh frame, so the clip cannot drive it.
pub(crate) fn channel_on_reframed_joint(what: &str, offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!("{what} on {offender} has no Ludion mapping"),
    )
    .with_help(
        "animate the joint itself, not through a helper node or a non-identity mesh frame; or apply \
         transforms in the DCC tool (Blender: Ctrl+A) so the helper is identity and re-export; or \
         pass `--allow-lossy` to import the rest pose and drop this channel",
    )
}

/// `L1202` — a node parented under a skin joint has no mapping: joints fold into the mesh's
/// `joints:`, so nothing can hang off one.
pub(crate) fn node_under_joint(offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!("a node parented to a skin joint on {offender} has no Ludion mapping"),
    )
    .with_help(
        "move the node out of the joint chain (parent it to the armature's parent or the scene \
         root), or make it a joint of the skin; or pass `--allow-lossy` to import the rest and \
         leave the node out",
    )
}

/// `L1202` — a non-joint node between two joints of a skin's chain has no mapping; the joints
/// under it keep its transform in their rest.
pub(crate) fn node_inside_joint_chain(offender: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_UNSUPPORTED_FEATURE,
        format!(
            "{offender} is a non-joint node inside a skin's joint chain and has no Ludion mapping \
             (the node is dropped; the joints under it keep its transform, folded into their rest)"
        ),
    )
    .with_help(
        "parent the joints to each other directly (Blender: bone to bone, not through an Empty or \
         another object) and re-export; or pass `--allow-lossy`: that keeps the rest pose, but \
         drops a clip channel on the joint directly under the node",
    )
}

/// `L1203` — a file or directory in the output tree could not be written.
pub(crate) fn output_io(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_OUTPUT_IO, detail.into()).with_help(
        "check that the `--out` directory's parent exists and is writable, and disk space",
    )
}

/// `L1205` — `count` vertices of `primitive` have four weights summing to zero and were bound to
/// the skin's `root` joint.
pub(crate) fn unweighted_vertices(count: usize, primitive: &str, root: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_SKIN_UNWEIGHTED,
        format!("{count} vertices of {primitive} have no weight; bound to root joint `{root}`"),
    )
    .with_help(
        "weight-paint those vertices in the DCC tool (every vertex of a skinned mesh needs an \
         influence) and re-export; until then they follow the root joint rigidly",
    )
}

/// `L1239` — an optional extension in `extensionsUsed` has no Ludion mapping and was ignored.
pub(crate) fn extension_ignored(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_EXTENSION_IGNORED,
        format!("the optional extension `{name}` has no Ludion mapping and was ignored"),
    )
    .with_help(
        "re-author the asset without it, or bake its effect into data the importer maps (a \
         base-colour map, an emissive factor); the import itself succeeded",
    )
}

/// `L1206` — a reduced clip strays from the source motion beyond the model-space bound. `detail`
/// names the animation, joint, frame, channel, error and bound.
pub(crate) fn clip_error_bound(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_CLIP_ERROR_BOUND, detail.into()).with_help(
        "key the motion more densely or less extremely in the DCC tool, or raise the matching \
         `bound_rotation_deg`/`bound_translation`/`bound_scale` in the animation's `extras` \
         (Blender: a custom property on the Action, exported with `export_extras=True`)",
    )
}

/// `L1206` — a CUBICSPLINE interval curves more than the importer's finest subdivision can follow
/// within the reduce tolerance, so the clip would stray from the source between its frames.
/// `detail` names the animation, joint, channel and the interval.
pub(crate) fn cubic_too_curved(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_CLIP_ERROR_BOUND, detail.into()).with_help(
        "key the curve more densely or export it LINEAR, or raise the matching \
         `reduce_rotation_deg`/`reduce_translation`/`reduce_scale` in the animation's `extras` \
         (Blender: a custom property on the Action, exported with `export_extras=True`)",
    )
}

/// `L1208` — every channel of an animation stays within the reduce tolerance of rest, so the clip
/// would have no tracks. `label` names the animation.
pub(crate) fn clip_at_rest(label: &str) -> Diagnostic {
    Diagnostic::new(
        code::IMPORT_CLIP_AT_REST,
        format!("{label} keys no joint away from its rest pose, so it would have no tracks"),
    )
    .with_help(
        "no joint moves beyond the reduce tolerances (`reduce_rotation_deg`, `reduce_translation`, \
         `reduce_scale`); delete the action in the DCC tool, key the motion you meant to export, or \
         lower the `reduce_*` values in its `extras` (Blender: a custom property on the Action, \
         exported with `export_extras=True`)",
    )
}

/// `L1207` — an animation's `extras` has an unknown key or an invalid value. `detail` names the
/// animation and the key.
pub(crate) fn clip_extras_invalid(detail: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::IMPORT_CLIP_EXTRAS_INVALID, detail.into()).with_help(
        "animation `extras` may hold `loop` (bool) and finite positive numbers for \
         `reduce_rotation_deg`, `reduce_translation`, `reduce_scale`, `bound_rotation_deg`, \
         `bound_translation` and `bound_scale`; fix or remove the key and re-export",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_special_refusals_have_no_whitespace_runs() {
        for found in [
            camera_unsupported("node 0"),
            rest_shears("node 1", "folded through node 2"),
            rest_collapses("node 1", "folded through node 2"),
            node_inside_joint_chain("node 2"),
            channel_on_reframed_joint("a channel", "node 3"),
            node_under_joint("node 4"),
        ] {
            let help = found.help.as_deref().unwrap_or_default();
            assert!(!found.message.contains("  "), "{}", found.message);
            assert!(!help.contains("  "), "{help}");
        }
    }
}
