//! The scene checks that need the filesystem, which `ludion-scene` (which does no IO) cannot make.
//!
//! [`ludion_scene::check`] answers everything the text can answer on its own. An atlas reference is
//! the one thing it cannot: an `atlas` + `frame` pair names a region of a *document on disk*, and a
//! pair that does not resolve draws nothing at all. This module is where that second pass lives, so
//! every client asking "is this scene valid" — `ludion check-scene`, a game binary's own
//! `--check-scene`, the wire's `scene.check` — runs the same one and cannot answer differently.

use std::collections::BTreeMap;
use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_ecs::Registry;
use ludion_scene::{ExpandedScene, Scene};

use crate::prefab::{prefab_key, read_prefab_text};
use ludion_components::validators::PREFAB_SINGLETONS;
pub use ludion_components::validators::{
    prefab_singleton_diagnostics, scene_hierarchy_body_diagnostics,
};
use ludion_core::diagnostics::pointer_token;

/// `L1314` for every clip track, in a prefab a scene places, that names no part of that prefab.
///
/// A clip is chosen at runtime and playback tolerates a miss, so this runs at the check surfaces
/// only, not at scene load. Each prefab is checked once, at the first outermost entry placing it:
/// the diagnostic reads ``in `{source}`: …`` and points at that entry, `/entities/{entry}` of the
/// scene document — already document-level, so it must not go through [`ExpandedScene::locate`].
/// A prefab that does not load, or binds no clips, is skipped here: its own check reports it.
///
/// # Example
/// ```
/// use ludion_engine::{expand_scene, scene_prefab_clip_diagnostics};
///
/// let root = std::env::temp_dir().join(format!("ludion-prefab-clip-doc-{}", std::process::id()));
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
/// let registry = ludion_engine::builtin_registry();
/// let scene = ludion_scene::check(
///     "format: 1\nname: doc\nentities:\n  - name: crate-1\n    prefab: crate.prefab.yaml\n",
///     &registry,
/// )
/// .unwrap();
/// let expanded = expand_scene(&root, &registry, &scene).unwrap();
/// let found = scene_prefab_clip_diagnostics(&root, &registry, &expanded);
/// assert_eq!(found.len(), 1);
/// assert_eq!(found[0].path.as_deref(), Some("/entities/0"));
/// assert_eq!(found[0].help.as_deref(), Some("nearest part is `lid`"));
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_prefab_clip_diagnostics(
    project_root: &Path,
    registry: &Registry,
    expanded: &ExpandedScene,
) -> Vec<Diagnostic> {
    let mut seen = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for instance in expanded.instances.iter().filter(|i| i.depth == 0) {
        if !seen.insert(instance.source.as_str()) {
            continue;
        }
        let Ok(data) = crate::prefab_clips::load(
            ludion_assets::AssetRoot::content(project_root),
            &instance.source,
        ) else {
            continue;
        };
        if data.clips.is_empty() {
            continue;
        }
        let Ok(parts) =
            crate::prefab_clips::prefab_part_names(project_root, registry, &instance.source)
        else {
            continue;
        };
        for mut diagnostic in crate::prefab_clips::prefab_clip_target_diagnostics(&data, &parts) {
            diagnostic.message = format!("in `{}`: {}", instance.source, diagnostic.message);
            diagnostic.path = Some(format!("/entities/{}", instance.entry));
            found.push(diagnostic);
        }
    }
    found
}

/// Expands the prefab instances of a checked `scene`, reading each prefab file from `project_root`.
///
/// The flat [`ExpandedScene::scene`] is what loads: run the engine's other passes over it and map
/// their diagnostics back to the document with [`ExpandedScene::locate`]. A scene with no instance
/// entries comes back equal, with no instances.
///
/// # Errors
/// `L1230` for an unreadable or absolute prefab path, `L1231`–`L1233` for a malformed instance, a
/// loop or a bad override, and the diagnostics of an invalid prefab file.
///
/// # Example
/// ```
/// use ludion_engine::expand_scene;
///
/// let root = std::env::temp_dir().join(format!("ludion-expand-scene-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
/// let registry = ludion_engine::builtin_registry();
/// let scene = ludion_scene::check(
///     "format: 1\nname: doc\nentities:\n  - name: crate-1\n    prefab: crate.prefab.yaml\n",
///     &registry,
/// )
/// .unwrap();
/// let expanded = expand_scene(&root, &registry, &scene).unwrap();
/// assert_eq!(expanded.scene.entities[0].name.as_deref(), Some("crate-1/lid"));
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn expand_scene(
    project_root: &Path,
    registry: &Registry,
    scene: &Scene,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    expand_scene_with(registry, scene, &mut |path| {
        let key = prefab_key(path).map_err(|mut found| found.swap_remove(0))?;
        read_prefab_text(project_root, &key)
    })
}

/// [`expand_scene`] over any `resolve`, which gets each prefab path as written.
pub(crate) fn expand_scene_with(
    registry: &Registry,
    scene: &Scene,
    resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
) -> Result<ExpandedScene, Vec<Diagnostic>> {
    let expanded = ludion_scene::expand(scene, registry, resolve)?;
    let singletons = instance_singleton_diagnostics(&expanded);
    if singletons.is_empty() {
        Ok(expanded)
    } else {
        Err(singletons)
    }
}

/// Every `L1229` an expanded scene's instance parts report: a part, authored or overridden, that
/// carries a scene-wide singleton. Located at the instance entry, naming the part.
fn instance_singleton_diagnostics(expanded: &ExpandedScene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instance in expanded.instances.iter().filter(|i| i.depth == 0) {
        for flat in instance.parts.clone() {
            for (name, _) in &expanded.scene.entities[flat].components {
                if !PREFAB_SINGLETONS.contains(&name.as_str()) {
                    continue;
                }
                diagnostics.push(
                    expanded.locate(
                        Diagnostic::new(
                            code::PREFAB_SINGLETON,
                            format!("a prefab part carries `{name}`, of which a scene has one"),
                        )
                        .with_path(format!("/entities/{flat}/components/{name}"))
                        .with_help("a prefab is spawned many times; put it in the scene instead"),
                    ),
                );
            }
        }
    }
    diagnostics
}

/// Every `L1151`/`L1152` a checked scene's atlas references report — one pass over each entity's
/// `Sprite`, each `UiRect`, and each `Tilemap` legend entry, through the same
/// [`check_atlas_reference`](crate::check_atlas_reference) `extract` resolves with, so a scene that
/// checks clean cannot fail to draw for a reason this pass could have named.
///
/// `project_root` is the content root the references resolve against — the scene file's own parent
/// directory, exactly as a host roots one.
pub fn scene_atlas_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            match name.as_str() {
                "Sprite" => diagnostics.extend(atlas_reference(
                    project_root,
                    &format!("/entities/{index}/components/Sprite"),
                    value,
                )),
                "UiRect" => diagnostics.extend(atlas_reference(
                    project_root,
                    &format!("/entities/{index}/components/UiRect"),
                    value,
                )),
                "UiRadial" => {
                    let items = value.get("items").and_then(serde_json::Value::as_array);
                    for (k, item) in items.into_iter().flatten().enumerate() {
                        diagnostics.extend(atlas_reference(
                            project_root,
                            &format!("/entities/{index}/components/UiRadial/items/{k}"),
                            item,
                        ));
                    }
                }
                "Tilemap" => {
                    let Some(legend) = value.get("legend").and_then(serde_json::Value::as_object)
                    else {
                        continue;
                    };
                    for (glyph, tile) in legend {
                        diagnostics.extend(atlas_reference(
                            project_root,
                            &format!(
                                "/entities/{index}/components/Tilemap/legend/{}",
                                pointer_token(glyph)
                            ),
                            tile,
                        ));
                    }
                }
                _ => {}
            }
        }
        diagnostics.extend(sprite_crossfade(index, &entity.components));
    }
    diagnostics
}

/// `L1209` for a `Sprite` drawn from an atlas whose `Animation` authors a crossfade: a flipbook
/// hard-cuts, so `fade` would silently do nothing.
fn sprite_crossfade(
    index: usize,
    components: &[(String, serde_json::Value)],
) -> Option<Diagnostic> {
    let find = |wanted: &str| {
        components
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value)
    };
    find("Sprite")?.get("atlas")?.as_str()?;
    let fade = find("Animation")?.get("fade")?.as_f64()?;
    (fade > 0.0).then(|| {
        Diagnostic::new(
            code::ANIMATION_SPRITE_CROSSFADE,
            "a sprite flipbook cannot crossfade: `Animation.fade` is ignored and the sprite hard-cuts to `clip`",
        )
        .with_path(format!("/entities/{index}/components/Animation/fade"))
        .with_help("drop `fade` and `from`, or use a skinned character or an assembly")
    })
}

/// Every `L1293` a checked scene reports: a `sensor: true` collider the 3D solver does not claim,
/// where the flag would silently do nothing.
pub fn scene_sensor_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
        else {
            continue;
        };
        let Ok(collider) = serde_json::from_value::<ludion_spatial::Collider>(value.clone()) else {
            continue;
        };
        if collider.sensor && !ludion_physics::claims(&collider) {
            diagnostics.push(
                Diagnostic::new(
                    code::COLLIDER_SENSOR_UNCLAIMED,
                    "collider `sensor` is ignored: the 3D solver does not claim this collider"
                        .to_string(),
                )
                .with_path(format!("/entities/{index}/components/Collider/sensor"))
                .with_help("give the collider a 3D shape (`sphere`, `box`, ...), or drop `sensor`"),
            );
        }
    }
    diagnostics
}

/// Every `L1186`/`L1187` a checked scene's collider `layer` fields report: a layer the scene's
/// `physics.layers` does not declare (with the nearest declared name), and a layer on a collider
/// the 3D solver never claims, where it would silently do nothing.
///
/// It lives here rather than in `ludion-scene` because the claim rule is
/// [`ludion_physics::claims`], and the scene crate names no spatial or physics type.
/// `layer: default` is always accepted; a scene with no `physics:` block declares nothing else.
pub fn scene_layer_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let matrix = scene.physics.as_ref().map(|physics| physics.layer_matrix());
    let declared: Vec<&str> = match &matrix {
        Some(matrix) => matrix.names().iter().map(String::as_str).collect(),
        None => vec![ludion_scene::DEFAULT_LAYER],
    };
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        diagnostics.extend(query_layer_diagnostics(index, entity, &declared));
        if let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "CharacterMotor")
            && let Some(name) = value.get("layer").and_then(serde_json::Value::as_str)
            && !declared.contains(&name)
        {
            diagnostics.push(unknown_layer(
                name,
                &declared,
                format!("/entities/{index}/components/CharacterMotor/layer"),
                "character motor",
            ));
        }
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
        else {
            continue;
        };
        let Ok(collider) = serde_json::from_value::<ludion_spatial::Collider>(value.clone()) else {
            continue;
        };
        let Some(name) = collider.layer.as_deref() else {
            continue;
        };
        let pointer = format!("/entities/{index}/components/Collider/layer");
        if !ludion_physics::claims(&collider) {
            diagnostics.push(
                Diagnostic::new(
                    code::COLLIDER_LAYER_UNCLAIMED,
                    format!(
                        "collider layer `{name}` is ignored: the 3D solver does not claim this \
                         collider"
                    ),
                )
                .with_path(pointer)
                .with_help(
                    "remove `layer`, or give the collider a 3D shape or a `rigid` body; the 2D \
                     tier has no filtering",
                ),
            );
        } else if !declared.contains(&name) {
            diagnostics.push(unknown_layer(name, &declared, pointer, "collider"));
        }
    }
    diagnostics
}

