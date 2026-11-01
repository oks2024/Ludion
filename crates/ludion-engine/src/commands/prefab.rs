//! `prefab.spawn`, `prefab.create`, `prefab.apply`, `prefab.revert`, `prefab.unpack` and
//! `prefab.overrides` handlers.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths::relative_refusal;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{
    PrefabApplyParams, PrefabCreateParams, PrefabOverridesParams, PrefabRevertParams,
    PrefabSpawnParams, PrefabUnpackParams,
};
use ludion_protocol::response::PrefabInstanceOverrides;

use crate::collapse::override_pointers;
use crate::commands::collapse;
use crate::components::Transform;
use crate::engine::Engine;

/// `L3040` for a path outside the content root.
fn refuse_path(path: &str) -> Result<(), Diagnostic> {
    match relative_refusal(path) {
        Some(refusal) => Err(Diagnostic::new(
            code::WIRE_PATH_OUTSIDE_ROOT,
            format!("prefab path `{path}` {}", refusal.describe()),
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
    refuse_path(&params.path)?;
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

/// `prefab.create` — write the live `entities` out as a new prefab at `path`; the world is untouched.
pub(crate) fn create(
    engine: &mut Engine,
    params: PrefabCreateParams,
) -> Result<ResponseBody, Diagnostic> {
    refuse_path(&params.path)?;
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
        refuse_path(&held.source)?;
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
