//! `entity.spawn` / `entity.destroy` / `component.set` / `component.get` / `component.remove` /
//! `query.entities` handlers.
//!
//! Component names and values are validated against the registry — the boundary that keeps the
//! protocol free of concrete component types. Unknown names surface as `L0004`, values `serde`
//! rejects that the schema allows as `L0006`, and a dead entity as `L0002`, all from `ludion-ecs`;
//! values that do not match a component's schema surface as `L1014` from `ludion-scene`'s checker —
//! the one the scene-file path runs. This module only adds the diagnostics dispatch genuinely
//! constructs itself: component-absent (`L3006`) and the `entity.spawn` name/`Name` conflict
//! (`L3014`). `entity.spawn` and `component.set` both check each value's schema, then its semantic
//! validator, before touching the world — the wire has no back door around a component's schema or
//! cross-field invariants (e.g. a self-joined `Joint`, `L1183`). They also check the entity references
//! a value holds: a `Parent` takes the `entity.reparent` rules, and any other marked field refuses
//! naming its own carrier where the schema forbids it (`L1017`) and a sim carrier naming a
//! presentation entity (`L1036`; on `entity.spawn` only a live one). A reference to a dead entity is allowed except on `Parent`, which
//! is `L0002`: an editor's undo respawns with stale ids and repairs them afterwards, and
//! `scene.save` (`L1018`) still refuses what stays dangling.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{ChildMap, Parent, Registry, World};
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{
    ComponentGetParams, ComponentRemoveParams, ComponentSetParams, EntityDestroyParams,
    EntityReparentParams, EntitySpawnParams, QueryEntitiesParams, ReparentKeep,
};
use ludion_protocol::response::{DestroyedEntity, EntityMatch, WorldTransform};
use ludion_spatial::Collider;

use crate::engine::Engine;
use crate::hierarchy::ParentCandidate;

/// `entity.spawn` — create an entity carrying the given components (all-or-nothing: a failure
/// mutates nothing, not even the entity allocator).
///
/// The convenience `name` field becomes the entity's [`Name`](crate::components::Name) component;
/// supplying both `name` and a `Name` entry in `components` is `L3014`. Every value is also
/// checked, in `components`' `BTreeMap` order — schema (`L1014`) then semantics, the same check
/// `component.set` runs (e.g. a `Joint` naming the same entity twice is `L1183`), so the wire is not a
/// back door around it — and then deserialized into a scratch world, so an unknown component (`L0004`)
/// or a value serde rejects that the schema allows (`L0006`) is reported before the real world is
/// touched.
///
/// The new entity has no id yet, so a reference to itself cannot be detected: a client that predicts
/// the next id can write a self reference on a non-`Parent` field here, and `scene.save` refuses it
/// (`L1018`). A predicted `Parent` is dead at check time and is `L0002`. The entity is always a sim
/// entity, so a reference to a live presentation entity is `L1036` (a dead one is accepted: an undo
/// respawn carries its own old id and repairs it next); a `Parent` and a rigid `Collider` in
/// the same spawn is `L1221`.
pub(crate) fn spawn(
    engine: &mut Engine,
    params: EntitySpawnParams,
) -> Result<ResponseBody, Diagnostic> {
    if params.name.is_some() && params.components.contains_key("Name") {
        return Err(spawn_name_conflict());
    }
    let (registry, world) = engine.parts_mut();
    for (name, value) in &params.components {
        check_value(registry, name, value)?;
    }
    // Every value goes through a scratch world first: a failing spawn on the real world would still
    // move its allocator (a despawn bumps the slot generation, which the `WorldHash` includes).
    let mut probe = World::new();
    let at = probe.spawn();
    for (name, value) in &params.components {
        registry.deserialize_insert(name, value, &mut probe, at)?;
    }
    for (name, value) in &params.components {
        let collider = probe.get::<Collider>(at).ok().flatten();
        check_refs(
            registry,
            world,
            name,
            value,
            ParentCandidate::Unspawned { collider },
        )?;
    }
    let entity = world.spawn();
    for (name, value) in &params.components {
        registry
            .deserialize_insert(name, value, world, entity)
            .expect("entity.spawn probed every component into a scratch world");
    }
    if let Some(name) = params.name {
        world
            .insert(entity, crate::components::Name(name))
            .expect("a freshly spawned entity is alive");
    }
    Ok(ResponseBody::Spawned { entity })
}

/// A wire value must fail as loudly as a scene value: its schema (`L1014`), then — on a schema-clean
/// value — the component's semantic validator (cross-field invariants JSON Schema can't express, e.g.
/// a tilemap's equal-length rows, `L1019`/`L1020`), both before the world is touched. Pointers are
/// relative to the component (there is no document position on the wire). An unregistered name passes
/// here; the insert reports `L0004`.
fn check_value(
    registry: &Registry,
    name: &str,
    value: &serde_json::Value,
) -> Result<(), Diagnostic> {
    if let Some(diagnostic) = ludion_scene::check_component(name, value, registry)
        .into_iter()
        .next()
    {
        return Err(diagnostic);
    }
    match registry
        .get(name)
        .and_then(|info| info.validate_semantics(value).into_iter().next())
    {
        Some(diagnostic) => Err(diagnostic),
        None => Ok(()),
    }
}

