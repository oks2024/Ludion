//! [`loaded_engine`] and [`GameplayDriver`] — the "build a game's engine, load its scene inline"
//! step every headless gameplay/replay/audio test starts from, plus a small scripted-input driver
//! for tests that press/hold/tap keys instead of hand-building `Command::InputInject`/`SimStep`
//! calls.
//!
//! Every game's `tests/gameplay.rs`, `tests/replay.rs`, and `tests/audio.rs` opened with the same
//! three lines — `builder.build().expect(...)` then `engine.load_inline_scene(scene).expect(...)`
//! — and several then hand-rolled a `frame` counter alongside it to address `input.inject` events at
//! the right tick. [`loaded_engine`] is that opening step, once; [`GameplayDriver`] adds the frame
//! cursor on top, for tests that script presses rather than driving raw [`Command`]s (those still
//! reach for [`ludion_engine::drive_hashes`] directly — this module doesn't replace that path, only
//! the setup before it).
//!
//! # A note on this crate's dependency edge
//!
//! Like [`crate::sim_assert`], this module reaches for a type
//! (here, [`Command`]/[`command`]) that `ludion-engine`'s public surface
//! does not re-export (`Engine::execute` takes `ludion_protocol::Command` directly — see its
//! `engine.rs`). Depending on `ludion-protocol` directly, rather than routing through `engine`, adds
//! no cycle risk (`ludion-engine` already depends on it) and keeps this a thin wrapper instead of
//! forcing a new re-export onto `ludion-engine` just to satisfy the architecture diagram's
//! simplified `ludion-testkit → engine, client` summary.

use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::{Command, command};
use serde_json::{Value, json};

/// Builds `builder` and loads `scene` inline — the "fresh engine, scene loaded" step every headless
/// gameplay, replay, and audio test starts from. Both steps are expected to succeed: a game's own
/// `EngineBuilder` failing to build, or its own scene failing to load against the registry it just
/// built, is a bug in the test fixture, not a condition under test.
///
/// # Panics
/// If `builder.build()` fails, or the loaded scene is rejected by the built registry (e.g. a scene
/// naming a component the builder never registered).
///
/// # Example
/// ```
/// use ludion_engine::EngineBuilder;
///
/// let engine = ludion_testkit::loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n");
/// assert_eq!(engine.world().entities().count(), 0);
/// ```
pub fn loaded_engine(builder: EngineBuilder, scene: &str) -> Engine {
    let mut engine = builder.build().expect("the game builds");
    engine
        .load_inline_scene(scene)
        .expect("the scene loads against the game registry");
    engine
}

/// A scripted-input driver over a [`loaded_engine`]: tracks a frame cursor so a test can
/// `hold`/`release`/`tap` keys (or [`inject_value`](GameplayDriver::inject_value) a richer event)
/// without re-deriving which frame each one lands on.
///
/// The cursor is not strictly necessary for correctness — `Command::InputInject`'s own `frame: None`
/// already means "apply at the next tick," i.e. the driver's current frame — but naming the frame
/// explicitly is what let the games this driver replaces interleave presses and steps in whatever
/// order a scripted session needs, and a test occasionally wants to read [`frame`](GameplayDriver::frame)
/// back (to time an assertion, or to build a raw [`Command`] list by hand alongside the driver).
pub struct GameplayDriver {
    engine: Engine,
    frame: u64,
}

impl GameplayDriver {
    /// Builds `builder`, loads `scene` inline (via [`loaded_engine`]), and starts the frame cursor
    /// at 0.
    ///
    /// # Panics
    /// Same conditions as [`loaded_engine`].
    pub fn new(builder: EngineBuilder, scene: &str) -> Self {
        GameplayDriver {
            engine: loaded_engine(builder, scene),
            frame: 0,
        }
    }

    /// The underlying engine, for world queries a game's own accessor helpers need
    /// (`driver.engine().world()`).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Mutable access to the underlying engine — for `world_mut()` queries, or a raw
    /// `Command::execute` call this driver has no shorthand for.
    pub fn engine_mut(&mut self) -> &mut Engine {
        &mut self.engine
    }

    /// The driver's current frame cursor (ticks elapsed since construction).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Steps the engine `ticks` fixed ticks, advancing the frame cursor by the same amount.
    ///
    /// # Panics
    /// If the `sim.step` command fails.
    pub fn step(&mut self, ticks: u64) {
        self.engine
            .execute(Command::SimStep(command::SimStepParams {
                ticks,
                hash: false,
            }))
            .expect("sim steps");
        self.frame += ticks;
    }

    /// Injects one keyboard event (`{"kind": kind, "key": key}`, e.g. `("key_down", "Right")`) at
    /// the current frame cursor.
    ///
    /// # Panics
    /// If the `input.inject` command fails.
    pub fn inject(&mut self, kind: &str, key: &str) {
        self.inject_value(json!({"kind": kind, "key": key}));
    }

    /// Injects an already-serialized input event at the current frame cursor — the seam a game with
    /// mouse/gamepad events builds its own `mouse_move`/`set_axis`-style helpers on top of, by
    /// serializing its own `InputEvent` and handing the resulting [`Value`] here (mirrors
    /// [`inject`](GameplayDriver::inject), which is just this for the keyboard-event shape).
    ///
    /// # Panics
    /// If the `input.inject` command fails.
    pub fn inject_value(&mut self, event: Value) {
        self.engine
            .execute(Command::InputInject(command::InputInjectParams {
                frame: Some(FrameIndex::new(self.frame)),
                event,
            }))
            .expect("input injects");
    }

    /// Presses `key` down and steps `ticks` ticks with it held. Does not release — call
    /// [`release`](GameplayDriver::release) after, if the script needs to let go.
    pub fn hold(&mut self, key: &str, ticks: u64) {
        self.inject("key_down", key);
        self.step(ticks);
    }

    /// Releases `key` (no step — the caller's next [`step`](GameplayDriver::step) or
    /// [`tap`](GameplayDriver::tap) carries it forward).
    pub fn release(&mut self, key: &str) {
        self.inject("key_up", key);
    }

    /// One full key tap: down for a tick, up for a tick — exactly one `was_pressed` edge.
    pub fn tap(&mut self, key: &str) {
        self.inject("key_down", key);
        self.step(1);
        self.inject("key_up", key);
        self.step(1);
    }
}
