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
use ludion_ecs::Registry;

use crate::assembly_mesh::{AssemblyClipData, AssemblyMaskData, read_clip};
use crate::asset_cache::SourceFiles;
use crate::blend_space::{BlendSpaceData, BoundClip, resolve_blends};
use crate::components::Transform;

/// Each top-level part's authored `Transform` by part name.
pub(crate) type PartRests = BTreeMap<String, Transform>;

/// A prefab's resolved clips, masks and blends, and its parts' authored rest transforms.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabClipData {
    /// The bound clips, sorted by short name (the prefab document's own canonical order).
    pub clips: Vec<AssemblyClipData>,
    /// Each top-level part's authored `Transform` by part name. A part with no `Transform`, and an
    /// instance entry (a nested prefab, whose parts have path names), has none.
    pub rests: PartRests,
    /// The name of every `entities` entry (instance entries included), in document order.
    pub parts: Vec<String>,
    /// The prefab's `masks:`, sorted by name, each resolved to one weight per [`Self::parts`]
    /// entry.
    pub masks: Vec<AssemblyMaskData>,
    /// The prefab's `blends:`, sorted by name, each resolved against the bound clips (speed 1, a
    /// clip's own events).
    pub blends: Vec<BlendSpaceData>,
}

impl PrefabClipData {
    /// The clip bound under the short `name`, or `None` when this prefab binds no such clip.
    pub fn clip(&self, name: &str) -> Option<&ludion_scene::AnimationDocument> {
        self.clips
            .iter()
            .find(|binding| binding.name == name)
            .map(|binding| &binding.clip)
    }

    /// The blend space named `name`, or `None` when this prefab defines no such blend.
    pub fn blend(&self, name: &str) -> Option<&BlendSpaceData> {
        self.blends.iter().find(|blend| blend.name == name)
    }

    /// The resolved weights of the mask named `name` (one per [`Self::parts`] entry), or `None`
    /// when this prefab defines no such mask.
    pub fn mask(&self, name: &str) -> Option<&[f32]> {
        self.masks
            .iter()
            .find(|mask| mask.name == name)
            .map(|mask| mask.weights.as_slice())
    }

    /// The position of the top-level part `name` in [`Self::parts`]. A nested instance's part (a
    /// path name) has none.
    pub fn part_index(&self, name: &str) -> Option<usize> {
        self.parts.iter().position(|part| part == name)
    }
}

/// Reads the prefab at `root`/`rel` and every clip file it names, for `check-scene`.
///
/// # Errors
/// As the engine's own loader: `L1101` for an unreadable file and the clip-document diagnostics.
pub fn load(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<PrefabClipData, Vec<Diagnostic>> {
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
/// - `L1674` for a blend the clips cannot satisfy.
pub(crate) fn load_with_sources(
    root: ludion_assets::AssetRoot<'_>,
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

    let documents: Vec<_> = clips.iter().map(|binding| binding.clip.clone()).collect();
    let refused = ludion_scene::check_prefab_blends(&doc, &documents);
    if !refused.is_empty() {
        return Err(refused
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.message = format!("in `{rel}`: {}", diagnostic.message);
                diagnostic
            })
            .collect());
    }

    let bound: Vec<BoundClip<'_>> = clips
        .iter()
        .map(|binding| BoundClip {
            name: &binding.name,
            duration: binding.clip.duration,
            speed: 1.0,
            events: &binding.clip.events,
        })
        .collect();
    let blends = resolve_blends(&doc.blends, &bound);
    let parts: Vec<String> = doc
        .entities
        .iter()
        .map(|entity| entity.name.clone().unwrap_or_default())
        .collect();
    let masks = masks_of(&doc, &parts);

    let rests = rests_of(&doc);
    Ok((
        PrefabClipData {
            clips,
            rests,
            parts,
            masks,
            blends,
        },
        sources,
    ))
}

