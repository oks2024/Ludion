//! The engine side of the action map: the `kind: input-actions` document as an engine-owned
//! singleton, its validation under the re-pathed `L163x` codes, the [`Actions`] install, and the
//! `L4270` drain for action names a game reads that the map does not define.
//!
//! The sim-side layer (document types, compiler, gestures) lives in `ludion_sim::actions`; this
//! module only connects it to the document store and the diagnostics. Opt in with
//! [`EngineBuilder::input_actions`](crate::EngineBuilder::input_actions); `ludion docs input`.

use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest;
use ludion_core::time::TickRate;
use ludion_ecs::World;
use ludion_sim::actions::{ActionMap, Actions, InputActionsDoc, unknown_action_diagnostic};

use crate::DocStore;
use crate::diagnostics::report_full;

/// The `kind:` of the action-map document.
pub const INPUT_ACTIONS_KIND: &str = "input-actions";

/// With fewer names than this, the `L4270` help lists them all instead of guessing the nearest.
const LIST_ALL_BELOW: usize = 8;

/// Compiles the loaded action-map document and reports every compile error (`L1630`–`L1635`) with
/// its path rewritten to `input-actions/<doc key><pointer>` (the `validate_docs` path convention).
///
/// Compile errors do not depend on the tick rate, so the default rate is used. The engine's own
/// singleton count (`L1315`) covers a missing or doubled document.
pub fn validate_input_actions(world: &World) -> Vec<Diagnostic> {
    let Some(store) = world.resource::<DocStore<InputActionsDoc>>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, doc) in store.iter() {
        if let Err(errors) = ActionMap::compile(doc, TickRate::DEFAULT) {
            out.extend(errors.into_iter().map(|error| {
                let pointer = error.path.clone().unwrap_or_default();
                error.with_path(format!("{INPUT_ACTIONS_KIND}/{key}{pointer}"))
            }));
        }
    }
    out
}

/// Inserts an [`Actions`] compiled from the store's single document at `rate`, unless the world
/// already holds one (a `carry_resource` moved it across a switch) or the document is absent or
/// invalid (the build already refused that).
pub(crate) fn install(world: &mut World, rate: TickRate) {
    if world.resource::<Actions>().is_some() {
        return;
    }
    let Some(doc) = world
        .resource::<DocStore<InputActionsDoc>>()
        .and_then(DocStore::single)
    else {
        return;
    };
    if let Ok(map) = ActionMap::compile(doc, rate) {
        world.insert_resource(Actions::new(map));
    }
}

/// Reports each action name read since the last drain that the map does not define, once per
/// name per session (`L4270`).
pub(crate) fn drain_unknown(world: &mut World) {
    let Some(actions) = world.resource_mut::<Actions>() else {
        return;
    };
    let unknown = actions.take_unknown();
    if unknown.is_empty() {
        return;
    }
    let names: Vec<String> = actions.map().names().map(str::to_owned).collect();
    for name in unknown {
        let help = if names.len() < LIST_ALL_BELOW {
            format!("the map defines: {}", names.join(", "))
        } else {
            match suggest::nearest(name, names.iter().map(String::as_str)) {
                Some(nearest) => format!("nearest action is `{nearest}`"),
                None => "check the name against the `kind: input-actions` document".to_owned(),
            }
        };
        let mut diagnostic = unknown_action_diagnostic(name);
        diagnostic.message = format!("no action `{name}` in the input map");
        report_full(
            world,
            format!("{INPUT_ACTIONS_KIND}/{name}"),
            diagnostic.with_help(help),
        );
    }
}
