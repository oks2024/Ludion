//! [`switch_skin_to_ragdoll`]: turns a live skinned-mesh entity into its `SkinBone` ragdoll by
//! spawning a parts prefab bound to that skin.
//!
//! Parts land at the bone **rest** pose, exactly where a scene load or [`spawn_prefab`](crate::spawn_prefab)
//! puts a `SkinBone` part; no clip is sampled. Each rigid part inherits the skin's velocity. The skin
//! keeps its id and its `Mesh` (the parts' bones drive it), and loses `Animation`, `Collider` and
//! `Velocity`.
//!
//! # The parts prefab
//! An ordinary `kind: prefab` whose **last** part is named `$skin`. That part carries the same skinned
//! `Mesh` as the live entity, and every part's `SkinBone.skin` points at its position, so the file
//! validates (bone names included) and previews with a plain `spawn_prefab`. At the switch `$skin`
//! resolves to the live entity and nothing spawns for it. Nothing else may reference `$skin`, and its
//! mesh document must be the live skin's own.
//!
//! All-or-nothing: every check runs before anything spawns or mutates, so a refusal leaves the world
//! exactly as it was.

use std::sync::Arc;

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::Vec3;
use ludion_ecs::World;
use ludion_spatial::{Body, Collider, Velocity};

use crate::components::{Animation, Mesh, Name, SkinBone};
use crate::prefab::{PrefabLibrary, prefab_key, spawn_expanded};
use crate::ragdoll::switch_invalid;
use crate::skin_bone::{apply_bone_poses, mesh_document_path};

/// The reserved name of the parts prefab's last part, standing for the live skin.
const SKIN_PART: &str = "$skin";

