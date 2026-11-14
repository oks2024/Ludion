//! The `Static` marker's eligibility rules (`L1341`, `L1342`) and its hot-write detector
//! (`L1343`): one classifier the scene check and the runtime share.

use std::path::Path;

use ludion_assets::AssetRoot;
use ludion_components::{Mesh, MeshGrid, MeshSource};
use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_scene::Scene;

/// Why an entity carrying `Static` cannot take the static path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticIneligible {
    /// Neither a `Mesh` nor a `MeshGrid`: there is nothing to keep.
    NothingToDraw,
    /// A component the static path cannot hold.
    Component(&'static str),
    /// A mesh shape the static path cannot hold.
    Shape(&'static str),
    /// A mesh asset with joints.
    SkinnedAsset,
}

impl StaticIneligible {
    /// The reason as a message fragment.
    pub fn reason(&self) -> String {
        match self {
            StaticIneligible::NothingToDraw => "it has no `Mesh` or `MeshGrid` to draw".to_string(),
            StaticIneligible::Component(name) => format!("it carries a `{name}`"),
            StaticIneligible::Shape(shape) => format!("its mesh shape is `{shape}`"),
            StaticIneligible::SkinnedAsset => "its mesh asset is skinned".to_string(),
        }
    }
}

/// Components that make a `Static` entity ineligible, in the order the first one is named.
const DISQUALIFYING: [&str; 13] = [
    "SkinBone",
    "ParentJoint",
    "Viewmodel",
    "Sprite",
    "ParticleEmitter",
    "Text",
    "Decal",
    "PointLight",
    "SpotLight",
    "DirectionalLight",
    "GroundMarker",
    "Volley",
    "PatternPlayer",
];

/// The first reason an entity cannot be static, or `None` when it can. `has` answers whether the
/// entity carries a component by name; `shapes` is its `Mesh.shape` plus every `MeshGrid.legend`
/// mesh shape. A skinned `asset` needs the file and is the caller's check.
pub(crate) fn static_ineligibility(
    has: &dyn Fn(&str) -> bool,
    shapes: &[&MeshSource],
) -> Option<StaticIneligible> {
    if !(has("Mesh") || has("MeshGrid")) {
        return Some(StaticIneligible::NothingToDraw);
    }
    if let Some(name) = DISQUALIFYING.iter().find(|name| has(name)) {
        return Some(StaticIneligible::Component(name));
    }
    shapes.iter().find_map(|shape| match shape {
        MeshSource::Assembly(_) => Some(StaticIneligible::Shape("assembly")),
        MeshSource::AssemblyPart { .. } => Some(StaticIneligible::Shape("assembly_part")),
        MeshSource::Character(_) => Some(StaticIneligible::Shape("character")),
        _ => None,
    })
}

/// Frames in a row a `Static` entity must be written before it is called hot.
pub const HOT_STATIC_FRAMES: u32 = 30;

/// Counts consecutive frames each static entity was written and reports a hot one once.
#[derive(Clone, Debug, Default)]
pub struct HotStatic {
    streak: StableMap<EntityId, u32>,
    reported: StableSet<EntityId>,
}

impl HotStatic {
    /// One call per frame with the entities written that frame. An entity absent from `written`
    /// starts over; reaching [`HOT_STATIC_FRAMES`] yields one `L1343`, never again for that entity.
    pub fn observe(&mut self, written: &[EntityId]) -> Vec<Diagnostic> {
        let mut next = StableMap::default();
        let mut hot = Vec::new();
        let mut seen = StableSet::default();
        for &id in written {
            if !seen.insert(id) {
                continue;
            }
            let count = self.streak.get(&id).copied().unwrap_or(0) + 1;
            next.insert(id, count);
            if count == HOT_STATIC_FRAMES && !self.reported.contains(&id) {
                self.reported.insert(id);
                hot.push(
                    Diagnostic::new(
                        code::STATIC_REWRITTEN,
                        format!(
                            "static entity {id} was written on {HOT_STATIC_FRAMES} consecutive \
                             frames, so it is re-placed every frame"
                        ),
                    )
                    .with_help("remove `Static` from it"),
                );
            }
        }
        self.streak = next;
        hot
    }

    /// Drops everything known about an entity that was evicted.
    pub fn forget(&mut self, id: EntityId) {
        self.streak.remove(&id);
        self.reported.remove(&id);
    }
}

/// `L1341` for each `Static` entity the static path cannot hold, and `L1342` for each whose `Parent`
/// chain reaches an entity without `Static`. Pointers are `/entities/{i}/components/Static`.
pub fn scene_static_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let root = AssetRoot::content(project_root);
    for (index, entity) in scene.entities.iter().enumerate() {
        if !entity.components.iter().any(|(name, _)| name == "Static") {
            continue;
        }
        let at = format!("/entities/{index}/components/Static");
        let find = |wanted: &str| {
            entity
                .components
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value)
        };
        let mesh: Option<Mesh> = find("Mesh").and_then(|v| serde_json::from_value(v.clone()).ok());
        let grid: Option<MeshGrid> =
            find("MeshGrid").and_then(|v| serde_json::from_value(v.clone()).ok());
        let mut meshes: Vec<&Mesh> = mesh.iter().collect();
        if let Some(grid) = &grid {
            meshes.extend(grid.legend.values());
        }
        let shapes: Vec<&MeshSource> = meshes.iter().map(|m| &m.shape).collect();
        let has = |name: &str| find(name).is_some();
        let ineligible = static_ineligibility(&has, &shapes).or_else(|| {
            meshes
                .iter()
                .filter(|m| matches!(m.shape, MeshSource::Asset(_)))
                .any(|m| {
                    matches!(
                        crate::skin_bone::mesh_document_path(root, m),
                        Some(Ok(path))
                            if matches!(
                                crate::skin_bone::load_mesh_joints(root, &path),
                                Ok(Some(_))
                            )
                    )
                })
                .then_some(StaticIneligible::SkinnedAsset)
        });
        if let Some(reason) = ineligible {
            diagnostics.push(
                Diagnostic::new(
                    code::STATIC_INELIGIBLE,
                    format!(
                        "this entity is `Static` but {}, so it takes the dynamic path",
                        reason.reason()
                    ),
                )
                .with_path(at.clone())
                .with_help("remove `Static`, or remove what makes it ineligible"),
            );
        }
        if let Some(ancestor) = non_static_ancestor(scene, index) {
            diagnostics.push(
                Diagnostic::new(
                    code::STATIC_PARENT_NOT_STATIC,
                    format!(
                        "this entity is `Static` but its ancestor {} is not, so it takes the \
                         dynamic path",
                        crate::skin_bone::named(&scene.entities[ancestor], ancestor)
                    ),
                )
                .with_path(at)
                .with_help("mark every ancestor `Static`, or remove `Static` from this entity"),
            );
        }
    }
    diagnostics
}

