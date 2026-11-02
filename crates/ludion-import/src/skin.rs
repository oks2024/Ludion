//! The skin mapping: a glTF skin's joints onto the mesh document's `joints`, and a primitive's
//! `JOINTS_0`/`WEIGHTS_0` onto its `weights`/`bone_indices`. Joint nodes are folded into `joints:`
//! rather than emitted as entities (`build_entities`).

use std::collections::{BTreeMap, BTreeSet};

use gltf::accessor::{DataType, Dimensions};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::f64::sqrt;
use ludion_core::math::glam::{DMat3, DMat4, DQuat, DVec3};
use ludion_scene::{JointRest, MESH_JOINT_LIMIT, MeshJoint};

use crate::accessors::check_accessor;
use crate::diagnostics::{malformed, rest_collapses, rest_shears, unsupported};
use crate::sources::Buffers;
use crate::{dedup, named, require_finite};

const MAT4_F32: &[(Dimensions, DataType)] = &[(Dimensions::Mat4, DataType::F32)];

const IDENTITY: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// One skin mapped onto the mesh document's joints.
pub(crate) struct SkinBinding {
    /// The joints, parent-first.
    pub(crate) joints: Vec<MeshJoint>,
    /// The index in `joints` of each glTF joint slot (`JOINTS_0` values are glTF slots).
    pub(crate) gltf_to_joint: Vec<u32>,
    /// The first root joint's name — where a vertex with no weight is bound.
    pub(crate) root_name: String,
    /// The glTF node of every joint, folded out of the entity list.
    pub(crate) joint_nodes: Vec<usize>,
    /// Per joint (in `joints` order): its rest was re-expressed in the mesh frame for a root, or
    /// through the non-joint nodes above it, so a clip channel on it would need the same
    /// re-expression.
    pub(crate) reframed: Vec<bool>,
    /// The non-joint glTF nodes that sit between two joints of the skin; their transforms are
    /// folded into the rest of the joint below.
    pub(crate) between: BTreeSet<usize>,
}

