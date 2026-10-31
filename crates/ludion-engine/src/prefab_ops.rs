//! Operations over live prefab instances: create a prefab from live entities, revert an instance
//! to its prefab's values, and unpack it into plain entities.
//!
//! An instance is every live entity whose [`PrefabInstance`] names the same `root`; any one of its
//! parts identifies it ([`instance_parts`]). Each operation computes everything before it changes
//! anything, so a refusal leaves the world as it was.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{Registry, World};
use ludion_scene::{InstantiateOptions, PrefabDocument, SceneEntity};
use serde_json::Value;

use crate::collapse::{PROVENANCE, instance_entry};
use crate::components::Name;
use crate::engine::{ref_target, remap_entity_refs};
use crate::prefab::{
    PrefabHooks, PrefabInstance, PrefabLibrary, Ready, prefab_key, read_prefab_text, root_parts,
};
use crate::skin_bone::apply_bone_poses;

const TRANSFORM: &str = "Transform";
const PARENT: &str = "Parent";

/// What [`revert_prefab`] changed.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabReverted {
    /// The parts with at least one component written or removed, in live order.
    pub entities: Vec<EntityId>,
    /// The prefab's part paths with no live part: removed from the instance, and not re-spawned.
    pub not_restored: Vec<String>,
}

/// A part's id and its (composed) path within the instance.
pub(crate) type Part = (EntityId, String);

fn one(diagnostic: Diagnostic) -> Vec<Diagnostic> {
    vec![diagnostic]
}

fn no_library(what: &str) -> Diagnostic {
    Diagnostic::new(
        code::PREFAB_UNRESOLVED,
        format!("cannot {what}: this world has no prefab library"),
    )
    .with_help("use prefabs from an engine-built game")
}

/// The instance `any` is a part of: its prefab key, its root, and every live part in live order
/// (sim entities by slot, then presentation ones), each with its part path.
///
/// # Errors
/// `L1235` when `any` is dead or carries no [`PrefabInstance`].
pub(crate) fn instance_parts(
    world: &World,
    any: EntityId,
) -> Result<(String, EntityId, Vec<Part>), Diagnostic> {
    let not_instance = |why: &str| {
        Diagnostic::new(code::PREFAB_NOT_INSTANCE, format!("entity {any} {why}"))
            .with_help("pass a part of a spawned or scene-loaded prefab instance")
    };
    let instance = match world.get::<PrefabInstance>(any) {
        Ok(Some(instance)) => instance.clone(),
        Ok(None) => return Err(not_instance("is not part of a prefab instance")),
        Err(_) => return Err(not_instance("is not alive")),
    };
    let parts = world
        .entities()
        .chain(world.presentation_entities())
        .filter_map(|id| match world.get::<PrefabInstance>(id) {
            Ok(Some(part)) if part.root == instance.root && part.source == instance.source => {
                Some((id, part.part.clone()))
            }
            _ => None,
        })
        .collect();
    Ok((instance.source, instance.root, parts))
}

/// The prefab `key` from the world's library, and the registry it validates against.
fn library_ready(
    world: &mut World,
    key: &str,
) -> Result<(Arc<Ready>, Arc<Registry>), Vec<Diagnostic>> {
    let Some(library) = world.resource_mut::<PrefabLibrary>() else {
        return Err(one(no_library("read the prefab")));
    };
    let ready = library.ready(key)?;
    Ok((ready, Arc::clone(library.registry())))
}

fn label(world: &World, id: EntityId) -> String {
    match world.get::<Name>(id) {
        Ok(Some(name)) => format!("entity {id} (`{}`)", name.0),
        _ => format!("entity {id}"),
    }
}

/// Rewrites each asset path of `value` from its content-root form to one relative to the prefab
/// `doc`; the first path that cannot be inverted is returned.
fn relativize_assets(
    info: &ludion_ecs::ComponentInfo,
    value: &mut Value,
    doc: &str,
) -> Result<(), String> {
    let mut failed = None;
    info.for_each_asset_path(value, &mut |_, path| {
        if path.is_empty() {
            return;
        }
        match ludion_core::paths::relative_to(doc, path) {
            Some(relative) => *path = relative,
            None => {
                failed.get_or_insert_with(|| path.clone());
            }
        }
    });
    failed.map_or(Ok(()), Err)
}