/// Every `L1690`/`L1692`/`L1693` a checked scene's `SurfaceTag`s and `SurfaceOverlay`s report
/// (an overlay on an entity without a solver-claimed sensor `Collider` is `L1693`): a surface the scene's
/// `physics.surfaces` does not declare (with the nearest declared name), and a tag on an entity
/// with neither a solver-claimed `Collider` nor a `CharacterMotor`, where no hit could report it.
/// An empty name is the component validator's (`L1691`). A scene with no `physics:` block declares
/// nothing, so any non-empty tag is unknown.
pub fn scene_surface_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let declared: Vec<&str> = scene
        .physics
        .as_ref()
        .map(|physics| physics.surfaces.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let component = |wanted: &str| {
            entity
                .components
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value)
        };
        if let Some(overlay) = component("SurfaceOverlay") {
            let at = format!("/entities/{index}/components/SurfaceOverlay");
            let sensor = component("Collider")
                .and_then(|value| {
                    serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok()
                })
                .is_some_and(|collider| collider.sensor && ludion_physics::claims(&collider));
            if !sensor {
                diagnostics.push(
                    Diagnostic::new(
                        code::SURFACE_OVERLAY_NO_SENSOR,
                        "this `SurfaceOverlay` has no solver-claimed `Collider` with `sensor: true` \
                         beside it, so it has no volume to contain a point",
                    )
                    .with_path(at.clone())
                    .with_help("add a 3D `Collider` with `sensor: true`, or remove `SurfaceOverlay`"),
                );
            }
            if let Some(name) = overlay.get("surface").and_then(serde_json::Value::as_str)
                && !name.is_empty()
                && !declared.contains(&name)
            {
                let help = match suggest::nearest(name, declared.iter().copied()) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => "declare it under `physics.surfaces`".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::SURFACE_UNKNOWN,
                        format!("surface `{name}` is not declared under `physics.surfaces`"),
                    )
                    .with_path(format!("{at}/surface"))
                    .with_help(help),
                );
            }
        }
        let Some(tag) = component("SurfaceTag") else {
            continue;
        };
        let at = format!("/entities/{index}/components/SurfaceTag");
        let claimed = component("CharacterMotor").is_some()
            || component("Collider")
                .and_then(|value| {
                    serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok()
                })
                .as_ref()
                .is_some_and(ludion_physics::claims);
        if !claimed {
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACE_TAG_UNCLAIMED,
                    "this `SurfaceTag` has no solver-claimed `Collider` or `CharacterMotor` beside \
                     it, so no hit can report its surface",
                )
                .with_path(at.clone())
                .with_help("add a 3D `Collider` or a `CharacterMotor`, or remove `SurfaceTag`"),
            );
        }
        let Some(name) = tag.get("surface").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !name.is_empty() && !declared.contains(&name) {
            let help = match suggest::nearest(name, declared.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => "declare it under `physics.surfaces`".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACE_UNKNOWN,
                    format!("surface `{name}` is not declared under `physics.surfaces`"),
                )
                .with_path(format!("{at}/surface"))
                .with_help(help),
            );
        }
        diagnostics.extend(surface_part_diagnostics(
            tag,
            &at,
            &declared,
            component("Collider"),
            component("Mesh"),
        ));
        diagnostics.extend(surface_cell_diagnostics(
            tag,
            &at,
            &declared,
            component("Collider"),
        ));
    }
    diagnostics
}

/// The `L1690`/`L1696` of one `SurfaceTag`'s `cells` table: a cell surface the scene does not
/// declare, `cells` on an entity whose `Collider` is not a `heightfield`, and run counts that miss
/// the grid's `(rows-1)*(cols-1)` cells. An empty or repeated name, a zero count and an out-of-range
/// index are the component validator's (`L1696`).
fn surface_cell_diagnostics(
    tag: &serde_json::Value,
    at: &str,
    declared: &[&str],
    collider: Option<&serde_json::Value>,
) -> Vec<Diagnostic> {
    let Some(cells) = tag.get("cells") else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    let names = cells
        .get("surfaces")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for (k, name) in names.iter().enumerate() {
        let Some(name) = name.as_str().filter(|name| !name.is_empty()) else {
            continue;
        };
        if !declared.contains(&name) {
            let help = match suggest::nearest(name, declared.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => "declare it under `physics.surfaces`".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACE_UNKNOWN,
                    format!("surface `{name}` is not declared under `physics.surfaces`"),
                )
                .with_path(format!("{at}/cells/surfaces/{k}"))
                .with_help(help),
            );
        }
    }
    let grid = collider
        .and_then(|value| serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok())
        .and_then(|collider| match collider.shape {
            ludion_spatial::Shape::Heightfield { rows, cols, .. } => Some((rows, cols)),
            _ => None,
        });
    let bad = |message: String, help: &str| {
        Diagnostic::new(code::MESH_SURFACES_INVALID, message)
            .with_path(format!("{at}/cells"))
            .with_help(help.to_string())
    };
    let Some((rows, cols)) = grid else {
        diagnostics.push(bad(
            "`cells` needs a `heightfield` `Collider` on this entity".to_string(),
            "tag a mesh collider's triangles in its mesh document instead, or remove `cells`",
        ));
        return diagnostics;
    };
    let expected = u64::from(rows.saturating_sub(1)) * u64::from(cols.saturating_sub(1));
    let total: u64 = cells
        .get("runs")
        .and_then(serde_json::Value::as_array)
        .map(|runs| {
            runs.iter()
                .filter_map(|run| run.get(0).and_then(serde_json::Value::as_u64))
                .sum()
        })
        .unwrap_or(0);
    if total != expected {
        diagnostics.push(bad(
            format!(
                "`cells` run counts sum to {total}, but the {rows}\u{d7}{cols} heightfield has \
                 {expected} cells"
            ),
            "the runs must cover every cell exactly once, row-major",
        ));
    }
    diagnostics
}

/// The `L1690`/`L1694` of one `SurfaceTag`'s `parts` list: a part surface the scene does not declare,
/// an integer key that is not a part of the entity's `compound` `Collider`, and a string key on an
/// entity whose `Mesh` is not an `assembly`. Empty and repeated keys are the component
/// validator's (`L1694`).
fn surface_part_diagnostics(
    tag: &serde_json::Value,
    at: &str,
    declared: &[&str],
    collider: Option<&serde_json::Value>,
    mesh: Option<&serde_json::Value>,
) -> Vec<Diagnostic> {
    let Some(parts) = tag.get("parts").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let compound_len = collider
        .and_then(|value| serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok())
        .and_then(|collider| match collider.shape {
            ludion_spatial::Shape::Compound { parts } => Some(parts.len()),
            _ => None,
        });
    let assembly = mesh
        .and_then(|value| serde_json::from_value::<crate::components::Mesh>(value.clone()).ok())
        .is_some_and(|mesh| matches!(mesh.shape, crate::components::MeshSource::Assembly(_)));
    let mut diagnostics = Vec::new();
    for (j, entry) in parts.iter().enumerate() {
        let here = format!("{at}/parts/{j}");
        if let Some(name) = entry.get("surface").and_then(serde_json::Value::as_str)
            && !name.is_empty()
            && !declared.contains(&name)
        {
            let help = match suggest::nearest(name, declared.iter().copied()) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => "declare it under `physics.surfaces`".to_string(),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACE_UNKNOWN,
                    format!("surface `{name}` is not declared under `physics.surfaces`"),
                )
                .with_path(format!("{here}/surface"))
                .with_help(help),
            );
        }
        let bad = |message: String, help: &str| {
            Diagnostic::new(code::SURFACE_PART_INVALID, message)
                .with_path(format!("{here}/part"))
                .with_help(help.to_string())
        };
        match entry.get("part") {
            Some(key) if key.is_u64() => {
                let index = key.as_u64().unwrap_or(0);
                match compound_len {
                    None => diagnostics.push(bad(
                        format!("part index {index} needs a `compound` `Collider` on this entity"),
                        "give the entity a `compound` collider, or name an assembly part instead",
                    )),
                    Some(len) if index >= len as u64 => diagnostics.push(bad(
                        format!("part index {index} is past the compound's {len} parts"),
                        "use an index below the compound's part count",
                    )),
                    Some(_) => {}
                }
            }
            Some(key) if key.as_str().is_some_and(|name| !name.is_empty()) && !assembly => {
                diagnostics.push(bad(
                    format!("part name {key} needs a `Mesh` whose `shape` is `assembly`"),
                    "name parts only on an assembly entity, or use a compound part index",
                ));
            }
            _ => {}
        }
    }
    diagnostics
}

/// The `L1186`/`L1500`/`L1501` of an entity's `QueryLayers`: an entry the scene does not declare, an
/// entry naming the entity's own body layer (a query already sees it), and the component on an
/// entity with neither a claimed `Collider` nor a `CharacterMotor`. Empty and repeated entries are
/// the component validator's (`L1500`).
fn query_layer_diagnostics(
    index: usize,
    entity: &ludion_scene::SceneEntity,
    declared: &[&str],
) -> Vec<Diagnostic> {
    let component = |wanted: &str| {
        entity
            .components
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value)
    };
    let Some(query) = component("QueryLayers") else {
        return Vec::new();
    };
    let at = format!("/entities/{index}/components/QueryLayers");
    let motor = component("CharacterMotor");
    let collider = component("Collider")
        .and_then(|value| serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok());
    let mut diagnostics = Vec::new();
    if motor.is_none() && !collider.as_ref().is_some_and(ludion_physics::claims) {
        diagnostics.push(
            Diagnostic::new(
                code::QUERY_LAYERS_UNCLAIMED,
                "this `QueryLayers` has no solver-claimed `Collider` or `CharacterMotor` beside it,                  so no query can reach its layers",
            )
            .with_path(at.clone())
            .with_help("add a 3D `Collider` or a `CharacterMotor`, or remove `QueryLayers`"),
        );
    }
    let own = match (&motor, &collider) {
        (Some(motor), _) => motor.get("layer").and_then(serde_json::Value::as_str),
        (None, Some(collider)) => collider.layer.as_deref(),
        _ => None,
    }
    .unwrap_or(ludion_scene::DEFAULT_LAYER);
    let entries = query
        .get("layers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(j, entry)| Some((j, entry.as_str()?)));
    for (j, name) in entries {
        let pointer = format!("{at}/layers/{j}");
        if name.is_empty() {
            continue;
        }
        if !declared.contains(&name) {
            diagnostics.push(unknown_layer(name, declared, pointer, "query-only"));
        } else if name == own {
            diagnostics.push(
                Diagnostic::new(
                    code::QUERY_LAYERS_INVALID,
                    format!("`QueryLayers` entry `{name}` is this entity's own body layer"),
                )
                .with_path(pointer)
                .with_help("a query already sees the body layer; delete the entry"),
            );
        }
    }
    diagnostics
}

/// The `L1186` for a layer `name` the scene does not declare, with the nearest declared name.
fn unknown_layer(name: &str, declared: &[&str], pointer: String, owner: &str) -> Diagnostic {
    let help = match suggest::nearest(name, declared.iter().copied()) {
        Some(candidate) => format!("nearest match is `{candidate}`"),
        None => "declare it under `physics.layers`".to_string(),
    };
    Diagnostic::new(
        code::COLLIDER_LAYER_UNKNOWN,
        format!("{owner} layer `{name}` is not declared under `physics.layers`"),
    )
    .with_path(pointer)
    .with_help(help)
}

/// Whether `entity` has a `Collider` that is not `body: rigid`, which is what a motor is carried by.
fn carries_surface(entity: &ludion_scene::SceneEntity) -> bool {
    entity
        .components
        .iter()
        .find(|(name, _)| name == "Collider")
        .and_then(|(_, value)| {
            serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok()
        })
        .is_some_and(|collider| !matches!(collider.body, ludion_spatial::Body::Rigid(_)))
}

