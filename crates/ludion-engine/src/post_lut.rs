//! Check-time diagnostics for a `PostProcess.lut`: the file a scene names is parsed the way the
//! extract parses it, so a LUT that checks clean cannot fail to load.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::diagnostics::Diagnostic;
use ludion_scene::Scene;

/// Every `L1101`/`L2390`/`L2391` a checked scene's `PostProcess.lut.path` reports, each file parsed
/// once with [`ludion_assets::cube::load_cube`] and the diagnostic pointed at
/// `/entities/{i}/components/PostProcess/lut/path`.
///
/// `project_root` is the content root the paths resolve against.
///
/// # Example
/// ```
/// use ludion_engine::scene_lut_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-lut-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - components:\n      PostProcess:\n        lut:\n          path: grade.cube\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_lut_diagnostics(&root, &scene);
/// assert_eq!(found[0].code.as_str(), "L1101");
/// assert_eq!(
///     found[0].path.as_deref(),
///     Some("/entities/0/components/PostProcess/lut/path")
/// );
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_lut_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut loaded: BTreeMap<String, Option<Diagnostic>> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(path) = entity
            .components
            .iter()
            .find(|(name, _)| name.as_str() == "PostProcess")
            .and_then(|(_, value)| value.get("lut")?.get("path")?.as_str())
        else {
            continue;
        };
        let fault = loaded
            .entry(path.to_string())
            .or_insert_with(|| {
                ludion_assets::cube::load_cube(
                    ludion_assets::AssetRoot::content(project_root),
                    path,
                )
                .err()
            })
            .clone();
        diagnostics
            .extend(fault.map(|d| {
                d.with_path(format!("/entities/{index}/components/PostProcess/lut/path"))
            }));
    }
    diagnostics
}
