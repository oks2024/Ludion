//! Scene → world instantiation.
//!
//! This crate is the one place that knows both [`Scene`] and the ECS, so it owns the step that
//! turns scene entities into live ones. [`instantiate_into`] is the one primitive: it spawns one
//! entity per scene entity into an existing [`World`], **in document order** — a sim entity from the
//! sim allocator, a `presentation: true` one from the presentation allocator — then inserts each
//! component through the registry's erased deserialization. Entity references in a document name
//! document positions and are remapped to the spawned ids (index and generation) before insertion.
//! [`instantiate`] is its fresh-world wrapper; on a fresh world the sim slots are `0, 1, 2…` and
//! [`instantiated_ids`] is the mapping. `ludion-ecs` never learns YAML exists, and `ludion-sim`
//! never learns scenes exist; this module is the bridge between them.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::Trs;
use ludion_ecs::{PRESENTATION_BASE, Phase, Registry, World};
use serde_json::Value;

use crate::yaml::pointer_token;
use crate::{Scene, SceneEntity};

/// The [`EntityId`] each scene entity gets when instantiated into a *fresh* world, by document position.
pub(crate) fn instantiated_ids(scene: &Scene) -> Vec<EntityId> {
    let (mut sim, mut presentation) = (0u32, 0u32);
    scene
        .entities
        .iter()
        .map(|entity| {
            let counter = if entity.presentation {
                &mut presentation
            } else {
                &mut sim
            };
            let index = *counter
                + if entity.presentation {
                    PRESENTATION_BASE
                } else {
                    0
                };
            *counter += 1;
            EntityId::new(index, 0)
        })
        .collect()
}

/// The engine-typed half of instantiation — `ludion-scene` never names a component type.
pub trait InstanceHooks {
    /// Inserts a part's final name (`<prefix>/<part>` or bare).
    fn name(&self, world: &mut World, entity: EntityId, name: &str);
    /// Bakes `placement` onto a root: its local pose becomes `placement ∘ local` (a root without a
    /// pose takes `placement` itself).
    fn place(&self, world: &mut World, root: EntityId, placement: &Trs);
    /// Marks a named part as spawned from a template; `root` is the group's first id.
    fn provenance(&self, _world: &mut World, _entity: EntityId, _part: &str, _root: EntityId) {}
}

/// What [`instantiate_into`](crate::instantiate_into) does beyond spawning and inserting.
pub struct InstantiateOptions<'a> {
    /// Baked onto every root of the group (no synthetic root); `None` leaves the poses as written.
    pub placement: Option<Trs>,
    /// Names the group's named parts `<prefix>/<part>`; bare names without it.
    pub name_prefix: Option<&'a str>,
    /// The engine-typed half.
    pub hooks: &'a dyn InstanceHooks,
}

/// Hooks for the fresh-world path: nothing is named, nothing is placed.
struct NoHooks;

impl InstanceHooks for NoHooks {
    fn name(&self, _: &mut World, _: EntityId, _: &str) {}
    fn place(&self, _: &mut World, _: EntityId, _: &Trs) {}
}

/// Rewrites each entity-reference field of `value` from a document position to the spawned id.
/// Null, non-object and out-of-range references are left alone (`validate` reports them as `L1017`).
fn remap_refs(registry: &Registry, name: &str, value: &mut Value, ids: &[EntityId]) {
    let Some(info) = registry.get(name) else {
        return;
    };
    let sites = info.entity_ref_sites(value);
    for field in &sites {
        let reference = if field.suffix.is_empty() {
            Some(&mut *value)
        } else {
            value.pointer_mut(&field.suffix)
        };
        let Some(object) = reference.and_then(Value::as_object_mut) else {
            continue;
        };
        let Some(id) = object
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|position| ids.get(position as usize))
        else {
            continue;
        };
        object.insert("index".to_owned(), Value::from(id.index()));
        object.insert("generation".to_owned(), Value::from(id.generation()));
    }
}

/// Whether the part holds a reference into `range` in a `hierarchy` field (a `Parent`).
pub(crate) fn links_into(
    entity: &SceneEntity,
    registry: &Registry,
    range: &std::ops::Range<usize>,
) -> bool {
    entity.components.iter().any(|(name, value)| {
        let Some(info) = registry.get(name) else {
            return false;
        };
        info.entity_refs()
            .iter()
            .filter(|field| field.hierarchy)
            .any(|field| {
                let reference = if field.suffix.is_empty() {
                    Some(value)
                } else {
                    value.pointer(&field.suffix)
                };
                reference
                    .and_then(|reference| reference.get("index"))
                    .and_then(Value::as_u64)
                    .is_some_and(|position| range.contains(&(position as usize)))
            })
    })
}