/// Maps `skin`, worn by `mesh_node`, onto document joints. `Ok(None)` (with an `L1202` pushed to
/// `lossy`) when the skin cannot be carried and the mesh imports unskinned.
///
/// # Errors
/// `L1201` for a non-finite joint transform or inverse bind matrix, an inverse bind accessor that
/// does not fit its buffer, or a duplicate joint.
pub(crate) fn bind_skin(
    skin: &gltf::Skin,
    mesh_node: &gltf::Node,
    nodes: &[gltf::Node],
    parent_of: &[Option<usize>],
    buffers: &Buffers,
    lossy: &mut Vec<Diagnostic>,
) -> Result<Option<SkinBinding>, Vec<Diagnostic>> {
    let label = named("skin", skin.index(), skin.name());
    let slots: Vec<gltf::Node> = skin.joints().collect();
    let count = slots.len();
    if count > MESH_JOINT_LIMIT {
        lossy.push(unsupported(
            &format!("a skin with {count} joints (the cap is {MESH_JOINT_LIMIT})"),
            &label,
        ));
        return Ok(None);
    }
    if count == 0 {
        return Err(vec![malformed(format!("{label} has no joints"))]);
    }

    let mut slot_of = BTreeMap::new();
    for (slot, node) in slots.iter().enumerate() {
        if slot_of.insert(node.index(), slot).is_some() {
            return Err(vec![malformed(format!(
                "{label} lists node {} as a joint twice",
                node.index()
            ))]);
        }
    }
    // A joint's parent is its nearest glTF ancestor that is also a joint of this skin; the
    // non-joint nodes passed on the way (nearest first) fold into the joint's rest.
    let mut between: Vec<Vec<usize>> = vec![Vec::new(); count];
    let parent_slot: Vec<Option<usize>> = slots
        .iter()
        .enumerate()
        .map(|(slot, node)| {
            let mut passed = Vec::new();
            let mut current = parent_of[node.index()];
            while let Some(ancestor) = current {
                if let Some(&parent) = slot_of.get(&ancestor) {
                    between[slot] = passed;
                    return Some(parent);
                }
                passed.push(ancestor);
                current = parent_of[ancestor];
            }
            None
        })
        .collect();

    // Parent-first stable reorder: an already ordered skin (every Blender export) is unchanged.
    let mut order: Vec<usize> = Vec::with_capacity(count);
    let mut placed = vec![false; count];
    while order.len() < count {
        let next = (0..count)
            .find(|&slot| !placed[slot] && parent_slot[slot].is_none_or(|parent| placed[parent]))
            .expect("check_structure refused a node cycle, and joints are distinct");
        placed[next] = true;
        order.push(next);
    }
    let mut gltf_to_joint = vec![0_u32; count];
    for (joint, &slot) in order.iter().enumerate() {
        gltf_to_joint[slot] = joint as u32;
    }

    let inverse_binds = match skin.inverse_bind_matrices() {
        None => vec![IDENTITY; count],
        Some(accessor) => {
            if accessor.sparse().is_some() {
                lossy.push(unsupported("a sparse accessor", &label));
                return Ok(None);
            }
            check_accessor(
                &accessor,
                MAT4_F32,
                "inverse bind matrices",
                &label,
                buffers,
            )
            .map_err(|d| vec![d])?;
            if accessor.count() < count {
                return Err(vec![malformed(format!(
                    "{label} has {count} joints but {} inverse bind matrices",
                    accessor.count()
                ))]);
            }
            let reader = skin.reader(|buffer| buffers.get(buffer.index()));
            let matrices = reader.read_inverse_bind_matrices().into_iter().flatten();
            let mut flat = Vec::with_capacity(count);
            for matrix in matrices.take(count) {
                let columns = matrix.as_flattened();
                require_finite(columns, "inverse bind matrix", &label).map_err(|d| vec![d])?;
                let mut values = [0.0_f64; 16];
                for (value, &column) in values.iter_mut().zip(columns) {
                    *value = f64::from(column);
                }
                flat.push(values);
            }
            flat
        }
    };

    let mesh_local = mesh_node.transform().decomposed();
    let mesh_is_identity = mesh_local == ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
    let mut used = BTreeSet::new();
    let mut joints = Vec::with_capacity(count);
    let mut reframed = Vec::with_capacity(count);
    for &slot in &order {
        let node = &slots[slot];
        let offender = named("node", node.index(), node.name());
        let (translation, rotation, scale) = node.transform().decomposed();
        for (values, what) in [
            (&translation[..], "translation"),
            (&rotation[..], "rotation"),
            (&scale[..], "scale"),
        ] {
            require_finite(values, what, &offender).map_err(|d| vec![d])?;
        }
        let parent = parent_slot[slot].map(|parent| gltf_to_joint[parent]);
        let reexpress = parent.is_none()
            && !(mesh_is_identity && parent_of[node.index()] == parent_of[mesh_node.index()]);
        let folded = !between[slot].is_empty();
        reframed.push(reexpress || folded);
        let rest = match parent {
            None if reexpress => {
                // A root's rest is relative to the skinned mesh's frame. A zero-scale mesh frame
                // draws nothing and has no inverse, so its joints collapse with it.
                let mesh = world(mesh_node.index(), nodes, parent_of);
                let matrix = if singular(&mesh) {
                    DMat4::ZERO
                } else {
                    mesh.inverse() * world(node.index(), nodes, parent_of)
                };
                rest_from(matrix, &offender, "relative to the skinned mesh", lossy)?
            }
            _ if folded => {
                // The non-joint nodes above the joint (farthest first) are part of its rest.
                let matrix = between[slot]
                    .iter()
                    .rev()
                    .fold(DMat4::IDENTITY, |m, &n| m * local(&nodes[n]))
                    * local(node);
                let n = between[slot][0];
                let through = format!("folded through {}", named("node", n, nodes[n].name()));
                rest_from(matrix, &offender, &through, lossy)?
            }
            _ => match node_matrix(node) {
                Some(matrix) => rest_from(matrix, &offender, "from its own `matrix`", lossy)?,
                None => JointRest {
                    translation: translation.map(f64::from),
                    rotation: rotation.map(f64::from),
                    scale: scale.map(f64::from),
                },
            },
        };
        let base = node
            .name()
            .map_or_else(|| format!("joint-{}", node.index()), str::to_string);
        joints.push(MeshJoint {
            name: dedup(&mut used, &base, node.index()),
            parent,
            rest,
            inverse_bind: inverse_binds[slot],
        });
    }

    let root_name = joints
        .iter()
        .find(|joint| joint.parent.is_none())
        .map(|joint| joint.name.clone())
        .expect("a parent-first joint list starts with a root");
    Ok(Some(SkinBinding {
        joints,
        gltf_to_joint,
        root_name,
        joint_nodes: slots.iter().map(gltf::Node::index).collect(),
        reframed,
        between: between.into_iter().flatten().collect(),
    }))
}