/// The first `Parent` ancestor of entity `index` that lacks `Static`, if any.
fn non_static_ancestor(scene: &Scene, index: usize) -> Option<usize> {
    let mut current = index;
    for _ in 0..scene.entities.len() {
        let parent = scene.entities[current]
            .components
            .iter()
            .find(|(name, _)| name == "Parent")
            .and_then(|(_, value)| value.get("index"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|p| usize::try_from(p).ok())?;
        let entity = scene.entities.get(parent)?;
        if !entity.components.iter().any(|(name, _)| name == "Static") {
            return Some(parent);
        }
        current = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(components: &str) -> Scene {
        ludion_scene::check(
            &format!("format: 1\nentities:\n{components}"),
            &crate::builtin_registry(),
        )
        .unwrap_or_else(|errors| panic!("{errors:?}"))
    }

    fn codes_of(text: &str) -> Vec<String> {
        let root = std::env::temp_dir();
        scene_static_diagnostics(&root, &scene(text))
            .iter()
            .map(|d| d.code.as_str().to_string())
            .collect()
    }

    #[test]
    fn eligible_entities_report_nothing() {
        let text = "  - components:\n      Static: {}\n      Mesh:\n        shape: cube\n  \
                    - components:\n      Static: {}\n      Mesh:\n        shape: sphere\n  \
                    - components:\n      Static: {}\n      MeshGrid:\n        legend:\n          \
                    '#':\n            shape: cube\n        rows:\n          - '#'\n";
        assert!(codes_of(text).is_empty());
    }

    #[test]
    fn ineligible_entities_are_l1341() {
        for body in [
            "      Static: {}\n      Transform: {}\n",
            "      Static: {}\n      Sprite: {}\n",
            "      Static: {}\n      Mesh:\n        shape:\n          assembly: a.assembly.yaml\n",
            "      Static: {}\n      Mesh:\n        shape:\n          character: a.character.yaml\n",
            "      Static: {}\n      Mesh:\n        shape: cube\n      PointLight: {}\n",
        ] {
            assert_eq!(
                codes_of(&format!("  - components:\n{body}")),
                ["L1341"],
                "{body}"
            );
        }
    }

    #[test]
    fn a_static_child_of_a_moving_parent_is_l1342() {
        let text = "  - components:\n      Mesh:\n        shape: cube\n  \
                    - components:\n      Static: {}\n      Mesh:\n        shape: cube\n      \
                    Parent:\n        index: 0\n        generation: 0\n";
        assert_eq!(codes_of(text), ["L1342"]);
    }

    #[test]
    fn a_static_chain_reports_nothing() {
        let text = "  - components:\n      Static: {}\n      Mesh:\n        shape: cube\n  \
                    - components:\n      Static: {}\n      Mesh:\n        shape: cube\n      \
                    Parent:\n        index: 0\n        generation: 0\n";
        assert!(codes_of(text).is_empty());
    }

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 0)
    }

    #[test]
    fn hot_static_reports_once_at_thirty_frames() {
        let mut hot = HotStatic::default();
        for _ in 0..HOT_STATIC_FRAMES - 1 {
            assert!(hot.observe(&[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(&[id(1)]).len(), 1);
        for _ in 0..5 {
            assert!(hot.observe(&[id(1)]).is_empty());
        }
    }

    #[test]
    fn a_gap_resets_the_streak() {
        let mut hot = HotStatic::default();
        for _ in 0..HOT_STATIC_FRAMES - 1 {
            hot.observe(&[id(1)]);
        }
        assert!(hot.observe(&[]).is_empty());
        for _ in 0..HOT_STATIC_FRAMES - 1 {
            assert!(hot.observe(&[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(&[id(1)]).len(), 1);
    }

    fn world_hash(presentation: bool, with_static: bool) -> ludion_sim::WorldHash {
        let marker = if with_static {
            "      Static: {}\n"
        } else {
            ""
        };
        let flag = if presentation {
            "    presentation: true\n"
        } else {
            ""
        };
        let document = format!(
            "format: 1\nseed: 1\nentities:\n  - name: crate\n{flag}    components:\n      Transform: {{}}\n      Mesh:\n        shape: cube\n{marker}"
        );
        let mut engine = crate::Engine::builder().build().expect("the game builds");
        engine
            .load_inline_scene(&document)
            .expect("the scene loads");
        engine.world_hash().expect("hashes")
    }

    #[test]
    fn static_is_hashed_on_a_sim_entity_and_not_on_a_presentation_one() {
        assert_ne!(world_hash(false, false), world_hash(false, true));
        assert_eq!(world_hash(true, false), world_hash(true, true));
    }
}