/// Reserved components are field-backed (e.g. `Name`, projected from `name:`) and are populated by
/// the owner of the reserved type, not from a `components:` block. `check` rejects such a block
/// outright (L1016); skipping them only guards the unchecked path.
fn is_reserved(registry: &Registry, component: &str) -> bool {
    registry.get(component).is_some_and(|info| info.reserved())
}

/// Spawns `entities` into `world`; see [`crate::instantiate_into`] for the API contract.
pub(crate) fn instantiate_into(
    world: &mut World,
    entities: &[SceneEntity],
    registry: &Registry,
    options: &InstantiateOptions<'_>,
) -> Result<Vec<EntityId>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    // The spawn would panic across the sim/presentation boundary; refuse instead.
    let refused = match world.phase() {
        Phase::Sim => Some((true, "a sim system", "presentation")),
        Phase::Presentation => Some((false, "a presentation system", "sim")),
        Phase::Host => None,
    };
    if let Some((refuse_presentation, who, kind)) = refused {
        for (index, entity) in entities.iter().enumerate() {
            if entity.presentation == refuse_presentation {
                diagnostics.push(
                    Diagnostic::new(
                        code::PRESENTATION_BOUNDARY,
                        format!(
                            "{who} instantiated a {kind} entity; the boundary refuses the spawn"
                        ),
                    )
                    .with_path(format!("/entities/{index}/presentation"))
                    .with_help(
                        "instantiate from a host path, or from a system of the matching phase",
                    ),
                );
            }
        }
    }
    for (index, entity) in entities.iter().enumerate() {
        if entity.instance.is_some() {
            diagnostics.push(
                Diagnostic::new(
                    code::PREFAB_INSTANCE_STRUCTURE,
                    "an unexpanded prefab instance cannot be spawned as an entity",
                )
                .with_path(format!("/entities/{index}/prefab"))
                .with_help("expand the document first"),
            );
        }
    }
    // Dry run: every component must deserialize before anything spawns (all-or-nothing). Entity
    // references need no remap — deserializing an `EntityId` checks nothing.
    let mut scratch = World::new();
    let probe = scratch.spawn();
    for (index, entity) in entities.iter().enumerate() {
        for (name, value) in &entity.components {
            if is_reserved(registry, name) {
                continue;
            }
            if let Err(diagnostic) = registry.deserialize_insert(name, value, &mut scratch, probe) {
                diagnostics.push(diagnostic.with_path(format!(
                    "/entities/{index}/components/{}",
                    pointer_token(name)
                )));
            }
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let ids: Vec<EntityId> = entities
        .iter()
        .map(|entity| {
            if entity.presentation {
                world.spawn_presentation()
            } else {
                world.spawn()
            }
        })
        .collect();
    for (entity, &id) in entities.iter().zip(&ids) {
        for (name, value) in &entity.components {
            if is_reserved(registry, name) {
                continue;
            }
            let mut remapped;
            let value = if registry
                .get(name)
                .is_some_and(|info| !info.entity_refs().is_empty())
            {
                remapped = value.clone();
                remap_refs(registry, name, &mut remapped, &ids);
                &remapped
            } else {
                value
            };
            registry
                .deserialize_insert(name, value, world, id)
                .expect("the dry run accepted every component, so the insert cannot fail");
        }
    }
    if let Some(placement) = &options.placement {
        for (entity, &id) in entities.iter().zip(&ids) {
            if !links_into(entity, registry, &(0..entities.len())) {
                options.hooks.place(world, id, placement);
            }
        }
    }
    for (entity, &id) in entities.iter().zip(&ids) {
        if let Some(name) = &entity.name {
            match options.name_prefix {
                Some(prefix) => options.hooks.name(world, id, &format!("{prefix}/{name}")),
                None => options.hooks.name(world, id, name),
            }
        }
    }
    for (entity, &id) in entities.iter().zip(&ids) {
        if let Some(name) = &entity.name {
            options.hooks.provenance(world, id, name, ids[0]);
        }
    }
    Ok(ids)
}

/// Builds a fresh [`World`] from `scene`; see [`crate::instantiate`] for the API contract.
pub(crate) fn instantiate(scene: &Scene, registry: &Registry) -> Result<World, Vec<Diagnostic>> {
    let mut world = World::new();
    let options = InstantiateOptions {
        placement: None,
        name_prefix: None,
        hooks: &NoHooks,
    };
    instantiate_into(&mut world, &scene.entities, registry, &options)?;
    Ok(world)
}
