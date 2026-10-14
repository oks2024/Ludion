#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `{{name}}` — a starter Ludion game: one custom component, one system, one scene that mixes
//! built-in visuals with that component.
//!
//! Read `LUDION-GUIDE.md` before extending this. The shape here is the everyday one: a game brings
//! its own components (plain serde/schemars structs) and systems (`fn(&mut World, &SystemCtx)`) and
//! gets scene load, the deterministic sim, `WorldHash`, recording/replay, the wire protocol, and the
//! golden harness from the engine.
//!
//! **This file is the wiring root.** Definitions live in per-concern modules — [`components`] holds
//! the data structs (and, at this scale, their tunables), [`systems`] holds the behaviors — and are
//! glob re-exported here so every item keeps one public path (`{{name_ident}}::Player`). What stays
//! in the root is the *wiring*: [`register`] lists the components, and [`game`] lists the systems in
//! the order they run. As the game grows, add modules (`enemies.rs`, `weapons.rs`, …) and re-export
//! them the same way — never let one module regrow into a catch-all, or the engine warns with `L1124`
//! at boot (`ludion explain L1124`). The engine repo's `games/shmup-stress` is a worked example of the split.

use ludion::prelude::*;

mod components;
mod systems;

pub use components::*;
pub use systems::*;

// --- Wiring --------------------------------------------------------------------------------------

/// Registers this game's components on top of a registry that already holds the built-ins.
///
/// Exposed as a named function so the golden test's registry seam (`GoldenTest::with_registry`) can
/// build the same registry. Add a `registry.register::<YourComponent>("YourComponent")?` line here
/// for every new component.
///
/// # Errors
/// An `L0005` diagnostic if a name collides with a built-in or another game component.
pub fn register(registry: &mut Registry) -> Result<(), Diagnostic> {
    registry.register::<Player>("Player")
}

/// This game's full component registry: the built-ins plus this game's components, in registration
/// order. A `fn() -> Registry` so it can be handed straight to `GoldenTest::with_registry`.
///
/// # Panics
/// If [`register`] fails — a build-time bug (a name colliding with a built-in), not a runtime state.
pub fn registry() -> Registry {
    let mut registry = builtin_registry();
    register(&mut registry).expect("components register cleanly on top of the built-ins");
    registry
}

/// Describes the engine that hosts this game: the built-in core plus this game's components and
/// systems, with `scenes/main.yaml` as the default scene `ludion::run` loads when `--scene` is
/// omitted.
///
/// The ordered `.system()` list is the schedule — **registration order is execution order**, so this
/// is where you sequence a tick. The system *definitions* live in [`systems`]; only the wiring lives
/// here. Returns a fresh builder each call (building consumes it), so `main` and the tests each get one.
pub fn game() -> EngineBuilder {
    Engine::builder()
        .components(register)
        .system("move_player", move_player)
        .default_scene("scenes/main.yaml")
}

/// The symbol a native host (the iOS app `ludion export` writes) starts this game through: [`game`]
/// handed to the facade's C ABI. The one `unsafe` in the crate; the host owns the returned handle and
/// ends it with `ludion_stop`.
///
/// # Safety
/// `params` must be null or point to a valid [`ludion::ffi::LudionStartParams`]; see
/// [`ludion::ffi::start`].
#[allow(unsafe_code)] // the one symbol the native host calls
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ludion_game_start(
    params: *const ludion::ffi::LudionStartParams,
) -> *mut ludion::ffi::LudionEmbed {
    // SAFETY: the caller upholds `start`'s contract.
    unsafe { ludion::ffi::start(game(), params) }
}
