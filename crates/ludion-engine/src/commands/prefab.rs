//! `prefab.spawn`, `map.spawn`, `prefab.despawn`, `prefab.create`, `prefab.apply`,
//! `prefab.revert`, `prefab.unpack` and `prefab.overrides` handlers.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::relative_refusal;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{
    MapMirror, MapSpawnParams, PrefabApplyParams, PrefabCreateParams, PrefabDespawnParams,
    PrefabOverridesParams, PrefabRevertParams, PrefabSpawnParams, PrefabUnpackParams,
};
use ludion_protocol::response::PrefabInstanceOverrides;

use crate::collapse::override_pointers;
use crate::commands::collapse;
use crate::components::Transform;
use crate::engine::Engine;

/// `L3040` for a path outside the content root.
fn refuse_path(what: &str, path: &str) -> Result<(), Diagnostic> {
    match relative_refusal(path) {
        Some(refusal) => Err(Diagnostic::new(
            code::WIRE_PATH_OUTSIDE_ROOT,
            format!("{what} `{path}` {}", refusal.describe()),
        )
        .with_help(
            "pass a path relative to the content root (the directory of the scene the engine was \
             started with), e.g. `prefabs/crate.prefab.yaml`",
        )),
        None => Ok(()),
    }
}

/// `prefab.spawn` — instantiate the prefab at `path` (relative to the content root), all-or-nothing.
/// Replies with the new ids in the prefab's part order. A wire client may not name a path outside
/// the root (`L3040`), though the in-system `spawn_prefab` trusts game code.
pub(crate) fn spawn(
    engine: &mut Engine,
    params: PrefabSpawnParams,
) -> Result<ResponseBody, Diagnostic> {
    refuse_path("prefab path", &params.path)?;
    let placement = params.transform.map(|placement| Transform {
        translation: placement.translation,
        rotation: placement.rotation,
        scale: placement.scale,
    });
    let spawned = crate::prefab::spawn_prefab(
        engine.world_mut(),
        &params.path,
        placement,
        params.name.as_deref(),
        params.presentation,
    );
    // Drain on the error path too: the text was read, and the Command record follows this one.
    engine.drain_prefab_texts();
    let entities = spawned.map_err(collapse)?;
    Ok(ResponseBody::PrefabSpawned { entities })
}

/// `map.spawn` — instantiate the `kind: map` file at `map` as a room, all-or-nothing, with the same
/// path refusal (`L3040`) as `prefab.spawn` on `map` and `kit`. Replies with the new ids, the
/// instance root first.
pub(crate) fn spawn_map(
    engine: &mut Engine,
    params: MapSpawnParams,
) -> Result<ResponseBody, Diagnostic> {
    refuse_path("map path", &params.map)?;
    if let Some(kit) = &params.kit {
        refuse_path("kit path", kit)?;
    }
    let spawn = crate::MapSpawn {
        kit: params.kit.as_deref(),
        seed: params.seed,
        mirror: match params.mirror {
            None => crate::Mirror::default(),
            Some(MapMirror::X) => crate::Mirror::X,
            Some(MapMirror::Z) => crate::Mirror::Z,
        },
        placement: params.placement.map(|placement| Transform {
            translation: placement.translation,
            rotation: placement.rotation,
            scale: placement.scale,
        }),
        name: params.name.as_deref(),
    };
    let spawned = crate::spawn_map(engine.world_mut(), &params.map, spawn);
    // Drain on the error path too: the map and kit texts were read, and the Command record follows.
    engine.drain_prefab_texts();
    let entities = spawned.map_err(collapse)?;
    Ok(ResponseBody::PrefabSpawned { entities })
}

/// `prefab.despawn` — remove every part of the instance rooted at `root`; replies with the removed
/// ids, sorted.
pub(crate) fn despawn(
    engine: &mut Engine,
    params: PrefabDespawnParams,
) -> Result<ResponseBody, Diagnostic> {
    let entities = crate::despawn_instance(engine.world_mut(), params.root)?;
    Ok(ResponseBody::PrefabDespawned { entities })
}