/// Every `L1295`/`L1296`/`L1297`/`L1299`/`L1302` a checked scene's character motors, belts and
/// lanes report: a `Collider` beside the motor (the motor owns its capsule), a scene with no
/// `physics:` block (the body is never built), a `Parent` (a motor writes world-space feet), a
/// `SurfaceVelocity` on an entity with no `static` or `kinematic` collider to carry from, and a
/// `LaneConstraint` on an entity with no motor. A motor authored `CharacterMotorState: { suspended: true }` may
/// have a `Parent` (it is parked; resuming requires a root). The field rules (`L1294`, `L1298`, `L1300`) and the
/// layer (`L1186`) are reported elsewhere.
pub fn scene_motor_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let has = |wanted: &str| entity.components.iter().any(|(name, _)| name == wanted);
        let at = format!("/entities/{index}/components");
        if has("SurfaceVelocity") && !carries_surface(entity) {
            diagnostics.push(
                Diagnostic::new(
                    code::SURFACE_VELOCITY_NO_SURFACE,
                    "this `SurfaceVelocity` has no `static` or `kinematic` `Collider` beside it to \
                     carry a character; a rigid support is never carried",
                )
                .with_path(format!("{at}/SurfaceVelocity"))
                .with_help("put it on the belt's `static` or `kinematic` collider"),
            );
        }
        if !has("CharacterMotor") {
            if has("LaneConstraint") {
                diagnostics.push(
                    Diagnostic::new(
                        code::LANE_WITHOUT_MOTOR,
                        "this `LaneConstraint` has no `CharacterMotor` beside it; nothing reads \
                         the lane",
                    )
                    .with_path(format!("{at}/LaneConstraint"))
                    .with_help("add a `CharacterMotor` to this entity, or remove the lane"),
                );
            }
            continue;
        }
        if has("Collider") {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_WITH_COLLIDER,
                    "this entity carries a `Collider` beside its `CharacterMotor`; the motor owns \
                     its capsule and the `Collider` would be ignored",
                )
                .with_path(format!("{at}/Collider"))
                .with_help("remove the `Collider`; the motor owns its capsule"),
            );
        }
        if scene.physics.is_none() {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_NO_PHYSICS_SCENE,
                    "this character motor belongs to the physics solver, but the scene has no \
                     `physics:` block to install it",
                )
                .with_path(format!("{at}/CharacterMotor"))
                .with_help("add `physics: { space: free }` to the scene"),
            );
        }
        let suspended = entity.components.iter().any(|(name, value)| {
            name == "CharacterMotorState"
                && value.get("suspended").and_then(|v| v.as_bool()) == Some(true)
        });
        let named_posture = entity
            .components
            .iter()
            .find(|(name, _)| name == "CharacterMotorState")
            .and_then(|(_, state)| state.get("posture")?.as_str());
        if let Some(posture) = named_posture {
            let declared: Vec<&str> = entity
                .components
                .iter()
                .find(|(name, _)| name == "CharacterMotor")
                .and_then(|(_, motor)| motor.get("postures")?.as_object())
                .map(|postures| postures.keys().map(String::as_str).collect())
                .unwrap_or_default();
            if !declared.contains(&posture) {
                let help = match suggest::nearest(posture, declared.iter().copied()) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => "declare it under `CharacterMotor.postures`".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::CHARACTER_MOTOR_POSTURE_UNKNOWN,
                        format!(
                            "posture `{posture}` is not declared by this entity's `CharacterMotor`"
                        ),
                    )
                    .with_path(format!("{at}/CharacterMotorState/posture"))
                    .with_help(help),
                );
            }
        }
        if has("Parent") && !suspended {
            diagnostics.push(
                Diagnostic::new(
                    code::CHARACTER_MOTOR_PARENTED,
                    "this character motor has a `Parent`; a motor writes world-space feet, so it \
                     must be a root entity",
                )
                .with_path(format!("{at}/Parent"))
                .with_help("remove the `Parent` and follow the mover from game code"),
            );
        }
    }
    diagnostics
}

/// Every `L1307`/`L1308`/`L1309`/`L1554` a checked scene's camera rigs report: a `CameraRig` on an
/// entity with no `Camera`, a second rig (the pointer is at the second, the message names both
/// entities), a rig on an entity with a `Parent` and a `CameraZone` with no sensor collider. The field rules (`L1304`) are reported
/// elsewhere.
pub fn scene_camera_rig_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut first: Option<usize> = None;
    for (index, entity) in scene.entities.iter().enumerate() {
        let has = |wanted: &str| entity.components.iter().any(|(name, _)| name == wanted);
        if !has("CameraRig") {
            continue;
        }
        let at = format!("/entities/{index}/components");
        if !has("Camera") {
            diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_RIG_WITHOUT_CAMERA,
                    "this `CameraRig` has no `Camera` beside it, so there is nothing to pose",
                )
                .with_path(format!("{at}/CameraRig"))
                .with_help("put the rig on the camera entity"),
            );
        }
        match first {
            None => first = Some(index),
            Some(earlier) => diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_RIG_DUPLICATE,
                    format!(
                        "entities {earlier} and {index} both carry a `CameraRig`; only the first \
                         runs"
                    ),
                )
                .with_path(format!("{at}/CameraRig"))
                .with_help("keep one rig, on the camera entity"),
            ),
        }
        if has("Parent") {
            diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_RIG_PARENTED,
                    "this `CameraRig` has a `Parent`; a rig writes a world-space pose, so the \
                     camera must be a root entity",
                )
                .with_path(format!("{at}/Parent"))
                .with_help("make the camera a root"),
            );
        }
    }
    for (index, entity) in scene.entities.iter().enumerate() {
        let has = |wanted: &str| entity.components.iter().any(|(name, _)| name == wanted);
        if !has("CameraZone") {
            continue;
        }
        let sensor = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
            .is_some_and(|(_, value)| value.get("sensor").and_then(|v| v.as_bool()) == Some(true));
        if !sensor {
            diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_ZONE_WITHOUT_SENSOR,
                    "this `CameraZone` has no `Collider` with `sensor: true`, so no target can \
                     enter it",
                )
                .with_path(format!("/entities/{index}/components/CameraZone"))
                .with_help("add a `Collider` with `sensor: true` and the zone's shape"),
            );
        }
    }
    diagnostics
}

/// The `L1650` a checked scene's audio listeners report: a second `AudioListener` (the pointer is at
/// the second, the message names both entities; the runtime hears from the lowest id).
pub fn scene_audio_listener_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut first: Option<usize> = None;
    for (index, entity) in scene.entities.iter().enumerate() {
        if !entity
            .components
            .iter()
            .any(|(name, _)| name == "AudioListener")
        {
            continue;
        }
        match first {
            None => first = Some(index),
            Some(earlier) => diagnostics.push(
                Diagnostic::new(
                    code::AUDIO_LISTENER_DUPLICATE,
                    format!(
                        "entities {earlier} and {index} both carry an `AudioListener`; only the \
                         first is heard from"
                    ),
                )
                .with_path(format!("/entities/{index}/components/AudioListener"))
                .with_help("keep one `AudioListener`, on the entity sound is measured from"),
            ),
        }
    }
    diagnostics
}

/// Every `L1333`/`L1334` a checked scene's camera offsets report: a `CameraOffset` on a sim entity
/// (never applied) and a second one on presentation entities (only the first is applied; the pointer
/// is at the second, the message names both). The field rules (`L1332`) are the component's own
/// validator.
pub fn scene_camera_offset_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut first: Option<usize> = None;
    for (index, entity) in scene.entities.iter().enumerate() {
        if !entity
            .components
            .iter()
            .any(|(name, _)| name == "CameraOffset")
        {
            continue;
        }
        let at = format!("/entities/{index}/components/CameraOffset");
        if !entity.presentation {
            diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_OFFSET_ON_SIM_ENTITY,
                    "this `CameraOffset` is on a sim entity, so it is hashed and never applied",
                )
                .with_path(at)
                .with_help("mark the entity `presentation: true`"),
            );
            continue;
        }
        match first {
            None => first = Some(index),
            Some(earlier) => diagnostics.push(
                Diagnostic::new(
                    code::CAMERA_OFFSET_DUPLICATE,
                    format!(
                        "entities {earlier} and {index} both carry a `CameraOffset`; only the \
                         first is applied"
                    ),
                )
                .with_path(at)
                .with_help("keep one offset entity"),
            ),
        }
    }
    diagnostics
}

/// `L1313` (a warning) for each `DirectionalLight` that sets `cascades` beside a `shadow_extent`
/// other than the default: a cascaded sun fits its frustums to the camera, so the extent is unused.
pub fn scene_sun_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let default_extent = f64::from(crate::DirectionalLight::default().shadow_extent);
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, light)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "DirectionalLight")
        else {
            continue;
        };
        let extent = light
            .get("shadow_extent")
            .and_then(crate::components::as_stored_f32);
        if light.get("cascades").is_some() && extent.is_some_and(|e| e != default_extent) {
            diagnostics.push(
                Diagnostic::new(
                    code::DIRECTIONAL_LIGHT_EXTENT_UNUSED,
                    "this `DirectionalLight` sets `cascades` and a `shadow_extent`; each cascade is \
                     fitted to the camera, so the extent and the `Transform` anchor are unused by the \
                     camera's shadows; reflection-probe captures still use the anchored box",
                )
                .with_path(format!(
                    "/entities/{index}/components/DirectionalLight/shadow_extent"
                ))
                .with_help(
                    "remove `shadow_extent`, or drop `cascades` to keep the anchored box",
                ),
            );
        }
    }
    diagnostics
}

/// Degrees `sun_disc_size` may differ from twice `angular_radius` before `L1331` fires.
const SUN_DISC_TOLERANCE_DEGREES: f64 = 0.05;

/// `L1330` for each `PostProcess` with `dof > 0` and an authored `focus_distance` of 0, and `L1331` for
/// each `Environment` whose positive `sun_disc_size` is not twice a positive
/// `DirectionalLight.angular_radius` authored anywhere in the scene.
pub fn scene_lens_and_sun_disc_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let field = |value: &serde_json::Value, key: &str| {
        value.get(key).and_then(crate::components::as_stored_f32)
    };
    let mut diagnostics = Vec::new();
    let radius = scene.entities.iter().find_map(|entity| {
        let (_, light) = entity
            .components
            .iter()
            .find(|(name, _)| name == "DirectionalLight")?;
        field(light, "angular_radius").filter(|r| *r > 0.0)
    });
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            match name.as_str() {
                "PostProcess"
                    if field(value, "dof").is_some_and(|d| d > 0.0)
                        && field(value, "focus_distance") == Some(0.0) =>
                {
                    diagnostics.push(
                        Diagnostic::new(
                            code::CAMERA_DOF_FOCUS_ZERO,
                            "this `PostProcess` enables `dof` with `focus_distance: 0`; the focal plane \
                             sits at the lens, so the whole scene is defocused",
                        )
                        .with_path(format!(
                            "/entities/{index}/components/PostProcess/focus_distance"
                        ))
                        .with_help("focus on the subject's distance from the camera"),
                    );
                }
                "Environment" => {
                    let (Some(disc), Some(radius)) =
                        (field(value, "sun_disc_size").filter(|d| *d > 0.0), radius)
                    else {
                        continue;
                    };
                    if (disc - 2.0 * radius).abs() > SUN_DISC_TOLERANCE_DEGREES {
                        diagnostics.push(
                            Diagnostic::new(
                                code::SUN_DISC_SIZE_MISMATCH,
                                format!(
                                    "`sun_disc_size` is {disc} (the full angle) but the sun's \
                                     `angular_radius` is {radius} (the half angle), so the sky \
                                     disc and the sun's highlight differ in size"
                                ),
                            )
                            .with_path(format!(
                                "/entities/{index}/components/Environment/sun_disc_size"
                            ))
                            .with_help(format!(
                                "set `sun_disc_size` to {} (twice `angular_radius`), or drop one",
                                2.0 * radius
                            )),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    diagnostics
}

/// `L1323` (a warning) for each `PointLight`/`SpotLight` that sets `shadow_distance` or a non-zero
/// `shadow_fade` without `cast_shadows: true`: with no shadow to fade, the field does nothing.
pub fn scene_local_light_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, light) in &entity.components {
            if name != "PointLight" && name != "SpotLight" {
                continue;
            }
            if light.get("cast_shadows").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let distance = light
                .get("shadow_distance")
                .and_then(crate::components::as_stored_f32);
            let fade = light
                .get("shadow_fade")
                .and_then(crate::components::as_stored_f32);
            let field = if distance.is_some() {
                "shadow_distance"
            } else if fade.is_some_and(|f| f != 0.0) {
                "shadow_fade"
            } else {
                continue;
            };
            diagnostics.push(
                Diagnostic::new(
                    code::LOCAL_LIGHT_SHADOW_FADE_UNUSED,
                    format!(
                        "this `{name}` sets `{field}` without `cast_shadows: true`; it has no \
                         shadow to fade, so the field does nothing"
                    ),
                )
                .with_path(format!("/entities/{index}/components/{name}/{field}"))
                .with_help(
                    "set `cast_shadows: true`, or remove `shadow_distance` and `shadow_fade`",
                ),
            );
        }
    }
    diagnostics
}

