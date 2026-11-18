//! Static `L1572` checks for perception names: the scene-authored layers, classes and noise kinds a
//! perception run would report as undeclared, found without running a tick.
//!
//! The layer pass needs only the scene (its `physics.layers`); the class and kind pass needs the
//! game's [`PerceptionConfig`]. Both name the nearest declared match and report every occurrence at
//! its JSON Pointer, where the runtime reports once per name.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest::nearest;
use ludion_scene::{Scene, SceneEntity};
use serde_json::Value;

use crate::perception::PerceptionConfig;

fn component<'a>(entity: &'a SceneEntity, wanted: &str) -> Option<&'a Value> {
    entity
        .components
        .iter()
        .find(|(name, _)| name == wanted)
        .map(|(_, value)| value)
}

/// The strings of the array at `value[key]`, with their indices.
fn names<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = (usize, &'a str)> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, n)| Some((i, n.as_str()?)))
}

fn items<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = (usize, &'a Value)> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
}

fn unknown(what: &str, name: &str, declared: &[&str], at: String, fix: &str) -> Diagnostic {
    let help = match nearest(name, declared.iter().copied()) {
        Some(near) => format!("did you mean `{near}`? {fix}"),
        None => fix.to_string(),
    };
    Diagnostic::new(
        code::PERCEPTION_NAME_UNKNOWN,
        format!("perception {what} `{name}` is not declared"),
    )
    .with_path(at)
    .with_help(help)
}

/// `L1572` (a warning) for each `see_through` and `blocked_by` layer a scene's perception components
/// name that its `physics.layers` does not declare. Silent for a scene with no perception component.
///
/// # Example
/// ```
/// use ludion_engine::scene_perception_diagnostics;
///
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - components:\n      Transform: {}\n      SightTarget:\n        class: player\n        \
///      points:\n          - offset:\n              - 0.0\n              - 1.0\n              - 0.0\n            \
///      blocked_by:\n              - wal\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_perception_diagnostics(&scene);
/// assert_eq!(found.len(), 1);
/// assert_eq!(
///     found[0].path.as_deref(),
///     Some("/entities/0/components/SightTarget/points/0/blocked_by/0")
/// );
/// ```
pub fn scene_perception_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let matrix = scene.physics.as_ref().map(|physics| physics.layer_matrix());
    let declared: Vec<&str> = match &matrix {
        Some(matrix) => matrix.names().iter().map(String::as_str).collect(),
        None => vec![ludion_scene::DEFAULT_LAYER],
    };
    let mut diagnostics = Vec::new();
    let mut check = |name: &str, at: String| {
        if !declared.contains(&name) {
            diagnostics.push(unknown(
                "layer",
                name,
                &declared,
                at,
                "declare the layer under `physics.layers`",
            ));
        }
    };
    for (index, entity) in scene.entities.iter().enumerate() {
        let base = format!("/entities/{index}/components");
        if let Some(perceiver) = component(entity, "Perceiver") {
            for (z, zone) in items(perceiver, "sight") {
                for (k, name) in names(zone, "see_through") {
                    check(name, format!("{base}/Perceiver/sight/{z}/see_through/{k}"));
                }
            }
        }
        if let Some(target) = component(entity, "SightTarget") {
            for (p, point) in items(target, "points") {
                for (k, name) in names(point, "blocked_by") {
                    check(
                        name,
                        format!("{base}/SightTarget/points/{p}/blocked_by/{k}"),
                    );
                }
            }
        }
    }
    diagnostics
}

/// `L1572` (a warning) for each `detects` or `SightTarget.class` the `config` does not declare in
/// `classes`, and each `hears` or `NoiseZone.kinds` entry it does not declare in `noise_kinds`.
///
/// # Example
/// ```
/// use ludion_engine::{PerceptionConfig, scene_perception_name_diagnostics};
///
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - components:\n      Transform: {}\n      SightTarget:\n        class: plyer\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let config = PerceptionConfig { classes: vec!["player".into()], ..Default::default() };
/// let found = scene_perception_name_diagnostics(&scene, &config);
/// assert_eq!(found.len(), 1);
/// assert!(found[0].help.as_deref().unwrap().contains("did you mean `player`"));
/// ```
pub fn scene_perception_name_diagnostics(
    scene: &Scene,
    config: &PerceptionConfig,
) -> Vec<Diagnostic> {
    let classes: Vec<&str> = config.classes.iter().map(String::as_str).collect();
    let kinds: Vec<&str> = config.noise_kinds.iter().map(String::as_str).collect();
    let mut diagnostics = Vec::new();
    let class_fix = "declare classes in `PerceptionConfig.classes`";
    let kind_fix = "declare noise kinds in `PerceptionConfig.noise_kinds`";
    for (index, entity) in scene.entities.iter().enumerate() {
        let base = format!("/entities/{index}/components");
        let mut report = |what: &str, name: &str, declared: &[&str], at: String, fix: &str| {
            if !declared.contains(&name) {
                diagnostics.push(unknown(what, name, declared, at, fix));
            }
        };
        if let Some(perceiver) = component(entity, "Perceiver") {
            for (z, zone) in items(perceiver, "sight") {
                for (k, name) in names(zone, "detects") {
                    let at = format!("{base}/Perceiver/sight/{z}/detects/{k}");
                    report("class", name, &classes, at, class_fix);
                }
            }
            if let Some(hearing) = perceiver.get("hearing") {
                for (k, name) in names(hearing, "hears") {
                    let at = format!("{base}/Perceiver/hearing/hears/{k}");
                    report("noise kind", name, &kinds, at, kind_fix);
                }
            }
        }
        if let Some(class) = component(entity, "SightTarget")
            .and_then(|target| target.get("class"))
            .and_then(Value::as_str)
        {
            let at = format!("{base}/SightTarget/class");
            report("class", class, &classes, at, class_fix);
        }
        if let Some(zone) = component(entity, "NoiseZone") {
            for (k, name) in names(zone, "kinds") {
                let at = format!("{base}/NoiseZone/kinds/{k}");
                report("noise kind", name, &kinds, at, kind_fix);
            }
        }
    }
    diagnostics
}