fn create_refusal(message: impl Into<String>, help: impl Into<String>) -> Vec<Diagnostic> {
    one(Diagnostic::new(code::PREFAB_CREATE, message).with_help(help))
}

/// Writes `entities` as a new `kind: prefab` at `path` under the world's content root and returns
/// the normalized key. See [`Engine::create_prefab`](crate::Engine::create_prefab).
pub(crate) fn create(
    world: &World,
    registry: &Registry,
    entities: &[EntityId],
    path: &str,
) -> Result<String, Vec<Diagnostic>> {
    let key = prefab_key(path)?;
    if !key.ends_with(".prefab.yaml") {
        return Err(create_refusal(
            format!("cannot create prefab `{path}`: the file name does not end in `.prefab.yaml`"),
            format!(
                "name it like `{}.prefab.yaml`",
                key.trim_end_matches(".yaml")
            ),
        ));
    }
    let Some(library) = world.resource::<PrefabLibrary>() else {
        return Err(one(no_library("create the prefab")));
    };
    if entities.is_empty() {
        return Err(create_refusal(
            "cannot create a prefab from no entities",
            "pass the entities that become the prefab's parts, the first being its root",
        ));
    }
    let mut position = BTreeMap::new();
    for (pos, &id) in entities.iter().enumerate() {
        if !world.is_alive(id) {
            return Err(create_refusal(
                format!("cannot create a prefab: entity {id} is not alive"),
                "pass live entities",
            ));
        }
        if position.insert(id, pos).is_some() {
            return Err(create_refusal(
                format!("cannot create a prefab: entity {id} is listed twice"),
                "list each entity once",
            ));
        }
    }

    let mut names = BTreeSet::new();
    let mut parts = Vec::with_capacity(entities.len());
    for &id in entities {
        let name = match world.get::<Name>(id) {
            Ok(Some(name)) if !name.0.is_empty() => name.0.clone(),
            _ => {
                return Err(create_refusal(
                    format!("cannot create a prefab: entity {id} has no `Name`"),
                    "give every entity a unique `Name` — it becomes the part's name",
                ));
            }
        };
        if name.contains('/') {
            return Err(create_refusal(
                format!(
                    "cannot create a prefab: {} has a `/` in its name",
                    label(world, id)
                ),
                "part names cannot contain `/`; rename the entity",
            ));
        }
        if !names.insert(name.clone()) {
            return Err(create_refusal(
                format!("cannot create a prefab: two entities are named `{name}`"),
                "give every entity a unique `Name`",
            ));
        }
        let mut components = Vec::new();
        for info in registry.iter() {
            if info.reserved() || info.name() == PROVENANCE {
                continue;
            }
            let Ok(Some(value)) = info.serialize(world, id) else {
                continue;
            };
            for field in info.entity_refs() {
                if let Some(target) = ref_target(&value, field)
                    && !position.contains_key(&target)
                {
                    return Err(create_refusal(
                        format!(
                            "cannot create a prefab: {} refers to entity {target} through `{}`, which is not in `entities`",
                            label(world, id),
                            info.name()
                        ),
                        "add it to `entities` or remove the reference",
                    ));
                }
            }
            let mut value = remap_entity_refs(world, id, info, value, &position).map_err(one)?;
            if let Err(path) = relativize_assets(info, &mut value, &key) {
                return Err(create_refusal(
                    format!(
                        "cannot create a prefab: {} has `{path}` in `{}`, which cannot be written relative to `{key}`",
                        label(world, id),
                        info.name()
                    ),
                    "the live value is an absolute path; use a path relative to the content root",
                ));
            }
            components.push((info.name().to_string(), value));
        }
        parts.push(SceneEntity {
            name: Some(name),
            presentation: ludion_ecs::is_presentation(id),
            components,
            instance: None,
        });
    }

    let text = ludion_scene::emit_prefab(
        &PrefabDocument {
            name: None,
            generated_from: None,
            clips: Vec::new(),
            entities: parts,
        },
        registry,
    );
    library.validate(&key, &text)?;
    if let Ok(held) = std::fs::read_to_string(library.root().join(&key))
        && let Ok(PrefabDocument {
            generated_from: Some(source),
            ..
        }) = ludion_scene::parse_prefab(&held)
    {
        return Err(create_refusal(
            format!("`{key}` is generated from `{source}`; delete it first to replace it"),
            "a generated prefab is write-only: the next import would overwrite the new one",
        ));
    }
    ludion_core::atomic::write_atomic(&library.root().join(&key), text).map_err(|error| {
        create_refusal(
            format!("cannot create prefab `{key}`: {error}"),
            "check the content root is writable",
        )
    })?;
    Ok(key)
}