/// What a scene's `LightExposure` subjects get wrong: `L4251` (a warning) for each light on a
/// `presentation: true` entity, which is drawn but never counted; `L4252` (a warning) for an
/// `occluder_layers` that names no layer any 3D-claimed collider carries, through `Collider.layer`
/// (`default` when absent) or `QueryLayers`; and the `L1186` for an undeclared name. Silent for a
/// scene with no `LightExposure`.
pub fn scene_light_exposure_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let component = |entity: &ludion_scene::SceneEntity, wanted: &str| {
        entity
            .components
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.clone())
    };
    let subjects: Vec<(usize, Vec<String>)> = scene
        .entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| {
            let value = component(entity, "LightExposure")?;
            let layers = value
                .get("occluder_layers")
                .and_then(serde_json::Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(|n| n.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            Some((index, layers))
        })
        .collect();
    if subjects.is_empty() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        if !entity.presentation {
            continue;
        }
        for name in ["PointLight", "SpotLight", "DirectionalLight"] {
            if component(entity, name).is_some() {
                diagnostics.push(
                    Diagnostic::new(
                        code::LIGHT_EXPOSURE_PRESENTATION_LIGHT,
                        format!(
                            "this `{name}` is on a `presentation: true` entity: it is drawn but \
                             never counts toward exposure"
                        ),
                    )
                    .with_path(format!("/entities/{index}/components/{name}"))
                    .with_help(
                        "drop `presentation: true` if the light should count, or add a sim-side \
                         light for what should",
                    ),
                );
            }
        }
    }
    let matrix = scene.physics.as_ref().map(|physics| physics.layer_matrix());
    let declared: Vec<&str> = match &matrix {
        Some(matrix) => matrix.names().iter().map(String::as_str).collect(),
        None => vec![ludion_scene::DEFAULT_LAYER],
    };
    let mut carried: Vec<String> = Vec::new();
    for entity in &scene.entities {
        let Some(collider) = component(entity, "Collider")
            .and_then(|v| serde_json::from_value::<ludion_spatial::Collider>(v).ok())
            .filter(ludion_physics::claims)
        else {
            continue;
        };
        carried.push(
            collider
                .layer
                .clone()
                .unwrap_or_else(|| ludion_scene::DEFAULT_LAYER.to_owned()),
        );
        if let Some(extra) = component(entity, "QueryLayers")
            .as_ref()
            .and_then(|v| v.get("layers"))
            .and_then(serde_json::Value::as_array)
        {
            carried.extend(extra.iter().filter_map(|n| n.as_str().map(str::to_owned)));
        }
    }
    for (index, layers) in subjects {
        if layers.is_empty() {
            continue;
        }
        let at = format!("/entities/{index}/components/LightExposure/occluder_layers");
        for (j, name) in layers.iter().enumerate() {
            if !name.is_empty() && !declared.contains(&name.as_str()) {
                diagnostics.push(unknown_layer(
                    name,
                    &declared,
                    format!("{at}/{j}"),
                    "light occluder",
                ));
            }
        }
        if !layers.iter().any(|name| carried.contains(name)) {
            diagnostics.push(
                Diagnostic::new(
                    code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED,
                    format!(
                        "no 3D collider in this scene is on {}: nothing can occlude this \
                         subject's lights",
                        layers
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_path(at)
                .with_help(
                    "set `Collider.layer` or `QueryLayers` on the occluders, or correct the names",
                ),
            );
        }
    }
    diagnostics
}

/// The `L2342` a scene's `GroundMarkerClip`s earn: a layer name the scene does not declare, a clip on
/// a `segment` marker (no apex to cast from), and a clip on an entity with no `GroundMarker`. The
/// numeric fields are the component validator's. Silent for a scene with no clip.
pub fn scene_ground_marker_clip_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let matrix = scene.physics.as_ref().map(|physics| physics.layer_matrix());
    let declared: Vec<&str> = match &matrix {
        Some(matrix) => matrix.names().iter().map(String::as_str).collect(),
        None => vec![ludion_scene::DEFAULT_LAYER],
    };
    let mut diagnostics = Vec::new();
    let mut reject = |path: String, message: String, help: String| {
        diagnostics.push(
            Diagnostic::new(code::GROUND_MARKER_CLIP_INVALID, message)
                .with_path(path)
                .with_help(help),
        );
    };
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, clip)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "GroundMarkerClip")
        else {
            continue;
        };
        let at = format!("/entities/{index}/components/GroundMarkerClip");
        let shape = entity
            .components
            .iter()
            .find(|(name, _)| name == "GroundMarker")
            .map(|(_, marker)| {
                marker
                    .pointer("/shape/type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            });
        match shape.as_deref() {
            None => reject(
                at.clone(),
                "this `GroundMarkerClip` has no `GroundMarker` beside it, so there is nothing to clip"
                    .into(),
                "add a `GroundMarker`, or remove the clip".into(),
            ),
            Some("segment") => reject(
                at.clone(),
                "this `GroundMarkerClip` is on a `segment` marker, which has no apex to cast from"
                    .into(),
                "clip a `disc`, `ring` or `wedge` marker".into(),
            ),
            Some(_) => {}
        }
        for field in ["layers", "partial_layers"] {
            let names = clip
                .get(field)
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .enumerate();
            for (j, name) in names {
                let Some(name) = name.as_str().filter(|n| !declared.contains(n)) else {
                    continue;
                };
                let help = match suggest::nearest(name, declared.iter().copied()) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => "declare it under `physics.layers`".to_string(),
                };
                reject(
                    format!("{at}/{field}/{j}"),
                    format!("clip layer `{name}` is not declared under `physics.layers`"),
                    help,
                );
            }
        }
    }
    diagnostics
}

/// Every `L1261` a checked scene's solver colliders report when the scene has no `physics:` block:
/// the solver is never installed, so a 3D shape or a rigid body would silently do nothing. The
/// claim rule is [`ludion_physics::claims`].
pub fn scene_physics_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    if scene.physics.is_some() {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(Ok(collider)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
            .map(|(_, value)| serde_json::from_value::<ludion_spatial::Collider>(value.clone()))
        else {
            continue;
        };
        if !ludion_physics::claims(&collider) {
            continue;
        }
        diagnostics.push(
            Diagnostic::new(
                code::COLLIDER_NO_PHYSICS_SCENE,
                "this collider belongs to the physics solver, but the scene has no `physics:` block \
                 to install it",
            )
            .with_path(format!("/entities/{index}/components/Collider"))
            .with_help(
                "add `physics: { space: free }` to the scene, or use a static or kinematic 2D \
                 `circle`/`aabb` collider",
            ),
        );
    }
    diagnostics
}

/// The shared help of the scale warning, authored and live.
pub(crate) const SCALE_HELP: &str = "nothing is dropped; measure it with `ludion profile` against a \
     budget on each pinned box — `ludion docs physics-scale` names the gated scenes";

/// `L4034` at `/physics` when the scene authors more non-static solver bodies (rigid and
/// kinematic, character motors included) than [`ludion_physics::GATED_BODIES`]. A scene with no
/// `physics:` block is reported by [`scene_physics_diagnostics`] instead.
fn scene_physics_scale_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    if scene.physics.is_none() {
        return Vec::new();
    }
    let has = |entity: &ludion_scene::SceneEntity, wanted: &str| {
        entity.components.iter().any(|(name, _)| name == wanted)
    };
    let bodies = scene
        .entities
        .iter()
        .filter(|entity| {
            has(entity, "CharacterMotor")
                || entity
                    .components
                    .iter()
                    .find(|(name, _)| name == "Collider")
                    .and_then(|(_, value)| {
                        serde_json::from_value::<ludion_spatial::Collider>(value.clone()).ok()
                    })
                    .is_some_and(|collider| {
                        ludion_physics::claims(&collider)
                            && !matches!(collider.body, ludion_spatial::Body::Static(_))
                    })
        })
        .count();
    if bodies <= ludion_physics::GATED_BODIES {
        return Vec::new();
    }
    vec![
        Diagnostic::new(
            code::PHYSICS_SCALE_UNGATED,
            format!(
                "the scene authors {bodies} non-static physics bodies, past the gated scale of {}",
                ludion_physics::GATED_BODIES
            ),
        )
        .with_path("/physics")
        .with_help(SCALE_HELP),
    ]
}

/// The diagnostics one component value's atlas reference reports; empty when it carries none (a
/// loose `texture:` or an untextured quad has no atlas to resolve).
fn atlas_reference(root: &Path, pointer: &str, value: &serde_json::Value) -> Vec<Diagnostic> {
    let Some(atlas) = value.get("atlas").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    crate::check_atlas_reference(
        ludion_assets::AssetRoot::content(root),
        pointer,
        atlas,
        value.get("frame").and_then(serde_json::Value::as_str),
    )
}

/// The load faults of the font table at `project_root` (`L1253` for `fonts.yaml` itself, `L1255` for
/// a face), each paired with the root-relative file it is in; their pointers are into that file,
/// never the scene.
pub fn font_table_faults(project_root: &Path) -> Vec<(String, Diagnostic)> {
    crate::FontLibrary::load(ludion_assets::AssetRoot::content(project_root))
        .faults()
        .map(|(file, d)| (file.to_string(), d.clone()))
        .collect()
}

