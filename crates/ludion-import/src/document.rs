//! Document pre-checks: the gltf crate panics on a few structurally incomplete documents — its
//! own validator indexes the accessor array unchecked for a primitive's `POSITION`, and its
//! wrappers unwrap an image's source and MIME type and a camera's typed projection after validation
//! has passed. So the parsed JSON is checked here first, and a bad document is an `L1201` naming it.
//! The node graph is checked to be a forest here too, so no later walk up the parents needs a guard.

use gltf::json::camera::Type;
use gltf::json::validation::Checked;
use gltf::json::{Index, Root, accessor};
use ludion_core::diagnostics::Diagnostic;

use crate::diagnostics::{malformed, node_graph_not_forest};
use crate::named;

/// Checks that every accessor a primitive or morph target names exists, that every image has a
/// source (and a MIME type when it names a buffer view), that every camera carries the
/// projection object its `type` names, and that the node graph is a forest.
///
/// # Errors
/// `L1201` naming the first offender by its JSON path.
pub(crate) fn check_structure(root: &Root) -> Result<(), Diagnostic> {
    let accessors = root.accessors.len();
    let check_accessor = |index: &Index<accessor::Accessor>, path: String| {
        if index.value() < accessors {
            Ok(())
        } else {
            Err(malformed(format!(
                "{path} names accessor {}, but the document has {accessors} accessors",
                index.value()
            )))
        }
    };

    for (m, mesh) in root.meshes.iter().enumerate() {
        for (p, primitive) in mesh.primitives.iter().enumerate() {
            let at = format!("meshes[{m}].primitives[{p}]");
            for (semantic, index) in &primitive.attributes {
                check_accessor(index, format!("{at}.attributes.{}", semantic.to_string()))?;
            }
            if let Some(index) = &primitive.indices {
                check_accessor(index, format!("{at}.indices"))?;
            }
            for (t, target) in primitive.targets.iter().flatten().enumerate() {
                for (key, index) in [
                    ("POSITION", &target.positions),
                    ("NORMAL", &target.normals),
                    ("TANGENT", &target.tangents),
                ] {
                    if let Some(index) = index {
                        check_accessor(index, format!("{at}.targets[{t}].{key}"))?;
                    }
                }
            }
        }
    }

    for (i, image) in root.images.iter().enumerate() {
        if image.uri.is_none() && image.buffer_view.is_none() {
            return Err(malformed(format!(
                "images[{i}] has neither a uri nor a bufferView"
            )));
        }
        if let Some(view) = &image.buffer_view
            && image.mime_type.is_none()
        {
            return Err(malformed(format!(
                "images[{i}] names bufferView {} but has no mimeType",
                view.value()
            )));
        }
    }

    for (i, camera) in root.cameras.iter().enumerate() {
        let (name, present) = match camera.type_ {
            Checked::Valid(Type::Perspective) => ("perspective", camera.perspective.is_some()),
            Checked::Valid(Type::Orthographic) => ("orthographic", camera.orthographic.is_some()),
            Checked::Invalid => continue,
        };
        if !present {
            return Err(malformed(format!(
                "cameras[{i}] is type {name} but has no {name} object"
            )));
        }
    }
    check_node_forest(&root.nodes)
}

/// Checks that the node graph is a forest: each node is listed as a child at most once and is never
/// its own ancestor. Every later walk up `parent_of` may then assume it terminates. A child index
/// past the node array is skipped here; the `gltf` validator reports it.
fn check_node_forest(nodes: &[gltf::json::Node]) -> Result<(), Diagnostic> {
    let label = |index: usize| named("node", index, nodes[index].name.as_deref());

    // parent_of[child] = (lister, position in its children)
    let mut parent_of: Vec<Option<(usize, usize)>> = vec![None; nodes.len()];
    for (parent, node) in nodes.iter().enumerate() {
        for (position, child) in node.children.iter().flatten().enumerate() {
            let child = child.value();
            if child >= nodes.len() {
                continue;
            }
            let pointer = format!("/nodes/{parent}/children/{position}");
            match parent_of[child] {
                Some((first, _)) if first == parent => {
                    return Err(node_graph_not_forest(
                        format!("{} lists {} as a child twice", label(parent), label(child)),
                        pointer,
                    ));
                }
                Some((first, _)) => {
                    return Err(node_graph_not_forest(
                        format!(
                            "{} is listed as a child by both {} and {}; a node has at most one parent",
                            label(child),
                            label(first),
                            label(parent)
                        ),
                        pointer,
                    ));
                }
                None => parent_of[child] = Some((parent, position)),
            }
        }
    }

    // Each node has at most one parent now, so walking up from every node finds any cycle.
    let mut walked_from = vec![usize::MAX; nodes.len()];
    for start in 0..nodes.len() {
        let mut current = start;
        let cycle_at = loop {
            walked_from[current] = start;
            let Some((parent, _)) = parent_of[current] else {
                break None;
            };
            match walked_from[parent] {
                stamp if stamp == start => break Some(parent),
                usize::MAX => current = parent,
                _ => break None,
            }
        };
        if let Some(node) = cycle_at {
            let (lister, position) = parent_of[node].expect("a node on a cycle has a parent");
            let pointer = format!("/nodes/{lister}/children/{position}");
            let detail = if lister == node {
                format!("{} lists itself as a child", label(node))
            } else {
                format!(
                    "{} lists {} as a child, which is also its ancestor, so the nodes form a cycle",
                    label(lister),
                    label(node)
                )
            };
            return Err(node_graph_not_forest(detail, pointer));
        }
    }
    Ok(())
}