/// Removes [`PrefabInstance`] from every part of the instance `instance` is part of, leaving plain
/// entities that keep their names, and returns the parts in live order.
///
/// # Errors
/// `L1235` when `instance` is not part of a prefab instance. Nothing changes on error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, PrefabInstance, spawn_prefab, unpack_prefab};
///
/// let root = std::env::temp_dir().join(format!("ludion-unpack-prefab-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("crate.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: crate\nentities:\n  - name: lid\n    components: {}\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let ids = spawn_prefab(engine.world_mut(), "crate.prefab.yaml", None, Some("crate-1")).unwrap();
/// let parts = unpack_prefab(engine.world_mut(), ids[0]).unwrap();
/// assert_eq!(parts, ids);
/// assert!(engine.world_mut().get::<PrefabInstance>(ids[0]).unwrap().is_none());
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn unpack_prefab(
    world: &mut World,
    instance: EntityId,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let (_, _, parts) = instance_parts(world, instance).map_err(one)?;
    let ids: Vec<EntityId> = parts.into_iter().map(|(id, _)| id).collect();
    for &id in &ids {
        world
            .remove::<PrefabInstance>(id)
            .expect("a part of the instance is alive");
    }
    Ok(ids)
}

/// One component of a prefab part as a fresh instantiation gives it: `None` when it holds a
/// reference to a part the live instance no longer has, which a revert leaves as it is.
type Wanted = BTreeMap<String, Option<Value>>;

/// A fresh, unplaced instantiation of the prefab, read back as its parts' components by part path.
struct Expected {
    /// The prefab's part paths in part order.
    order: Vec<String>,
    parts: BTreeMap<String, Wanted>,
    /// Parts with no `Parent`: their `Transform` and `Parent` are the instance's placement.
    roots: BTreeSet<String>,
}

fn expected_parts(
    ready: &Ready,
    registry: &Arc<Registry>,
    source: &str,
    live: &BTreeMap<&str, EntityId>,
) -> Result<Expected, Vec<Diagnostic>> {
    let mut scratch = World::new();
    let ids = ludion_scene::instantiate_expanded(
        &mut scratch,
        &ready.expanded,
        registry,
        &InstantiateOptions {
            placement: None,
            name_prefix: None,
            hooks: &PrefabHooks { source },
        },
    )?;
    apply_bone_poses(&mut scratch, &ids, &ready.poses);

    let mut path_of = BTreeMap::new();
    for &id in &ids {
        if let Ok(Some(instance)) = scratch.get::<PrefabInstance>(id) {
            path_of.insert(id, instance.part.clone());
        }
    }
    let mut expected = Expected {
        order: Vec::new(),
        parts: BTreeMap::new(),
        roots: root_parts(&ready.expanded).into_iter().collect(),
    };
    for &id in &ids {
        let Some(path) = path_of.get(&id) else {
            continue;
        };
        let mut wanted = Wanted::new();
        for info in registry.iter() {
            if info.reserved() || info.name() == PROVENANCE {
                continue;
            }
            let Ok(Some(mut value)) = info.serialize(&scratch, id) else {
                continue;
            };
            let mut restorable = true;
            for field in info.entity_refs() {
                let Some(target) = ref_target(&value, field) else {
                    continue;
                };
                let live_target = path_of
                    .get(&target)
                    .and_then(|part| live.get(part.as_str()));
                let Some(&live_target) = live_target else {
                    restorable = false;
                    break;
                };
                let id_value = serde_json::to_value(live_target).expect("an EntityId serializes");
                if field.suffix.is_empty() {
                    value = id_value;
                } else if let Some(slot) = value.pointer_mut(&field.suffix) {
                    *slot = id_value;
                }
            }
            wanted.insert(info.name().to_owned(), restorable.then_some(value));
        }
        expected.order.push(path.clone());
        expected.parts.insert(path.clone(), wanted);
    }
    Ok(expected)
}

