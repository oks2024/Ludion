//! The `Static` marker's eligibility rules (`L1341`, `L1342`) and its hot-write detector
//! (`L1343`): one classifier the scene check and the runtime share.

use std::path::Path;

use ludion_assets::AssetRoot;
use ludion_components::{Mesh, MeshGrid, MeshSource};
use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_scene::Scene;

/// Why an entity carrying `Static` cannot take the static path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticIneligible {
    /// A component the static path cannot hold.
    Component(&'static str),
    /// A mesh shape the static path cannot hold.
    Shape(&'static str),
    /// A mesh asset with joints.
    SkinnedAsset,
    /// A `Mesh` with `blend`, drawn in `transparents` rather than from the static prefix.
    Blended,
    /// Posed every frame by the presentation pose table, which no write journals.
    Posed {
        /// The `Parent` ancestor that is posed, `None` when the entity itself is.
        ancestor: Option<String>,
        /// What poses it: `ParentJoint`, `SkinBone`, or `Animation` on a prefab root.
        by: &'static str,
    },
}

impl StaticIneligible {
    /// The reason as a message fragment.
    pub fn reason(&self) -> String {
        match self {
            StaticIneligible::Component(name) => format!("it carries a `{name}`"),
            StaticIneligible::Shape(shape) => format!("its mesh shape is `{shape}`"),
            StaticIneligible::SkinnedAsset => "its mesh asset is skinned".to_string(),
            StaticIneligible::Blended => {
                "its `Mesh` is blended (`blend`, or its document's `alpha_mode: blend`)".to_string()
            }
            StaticIneligible::Posed { ancestor, by } => {
                let source = match *by {
                    "Animation" => "its prefab root's `Animation`".to_string(),
                    by => format!("a `{by}`"),
                };
                match ancestor {
                    Some(ancestor) => {
                        format!("its ancestor {ancestor} is posed every frame by {source}")
                    }
                    None => format!("it is posed every frame by {source}"),
                }
            }
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

/// The first reason an entity cannot be static, or `None` when it can. An entity with no `Mesh`
/// or `MeshGrid` and no disqualifying component is a transform group: never ineligible. `has` answers whether the
/// entity carries a component by name; `shapes` is its `Mesh.shape` plus every `MeshGrid.legend`
/// mesh shape; `blended` is whether any of those meshes sets `blend`. A skinned `asset` needs the file
/// and is the caller's check.
pub(crate) fn static_ineligibility(
    has: &dyn Fn(&str) -> bool,
    shapes: &[&MeshSource],
    blended: bool,
) -> Option<StaticIneligible> {
    if let Some(name) = DISQUALIFYING.iter().find(|name| has(name)) {
        return Some(StaticIneligible::Component(name));
    }
    shapes
        .iter()
        .find_map(|shape| match shape {
            MeshSource::Assembly(_) => Some(StaticIneligible::Shape("assembly")),
            MeshSource::AssemblyPart { .. } => Some(StaticIneligible::Shape("assembly_part")),
            MeshSource::Character(_) => Some(StaticIneligible::Shape("character")),
            _ => None,
        })
        .or(blended.then_some(StaticIneligible::Blended))
}

/// What poses a node from the presentation pose table, in the order the first one is named:
/// `ParentJoint`, `SkinBone`, or (`prefab_root_animated`) the `Animation` of its prefab's root.
pub(crate) fn pose_source(
    has: &dyn Fn(&str) -> bool,
    prefab_root_animated: bool,
) -> Option<&'static str> {
    ["ParentJoint", "SkinBone"]
        .into_iter()
        .find(|name| has(name))
        .or(prefab_root_animated.then_some("Animation"))
}

/// Frames in a row a `Static` entity must be written before it is called hot.
pub const HOT_STATIC_FRAMES: u32 = 30;

/// Longest gap between two observations that still counts as one streak: a paused or minimized
/// extract must not turn two distant writes into a write-every-frame streak.
const HOT_STATIC_MAX_GAP: u64 = 4;

/// Counts consecutive sim frames each static entity was written and reports a hot one once.
#[derive(Clone, Debug, Default)]
pub struct HotStatic {
    /// Per entity: the streak length and the last observed frame it was written at.
    streak: StableMap<EntityId, (u32, FrameIndex)>,
    reported: StableSet<EntityId>,
    last: Option<FrameIndex>,
    prev: Option<FrameIndex>,
}

impl HotStatic {
    /// The entities written at sim `frame`. Calls with the same `frame` merge. On a new frame a
    /// streak survives only if its entity was written at the previous observed frame, and then
    /// grows by the frames in between, so a sparse observer still sees a write-every-frame streak.
    /// A lower `frame` (a restore) clears every streak. Reaching [`HOT_STATIC_FRAMES`] yields one
    /// `L1343`, never again for that entity.
    pub fn observe(
        &mut self,
        frame: FrameIndex,
        written: &[EntityId],
    ) -> Vec<(EntityId, Diagnostic)> {
        if self.last.is_some_and(|last| frame < last) {
            self.streak = StableMap::default();
            self.last = None;
        }
        if self.last != Some(frame) {
            self.prev = self.last;
            self.last = Some(frame);
            let prev = self.prev;
            self.streak = std::mem::take(&mut self.streak)
                .into_iter()
                .filter(|(_, (_, at))| Some(*at) == prev)
                .collect();
        }
        let mut hot = Vec::new();
        for &id in written {
            let count = match self.streak.get(&id).copied() {
                Some((_, at)) if at == frame => continue,
                Some((count, at)) if frame.0 - at.0 <= HOT_STATIC_MAX_GAP => {
                    count.saturating_add(u32::try_from(frame.0 - at.0).unwrap_or(u32::MAX))
                }
                Some(_) => 1,
                None => 1,
            };
            self.streak.insert(id, (count, frame));
            if count >= HOT_STATIC_FRAMES && self.reported.insert(id) {
                hot.push((
                    id,
                    Diagnostic::new(
                        code::STATIC_REWRITTEN,
                        format!(
                            "static entity {id} was written on {HOT_STATIC_FRAMES} consecutive \
                             sim frames, so it is re-placed every frame"
                        ),
                    )
                    .with_help("remove `Static` from it"),
                ));
            }
        }
        hot
    }

    /// Drops everything known about an entity that was evicted.
    pub fn forget(&mut self, id: EntityId) {
        self.streak.remove(&id);
        self.reported.remove(&id);
    }
}

/// The `L1341` for `subject` (a `Static` entity), unpointed.
pub(crate) fn ineligible_diagnostic(subject: &str, reason: &StaticIneligible) -> Diagnostic {
    Diagnostic::new(
        code::STATIC_INELIGIBLE,
        format!(
            "{subject} is `Static` but {}, so it takes the dynamic path",
            reason.reason()
        ),
    )
    .with_help("remove `Static`, or remove what makes it ineligible")
}

/// The `L1342` for `subject` (a `Static` entity) whose ancestor `ancestor` is not `Static`,
/// unpointed.
pub(crate) fn parent_diagnostic(subject: &str, ancestor: &str) -> Diagnostic {
    Diagnostic::new(
        code::STATIC_PARENT_NOT_STATIC,
        format!(
            "{subject} is `Static` but its ancestor {ancestor} is not, so it takes the dynamic path"
        ),
    )
    .with_help("mark every ancestor `Static`, or remove `Static` from this entity")
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
        // A read error is ignored here: the mesh validator reports it.
        let blended = meshes.iter().any(|m| {
            m.blend.is_some()
                || matches!(&m.shape, MeshSource::Asset(path)
                if matches!(
                    crate::mesh_asset::with_mesh_source(
                        root,
                        path,
                        ludion_scene::read_mesh_alpha_mode,
                    ),
                    Ok((ludion_scene::AlphaMode::Blend, _))
                ))
        });
        let ineligible = static_ineligibility(&has, &shapes, blended).or_else(|| {
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
        let posed = ineligible
            .is_none()
            .then(|| posed_ancestor(scene, index))
            .flatten();
        if let Some(reason) = ineligible {
            diagnostics.push(ineligible_diagnostic("this entity", &reason).with_path(at.clone()));
        } else if let Some((ancestor, by)) = posed {
            let named = crate::skin_bone::named(&scene.entities[ancestor], ancestor);
            let reason = StaticIneligible::Posed {
                ancestor: Some(named),
                by,
            };
            diagnostics.push(ineligible_diagnostic("this entity", &reason).with_path(at.clone()));
        }
        if let Some(ancestor) = non_static_ancestor(scene, index) {
            let named = crate::skin_bone::named(&scene.entities[ancestor], ancestor);
            diagnostics.push(parent_diagnostic("this entity", &named).with_path(at));
        }
    }
    diagnostics
}

/// The nearest `Static` `Parent` ancestor of entity `index` that a pose source poses, with the
/// source; the walk stops at the first non-`Static` ancestor (the `L1342` case). A prefab root's
/// `Animation` is runtime-only: a scene holds unexpanded `prefab:` entries.
fn posed_ancestor(scene: &Scene, index: usize) -> Option<(usize, &'static str)> {
    let mut current = index;
    for _ in 0..scene.entities.len() {
        let parent = parent_index(scene, current)?;
        let entity = scene.entities.get(parent)?;
        let has = |name: &str| entity.components.iter().any(|(n, _)| n == name);
        if !has("Static") {
            return None;
        }
        if let Some(by) = pose_source(&has, false) {
            return Some((parent, by));
        }
        current = parent;
    }
    None
}

fn parent_index(scene: &Scene, index: usize) -> Option<usize> {
    scene.entities[index]
        .components
        .iter()
        .find(|(name, _)| name == "Parent")
        .and_then(|(_, value)| value.get("index"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| usize::try_from(p).ok())
}

/// The first `Parent` ancestor of entity `index` that lacks `Static`, if any.
fn non_static_ancestor(scene: &Scene, index: usize) -> Option<usize> {
    let mut current = index;
    for _ in 0..scene.entities.len() {
        let parent = parent_index(scene, current)?;
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
    fn a_static_asset_whose_document_is_blended_is_l1341() {
        let dir = std::env::temp_dir().join(format!("ludion-static-blend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        let tri = "format: 1\nkind: mesh\npositions:\n  - 0.0\n  - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\n  \
                   - 0.0\n  - 0.0\n  - 1.0\n  - 0.0\nindices:\n  - 0\n  - 1\n  - 2\n";
        std::fs::write(dir.join("meshes/opaque.mesh.yaml"), tri).unwrap();
        std::fs::write(
            dir.join("meshes/glass.mesh.yaml"),
            format!("{tri}alpha_mode: blend\n"),
        )
        .unwrap();
        let check = |rel: &str| {
            let text = format!(
                "  - components:\n      Static: {{}}\n      Mesh:\n        shape:\n          asset: {rel}\n"
            );
            scene_static_diagnostics(&dir, &scene(&text))
                .iter()
                .map(|d| d.code.as_str().to_string())
                .collect::<Vec<_>>()
        };
        assert!(check("meshes/opaque.mesh.yaml").is_empty());
        assert_eq!(check("meshes/glass.mesh.yaml"), ["L1341"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_static_with_no_renderable_is_a_silent_transform_group() {
        assert!(codes_of("  - components:\n      Static: {}\n      Transform: {}\n").is_empty());
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

    #[test]
    fn a_static_under_a_parent_joint_is_l1341_naming_it() {
        let text = "  - components:\n      Static: {}\n      ParentJoint:\n        joint: hand\n  \
                    - components:\n      Static: {}\n      Mesh:\n        shape: cube\n      \
                    Parent:\n        index: 0\n        generation: 0\n";
        let diagnostics = scene_static_diagnostics(&std::env::temp_dir(), &scene(text));
        let codes: Vec<_> = diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, ["L1341", "L1341"], "{diagnostics:?}");
        assert!(diagnostics[1].message.contains("ParentJoint"));
    }

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 0)
    }

    fn f(n: u64) -> FrameIndex {
        FrameIndex::new(n)
    }

    #[test]
    fn hot_static_reports_once_at_thirty_frames() {
        let mut hot = HotStatic::default();
        for n in 1..u64::from(HOT_STATIC_FRAMES) {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(f(30), &[id(1)]).len(), 1);
        for n in 31..36 {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
    }

    #[test]
    fn a_gap_resets_the_streak() {
        let mut hot = HotStatic::default();
        for n in 1..u64::from(HOT_STATIC_FRAMES) {
            hot.observe(f(n), &[id(1)]);
        }
        assert!(hot.observe(f(30), &[]).is_empty());
        for n in 31..60 {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(f(60), &[id(1)]).len(), 1);
    }

    #[test]
    fn repeat_calls_in_one_frame_merge() {
        let mut hot = HotStatic::default();
        for n in 1..=29 {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
            assert!(hot.observe(f(n), &[]).is_empty());
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(f(30), &[id(1)]).len(), 1);
    }

    #[test]
    fn sparse_observation_of_a_write_every_frame_still_reaches_thirty() {
        let mut hot = HotStatic::default();
        let reported: usize = (0..=30)
            .step_by(2)
            .map(|n| hot.observe(f(n), &[id(1)]).len())
            .sum();
        assert_eq!(reported, 1);
    }

    #[test]
    fn a_long_observation_gap_restarts_the_streak() {
        let mut hot = HotStatic::default();
        assert!(hot.observe(f(0), &[id(1)]).is_empty());
        assert!(hot.observe(f(100), &[id(1)]).is_empty());
        for n in 101..129 {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(f(129), &[id(1)]).len(), 1);
    }

    #[test]
    fn writes_on_alternate_frames_never_report() {
        let mut hot = HotStatic::default();
        for n in 0..200 {
            let written = if n % 2 == 0 { vec![id(1)] } else { vec![] };
            assert!(hot.observe(f(n), &written).is_empty());
        }
    }

    #[test]
    fn a_backwards_frame_clears_the_streaks() {
        let mut hot = HotStatic::default();
        for n in 100..129 {
            hot.observe(f(n), &[id(1)]);
        }
        assert!(hot.observe(f(3), &[id(1)]).is_empty());
        for n in 4..32 {
            assert!(hot.observe(f(n), &[id(1)]).is_empty());
        }
        assert_eq!(hot.observe(f(32), &[id(1)]).len(), 1);
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