/// How small a rest matrix's determinant may be, relative to the product of its axis lengths (its
/// Hadamard bound), before it counts as singular: zero scale on an axis, or axes near parallel.
const SINGULAR: f64 = 1e-9;

fn singular(matrix: &DMat4) -> bool {
    let linear = DMat3::from_mat4(*matrix);
    let det = linear.determinant();
    det * det
        <= SINGULAR
            * SINGULAR
            * linear.x_axis.length_squared()
            * linear.y_axis.length_squared()
            * linear.z_axis.length_squared()
}

/// A joint rest from a matrix: its decomposition, with an `L1202` pushed to `lossy` when the matrix
/// shears (a non-uniform scale above a rotation), which a translation/rotation/scale rest cannot
/// hold, or is singular (zero scale on an axis), which imports as scale 0 with an identity rotation.
/// `frame` says what the matrix is relative to, for the messages.
fn rest_from(
    matrix: DMat4,
    offender: &str,
    frame: &str,
    lossy: &mut Vec<Diagnostic>,
) -> Result<JointRest, Vec<Diagnostic>> {
    if singular(&matrix) {
        lossy.push(rest_collapses(offender, frame));
        return Ok(JointRest {
            translation: matrix.w_axis.truncate().to_array(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [0.0; 3],
        });
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    if !(scale.is_finite() && rotation.is_finite() && translation.is_finite()) {
        return Err(vec![malformed(format!(
            "{offender}'s rest transform {frame} is not finite"
        ))]);
    }
    let mut rotation = rotation;
    if decomposition_shears(&matrix, scale, rotation, translation) {
        // glam returns a non-unit quaternion for a sheared matrix; a rest rotation must be unit.
        rotation /= sqrt(rotation.dot(rotation));
        lossy.push(rest_shears(offender, frame));
    }
    Ok(JointRest {
        translation: translation.to_array(),
        rotation: rotation.to_array(),
        scale: scale.to_array(),
    })
}

/// Whether recomposing a matrix's decomposition fails to give the matrix back: the mark of a shear
/// (a non-uniform scale above a rotation, or a skew). The tolerance scales with the 3×3 block only:
/// the translation recomposes exactly, so letting it set the bound would hide a real shear under a
/// large translation.
fn decomposition_shears(matrix: &DMat4, scale: DVec3, rotation: DQuat, translation: DVec3) -> bool {
    let recomposed = DMat4::from_scale_rotation_translation(scale, rotation, translation);
    let magnitude = [matrix.x_axis, matrix.y_axis, matrix.z_axis]
        .iter()
        .flat_map(|axis| axis.truncate().to_array())
        .fold(1.0_f64, |m, e| m.max(e.abs()));
    !recomposed.abs_diff_eq(*matrix, 1e-6 * magnitude)
}

/// Whether a node's authored `matrix` shears. A singular or non-finite matrix is not a shear: the
/// former collapses, the latter has no decomposition to compare.
pub(crate) fn shears(matrix: &DMat4) -> bool {
    if singular(matrix) {
        return false;
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    scale.is_finite()
        && rotation.is_finite()
        && translation.is_finite()
        && decomposition_shears(matrix, scale, rotation, translation)
}

/// A node's authored `matrix` (column-major in the file), or `None` for a translation/rotation/
/// scale node.
pub(crate) fn node_matrix(node: &gltf::Node) -> Option<DMat4> {
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => {
            let mut flat = [0.0_f64; 16];
            for (value, column) in flat.iter_mut().zip(matrix.iter().flatten()) {
                *value = f64::from(*column);
            }
            Some(DMat4::from_cols_array(&flat))
        }
        gltf::scene::Transform::Decomposed { .. } => None,
    }
}

/// A node's model-to-world matrix: its ancestors' local TRS composed root-first, in `f64`.
fn world(index: usize, nodes: &[gltf::Node], parent_of: &[Option<usize>]) -> DMat4 {
    let mut chain = vec![index];
    while let Some(parent) = parent_of[*chain.last().expect("the chain starts non-empty")] {
        chain.push(parent);
    }
    chain
        .iter()
        .rev()
        .fold(DMat4::IDENTITY, |acc, &node| acc * local(&nodes[node]))
}

fn local(node: &gltf::Node) -> DMat4 {
    if let Some(matrix) = node_matrix(node) {
        return matrix;
    }
    let (t, r, s) = node.transform().decomposed();
    DMat4::from_scale_rotation_translation(
        DVec3::new(f64::from(s[0]), f64::from(s[1]), f64::from(s[2])),
        DQuat::from_xyzw(
            f64::from(r[0]),
            f64::from(r[1]),
            f64::from(r[2]),
            f64::from(r[3]),
        )
        .normalize(),
        DVec3::new(f64::from(t[0]), f64::from(t[1]), f64::from(t[2])),
    )
}

/// A primitive's skin weights in the document's form.
pub(crate) struct Weights {
    pub(crate) weights: Vec<f64>,
    pub(crate) bone_indices: Vec<u32>,
    /// Vertices whose weights summed to zero, bound to the root joint.
    pub(crate) unweighted: usize,
}

/// Maps `JOINTS_0`/`WEIGHTS_0` onto `binding`'s joints: weights renormalized to sum to one and
/// sorted heaviest first, a zero-weight slot bound to joint 0, and a vertex with no weight at all
/// bound fully to the root (index 0 after the parent-first reorder).
///
/// # Errors
/// `L1201` naming `label` for a negative or non-finite weight, a weighted slot naming a joint the
/// skin does not have, or the two attributes disagreeing in length.
pub(crate) fn map_weights(
    binding: &SkinBinding,
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    label: &str,
) -> Result<Weights, Diagnostic> {
    if joints.len() != weights.len() {
        return Err(malformed(format!(
            "{label}: JOINTS_0 has {} vertices but WEIGHTS_0 has {}",
            joints.len(),
            weights.len()
        )));
    }
    let mut out = Weights {
        weights: Vec::with_capacity(weights.len() * 4),
        bone_indices: Vec::with_capacity(weights.len() * 4),
        unweighted: 0,
    };
    for (vertex, (slots, values)) in joints.iter().zip(weights).enumerate() {
        let mut sum = 0.0_f64;
        for (&slot, &value) in slots.iter().zip(values) {
            let weight = f64::from(value);
            if !weight.is_finite() || weight < 0.0 {
                return Err(malformed(format!(
                    "{label}: vertex {vertex} has weight {value}, which is not a finite value of at least 0"
                )));
            }
            if weight > 0.0 && usize::from(slot) >= binding.gltf_to_joint.len() {
                return Err(malformed(format!(
                    "{label}: vertex {vertex} is weighted to joint slot {slot}, but the skin has {} joints",
                    binding.gltf_to_joint.len()
                )));
            }
            sum += weight;
        }
        if sum == 0.0 {
            out.unweighted += 1;
            out.weights.extend([1.0, 0.0, 0.0, 0.0]);
            out.bone_indices.extend([0; 4]);
            continue;
        }
        let mut pairs: [(f64, u32); 4] = std::array::from_fn(|k| {
            let weight = f64::from(values[k]);
            if weight > 0.0 {
                (weight / sum, binding.gltf_to_joint[usize::from(slots[k])])
            } else {
                (0.0, 0)
            }
        });
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        out.weights.extend(pairs.iter().map(|pair| pair.0));
        out.bone_indices.extend(pairs.iter().map(|pair| pair.1));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use ludion_core::math::glam::{DVec4, EulerRot, Mat4, Quat, Vec3};

    use super::*;

    /// An f32 TRS matrix widened to f64 the way `node_matrix` reads a glTF `matrix`.
    fn widened_trs(translation: f32) -> DMat4 {
        let m = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 0.5, 1.0),
            Quat::from_euler(EulerRot::XYZ, 0.3, -1.1, 2.0),
            Vec3::new(translation, 0.0, 0.0),
        );
        DMat4::from_cols_array(&m.to_cols_array().map(f64::from))
    }

    fn skewed(translation: f64) -> DMat4 {
        DMat4::from_cols(
            DVec4::X,
            DVec4::new(1e-3, 1.0, 0.0, 0.0),
            DVec4::Z,
            DVec4::new(translation, 0.0, 0.0, 1.0),
        )
    }

    #[test]
    fn a_trs_matrix_under_a_large_translation_is_not_a_shear() {
        for translation in [1e4, 1e6] {
            assert!(
                !shears(&widened_trs(translation)),
                "translation {translation}"
            );
        }
    }

    #[test]
    fn a_shear_under_a_large_translation_is_still_a_shear() {
        assert!(shears(&skewed(1e4)));
    }

    #[test]
    fn the_same_shear_without_a_translation_is_a_shear() {
        assert!(shears(&skewed(0.0)));
    }
}