/// Every font diagnostic a checked scene's text components report, against the `fonts.yaml` at
/// `project_root`: a `font` naming a face the table lacks (`L1254`, with the nearest name), and one
/// `L1256` **warning** per text field whose chars the resolved table face has no glyph for (line
/// breaks are not chars and a tab lays out as a space). Coverage is checked for table faces only,
/// never the embedded 8×8 face, whose gaps draw blank by design. The table's own faults come from
/// [`font_table_faults`].
///
/// Errors and warnings come back together; `Diagnostic::severity` tells them apart.
pub fn scene_font_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    use crate::fonts::TableState;

    let library = crate::FontLibrary::load(ludion_assets::AssetRoot::content(project_root));
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            let fields: Vec<(String, &str)> = match name.as_str() {
                "Text" => text_field(value, "text", "text").into_iter().collect(),
                "UiText" => text_field(value, "text", "text").into_iter().collect(),
                "UiList" => {
                    let mut fields: Vec<(String, &str)> =
                        text_field(value, "title", "title").into_iter().collect();
                    if let Some(items) = value.get("items").and_then(serde_json::Value::as_array) {
                        for (k, item) in items.iter().enumerate() {
                            fields.extend(text_field(item, "text", &format!("items/{k}/text")));
                        }
                    }
                    fields
                }
                "UiRadial" => {
                    let mut fields: Vec<(String, &str)> = Vec::new();
                    if let Some(items) = value.get("items").and_then(serde_json::Value::as_array) {
                        for (k, item) in items.iter().enumerate() {
                            fields.extend(text_field(item, "text", &format!("items/{k}/text")));
                            fields.extend(text_field(item, "detail", &format!("items/{k}/detail")));
                        }
                    }
                    fields
                }
                _ => continue,
            };
            let base = format!("/entities/{index}/components/{name}");
            let font = value.get("font").and_then(serde_json::Value::as_str);
            if let Some(font) = font.filter(|f| !library.contains(f)) {
                let help = match suggest::nearest(font, library.names()) {
                    Some(near) => format!("did you mean `{near}`?"),
                    None if library.names().next().is_some() => {
                        format!(
                            "`fonts.yaml` defines: {}",
                            library.names().collect::<Vec<_>>().join(", ")
                        )
                    }
                    None => match library.table_state() {
                        TableState::Absent => {
                            "add a `fonts.yaml` (`kind: fonts`) at the content root".to_string()
                        }
                        TableState::Faulted => {
                            "fix `fonts.yaml` first (its error is reported above)".to_string()
                        }
                        TableState::Loaded => "`fonts.yaml` defines no faces".to_string(),
                    },
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::FONT_NAME_UNKNOWN,
                        format!(
                            "`{name}` names the font `{font}`, which `fonts.yaml` does not define"
                        ),
                    )
                    .with_path(format!("{base}/font"))
                    .with_help(help),
                );
                continue;
            }
            let Some(face) = library.table_face(font) else {
                continue;
            };
            for (field, text) in fields {
                let mut missing: Vec<char> = text
                    .chars()
                    .map(|c| if c == '\t' { ' ' } else { c })
                    .filter(|&c| c != '\n' && c != '\r' && face.glyph(c).is_none())
                    .collect();
                missing.sort_unstable();
                missing.dedup();
                if missing.is_empty() {
                    continue;
                }
                let listed: String = missing.iter().collect();
                let dir = library
                    .paths(font)
                    .and_then(|(doc, _)| doc.rsplit_once('/'))
                    .map_or("", |(dir, _)| dir);
                let help = match face.document().generated_from.as_deref() {
                    Some(source) => {
                        let source = if dir.is_empty() {
                            source.to_string()
                        } else {
                            format!("{dir}/{source}")
                        };
                        format!(
                            "add them to `charset.chars` in `{source}` and re-run `ludion generate-atlas {source}`"
                        )
                    }
                    None => "add them to the face's `charset.chars` and re-bake it".to_string(),
                };
                diagnostics.push(
                    Diagnostic::new(
                        code::FONT_GLYPH_MISSING,
                        format!("the text has characters the font lacks: {listed:?}"),
                    )
                    .with_path(format!("{base}/{field}"))
                    .with_help(help),
                );
            }
        }
    }
    diagnostics
}

/// Every `L1101`/`L1104` a checked scene's `AudioEmitter.sound` and `AudioZone` bed sound paths report: each is decoded with
/// [`ludion_assets::load_wav`], the call the audio sink makes, so a sound that checks clean cannot
/// fail to play for a reason this pass could have named. An empty `sound` is `L1101` too. Check-time
/// only: audio never moves the sim, so scene loading does not run it.
///
/// `project_root` is the content root the paths resolve against.
///
/// # Example
/// ```
/// use ludion_engine::scene_audio_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-audio-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - name: lamp\n    components:\n      AudioEmitter:\n        sound: hum.wav\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_audio_diagnostics(&root, &scene);
/// assert_eq!(found[0].code.as_str(), "L1101");
/// assert_eq!(
///     found[0].path.as_deref(),
///     Some("/entities/0/components/AudioEmitter/sound")
/// );
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_audio_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut decoded: BTreeMap<String, Option<Diagnostic>> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let mut sounds: Vec<(String, &str, &str)> = Vec::new();
        for (name, value) in &entity.components {
            if name == "AudioEmitter" {
                let sound = value
                    .get("sound")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let pointer = format!("/entities/{index}/components/AudioEmitter/sound");
                sounds.push((pointer, sound, "AudioEmitter"));
            } else if name == "AudioZone" {
                let beds = value.get("beds").and_then(serde_json::Value::as_array);
                for (bed, entry) in beds.into_iter().flatten().enumerate() {
                    let sound = entry
                        .get("sound")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    let pointer =
                        format!("/entities/{index}/components/AudioZone/beds/{bed}/sound");
                    sounds.push((pointer, sound, "AudioZone"));
                }
            }
        }
        for (pointer, sound, owner) in sounds {
            let fault = if sound.is_empty() {
                Some(
                    Diagnostic::new(
                        code::ASSET_FILE_IO,
                        format!("this `{owner}` names no sound, so it plays nothing"),
                    )
                    .with_help(
                        "set `sound` to a WAV file under the content root, e.g. `sfx/hum.wav`",
                    ),
                )
            } else {
                decoded
                    .entry(sound.to_string())
                    .or_insert_with(|| {
                        ludion_assets::load_wav(
                            ludion_assets::AssetRoot::content(project_root),
                            sound,
                        )
                        .err()
                    })
                    .clone()
            };
            diagnostics.extend(fault.map(|d| d.with_path(pointer)));
        }
    }
    diagnostics
}

/// Every engine pass over an expanded scene, in one list: atlas, font, collider layer, parented
/// body, physics, physics scale, preload, skin bone, mesh asset, audio and reflection probe diagnostics, errors and warnings together
/// (`severity` tells them apart). `ludion check-scene`, a game's `--check-scene` and `scene.check` all run exactly
/// this, so no client answers "is this scene valid" differently.
///
/// The diagnostics are unlocated: map each through [`ExpandedScene::locate`] to point into the
/// authored document. The font table's own faults point into `fonts.yaml`, not the scene, and
/// come from [`font_table_faults`].
///
/// # Example
/// ```
/// use ludion_engine::scene_engine_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-engine-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check("format: 1\nentities: []\n", &ludion_engine::builtin_registry())
///     .unwrap();
/// assert!(scene_engine_diagnostics(&root, &scene).is_empty());
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_engine_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = scene_atlas_diagnostics(project_root, scene);
    diagnostics.extend(scene_font_diagnostics(project_root, scene));
    diagnostics.extend(scene_layer_diagnostics(scene));
    diagnostics.extend(scene_surface_diagnostics(scene));
    diagnostics.extend(scene_sensor_diagnostics(scene));
    diagnostics.extend(scene_hierarchy_body_diagnostics(scene));
    diagnostics.extend(scene_physics_diagnostics(scene));
    diagnostics.extend(scene_motor_diagnostics(scene));
    diagnostics.extend(scene_physics_scale_diagnostics(scene));
    diagnostics.extend(scene_camera_rig_diagnostics(scene));
    diagnostics.extend(scene_audio_listener_diagnostics(scene));
    diagnostics.extend(scene_camera_offset_diagnostics(scene));
    diagnostics.extend(scene_sun_diagnostics(scene));
    diagnostics.extend(scene_local_light_diagnostics(scene));
    diagnostics.extend(scene_light_exposure_diagnostics(scene));
    diagnostics.extend(crate::perception_check::scene_perception_diagnostics(scene));
    diagnostics.extend(crate::secondary_view::scene_render_view_diagnostics(scene));
    diagnostics.extend(crate::render_layers::scene_render_layer_diagnostics(scene));
    diagnostics.extend(crate::rain::scene_rain_diagnostics(scene));
    diagnostics.extend(scene_ground_marker_clip_diagnostics(scene));
    diagnostics.extend(scene_lens_and_sun_disc_diagnostics(scene));
    diagnostics.extend(crate::ui::group::scene_ui_group_diagnostics(scene));
    diagnostics.extend(scene_preload_diagnostics(project_root, scene));
    // The mesh-asset pass reports a missing skinned mesh at its entity; the skin-bone and
    // parent-joint passes read the same files and push the reader's bare diagnostic, dropped here.
    let meshes = crate::mesh_check::scene_mesh_asset_diagnostics(project_root, scene);
    let mut joints = crate::skin_bone::scene_skin_bone_diagnostics(
        ludion_assets::AssetRoot::content(project_root),
        scene,
    );
    joints.extend(crate::parent_joint::scene_parent_joint_diagnostics(
        project_root,
        scene,
    ));
    joints.retain(|bare| {
        bare.path.is_some()
            || !meshes
                .iter()
                .any(|d| d.code == bare.code && d.message.ends_with(&bare.message))
    });
    diagnostics.extend(joints);
    diagnostics.extend(meshes);
    diagnostics.extend(scene_audio_diagnostics(project_root, scene));
    diagnostics.extend(crate::probe_check::scene_probe_diagnostics(scene));
    diagnostics.extend(crate::static_check::scene_static_diagnostics(
        project_root,
        scene,
    ));
    diagnostics
}

/// `L1230` at `/preload/{i}` for each preload entry whose prefab text cannot be read. Existence
/// only: the full prefab passes need the registry's spawn path and run at load.
fn scene_preload_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for (index, path) in scene.preload.iter().enumerate() {
        let read = crate::prefab::prefab_key(path)
            .map_err(|mut found| found.swap_remove(0))
            .and_then(|key| crate::prefab::read_prefab_text(project_root, &key));
        if let Err(mut error) = read {
            error.message = format!("the scene preloads `{path}`: {}", error.message);
            found.push(error.with_path(format!("/preload/{index}")));
        }
    }
    found
}