/// `prefab.create` — write the live `entities` out as a new prefab at `path`; the world's entities are
/// untouched, and the library takes the new text of a prefab it already holds.
pub(crate) fn create(
    engine: &mut Engine,
    params: PrefabCreateParams,
) -> Result<ResponseBody, Diagnostic> {
    refuse_path("prefab path", &params.path)?;
    let path = engine
        .create_prefab(&params.entities, &params.path)
        .map_err(collapse)?;
    Ok(ResponseBody::PrefabCreated { path })
}

/// `prefab.apply` — write the instance's edits back into its prefab file. The engine call drains
/// the reload record it produced.
pub(crate) fn apply(
    engine: &mut Engine,
    params: PrefabApplyParams,
) -> Result<ResponseBody, Diagnostic> {
    // A missing provenance falls through to the engine's own refusal.
    if let Ok(Some(held)) = engine.world().get::<crate::PrefabInstance>(params.instance) {
        refuse_path("prefab path", &held.source)?;
    }
    let applied = engine
        .apply_prefab(params.instance, params.parts.as_deref())
        .map_err(collapse)?;
    Ok(ResponseBody::PrefabApplied {
        path: applied.path,
        parts: applied.parts,
        removed: applied.removed,
    })
}

/// `prefab.revert` — restore the instance (or the `pointer` inside it) from its prefab file.
pub(crate) fn revert(
    engine: &mut Engine,
    params: PrefabRevertParams,
) -> Result<ResponseBody, Diagnostic> {
    let reverted = crate::revert_prefab(
        engine.world_mut(),
        params.instance,
        params.pointer.as_deref(),
    );
    // The revert may have read the prefab: its text must precede the Command record.
    engine.drain_prefab_texts();
    let reverted = reverted.map_err(collapse)?;
    Ok(ResponseBody::PrefabReverted {
        entities: reverted.entities,
        not_restored: reverted.not_restored,
    })
}

/// `prefab.unpack` — turn the instance into plain entities.
pub(crate) fn unpack(
    engine: &mut Engine,
    params: PrefabUnpackParams,
) -> Result<ResponseBody, Diagnostic> {
    let unpacked = crate::unpack_prefab(engine.world_mut(), params.instance);
    engine.drain_prefab_texts();
    let entities = unpacked.map_err(collapse)?;
    Ok(ResponseBody::PrefabUnpacked { entities })
}

/// `prefab.overrides` — what `scene.save` would write for each instance, read through the save's own
/// collapse. A pure read: nothing is drained or recorded.
pub(crate) fn overrides(
    engine: &mut Engine,
    params: PrefabOverridesParams,
) -> Result<ResponseBody, Diagnostic> {
    let read = engine.prefab_overrides(params.instances.as_deref())?;
    let instances = read
        .into_iter()
        .map(|held| {
            let mut out = PrefabInstanceOverrides {
                root: held.root,
                prefab: held.prefab,
                name: None,
                presentation: false,
                placement_differs: false,
                overrides: BTreeMap::new(),
                removed: Vec::new(),
                root_parts: held.root_parts,
                diagnostic: None,
            };
            match held.result {
                Ok(Some(diff)) => {
                    out.name = Some(diff.name);
                    out.presentation = diff.entry.presentation;
                    out.placement_differs = diff.placement_differs;
                    out.removed = diff.entry.removed;
                    for (part, components) in &diff.entry.overrides {
                        let gained = diff.gained.get(part).map_or(&[][..], Vec::as_slice);
                        out.overrides
                            .insert(part.clone(), override_pointers(components, gained));
                    }
                }
                Ok(None) => {}
                Err(refusal) => out.diagnostic = Some(refusal),
            }
            out
        })
        .collect();
    Ok(ResponseBody::PrefabOverrides { instances })
}