/// Which part, component and field of an instance a revert covers.
#[derive(Default)]
struct Scope {
    part: Option<String>,
    component: Option<String>,
    /// A JSON pointer inside the component, with its leading `/`.
    field: Option<String>,
}

fn revert_refusal(message: impl Into<String>, help: impl Into<String>) -> Vec<Diagnostic> {
    one(Diagnostic::new(code::PREFAB_REVERT, message).with_help(help))
}

const POINTER_HELP: &str =
    "use `/<part path>[/<Component>[/<field>...]]`, e.g. `/arm/PointLight/intensity`";

/// Parses `pointer` against the prefab's part paths: the longest run of leading segments naming a
/// part is the part (paths contain `/`), the next segment the component, the rest a field pointer.
fn parse_scope(pointer: &str, order: &[String]) -> Result<Scope, Vec<Diagnostic>> {
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(revert_refusal(
            format!("`{pointer}` is not a pointer: it must start with `/`"),
            POINTER_HELP,
        ));
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let Some(count) = (1..=segments.len()).rev().find(|&count| {
        order
            .iter()
            .any(|path| *path == segments[..count].join("/"))
    }) else {
        return Err(revert_refusal(
            format!(
                "`{pointer}` names no part of the prefab (its parts: {})",
                order.join(", ")
            ),
            POINTER_HELP,
        ));
    };
    let mut scope = Scope {
        part: Some(segments[..count].join("/")),
        ..Scope::default()
    };
    if let Some(component) = segments.get(count) {
        scope.component = Some((*component).to_owned());
        if segments.len() > count + 1 {
            scope.field = Some(format!("/{}", segments[count + 1..].join("/")));
        }
    }
    Ok(scope)
}

enum Write {
    Set(EntityId, String, Value),
    Remove(EntityId, String),
}

