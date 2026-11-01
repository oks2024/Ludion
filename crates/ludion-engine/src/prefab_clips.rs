//! Loading the clips a `kind: prefab` document binds, for playback on an instance's parts.
//!
//! A prefab's `clips:` map (short name → `kind: animation` file, relative to the prefab) is the
//! assembly's `clips:` shape, so the clip documents load through the same
//! [`read_clip`](crate::assembly_mesh::read_clip) and play through the same `Animation` component.
//! This module is the engine-side IO seam: read the prefab, parse it, read each clip. Alongside the
//! clips it keeps each top-level part's authored `Transform` — the **rest** a clip moves a part
//! relative to (`crate::extract`'s `prefab_part_poses`). Nothing here samples or poses anything.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::sibling;

use crate::assembly_mesh::{AssemblyClipData, read_clip};
use crate::asset_cache::SourceFiles;
use crate::components::Transform;

/// A prefab's resolved clips and its parts' authored rest transforms.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabClipData {
    /// The bound clips, sorted by short name (the prefab document's own canonical order).
    pub clips: Vec<AssemblyClipData>,
    /// Each top-level part's authored `Transform` by part name. A part with no `Transform`, and an
    /// instance entry (a nested prefab, whose parts have path names), has none.
    pub rests: BTreeMap<String, Transform>,
}

/// Reads the prefab at `root`/`rel` and every clip file it names, for `check-scene`.
///
/// # Errors
/// As the engine's own loader: `L1101` for an unreadable file and the clip-document diagnostics.
pub fn load(root: &Path, rel: &str) -> Result<PrefabClipData, Vec<Diagnostic>> {
    load_with_sources(root, rel).map(|(data, _)| data)
}

/// Reads the prefab at `root`/`rel` and every clip file it names, reporting **every file it read**
/// — the prefab first, then each clip, as `root`-relative paths, each with the content hash of the
/// bytes it decoded: the entry's reload dependency set (see [`crate::asset_cache`]).
///
/// Clip diagnostics accumulate across all clips, each prefixed with the binding and its path.
///
/// # Errors
/// - `L1101` if the prefab or any clip file cannot be read.
/// - `L1226` for a prefab that is not UTF-8 text or does not parse.
/// - `L1141`/`L1142`/`L1198` for a malformed clip document.
pub(crate) fn load_with_sources(
    root: &Path,
    rel: &str,
) -> Result<(PrefabClipData, SourceFiles), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let not_a_prefab = |message: String| {
        Diagnostic::new(code::PREFAB_DOCUMENT_STRUCTURE, message)
            .with_help("a prefab is a strict-YAML text file (`*.prefab.yaml`)")
    };
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![not_a_prefab(format!(
            "prefab `{rel}` is not valid UTF-8 text"
        ))]
    })?;
    let doc = ludion_scene::parse_prefab(source).map_err(|errors| {
        errors
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.message = format!("in `{rel}`: {}", diagnostic.message);
                diagnostic
            })
            .collect::<Vec<_>>()
    })?;

    let mut sources = vec![(rel.to_string(), bytes.hash)];
    let mut diagnostics = Vec::new();
    let mut clips = Vec::with_capacity(doc.clips.len());
    for binding in &doc.clips {
        let file = sibling(rel, &binding.file);
        match read_clip(root, &file, &binding.name) {
            Ok((clip, hash)) => {
                clips.push(AssemblyClipData {
                    name: binding.name.clone(),
                    file: binding.file.clone(),
                    clip,
                });
                sources.push((file, hash));
            }
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let rests = doc
        .entities
        .iter()
        .filter(|entity| entity.instance.is_none())
        .filter_map(|entity| {
            let name = entity.name.as_ref()?;
            let (_, value) = entity.components.iter().find(|(c, _)| c == "Transform")?;
            let transform = serde_json::from_value::<Transform>(value.clone()).ok()?;
            Some((name.clone(), transform))
        })
        .collect();
    Ok((PrefabClipData { clips, rests }, sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected")
    }

    #[test]
    fn the_hinge_fixture_loads_two_clips_and_three_rests() {
        let (data, sources) = load_with_sources(&fixture_root(), "model.prefab.yaml").unwrap();
        let names: Vec<&str> = data.clips.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["open", "snap"]);
        assert_eq!(data.rests.len(), 3);
        assert_eq!(data.rests["lid"].translation, [0.0, 0.1, -0.4]);
        let files: Vec<&str> = sources.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(
            files,
            [
                "model.prefab.yaml",
                "clips/open.anim.yaml",
                "clips/snap.anim.yaml"
            ]
        );
    }

    #[test]
    fn a_missing_prefab_is_an_error_not_a_panic() {
        let errors = load_with_sources(&fixture_root(), "absent.prefab.yaml").unwrap_err();
        assert!(!errors.is_empty());
    }
}