/// The entity-reference checks of one wire value, after its schema and serialization are clean:
/// the non-tree fields through `ludion_scene::check_component_refs`, then each `Parent`-shaped field
/// through the reparent rules. Pointers are relative to the component.
fn check_refs(
    registry: &Registry,
    world: &World,
    name: &str,
    value: &serde_json::Value,
    candidate: ParentCandidate<'_>,
) -> Result<(), Diagnostic> {
    let carrier = match candidate {
        ParentCandidate::Live(id) => Some(id),
        ParentCandidate::Unspawned { .. } => None,
    };
    if let Some(diagnostic) =
        ludion_scene::check_component_refs(name, value, registry, carrier, &|id| world.is_alive(id))
            .into_iter()
            .next()
    {
        return Err(diagnostic);
    }
    let Some(info) = registry.get(name) else {
        return Ok(());
    };
    for field in info.entity_refs().iter().filter(|field| field.hierarchy) {
        if let Some(parent) = field.target(value) {
            crate::hierarchy::check_parent(world, candidate, parent)
                .map_err(|diagnostic| diagnostic.with_path(format!("{}/index", field.suffix)))?;
        }
    }
    Ok(())
}

/// Builds the `L3014` diagnostic for an `entity.spawn` that named the entity both ways at once.
fn spawn_name_conflict() -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_SPAWN_NAME_CONFLICT,
        "entity.spawn was given both a `name` and a `Name` component",
    )
    .with_help(
        "pass the name one way — the top-level `name` field, or a `Name` component, not both",
    )
}

/// `entity.destroy` — despawn an entity and its descendants, returning the entity's full component
/// state at the moment of destruction plus each descendant's, parents before children (the undo
/// inverse: `entity.spawn` with these components respawns each, under a fresh id). `L0002` if the
/// handle is not alive.
pub(crate) fn destroy(
    engine: &mut Engine,
    params: EntityDestroyParams,
) -> Result<ResponseBody, Diagnostic> {
    let registry = engine.registry();
    let world = engine.world();
    let components = snapshot(registry, world, params.entity)?;
    let map = ChildMap::build(world);
    let set: std::collections::BTreeSet<_> = map.descendants(params.entity).into_iter().collect();
    let mut descendants = Vec::with_capacity(set.len());
    let mut queue = std::collections::VecDeque::from([params.entity]);
    while let Some(node) = queue.pop_front() {
        for &child in map.children(node) {
            if set.contains(&child) {
                descendants.push(DestroyedEntity {
                    entity: child,
                    components: snapshot(registry, world, child)?,
                });
                queue.push_back(child);
            }
        }
    }
    engine.world_mut().despawn(params.entity)?;
    Ok(ResponseBody::EntityDestroyed {
        components,
        descendants,
    })
}

/// Every registered component `entity` carries, keyed by registry name; `L0002` if it is not alive.
fn snapshot(
    registry: &Registry,
    world: &World,
    entity: EntityId,
) -> Result<BTreeMap<String, serde_json::Value>, Diagnostic> {
    let mut components = BTreeMap::new();
    for info in registry.iter() {
        // `L0002` surfaces here on the first component if the handle is not alive — the registry is
        // never empty, so this is equivalent to checking liveness up front.
        if let Some(value) = info.serialize(world, entity)? {
            components.insert(info.name().to_string(), value);
        }
    }
    Ok(components)
}

/// `entity.reparent` — re-point an entity's `Parent` (`null` removes it), keeping its world pose or
/// its local `Transform`. Replies with the parent and `Transform` it had before, the undo inverse.
/// Refusals (`L1221`/`L1222`/`L1224`/`L1225`/`L1260`, `L0002`) change nothing.
pub(crate) fn reparent(
    engine: &mut Engine,
    params: EntityReparentParams,
) -> Result<ResponseBody, Diagnostic> {
    let (registry, world) = engine.parts_mut();
    let previous_parent = world.get::<Parent>(params.entity)?.map(|parent| parent.0);
    let previous_transform = registry.serialize("Transform", world, params.entity)?;
    let keep = match params.keep {
        ReparentKeep::World => crate::hierarchy::Keep::World,
        ReparentKeep::Local => crate::hierarchy::Keep::Local,
    };
    crate::hierarchy::reparent(world, params.entity, params.parent, keep)?;
    Ok(ResponseBody::Reparented {
        previous_parent,
        previous_transform,
    })
}