/// Re-applies the prefab's values to the live parts of the instance `instance` is part of,
/// dropping overrides: every component the prefab gives a part is written back, and every component
/// a part gained is removed. Returns the parts that changed and the prefab parts the instance no
/// longer has — removed parts are not re-spawned.
///
/// A prefab root (a part with no `Parent`) keeps its `Transform` and `Parent`: they are the
/// instance's placement, not the prefab's values. A component whose prefab value refers to a part
/// the instance no longer has is left as it is.
///
/// `pointer` narrows the revert to `/<part path>`, `/<part path>/<Component>` or
/// `/<part path>/<Component>/<field>...` (a JSON pointer inside the component); part paths of
/// nested instances contain `/`.
///
/// # Errors
/// `L1235` when `instance` is not part of a prefab instance; `L1238` for a malformed pointer or
/// one naming no part, component or field, or a part the instance lost; the prefab's own
/// diagnostics when it cannot be read or fails validation. Nothing changes on error.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, Transform, revert_prefab, spawn_prefab};
///
/// let root = std::env::temp_dir().join(format!("ludion-revert-prefab-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// std::fs::write(
///     root.join("lamp.prefab.yaml"),
///     "format: 1\nkind: prefab\nname: lamp\nentities:\n  - name: post\n    components: {}\n  - name: bulb\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      Transform:\n        translation:\n        - 0.0\n        - 2.0\n        - 0.0\n",
/// )
/// .unwrap();
///
/// let mut engine = Engine::builder().project_root(&root).build().unwrap();
/// let ids = spawn_prefab(engine.world_mut(), "lamp.prefab.yaml", None, Some("lamp-1")).unwrap();
/// let moved = Transform { translation: [0.0, 5.0, 0.0], rotation: [0.0; 3], scale: [1.0; 3] };
/// engine.world_mut().insert(ids[1], moved).unwrap();
///
/// let reverted = revert_prefab(engine.world_mut(), ids[0], None).unwrap();
/// assert_eq!(reverted.entities, vec![ids[1]]);
/// let bulb = engine.world_mut().get::<Transform>(ids[1]).unwrap().copied().unwrap();
/// assert_eq!(bulb.translation, [0.0, 2.0, 0.0]);
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn revert_prefab(
    world: &mut World,
    instance: EntityId,
    pointer: Option<&str>,
) -> Result<PrefabReverted, Vec<Diagnostic>> {
    let (source, _, parts) = instance_parts(world, instance).map_err(one)?;
    let (ready, registry) = library_ready(world, &source)?;
    let live: BTreeMap<&str, EntityId> = parts
        .iter()
        .map(|(id, path)| (path.as_str(), *id))
        .collect();
    let expected = expected_parts(&ready, &registry, &source, &live)?;

    let scope = match pointer.filter(|pointer| !pointer.is_empty()) {
        Some(pointer) => parse_scope(pointer, &expected.order)?,
        None => Scope::default(),
    };
    if let Some(part) = &scope.part
        && !live.contains_key(part.as_str())
    {
        return Err(revert_refusal(
            format!(
                "part `{part}` is no longer in the instance, and removed parts are not re-spawned in v1"
            ),
            "revert the parts the instance still has",
        ));
    }
    if let Some(component) = &scope.component
        && registry.get(component).is_none()
    {
        return Err(revert_refusal(
            format!("`{component}` is not a registered component"),
            POINTER_HELP,
        ));
    }

    let mut writes = Vec::new();
    let mut changed = Vec::new();
    for (id, path) in &parts {
        if scope.part.as_ref().is_some_and(|part| part != path) {
            continue;
        }
        let Some(wanted) = expected.parts.get(path) else {
            continue;
        };
        let is_root = expected.roots.contains(path);
        let before = writes.len();
        for info in registry.iter() {
            let name = info.name();
            if info.reserved()
                || name == PROVENANCE
                || scope.component.as_deref().is_some_and(|c| c != name)
                || (is_root && (name == TRANSFORM || name == PARENT))
            {
                continue;
            }
            let held = info.serialize(world, *id).ok().flatten();
            match (wanted.get(name), held) {
                (Some(None), _) => {}
                (Some(Some(want)), held) => match (&scope.field, held) {
                    (Some(field), Some(mut held)) => {
                        let Some(sub) = want.pointer(field) else {
                            return Err(revert_refusal(
                                format!(
                                    "`{path}` has no field `{field}` in `{name}` in the prefab"
                                ),
                                POINTER_HELP,
                            ));
                        };
                        let Some(slot) = held.pointer_mut(field) else {
                            return Err(revert_refusal(
                                format!("`{name}` on `{path}` has no field `{field}`"),
                                POINTER_HELP,
                            ));
                        };
                        if slot != sub {
                            *slot = sub.clone();
                            writes.push(Write::Set(*id, name.to_owned(), held));
                        }
                    }
                    (_, held) => {
                        if held.as_ref() != Some(want) {
                            writes.push(Write::Set(*id, name.to_owned(), want.clone()));
                        }
                    }
                },
                (None, held) => {
                    if scope.field.is_some() {
                        return Err(revert_refusal(
                            format!("the prefab's `{path}` has no `{name}` component"),
                            POINTER_HELP,
                        ));
                    }
                    if held.is_some() {
                        writes.push(Write::Remove(*id, name.to_owned()));
                    }
                }
            }
        }
        if writes.len() > before {
            changed.push(*id);
        }
    }

    // Validate every set against a throwaway world first, so a value the component refuses fails the
    // revert before any live write rather than leaving the instance half-reverted.
    let mut probe = ludion_ecs::World::new();
    let probe_id = probe.spawn();
    for write in &writes {
        if let Write::Set(_, name, value) = write {
            let info = registry.get(name).expect("a registered component");
            info.deserialize_insert(value, &mut probe, probe_id)
                .map_err(one)?;
        }
    }
    for write in writes {
        match write {
            Write::Set(id, name, value) => {
                let info = registry.get(&name).expect("a registered component");
                info.deserialize_insert(&value, world, id).map_err(one)?;
            }
            Write::Remove(id, name) => {
                let info = registry.get(&name).expect("a registered component");
                info.remove(world, id).map_err(one)?;
            }
        }
    }
    let not_restored = if scope.part.is_some() {
        Vec::new()
    } else {
        expected
            .order
            .iter()
            .filter(|path| !live.contains_key(path.as_str()))
            .cloned()
            .collect()
    };
    Ok(PrefabReverted {
        entities: changed,
        not_restored,
    })
}

/// What [`Engine::apply_prefab`](crate::Engine::apply_prefab) wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefabApplied {
    /// The prefab file written, relative to the content root.
    pub path: String,
    /// The part paths whose component overrides were written into the prefab.
    pub parts: Vec<String>,
    /// The part paths deleted from the prefab.
    pub removed: Vec<String>,
}

