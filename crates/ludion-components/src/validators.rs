//! Scene validators over the registered components, needing no renderer or assets.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::Scene;

/// Every `L1221`/`L1222` a checked scene's parented colliders report: a rigid body or a static
/// solver collider on an entity with a `Parent` gets no physics body at runtime. A kinematic body
/// and a collider the solver does not claim (an in-plane static or kinematic `circle`/`aabb`) are
/// accepted. The claim rule is [`ludion_physics::claims`], which is why this lives here and not in
/// `ludion-scene`.
pub fn scene_hierarchy_body_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let find = |wanted: &str| {
            entity
                .components
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value)
        };
        let Some(parent) = find("Parent")
            .and_then(|value| value.get("index"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|parent| usize::try_from(parent).ok())
        else {
            continue;
        };
        let Some(Ok(collider)) = find("Collider")
            .map(|value| serde_json::from_value::<ludion_spatial::Collider>(value.clone()))
        else {
            continue;
        };
        if !ludion_physics::claims(&collider) {
            continue;
        }
        let (diagnostic, kind, help) = match collider.body {
            ludion_spatial::Body::Rigid(_) => (
                code::HIERARCHY_RIGID_BODY,
                "a rigid body",
                "un-parent it, or join it to the parent with a `Joint`",
            ),
            ludion_spatial::Body::Static(_) => (
                code::HIERARCHY_STATIC_COLLIDER,
                "a static solver collider",
                "make it `body: kinematic`, or un-parent it",
            ),
            ludion_spatial::Body::Kinematic(_) => continue,
        };
        let parent_label = match scene.entities.get(parent).and_then(|p| p.name.as_ref()) {
            Some(name) => format!("entity {parent} (`{name}`)"),
            None => format!("entity {parent}"),
        };
        diagnostics.push(
            Diagnostic::new(
                diagnostic,
                format!("{kind} sits under {parent_label} and would get no physics body"),
            )
            .with_path(format!("/entities/{index}/components/Collider/body"))
            .with_help(help),
        );
    }
    diagnostics
}

/// The components a scene has one of: a prefab is spawned many times, so a part carrying one is
/// refused (`L1229`).
#[doc(hidden)]
pub const PREFAB_SINGLETONS: [&str; 5] = [
    "Camera",
    "DirectionalLight",
    "Environment",
    "PostProcess",
    "GiVolume",
];

/// Every `L1229` a prefab body reports: one per part component that is a scene-wide singleton
/// (`Camera`, `DirectionalLight`, `Environment`, `PostProcess`, `GiVolume`). Takes the body as a
/// [`Scene`] (`PrefabDocument::to_scene`) like its siblings; it names concrete components, which is
/// why it lives here and not in `ludion-scene`.
pub fn prefab_singleton_diagnostics(scene: &Scene) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, _) in &entity.components {
            if !PREFAB_SINGLETONS.contains(&name.as_str()) {
                continue;
            }
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_SINGLETON,
                    format!("a prefab part carries `{name}`, of which a scene has one"),
                )
                .with_path(format!("/entities/{index}/components/{name}"))
                .with_help("a prefab is spawned many times; put it in the scene instead"),
            );
        }
    }
    diagnostics
}