/// Switches the skinned mesh `skin` into its `SkinBone` ragdoll, spawning the parts prefab `parts`
/// (relative to the content root) and returning the spawned part ids in prefab order, `$skin`
/// excluded.
///
/// See the module docs for the parts-prefab contract. Parts are named `<skin name>/<part>` when the
/// skin has a [`Name`]. Each rigid part gets `linear = v + ω × (part − skin)` and `angular = ω` from
/// the skin's [`Velocity`] (zero without one), where `part` is the part's spawn translation (its bone
/// origin) and `skin` the skin's world translation. Works from a sim system and from host code.
///
/// # Errors
/// `L3036` when `skin` is dead, has no `Transform`, has a scale that is not uniform, positive and
/// finite, is not a skinned `asset`/`character` mesh, or `parts` breaks the `$skin` rules (not last, a
/// part bound to another skin, another reference to it, a different mesh). `L1230` when `parts` cannot
/// be read, and the prefab's own diagnostics (`L1194`–`L1197`, component errors) when it is invalid.
/// Nothing is mutated on any error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, switch_skin_to_ragdoll};
/// use ludion_scene::{JointRest, MeshDocument, MeshJoint, MeshSkin};
///
/// let root = std::env::temp_dir().join(format!("ludion-skin-ragdoll-doc-{}", std::process::id()));
/// std::fs::create_dir_all(root.join("meshes")).unwrap();
/// let joint = |name: &str, parent: Option<u32>| MeshJoint {
///     name: name.to_string(),
///     parent,
///     rest: JointRest { translation: [0.0, 1.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
///     inverse_bind: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
/// };
/// let doc = MeshDocument {
///     positions: vec![0.0; 3],
///     skin: Some(MeshSkin {
///         joints: vec![joint("root", None), joint("arm", Some(0))],
///         weights: vec![1.0, 0.0, 0.0, 0.0],
///         bone_indices: vec![0, 0, 0, 0],
///     }),
///     ..MeshDocument::default()
/// };
/// std::fs::write(root.join("meshes/rig.mesh.yaml"), ludion_scene::emit_mesh(&doc)).unwrap();
/// let bone = |bone: &str| format!(
///     "  - name: {bone}\n    components:\n      SkinBone:\n        skin:\n          index: 2\n          generation: 0\n        bone: {bone}\n"
/// );
/// let mesh = "      Mesh:\n        shape:\n          asset: \"meshes/rig.mesh.yaml\"\n";
/// std::fs::write(
///     root.join("parts.prefab.yaml"),
///     format!("format: 1\nkind: prefab\nname: parts\nentities:\n{}{}  - name: $skin\n    components:\n{mesh}", bone("root"), bone("arm")),
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// engine
///     .load_inline_scene(&format!("format: 1\nentities:\n  - name: enemy\n    components:\n      Transform: {{}}\n{mesh}"))
///     .unwrap();
/// let enemy = engine.world().entities().next().unwrap();
/// let parts = switch_skin_to_ragdoll(engine.world_mut(), enemy, "parts.prefab.yaml").unwrap();
/// assert_eq!(parts.len(), 2);
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn switch_skin_to_ragdoll(
    world: &mut World,
    skin: EntityId,
    parts: &str,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let refuse =
        |named: &str, message: String, help: &str| vec![switch_invalid(named, message, help)];
    if !world.is_alive(skin) {
        return Err(refuse(
            &format!("entity `{skin}`"),
            "is not alive".into(),
            "switch an entity that still exists",
        ));
    }
    let named = crate::scene_physics::entity_label(world, skin);
    if ludion_ecs::is_presentation(skin) {
        return Err(refuse(
            &named,
            "is a presentation entity; the switch spawns sim parts, which would cross the \
             sim/presentation boundary (L0028)"
                .into(),
            "switch a sim skin; a presentation skin has no ragdoll",
        ));
    }
    let Some(skin_transform) = crate::hierarchy::world_pose_transform(world, skin) else {
        return Err(refuse(
            &named,
            "has no `Transform`".into(),
            "give the entity a `Transform` before switching it",
        ));
    };
    let scale = skin_transform.scale;
    if scale[0] != scale[1] || scale[1] != scale[2] {
        return Err(refuse(
            &named,
            format!(
                "its `Transform.scale` {scale:?} is not uniform; the switch has no single factor \
                 to size each part by"
            ),
            "give the entity a uniform scale (equal x, y and z) before switching it",
        ));
    }
    if !(scale[0] > 0.0 && scale[0].is_finite()) {
        return Err(refuse(
            &named,
            format!("its `Transform.scale` {scale:?} is not strictly positive and finite"),
            "give the entity a strictly positive, finite scale before switching it",
        ));
    }

    let key = prefab_key(parts)?;
    let Some(library) = world.resource_mut::<PrefabLibrary>() else {
        return Err(vec![
            Diagnostic::new(
                code::PREFAB_UNRESOLVED,
                format!("cannot switch with parts `{parts}`: this world has no prefab library"),
            )
            .with_help("switch from a system of an engine-built game"),
        ]);
    };
    let root = library.root().to_path_buf();
    let registry = Arc::clone(library.registry());
    let ready = library.ready(&key)?;
    let content = AssetRoot::content(&root);

    let live_mesh = world.get::<Mesh>(skin).ok().flatten().cloned();
    let live_path = match live_mesh
        .as_ref()
        .and_then(|m| mesh_document_path(content, m))
    {
        Some(Ok(path)) => path,
        Some(Err((_, errors))) => return Err(errors),
        None => {
            return Err(refuse(
                &named,
                "is not a skinned mesh: it has no `Mesh`, or its `shape` is not `asset` or \
                 `character`"
                    .into(),
                "switch an entity whose `Mesh` is a skinned `asset` or `character` mesh",
            ));
        }
    };

    let entities = &ready.expanded.scene.entities;
    let part_error = |message: String, help: &str| {
        Err(refuse(&named, format!("parts `{parts}` {message}"), help))
    };
    let Some(last) = entities.len().checked_sub(1) else {
        return part_error(
            format!("hold no `{SKIN_PART}` part"),
            "end the parts prefab with a part named `$skin` carrying the skin's `Mesh`",
        );
    };
    if entities[last].name.as_deref() != Some(SKIN_PART) {
        let help = if entities
            .iter()
            .any(|e| e.name.as_deref() == Some(SKIN_PART))
        {
            "move the `$skin` part last"
        } else {
            "end the parts prefab with a part named `$skin` carrying the skin's `Mesh`"
        };
        return part_error(format!("do not end with a `{SKIN_PART}` part"), help);
    }
    for (position, entity) in entities.iter().enumerate() {
        for (name, value) in &entity.components {
            if name == "SkinBone" {
                let target = value.pointer("/skin/index").and_then(|v| v.as_u64());
                if target != Some(last as u64) {
                    return part_error(
                        format!(
                            "bind part {position} to a skin other than `{SKIN_PART}` (entity {last})"
                        ),
                        "point every `SkinBone.skin` at the `$skin` part",
                    );
                }
                continue;
            }
            let Some(info) = registry.get(name) else {
                continue;
            };
            for field in info.entity_refs() {
                if field
                    .target(value)
                    .is_some_and(|id| id.index() as usize == last)
                {
                    return part_error(
                        format!(
                            "reference `{SKIN_PART}` from `{name}{}` on part {position}",
                            field.suffix
                        ),
                        "only `SkinBone.skin` may name the `$skin` part",
                    );
                }
            }
        }
    }
    let skin_part_mesh = entities[last]
        .components
        .iter()
        .find(|(name, _)| name == "Mesh")
        .and_then(|(_, value)| serde_json::from_value::<Mesh>(value.clone()).ok())
        .and_then(|mesh| mesh_document_path(content, &mesh));
    match skin_part_mesh {
        Some(Ok(path)) if path == live_path => {}
        Some(Ok(path)) => {
            return Err(vec![switch_invalid(
                &named,
                format!(
                    "parts `{parts}` were authored for mesh `{path}`, but this entity draws \
                         `{live_path}`"
                ),
                "use the parts prefab authored for this skin's mesh",
            )]);
        }
        Some(Err((_, errors))) => return Err(errors),
        None => {
            return part_error(
                format!("carry no skinned `Mesh` on the `{SKIN_PART}` part"),
                "give the `$skin` part the same skinned `Mesh` as the entity",
            );
        }
    }

    if !entities[..last]
        .iter()
        .any(|e| e.components.iter().any(|(name, _)| name == "SkinBone"))
    {
        return part_error(
            "hold no `SkinBone` part besides `$skin`".into(),
            "add at least one part with a `SkinBone` for the skin's bones to follow",
        );
    }
    if world
        .query_ids::<(&SkinBone,)>()
        .any(|(_, (link,))| link.skin == skin)
    {
        return Err(refuse(
            &named,
            "is already driven by SkinBone parts".into(),
            "switch once, on the transition tick, comparing the frame with `==` rather than `>=`",
        ));
    }

    // --- Validated: every step below mutates the world. ---

    let mut trimmed = ready.expanded.clone();
    let skin_entity = trimmed
        .scene
        .entities
        .pop()
        .expect("checked non-empty above");
    drop(skin_entity);
    let popped = trimmed.entry_of.pop();
    debug_assert!(
        popped.is_some()
            && trimmed
                .instances
                .iter()
                .all(|instance| instance.parts.end <= last),
        "`$skin` is a top-level part, so no instance range holds it"
    );
    let mut bones = Vec::with_capacity(trimmed.scene.entities.len());
    for entity in &mut trimmed.scene.entities {
        let mut bone = None;
        entity.components.retain(|(name, value)| {
            if name != "SkinBone" {
                return true;
            }
            bone = value
                .get("bone")
                .and_then(|b| b.as_str())
                .map(str::to_owned);
            false
        });
        bones.push(bone);
    }

    let prefix = world.get::<Name>(skin).ok().flatten().cloned();
    let ids = spawn_expanded(
        world,
        &key,
        &trimmed,
        Some(skin_transform),
        prefix.as_ref().map(|n| n.0.as_str()),
    )?;
    let mut posed = ids.clone();
    posed.push(skin);
    apply_bone_poses(world, &posed, &ready.poses);
    for (&part, bone) in ids.iter().zip(bones) {
        if let Some(bone) = bone {
            world
                .insert(part, SkinBone { skin, bone })
                .expect("the part was just spawned");
        }
    }

    let velocity = world
        .get::<Velocity>(skin)
        .ok()
        .flatten()
        .copied()
        .unwrap_or_default();
    let skin_at = Vec3::from(skin_transform.translation);
    for &part in &ids {
        let rigid = world
            .get::<Collider>(part)
            .ok()
            .flatten()
            .is_some_and(|c| matches!(c.body, Body::Rigid(_)));
        let Some(at) = crate::hierarchy::world_pose_transform(world, part) else {
            continue;
        };
        if !rigid {
            continue;
        }
        let linear = Vec3::from(velocity.linear)
            + Vec3::from(velocity.angular).cross(Vec3::from(at.translation) - skin_at);
        world
            .insert(
                part,
                Velocity {
                    linear: linear.into(),
                    angular: velocity.angular,
                },
            )
            .expect("the part was just spawned");
    }

    world
        .remove::<Animation>(skin)
        .expect("checked alive above");
    world.remove::<Collider>(skin).expect("checked alive above");
    world.remove::<Velocity>(skin).expect("checked alive above");
    crate::motor::suspend_motor_of(world, skin);
    Ok(ids)
}