/// `component.set` — insert or overwrite one component value on an entity, returning the value it
/// held immediately before (`None` for a fresh insert — the undo inverse).
pub(crate) fn set(
    engine: &mut Engine,
    params: ComponentSetParams,
) -> Result<ResponseBody, Diagnostic> {
    let (registry, world) = engine.parts_mut();
    check_value(registry, &params.component, &params.value)?;
    let previous = registry.serialize(&params.component, world, params.entity)?;
    check_refs(
        registry,
        world,
        &params.component,
        &params.value,
        ParentCandidate::Live(params.entity),
    )?;
    registry.deserialize_insert(&params.component, &params.value, world, params.entity)?;
    Ok(ResponseBody::ComponentSet { previous })
}

/// `component.remove` — remove one component from an entity, returning the value it held (the undo
/// inverse: `component.set` with this value re-inserts it).
///
/// Shares `component.get`'s exact error semantics: `L0004` if the component name is unregistered,
/// `L0002` if the entity is not alive, `L3006` if the entity is alive and the name valid but the
/// entity does not carry that component.
pub(crate) fn remove(
    engine: &mut Engine,
    params: ComponentRemoveParams,
) -> Result<ResponseBody, Diagnostic> {
    let previous =
        match engine
            .registry()
            .serialize(&params.component, engine.world(), params.entity)?
        {
            Some(value) => value,
            None => {
                return Err(component_absent(
                    engine.registry(),
                    engine.world(),
                    params.entity,
                    &params.component,
                ));
            }
        };
    let (registry, world) = engine.parts_mut();
    registry
        .get(&params.component)
        .expect("just validated present above")
        .remove(world, params.entity)?;
    Ok(ResponseBody::ComponentRemoved { previous })
}

/// `component.get` — read one component value from an entity.
///
/// `L0004` if the component name is unregistered, `L0002` if the entity is not alive, `L3006` if the
/// entity is alive and the name valid but the entity does not carry that component.
pub(crate) fn get(
    engine: &mut Engine,
    params: ComponentGetParams,
) -> Result<ResponseBody, Diagnostic> {
    match engine
        .registry()
        .serialize(&params.component, engine.world(), params.entity)?
    {
        Some(value) => Ok(ResponseBody::ComponentValue { value }),
        None => Err(component_absent(
            engine.registry(),
            engine.world(),
            params.entity,
            &params.component,
        )),
    }
}

/// `query.entities` — list entities carrying all `filter` components, returning each one's requested
/// `include` component values (unknown names in either list are `L0004`) and, with `world`, its
/// composed world `Transform`. With `entities`, only those ids that are live (same generation) are
/// candidates, in canonical order; others are skipped silently.
pub(crate) fn query(
    engine: &mut Engine,
    params: QueryEntitiesParams,
) -> Result<ResponseBody, Diagnostic> {
    let registry = engine.registry();
    for name in params.filter.iter().chain(params.include.iter()) {
        if registry.get(name).is_none() {
            return Err(registry.unknown_component(name));
        }
    }

    let world = engine.world();
    let candidates: Vec<EntityId> = match params.entities {
        Some(mut ids) => {
            ids.retain(|&id| world.is_alive(id));
            ids.sort_unstable_by_key(|id| id.index());
            ids.dedup();
            ids
        }
        None => world
            .entities()
            .chain(world.presentation_entities())
            .collect(),
    };
    let mut entities = Vec::new();
    for id in candidates {
        let matches_filter = params
            .filter
            .iter()
            .all(|name| registry.get(name).is_some_and(|info| info.has(world, id)));
        if !matches_filter {
            continue;
        }
        let mut components = BTreeMap::new();
        for name in &params.include {
            if let Some(value) = registry.serialize(name, world, id)? {
                components.insert(name.clone(), value);
            }
        }
        let world_transform = if params.world {
            crate::hierarchy::world_pose_transform(world, id).map(|pose| WorldTransform {
                translation: pose.translation,
                rotation: pose.rotation,
                scale: pose.scale,
                sheared: crate::hierarchy::world_transform(world, id).is_some_and(|w| w.sheared),
            })
        } else {
            None
        };
        entities.push(EntityMatch {
            entity: id,
            components,
            world: world_transform,
        });
    }
    Ok(ResponseBody::QueryResult { entities })
}

/// Builds the `L3006` component-absent diagnostic, listing the components the entity does have.
fn component_absent(
    registry: &Registry,
    world: &World,
    entity: EntityId,
    component: &str,
) -> Diagnostic {
    let present: Vec<&str> = registry
        .iter()
        .filter(|info| info.has(world, entity))
        .map(|info| info.name())
        .collect();
    let help = if present.is_empty() {
        format!("entity {entity} carries no components")
    } else {
        format!("entity {entity} carries: {}", present.join(", "))
    };
    Diagnostic::new(
        code::PROTOCOL_COMPONENT_ABSENT,
        format!("entity {entity} does not have a `{component}` component"),
    )
    .with_help(help)
}