/// `doc`'s masks as one weight per entity, walked over each entry's `Parent` position.
fn masks_of(doc: &ludion_scene::PrefabDocument, parts: &[String]) -> Vec<AssemblyMaskData> {
    let rig: Vec<(&str, Option<u32>)> = doc
        .entities
        .iter()
        .zip(parts)
        .map(|(entity, name)| {
            let parent = entity
                .components
                .iter()
                .find(|(component, _)| component == "Parent")
                .and_then(|(_, value)| value.get("index")?.as_u64())
                .and_then(|index| u32::try_from(index).ok());
            (name.as_str(), parent)
        })
        .collect();
    doc.masks
        .iter()
        .map(|mask| AssemblyMaskData {
            name: mask.name.clone(),
            weights: ludion_scene::resolve_rig_mask(mask, &rig),
        })
        .collect()
}

/// The part names an instance of the prefab at `root`/`rel` spawns (`entities[].name`, a nested
/// instance's parts by their composed path) — what playback binds a clip's tracks against.
///
/// # Errors
/// The diagnostics of an unreadable or invalid prefab.
///
/// # Example
/// ```
/// let root = std::env::temp_dir().join(format!("ludion-part-names-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
/// let registry = ludion_engine::builtin_registry();
/// let names = ludion_engine::prefab_part_names(&root, &registry, "crate.prefab.yaml").unwrap();
/// assert_eq!(names, ["lid"]);
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn prefab_part_names(
    root: &Path,
    registry: &Registry,
    rel: &str,
) -> Result<Vec<String>, Vec<Diagnostic>> {
    let probe = format!(
        "format: 1
entities:
  - name: subject
    prefab: {}
",
        serde_json::Value::from(rel)
    );
    let scene = ludion_scene::check(&probe, registry)?;
    let expanded = crate::scene_check::expand_scene(root, registry, &scene)?;
    Ok(expanded
        .instances
        .into_iter()
        .find(|instance| instance.depth == 0)
        .map(|instance| instance.paths)
        .unwrap_or_default())
}

/// One `L1314` per clip track of `data` whose `target` is not one of the prefab's `parts`, at
/// `/clips/{name}` of the prefab document. The `$root` root-motion track targets no part and is
/// exempt.
///
/// # Example
/// ```
/// let root = std::env::temp_dir().join(format!("ludion-clip-target-doc-{}", std::process::id()));
/// std::fs::create_dir_all(root.join("clips")).unwrap();
/// std::fs::write(
///     root.join("clips/swing.anim.yaml"),
///     "format: 1\nkind: animation\nname: swing\nduration: 1.0\nloop: false\ntracks:\n  lidd:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n      - t: 1.0\n        v:\n          - 0.0\n          - 0.0\n          - 10.0\n",
/// )
/// .unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nclips:\n  swing: clips/swing.anim.yaml\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
/// let data = ludion_engine::load_prefab_clips(
///     ludion_assets::AssetRoot::content(&root),
///     "crate.prefab.yaml",
/// )
/// .unwrap();
/// let found = ludion_engine::prefab_clip_target_diagnostics(&data, &["lid".to_string()]);
/// assert_eq!(found.len(), 1);
/// assert_eq!(found[0].path.as_deref(), Some("/clips/swing"));
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn prefab_clip_target_diagnostics(data: &PrefabClipData, parts: &[String]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for binding in &data.clips {
        for track in &binding.clip.tracks {
            if track.target == ludion_scene::ROOT_TRACK || parts.contains(&track.target) {
                continue;
            }
            let help = match ludion_core::suggest::nearest(
                &track.target,
                parts.iter().map(String::as_str),
            ) {
                Some(candidate) => format!("nearest part is `{candidate}`"),
                None => "a track's target must be a part name (`entities[].name`)".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_CLIP_TARGET,
                    format!(
                        "the clip `{}` (`{}`) animates `{}`, which is not a part of this prefab",
                        binding.name, binding.file, track.target
                    ),
                )
                .with_path(format!(
                    "/clips/{}",
                    ludion_core::diagnostics::pointer_token(&binding.name)
                ))
                .with_help(help),
            );
        }
    }
    diagnostics
}