fn apply_refusal(message: impl Into<String>, help: impl Into<String>) -> Vec<Diagnostic> {
    one(Diagnostic::new(code::PREFAB_APPLY, message).with_help(help))
}

/// Writes the edits of the instance `instance` is part of back into its prefab file. See
/// [`Engine::apply_prefab`](crate::Engine::apply_prefab).
pub(crate) fn apply(
    world: &mut World,
    instance: EntityId,
    parts: Option<&[String]>,
) -> Result<PrefabApplied, Vec<Diagnostic>> {
    let (source, root_id, live) = instance_parts(world, instance).map_err(one)?;
    let Some(library) = world.resource::<PrefabLibrary>() else {
        return Err(one(no_library("apply to the prefab")));
    };
    let (root, registry) = (library.root().to_path_buf(), Arc::clone(library.registry()));
    // After a restore the placement is unknown and the anchor's live pose recovers it.
    let placement = library.placement(root_id).cloned();
    let entry = instance_entry(world, &registry, &root, &live, placement).map_err(one)?;
    let (mut overrides, mut removed) = (entry.overrides, entry.removed);

    if let Some(list) = parts {
        let mut valid: Vec<&str> = live.iter().map(|(_, path)| path.as_str()).collect();
        valid.extend(removed.iter().map(String::as_str));
        if let Some(unknown) = list.iter().find(|path| !valid.contains(&path.as_str())) {
            return Err(apply_refusal(
                format!(
                    "`{unknown}` is not a part of the instance of `{source}` (parts: {})",
                    valid.join(", ")
                ),
                "pass part paths of the instance, or of parts it removed",
            ));
        }
        overrides.retain(|path, _| list.contains(path));
        removed.retain(|path| list.contains(path));
    }
    for (path, components) in &mut overrides {
        for (name, value) in components {
            let Some(info) = registry.get(name) else {
                continue;
            };
            if let Err(asset) = relativize_assets(info, value, &source) {
                return Err(apply_refusal(
                    format!(
                        "part `{path}` overrides `{name}` with `{asset}`, which cannot be written relative to `{source}`"
                    ),
                    "the override value is an absolute path; use a path relative to the content root",
                ));
            }
            if info
                .entity_refs()
                .iter()
                .any(|field| ref_target(value, field).is_some())
            {
                return Err(apply_refusal(
                    format!(
                        "part `{path}` overrides `{name}` with an entity reference, which has no position in `{source}`"
                    ),
                    "re-author the reference inside the prefab, or leave that part out of `parts`",
                ));
            }
        }
    }

    // A nested instance that lost every part is removed as its entry: its remaining `removed` paths
    // would leave an entry the prefab no longer loads.
    let reported = removed.clone();
    for head in reported
        .iter()
        .filter_map(|path| path.split_once('/').map(|(head, _)| head.to_owned()))
        .collect::<BTreeSet<_>>()
    {
        let prefix = format!("{head}/");
        if !live.iter().any(|(_, path)| path.starts_with(&prefix)) {
            removed.retain(|path| !path.starts_with(&prefix));
            removed.push(head);
        }
    }

    let old_text = read_prefab_text(&root, &source).map_err(one)?;
    let mut document = ludion_scene::parse_prefab(&old_text)?;
    if let Some(generated) = &document.generated_from {
        return Err(apply_refusal(
            format!(
                "`{source}` is generated from `{generated}`; edit the source and re-import, or keep \
                 the change as an instance override"
            ),
            "a generated prefab is write-only: the next import would overwrite the applied edit",
        ));
    }
    document
        .apply_instance(&overrides, &removed, &registry)
        .map_err(one)?;
    let new_text = ludion_scene::emit_prefab(&document, &registry);

    let library = world
        .resource_mut::<PrefabLibrary>()
        .expect("checked above");
    let saved = library.clone();
    library.replace(&source, &new_text)?;
    if let Err(error) = ludion_core::atomic::write_atomic(&root.join(&source), &new_text) {
        *library = saved;
        return Err(apply_refusal(
            format!("cannot write prefab `{source}`: {error}"),
            "the file must be writable",
        ));
    }
    world
        .resource_mut::<PrefabLibrary>()
        .expect("checked above")
        .bind(root_id);
    Ok(PrefabApplied {
        path: source,
        parts: overrides.into_keys().collect(),
        removed: reported,
    })
}
