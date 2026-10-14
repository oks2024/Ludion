//! Document pre-checks: the gltf crate panics on a few structurally incomplete documents — its
//! own validator indexes the accessor array unchecked for a primitive's `POSITION`, and its
//! wrappers unwrap an image's source and MIME type and a camera's typed projection after validation
//! has passed. So the parsed JSON is checked here first, and a bad document is an `L1201` naming it.

use gltf::json::camera::Type;
use gltf::json::validation::Checked;
use gltf::json::{Index, Root, accessor};
use ludion_core::diagnostics::Diagnostic;

use crate::diagnostics::malformed;

/// Checks that every accessor a primitive or morph target names exists, that every image has a
/// source (and a MIME type when it names a buffer view), and that every camera carries the
/// projection object its `type` names.
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
    Ok(())
}