/// A string field of a text component with the pointer suffix it reports under.
fn text_field<'a>(
    value: &'a serde_json::Value,
    key: &str,
    pointer: &str,
) -> Option<(String, &'a str)> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(|text| (pointer.to_string(), text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown_font_help(table: Option<&str>) -> String {
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-check-fonts-{}-{}",
            table.is_some(),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        if let Some(table) = table {
            std::fs::write(dir.join("fonts.yaml"), table).expect("table");
        }
        let scene = ludion_scene::check(
            "format: 1\nseed: 1\nentities:\n  - name: t\n    components:\n      Text:\n        \
             text: HI\n        font: body\n",
            &crate::builtin_registry(),
        )
        .expect("fixture scene");
        let found = scene_font_diagnostics(&dir, &scene);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::FONT_NAME_UNKNOWN);
        found[0].help.clone().expect("a help")
    }

    #[test]
    fn the_unknown_font_help_tells_the_three_table_states_apart() {
        assert!(unknown_font_help(None).contains("add a `fonts.yaml`"));
        assert!(
            unknown_font_help(Some("format: 1\nkind: fonts\nfonts: 3\n"))
                .contains("fix `fonts.yaml` first")
        );
        assert!(
            unknown_font_help(Some("format: 1\nkind: fonts\nfonts: {}\n"))
                .contains("defines no faces")
        );
    }

    const LAYERS: &str = concat!(
        "physics:\n  space: free\n",
        "  layers:\n    - name: dummy\n      ignores:\n        - dummy\n"
    );

    fn collider(shape: &str, body: &str, layer: &str) -> String {
        format!(
            "entities:\n  - name: part\n    components:\n      Collider:\n        shape:\n\
             {pad}{shape}\n        body: {body}\n        layer: {layer}\n",
            pad = " ".repeat(10)
        )
    }

    fn diagnostics(physics: &str, entities: &str) -> Vec<Diagnostic> {
        let source = format!("format: 1\nseed: 1\n{physics}{entities}");
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_layer_diagnostics(&scene)
    }

    fn sensor_diagnostics(shape: &str, body: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1
seed: 1
entities:
  - name: zone
    components:
      Collider:
        shape:
          {shape}
        body: {body}
        sensor: true
"
        );
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_sensor_diagnostics(&scene)
    }

    #[test]
    fn a_sensor_on_an_unclaimed_collider_is_l1293_and_a_3d_one_is_clean() {
        let found = sensor_diagnostics(
            "circle:
            radius: 0.5",
            "static",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code.as_str(), "L1293");
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider/sensor")
        );
        let clean = sensor_diagnostics(
            "sphere:
            radius: 0.5",
            "static",
        );
        assert!(clean.is_empty(), "{clean:?}");
    }

    fn parented_collider(shape: &str, body: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1\nseed: 1\nentities:\n  - name: platform\n    components: {{}}\n  \
             - name: part\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      \
             Collider:\n        shape:\n          {shape}\n        body: {body}\n"
        );
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene_hierarchy_body_diagnostics(&scene)
    }

    const BOX: &str = "box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5";
    const AABB: &str = "aabb:
            half_extents:
              - 0.5
              - 0.5";

    #[test]
    fn a_rigid_child_is_l1221_and_a_static_solver_child_is_l1222() {
        let rigid = parented_collider(
            BOX,
            "
          rigid: {}",
        );
        assert_eq!(rigid.len(), 1);
        assert_eq!(rigid[0].code, code::HIERARCHY_RIGID_BODY);
        assert_eq!(
            rigid[0].path.as_deref(),
            Some("/entities/1/components/Collider/body")
        );
        assert!(rigid[0].message.contains("`platform`"));
        let fixed = parented_collider(BOX, "static");
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].code, code::HIERARCHY_STATIC_COLLIDER);
    }

    #[test]
    fn a_solver_collider_without_a_physics_block_is_l1261() {
        let entities = "entities:\n  - name: ball\n    components:\n      Collider:\n        \
                        shape:\n          sphere:\n            radius: 0.5\n        body:\n          \
                        rigid: {}\n";
        let check = |physics: &str| {
            let source = format!("format: 1\nseed: 1\n{physics}{entities}");
            let scene = ludion_scene::check(&source, &crate::builtin_registry())
                .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
            scene_physics_diagnostics(&scene)
        };
        let missing = check("");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].code, code::COLLIDER_NO_PHYSICS_SCENE);
        assert_eq!(
            missing[0].path.as_deref(),
            Some("/entities/0/components/Collider")
        );
        assert!(check("physics:\n  space: free\n").is_empty());
    }

    #[test]
    fn a_kinematic_child_and_a_2d_tier_child_are_accepted() {
        assert!(parented_collider(BOX, "kinematic").is_empty());
        assert!(parented_collider(AABB, "static").is_empty());
    }

    fn prefab_body(component: &str) -> Vec<Diagnostic> {
        let source = format!(
            "format: 1\nkind: prefab\nentities:\n  - name: part\n    components:\n      {component}\n"
        );
        let doc = ludion_scene::check_prefab(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture prefab invalid: {errors:?}"));
        prefab_singleton_diagnostics(&doc.to_scene())
    }

    #[test]
    fn a_camera_part_is_l1229_and_a_point_light_part_is_accepted() {
        let refused = prefab_body("Camera: {}");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].code, code::PREFAB_SINGLETON);
        assert_eq!(
            refused[0].path.as_deref(),
            Some("/entities/0/components/Camera")
        );
        assert!(prefab_body("PointLight: {}").is_empty());
    }

    #[test]
    fn an_atlas_sprite_crossfade_is_l1209() {
        let scene_with = |animation: &str| {
            let source = format!(
                "format: 1\nseed: 1\nentities:\n  - name: hero\n    components:\n      Sprite:\n        \
                 atlas: atlas.yaml\n        frame: idle\n      Animation:\n{animation}"
            );
            ludion_scene::check(&source, &crate::builtin_registry())
                .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
        };
        let count = |scene: &Scene| {
            scene_atlas_diagnostics(Path::new("."), scene)
                .into_iter()
                .filter(|d| d.code == code::ANIMATION_SPRITE_CROSSFADE)
                .collect::<Vec<_>>()
        };
        let fading = count(&scene_with(
            "        clip: run\n        from: walk\n        fade: 0.5\n",
        ));
        assert_eq!(fading.len(), 1);
        assert_eq!(
            fading[0].path.as_deref(),
            Some("/entities/0/components/Animation/fade")
        );
        assert!(count(&scene_with("        clip: run\n")).is_empty());
    }

    #[test]
    fn a_typo_names_the_nearest_declared_layer() {
        let found = diagnostics(
            LAYERS,
            &collider("sphere:\n            radius: 0.5", "static", "dummi"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider/layer")
        );
        assert_eq!(found[0].help.as_deref(), Some("nearest match is `dummy`"));
    }

    /// A checked scene with `LAYERS` and the entities in `body`, then `QueryLayers` pushed onto
    /// entity 0 by hand: the component is registered with the engine's registry separately.
    fn query_layers_scene(body: &str, layers: &[&str]) -> Scene {
        let mut scene = ludion_scene::check(
            &format!("format: 1\nseed: 1\n{LAYERS}{body}"),
            &crate::builtin_registry(),
        )
        .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene.entities[0].components.push((
            "QueryLayers".to_string(),
            serde_json::json!({ "layers": layers }),
        ));
        scene
    }

    #[test]
    fn query_layers_report_unknown_own_and_unclaimed() {
        let body = collider("sphere:\n            radius: 0.5", "static", "dummy");
        let found = scene_layer_diagnostics(&query_layers_scene(&body, &["dummi", "dummy"]));
        let codes: Vec<_> = found.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::COLLIDER_LAYER_UNKNOWN, code::QUERY_LAYERS_INVALID],
            "{found:?}"
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/QueryLayers/layers/0")
        );
        assert_eq!(found[0].help.as_deref(), Some("nearest match is `dummy`"));
        assert_eq!(
            found[1].path.as_deref(),
            Some("/entities/0/components/QueryLayers/layers/1")
        );

        let bare = "entities:\n  - name: part\n    components: {}\n";
        let found = scene_layer_diagnostics(&query_layers_scene(bare, &["default"]));
        let codes: Vec<_> = found.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::QUERY_LAYERS_UNCLAIMED, code::QUERY_LAYERS_INVALID],
            "`default` is the own layer of an entity with no collider: {found:?}"
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/QueryLayers")
        );
    }

    #[test]
    fn query_layers_beside_a_motor_or_a_claimed_collider_are_clean() {
        let body = collider("sphere:\n            radius: 0.5", "static", "default");
        assert!(scene_layer_diagnostics(&query_layers_scene(&body, &["dummy"])).is_empty());
        let motor = "entities:\n  - name: hero\n    components:\n      CharacterMotor:\n        layer: dummy\n";
        assert!(scene_layer_diagnostics(&query_layers_scene(motor, &["default"])).is_empty());
    }

    const SURFACES: &str =
        "physics:\n  space: free\n  surfaces:\n    - concrete\n    - metal_grate\n";

    /// A checked scene with `SURFACES` and the entities in `body`, then `SurfaceTag` pushed onto
    /// entity 0 by hand (the component is registered with the engine's registry separately).
    fn surface_scene(body: &str, surface: &str) -> Scene {
        let mut scene = ludion_scene::check(
            &format!("format: 1\nseed: 1\n{SURFACES}{body}"),
            &crate::builtin_registry(),
        )
        .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        scene.entities[0].components.push((
            "SurfaceTag".to_string(),
            serde_json::json!({ "surface": surface }),
        ));
        scene
    }

    #[test]
    fn surface_tags_report_unknown_and_unclaimed() {
        let body = "entities:\n  - name: part\n    components: {}\n";
        let found = scene_surface_diagnostics(&surface_scene(body, "metal_grat"));
        let codes: Vec<_> = found.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::SURFACE_TAG_UNCLAIMED, code::SURFACE_UNKNOWN],
            "{found:?}"
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceTag")
        );
        assert_eq!(
            found[1].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/surface")
        );
        assert_eq!(
            found[1].help.as_deref(),
            Some("nearest match is `metal_grate`")
        );
    }

    #[test]
    fn a_declared_surface_beside_a_collider_or_motor_is_clean() {
        let body = collider("sphere:\n            radius: 0.5", "static", "default");
        assert!(scene_surface_diagnostics(&surface_scene(&body, "concrete")).is_empty());
        let motor = "entities:\n  - name: hero\n    components:\n      CharacterMotor: {}\n";
        assert!(scene_surface_diagnostics(&surface_scene(motor, "metal_grate")).is_empty());
    }

    /// `surface_scene` with a `SurfaceOverlay` instead of a `SurfaceTag` on entity 0.
    fn overlay_scene(body: &str, surface: &str) -> Scene {
        let mut scene = surface_scene(body, "concrete");
        scene.entities[0].components.pop();
        scene.entities[0].components.push((
            "SurfaceOverlay".to_string(),
            serde_json::json!({ "surface": surface }),
        ));
        scene
    }

    #[test]
    fn a_surface_overlay_needs_a_sensor_collider_and_a_declared_surface() {
        let solid = collider("sphere:\n            radius: 0.5", "static", "default");
        let found = scene_surface_diagnostics(&overlay_scene(&solid, "concret"));
        let codes: Vec<_> = found.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::SURFACE_OVERLAY_NO_SENSOR, code::SURFACE_UNKNOWN],
            "{found:?}"
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceOverlay")
        );
        assert_eq!(
            found[1].path.as_deref(),
            Some("/entities/0/components/SurfaceOverlay/surface")
        );
        assert_eq!(
            found[1].help.as_deref(),
            Some("nearest match is `concrete`")
        );

        let bare = "entities:\n  - name: part\n    components: {}\n";
        let found = scene_surface_diagnostics(&overlay_scene(bare, "concrete"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SURFACE_OVERLAY_NO_SENSOR);

        let sensor = format!("{solid}        sensor: true\n");
        assert!(scene_surface_diagnostics(&overlay_scene(&sensor, "concrete")).is_empty());
    }

    /// `surface_scene` with `parts` set on the entity-0 `SurfaceTag`.
    fn parts_scene(body: &str, parts: serde_json::Value) -> Scene {
        let mut scene = surface_scene(body, "concrete");
        scene.entities[0].components.pop();
        scene.entities[0].components.push((
            "SurfaceTag".to_string(),
            serde_json::json!({ "surface": "concrete", "parts": parts }),
        ));
        scene
    }

    const COMPOUND: &str = "entities:\n  - name: part\n    components:\n      Collider:\n        shape:\n          compound:\n            parts:\n              - shape:\n                  sphere:\n                    radius: 0.5\n              - shape:\n                  sphere:\n                    radius: 0.5\n        body: static\n";

    #[test]
    fn a_part_index_must_name_a_compound_part_and_a_declared_surface() {
        let parts = serde_json::json!([
            { "part": 1, "surface": "metal_grate" },
            { "part": 2, "surface": "metal_grat" },
        ]);
        let found = scene_surface_diagnostics(&parts_scene(COMPOUND, parts));
        let codes: Vec<_> = found.iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [code::SURFACE_UNKNOWN, code::SURFACE_PART_INVALID],
            "{found:?}"
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/parts/1/surface")
        );
        assert_eq!(
            found[0].help.as_deref(),
            Some("nearest match is `metal_grate`")
        );
        assert_eq!(
            found[1].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/parts/1/part")
        );
    }

    #[test]
    fn a_part_index_without_a_compound_is_l1694() {
        let solid = collider("sphere:\n            radius: 0.5", "static", "default");
        let parts = serde_json::json!([{ "part": 0, "surface": "metal_grate" }]);
        let found = scene_surface_diagnostics(&parts_scene(&solid, parts));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::SURFACE_PART_INVALID);
    }

    #[test]
    fn a_part_name_needs_an_assembly_mesh() {
        let parts = serde_json::json!([{ "part": "helmet", "surface": "metal_grate" }]);
        let found = scene_surface_diagnostics(&parts_scene(COMPOUND, parts.clone()));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::SURFACE_PART_INVALID);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/parts/0/part")
        );

        let mut scene = parts_scene(COMPOUND, parts);
        scene.entities[0].components.push((
            "Mesh".to_string(),
            serde_json::json!({ "shape": { "assembly": "models/figure.assembly.yaml" } }),
        ));
        assert!(scene_surface_diagnostics(&scene).is_empty());
    }

    const HEIGHTFIELD: &str = "entities:\n  - name: ground\n    components:\n      Collider:\n        shape:\n          heightfield:\n            rows: 3\n            cols: 3\n            heights:\n              - 0\n              - 0\n              - 0\n              - 0\n              - 0\n              - 0\n              - 0\n              - 0\n              - 0\n        body: static\n";

    fn cells_scene(body: &str, cells: serde_json::Value) -> Scene {
        let mut scene = surface_scene(body, "concrete");
        let tag = scene.entities[0].components.last_mut().expect("the tag");
        tag.1 = serde_json::json!({ "surface": "concrete", "cells": cells });
        scene
    }

    #[test]
    fn heightfield_cells_must_cover_the_grid_and_name_declared_surfaces() {
        let ok = serde_json::json!({ "surfaces": ["concrete", "metal_grate"], "runs": [[3, 0], [1, 1]] });
        assert!(scene_surface_diagnostics(&cells_scene(HEIGHTFIELD, ok)).is_empty());

        let short = serde_json::json!({ "surfaces": ["concrete"], "runs": [[3, 0]] });
        let found = scene_surface_diagnostics(&cells_scene(HEIGHTFIELD, short));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::MESH_SURFACES_INVALID);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/cells")
        );

        let unknown = serde_json::json!({ "surfaces": ["metal_grat"], "runs": [[4, 0]] });
        let found = scene_surface_diagnostics(&cells_scene(HEIGHTFIELD, unknown));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::SURFACE_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/SurfaceTag/cells/surfaces/0")
        );
    }

    #[test]
    fn cells_on_a_non_heightfield_collider_are_l1696() {
        let solid = collider("sphere:\n            radius: 0.5", "static", "default");
        let cells = serde_json::json!({ "surfaces": ["concrete"], "runs": [[1, 0]] });
        let found = scene_surface_diagnostics(&cells_scene(&solid, cells));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::MESH_SURFACES_INVALID);
    }

    fn motor_scene(physics: &str, components: &str) -> Scene {
        let source = format!(
            "format: 1\nseed: 1\n{physics}entities:\n  - name: hero\n    components:\n{components}  - name: base\n    components: {{}}\n"
        );
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    #[test]
    fn a_motor_layer_typo_names_the_nearest_declared_layer() {
        let scene = motor_scene(LAYERS, "      CharacterMotor:\n        layer: dummi\n");
        let found = scene_layer_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CharacterMotor/layer")
        );
        assert_eq!(found[0].help.as_deref(), Some("nearest match is `dummy`"));
    }

    #[test]
    fn a_clean_motor_scene_reports_nothing() {
        let scene = motor_scene(LAYERS, "      CharacterMotor:\n        layer: dummy\n");
        assert!(scene_layer_diagnostics(&scene).is_empty());
        assert!(scene_motor_diagnostics(&scene).is_empty());
    }

    #[test]
    fn a_motor_beside_a_collider_is_l1295() {
        let scene = motor_scene(
            LAYERS,
            "      CharacterMotor: {}\n      Collider:\n        shape:\n          sphere:\n            radius: 0.5\n",
        );
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_WITH_COLLIDER);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Collider")
        );
    }

    #[test]
    fn a_motor_without_a_physics_block_is_l1296() {
        let scene = motor_scene("", "      CharacterMotor: {}\n");
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_NO_PHYSICS_SCENE);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CharacterMotor")
        );
    }

    #[test]
    fn an_undeclared_posture_is_l1540() {
        let scene = motor_scene(
            LAYERS,
            "      CharacterMotor:\n        postures:\n          crouch:\n            shape:\n              upright:\n                height: 1.0\n      CharacterMotorState:\n        posture: crouhc\n",
        );
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_POSTURE_UNKNOWN);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CharacterMotorState/posture")
        );
        assert!(
            found[0]
                .help
                .as_deref()
                .unwrap_or_default()
                .contains("crouch")
        );
        let declared = motor_scene(
            LAYERS,
            "      CharacterMotor:\n        postures:\n          crouch:\n            shape:\n              upright:\n                height: 1.0\n      CharacterMotorState:\n        posture: crouch\n",
        );
        assert!(scene_motor_diagnostics(&declared).is_empty());
    }

    #[test]
    fn a_parented_motor_is_l1297() {
        let scene = motor_scene(
            LAYERS,
            "      CharacterMotor: {}\n      Parent:\n        index: 1\n        generation: 0\n",
        );
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_PARENTED);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Parent")
        );
    }

    #[test]
    fn a_suspended_parented_motor_is_quiet() {
        const PARENT: &str = "      Parent:\n        index: 1\n        generation: 0\n";
        let parked = motor_scene(
            LAYERS,
            &format!(
                "      CharacterMotor: {{}}\n      CharacterMotorState:\n        suspended: true\n{PARENT}"
            ),
        );
        assert!(scene_motor_diagnostics(&parked).is_empty());
        let live = motor_scene(
            LAYERS,
            &format!(
                "      CharacterMotor: {{}}\n      CharacterMotorState:\n        grounded: true\n{PARENT}"
            ),
        );
        let found = scene_motor_diagnostics(&live);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CHARACTER_MOTOR_PARENTED);
    }

    #[test]
    fn a_surface_velocity_needs_a_static_or_kinematic_collider() {
        const BOX: &str = "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.0\n              - 0.1\n              - 1.0\n";
        for (body, clean) in [
            (" static", true),
            (" kinematic", true),
            ("\n          rigid:\n            mass: 1.0", false),
        ] {
            let scene = motor_scene(
                LAYERS,
                &format!(
                    "      SurfaceVelocity:\n        velocity:\n          - 1.5\n          - 0.0\n          - 0.0\n{BOX}        body:{body}\n"
                ),
            );
            let found = scene_motor_diagnostics(&scene);
            assert_eq!(found.is_empty(), clean, "{body}: {found:?}");
            if !clean {
                assert_eq!(found[0].code, code::SURFACE_VELOCITY_NO_SURFACE);
                assert_eq!(
                    found[0].path.as_deref(),
                    Some("/entities/0/components/SurfaceVelocity")
                );
            }
        }
    }

    #[test]
    fn a_surface_velocity_without_a_collider_is_l1299() {
        let scene = motor_scene(LAYERS, "      SurfaceVelocity: {}\n");
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SURFACE_VELOCITY_NO_SURFACE);
    }

    #[test]
    fn a_lane_needs_a_motor_beside_it() {
        let scene = motor_scene(LAYERS, "      LaneConstraint: {}\n");
        let found = scene_motor_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::LANE_WITHOUT_MOTOR);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/LaneConstraint")
        );

        let scene = motor_scene(
            LAYERS,
            "      LaneConstraint: {}\n      CharacterMotor: {}\n",
        );
        assert!(scene_motor_diagnostics(&scene).is_empty());
    }

    fn scale_scene(blocks: &[(usize, &str)]) -> Scene {
        let mut source = String::from("format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n");
        for (count, components) in blocks {
            for _ in 0..*count {
                source.push_str("  - components:\n");
                source.push_str(components);
            }
        }
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    const RIGID_SPHERE: &str = "      Collider:\n        shape:\n          sphere:\n            radius: 0.5\n        body:\n          rigid:\n            mass: 1.0\n";
    const MOTOR: &str = "      CharacterMotor: {}\n";

    #[test]
    fn past_the_gated_scale_warns_once() {
        let scene = scale_scene(&[(513, RIGID_SPHERE)]);
        let found = scene_physics_scale_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::PHYSICS_SCALE_UNGATED);
        assert_eq!(found[0].path.as_deref(), Some("/physics"));
        assert_eq!(found[0].help.as_deref(), Some(SCALE_HELP));
    }

    #[test]
    fn the_gated_scale_itself_is_silent() {
        let scene = scale_scene(&[(512, RIGID_SPHERE)]);
        assert!(scene_physics_scale_diagnostics(&scene).is_empty());
    }

    #[test]
    fn unclaimed_2d_kinematic_colliders_are_not_counted() {
        let aabb = "      Collider:\n        shape:\n          aabb:\n            half_extents:\n              - 0.5\n              - 0.5\n        body: kinematic\n";
        let scene = scale_scene(&[(600, aabb)]);
        assert!(scene_physics_scale_diagnostics(&scene).is_empty());
    }

    #[test]
    fn motors_count_with_rigid_bodies() {
        let scene = scale_scene(&[(300, MOTOR), (213, RIGID_SPHERE)]);
        assert_eq!(scene_physics_scale_diagnostics(&scene).len(), 1);
    }

    fn rig_scene(entities: &str) -> Scene {
        let source = format!(
            "format: 1
seed: 1
entities:
{entities}"
        );
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    fn sun_scene(light: &str) -> Scene {
        let source =
            format!("format: 1\nentities:\n  - components:\n      DirectionalLight:\n{light}");
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    #[test]
    fn cascades_beside_a_custom_shadow_extent_is_l1313() {
        let scene = sun_scene("        cascades: 2\n        shadow_extent: 34\n");
        let found = scene_sun_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::DIRECTIONAL_LIGHT_EXTENT_UNUSED);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/DirectionalLight/shadow_extent")
        );
    }

    /// A checked scene with `LAYERS`: a subject whose `LightExposure` has `exposure`, a static box
    /// on `layer`, and `extra` entities.
    fn exposure_scene(exposure: &str, layer: &str, extra: &str) -> Scene {
        let source = format!(
            "format: 1\nseed: 1\n{LAYERS}entities:\n  - name: subject\n    components:\n      \
             LightExposure:\n{exposure}  - name: box\n    components:\n      Collider:\n        \
             shape:\n          box:\n            half_extents:\n              - 1.0\n              \
             - 1.0\n              - 1.0\n        body: static\n        layer: {layer}\n{extra}"
        );
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    #[test]
    fn light_exposure_warns_about_presentation_lights_and_unmatched_layers() {
        let layers = "        occluder_layers:\n          - dummy\n";
        let codes = |scene: &Scene| -> Vec<_> {
            scene_light_exposure_diagnostics(scene)
                .iter()
                .map(|d| d.code)
                .collect()
        };
        assert!(codes(&exposure_scene(layers, "dummy", "")).is_empty());
        let no_layers = "        combine: max\n";
        assert!(codes(&exposure_scene(no_layers, "default", "")).is_empty());
        let unmatched = scene_light_exposure_diagnostics(&exposure_scene(layers, "default", ""));
        assert_eq!(unmatched.len(), 1, "{unmatched:?}");
        assert_eq!(unmatched[0].code, code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED);
        assert_eq!(
            unmatched[0].path.as_deref(),
            Some("/entities/0/components/LightExposure/occluder_layers")
        );
        let typo = "        occluder_layers:\n          - dummi\n";
        assert_eq!(
            codes(&exposure_scene(typo, "dummy", "")),
            [
                code::COLLIDER_LAYER_UNKNOWN,
                code::LIGHT_EXPOSURE_OCCLUDERS_UNMATCHED
            ]
        );
        let lamp =
            "  - name: glow\n    presentation: true\n    components:\n      PointLight: {}\n";
        let presented = scene_light_exposure_diagnostics(&exposure_scene(layers, "dummy", lamp));
        assert_eq!(presented.len(), 1, "{presented:?}");
        assert_eq!(presented[0].code, code::LIGHT_EXPOSURE_PRESENTATION_LIGHT);
        assert_eq!(
            presented[0].path.as_deref(),
            Some("/entities/2/components/PointLight")
        );
    }

    #[test]
    fn a_scene_without_light_exposure_has_no_light_exposure_diagnostics() {
        let lamp = "format: 1\nentities:\n  - presentation: true\n    components:\n      \
                    PointLight: {}\n";
        let scene = ludion_scene::check(lamp, &crate::builtin_registry()).expect("fixture");
        assert!(scene_light_exposure_diagnostics(&scene).is_empty());
    }

    fn local_scene(component: &str, light: &str) -> Scene {
        let source = format!("format: 1\nentities:\n  - components:\n      {component}:\n{light}");
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    fn lens_scene(body: &str) -> Scene {
        let source = format!("format: 1\nentities:\n{body}");
        ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"))
    }

    #[test]
    fn dof_with_a_zero_focus_distance_is_l1324() {
        let found = scene_lens_and_sun_disc_diagnostics(&lens_scene(
            "  - components:\n      PostProcess:\n        dof: 0.1\n        focus_distance: 0.0\n",
        ));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_DOF_FOCUS_ZERO);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/PostProcess/focus_distance")
        );
    }

    #[test]
    fn dof_with_a_focus_distance_or_no_dof_is_clean() {
        for camera in [
            "        dof: 0.1\n        focus_distance: 6.0\n",
            "        dof: 0.1\n",
            "        focus_distance: 0.0\n",
        ] {
            let scene = lens_scene(&format!("  - components:\n      PostProcess:\n{camera}"));
            assert!(
                scene_lens_and_sun_disc_diagnostics(&scene).is_empty(),
                "{camera}"
            );
        }
    }

    #[test]
    fn a_sun_disc_that_is_not_twice_the_radius_is_l1325() {
        let found = scene_lens_and_sun_disc_diagnostics(&lens_scene(
            "  - components:\n      DirectionalLight:\n        angular_radius: 0.5\n  - components:\n      Environment:\n        sun_disc_size: 0.5\n",
        ));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::SUN_DISC_SIZE_MISMATCH);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/Environment/sun_disc_size")
        );
    }

    #[test]
    fn a_matching_or_half_authored_sun_disc_is_clean() {
        for body in [
            "  - components:\n      DirectionalLight:\n        angular_radius: 0.5\n  - components:\n      Environment:\n        sun_disc_size: 1.0\n",
            "  - components:\n      DirectionalLight:\n        angular_radius: 0.5\n",
            "  - components:\n      Environment:\n        sun_disc_size: 1.0\n",
        ] {
            assert!(
                scene_lens_and_sun_disc_diagnostics(&lens_scene(body)).is_empty(),
                "{body}"
            );
        }
    }

    #[test]
    fn a_shadow_distance_without_cast_shadows_is_l1323() {
        for component in ["PointLight", "SpotLight"] {
            let found = scene_local_light_diagnostics(&local_scene(
                component,
                "        shadow_distance: 20.0\n        shadow_fade: 5.0\n",
            ));
            assert_eq!(found.len(), 1, "{component}");
            assert_eq!(found[0].code, code::LOCAL_LIGHT_SHADOW_FADE_UNUSED);
            assert_eq!(
                found[0].path.as_deref(),
                Some(format!("/entities/0/components/{component}/shadow_distance").as_str())
            );
        }
    }

    #[test]
    fn local_lights_that_cast_or_set_nothing_are_clean() {
        for light in [
            "        cast_shadows: true\n        shadow_distance: 20.0\n",
            "        shadow_fade: 0.0\n",
            "        range: 5.0\n",
        ] {
            assert!(
                scene_local_light_diagnostics(&local_scene("PointLight", light)).is_empty(),
                "{light}"
            );
        }
    }

    #[test]
    fn a_default_extent_or_no_cascades_is_clean() {
        for light in [
            "        cascades: 2\n        shadow_extent: 40.0\n",
            "        shadow_extent: 34\n",
        ] {
            assert!(
                scene_sun_diagnostics(&sun_scene(light)).is_empty(),
                "{light}"
            );
        }
    }

    #[test]
    fn a_rig_on_a_camera_root_is_clean() {
        let scene = rig_scene(
            "  - name: cam
    components:
      Camera: {}
      CameraRig: {}
  - name: hero
    components: {}
",
        );
        assert!(scene_camera_rig_diagnostics(&scene).is_empty());
    }

    #[test]
    fn a_zone_without_a_sensor_is_l1554() {
        let scene = rig_scene(
            "  - name: bare
    components:
      CameraZone: {}
  - name: solid
    components:
      CameraZone: {}
      Collider:
        shape:
          box:
            half_extents:
              - 1.0
              - 1.0
              - 1.0
  - name: ok
    components:
      CameraZone: {}
      Collider:
        shape:
          box:
            half_extents:
              - 1.0
              - 1.0
              - 1.0
        sensor: true
",
        );
        let found = scene_camera_rig_diagnostics(&scene);
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|d| d.code == code::CAMERA_ZONE_WITHOUT_SENSOR)
        );
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CameraZone")
        );
        assert_eq!(
            found[1].path.as_deref(),
            Some("/entities/1/components/CameraZone")
        );
    }

    #[test]
    fn a_rig_without_a_camera_is_l1307() {
        let scene = rig_scene(
            "  - name: cam
    components:
      CameraRig: {}
",
        );
        let found = scene_camera_rig_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_RIG_WITHOUT_CAMERA);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CameraRig")
        );
    }

    #[test]
    fn a_second_rig_is_l1308_naming_both_entities() {
        let scene = rig_scene(
            "  - name: a
    components:
      Camera: {}
      CameraRig: {}
  - name: b
    components:
      Camera: {}
      CameraRig: {}
",
        );
        let found = scene_camera_rig_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_RIG_DUPLICATE);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/CameraRig")
        );
        assert!(found[0].message.contains("0 and 1"), "{}", found[0].message);
        assert!(!found[0].message.contains("  "), "{}", found[0].message);
    }

    #[test]
    fn one_audio_listener_is_clean() {
        let scene = rig_scene(
            "  - name: ear
    components:
      AudioListener: {}
  - name: other
    components: {}
",
        );
        assert!(scene_audio_listener_diagnostics(&scene).is_empty());
    }

    #[test]
    fn a_second_audio_listener_is_l1650_naming_both_entities() {
        let scene = rig_scene(
            "  - name: a
    components:
      AudioListener: {}
  - name: b
    components:
      AudioListener:
        orientation: self
",
        );
        let found = scene_audio_listener_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::AUDIO_LISTENER_DUPLICATE);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/AudioListener")
        );
        assert!(found[0].message.contains("0 and 1"), "{}", found[0].message);
    }

    #[test]
    fn a_parented_rig_is_l1309() {
        let scene = rig_scene(
            "  - name: cam
    components:
      Camera: {}
      CameraRig: {}
      Parent:
        index: 1
        generation: 0
  - name: base
    components: {}
",
        );
        let found = scene_camera_rig_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_RIG_PARENTED);
        assert!(!found[0].message.contains("  "), "{}", found[0].message);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/Parent")
        );
    }

    #[test]
    fn a_camera_offset_on_a_presentation_entity_is_clean() {
        let scene = rig_scene(
            "  - name: intro
    presentation: true
    components:
      CameraOffset:
        translation:
          - 0.0
          - 0.0
          - 4.0
",
        );
        assert!(scene_camera_offset_diagnostics(&scene).is_empty());
    }

    #[test]
    fn a_camera_offset_on_a_sim_entity_is_l1333() {
        let scene = rig_scene(
            "  - name: hero
    components:
      CameraOffset: {}
",
        );
        let found = scene_camera_offset_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_OFFSET_ON_SIM_ENTITY);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/CameraOffset")
        );
    }

    #[test]
    fn a_second_camera_offset_is_l1334_naming_both_entities() {
        let scene = rig_scene(
            "  - name: a
    presentation: true
    components:
      CameraOffset: {}
  - name: b
    presentation: true
    components:
      CameraOffset: {}
",
        );
        let found = scene_camera_offset_diagnostics(&scene);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::CAMERA_OFFSET_DUPLICATE);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/CameraOffset")
        );
        assert!(found[0].message.contains("0 and 1"), "{}", found[0].message);
    }

    #[test]
    fn a_2d_static_circle_with_a_layer_is_unclaimed() {
        let found = diagnostics(
            LAYERS,
            &collider("circle:\n            radius: 0.5", "static", "dummy"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNCLAIMED);
    }

    #[test]
    fn a_rigid_circle_with_a_declared_layer_is_clean() {
        let found = diagnostics(
            LAYERS,
            &collider(
                "circle:\n            radius: 0.5",
                "\n          rigid:\n            mass: 1.0",
                "dummy",
            ),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn default_is_always_declared() {
        let found = diagnostics(
            "",
            &collider("sphere:\n            radius: 0.5", "static", "default"),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    fn tiny_wav() -> Vec<u8> {
        let samples = [0i16, 1000, -1000, 0];
        let data_len = samples.len() * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&16000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes
    }

    /// The audio diagnostics of a scene with one `AudioEmitter` per `sounds` entry, against a scratch
    /// root holding `good.wav` (valid) and `bad.wav` (not a WAV).
    fn audio_diagnostics(sounds: &[&str]) -> Vec<Diagnostic> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-check-audio-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("good.wav"), tiny_wav()).expect("good.wav");
        std::fs::write(dir.join("bad.wav"), b"not a wav").expect("bad.wav");
        let mut source = String::from("format: 1\nseed: 1\nentities:\n");
        for sound in sounds {
            source.push_str(&format!(
                "  - components:\n      AudioEmitter:\n        sound: \"{sound}\"\n"
            ));
        }
        let scene = ludion_scene::check(&source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        let found = scene_audio_diagnostics(&dir, &scene);
        std::fs::remove_dir_all(&dir).ok();
        found
    }

    #[test]
    fn a_missing_sound_is_l1101_at_its_pointer() {
        let found = audio_diagnostics(&["good.wav", "gone.wav"]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_FILE_IO);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/1/components/AudioEmitter/sound")
        );
    }

    #[test]
    fn a_non_wav_sound_is_l1104() {
        let found = audio_diagnostics(&["bad.wav"]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_WAV_DECODE);
    }

    #[test]
    fn an_empty_sound_is_l1101_with_its_own_message() {
        let found = audio_diagnostics(&[""]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_FILE_IO);
        assert!(
            found[0].message.contains("names no sound"),
            "{}",
            found[0].message
        );
    }

    #[test]
    fn two_emitters_sharing_a_bad_path_each_report_it() {
        let found = audio_diagnostics(&["bad.wav", "good.wav", "bad.wav"]);
        let pointers: Vec<_> = found.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(
            pointers,
            [
                "/entities/0/components/AudioEmitter/sound",
                "/entities/2/components/AudioEmitter/sound"
            ]
        );
    }

    #[test]
    fn a_zone_bed_sound_is_checked_like_an_emitters() {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ludion-scene-check-zone-audio-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        std::fs::write(dir.join("good.wav"), tiny_wav()).expect("good.wav");
        let source = "format: 1
seed: 1
entities:
  - components:
      AudioZone:
        shape:
          sphere:
            radius: 4.0
        beds:
          - sound: good.wav
          - sound: gone.wav
";
        let scene = ludion_scene::check(source, &crate::builtin_registry())
            .unwrap_or_else(|errors| panic!("fixture scene invalid: {errors:?}"));
        let found = scene_audio_diagnostics(&dir, &scene);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, code::ASSET_FILE_IO);
        assert_eq!(
            found[0].path.as_deref(),
            Some("/entities/0/components/AudioZone/beds/1/sound")
        );
    }

    #[test]
    fn a_valid_sound_is_clean() {
        assert!(audio_diagnostics(&["good.wav"]).is_empty());
    }

    #[test]
    fn no_physics_block_declares_nothing() {
        let found = diagnostics(
            "",
            &collider("sphere:\n            radius: 0.5", "static", "dummy"),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, code::COLLIDER_LAYER_UNKNOWN);
    }
}