/// The authored `Transform` of each top-level part of `doc` that has one and is not an instance
/// entry.
pub(crate) fn rests_of(doc: &ludion_scene::PrefabDocument) -> PartRests {
    doc.entities
        .iter()
        .filter(|entity| entity.instance.is_none())
        .filter_map(|entity| {
            let name = entity.name.as_ref()?;
            let (_, value) = entity.components.iter().find(|(c, _)| c == "Transform")?;
            let transform = serde_json::from_value::<Transform>(value.clone()).ok()?;
            Some((name.clone(), transform))
        })
        .collect()
}

/// [`rests_of`] for prefab source `text`; `None` when it does not parse.
pub(crate) fn rests_of_text(text: &str) -> Option<PartRests> {
    ludion_scene::parse_prefab(text)
        .ok()
        .map(|doc| rests_of(&doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected")
    }

    #[test]
    fn the_hinge_fixture_loads_two_clips_and_three_rests() {
        let (data, sources) = load_with_sources(
            ludion_assets::AssetRoot::content(&fixture_root()),
            "model.prefab.yaml",
        )
        .unwrap();
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

    fn rig_project(label: &str, looped: bool) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("ludion-prefab-rig-{label}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("clips")).unwrap();
        std::fs::write(
            root.join("clips/swing.anim.yaml"),
            format!(
                "format: 1\nkind: animation\nname: swing\nduration: 1.0\nloop: {looped}\ntracks:\n  \
                 base:\n    rotation:\n      - t: 0.0\n        v:\n          - 0.0\n          - 0.0\n          - 0.0\n"
            ),
        )
        .unwrap();
        // `tip` is written before its ancestors; `other` hangs off `base` outside the mask.
        std::fs::write(
            root.join("rig.prefab.yaml"),
            "format: 1
kind: prefab
clips:
  again: clips/swing.anim.yaml
  swing: clips/swing.anim.yaml
masks:
  arm:
    joints:
      - upper
    descendants: true
blends:
  sway:
    points:
      - clip: swing
        at: 0.0
      - clip: again
        at: 1.0
entities:
  - name: tip
    components:
      Parent:
        index: 2
        generation: 0
  - name: base
    components: {}
  - name: lower
    components:
      Parent:
        index: 3
        generation: 0
  - name: upper
    components:
      Parent:
        index: 1
        generation: 0
  - name: other
    components:
      Parent:
        index: 1
        generation: 0
",
        )
        .unwrap();
        root
    }

    #[test]
    fn a_descendants_mask_walks_parents_written_out_of_order() {
        let root = rig_project("mask", true);
        let data = load(ludion_assets::AssetRoot::content(&root), "rig.prefab.yaml").unwrap();
        assert_eq!(data.parts, ["tip", "base", "lower", "upper", "other"]);
        assert_eq!(data.mask("arm").unwrap(), [1.0, 0.0, 1.0, 1.0, 0.0]);
        assert_eq!(data.part_index("lower"), Some(2));
        assert!(data.blend("sway").is_some() && data.clip("again").is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_blend_over_a_non_looping_clip_is_refused() {
        let root = rig_project("blend", false);
        let errors = load(ludion_assets::AssetRoot::content(&root), "rig.prefab.yaml").unwrap_err();
        assert!(
            errors.iter().all(|e| e.code == code::PREFAB_RIG_INVALID),
            "{errors:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_prefab_is_an_error_not_a_panic() {
        let errors = load_with_sources(
            ludion_assets::AssetRoot::content(&fixture_root()),
            "absent.prefab.yaml",
        )
        .unwrap_err();
        assert!(!errors.is_empty());
    }
}
