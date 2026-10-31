//! [`GameplayDriver`] and the free [`step`]/[`inject`] — headless gameplay tests over any [`Engine`]:
//! one built from a scene file ([`GameplayDriver::from_scene_file`]), from an inline scene
//! ([`GameplayDriver::new`], [`loaded_engine`]), or one the test already built and stepped
//! ([`GameplayDriver::from_engine`]). The driver keeps a frame cursor so scripted presses land on the
//! right tick, and adds typed input, an engine-driven [`run_until`](GameplayDriver::run_until), and
//! `snapshot`/`restore`. A test that keeps a bare [`Engine`] uses the free [`step`] and [`inject`].
//!
//! # A note on this crate's dependency edge
//!
//! Like [`crate::sim_assert`], this module reaches for a type
//! ([`Command`]/[`command`]) that `ludion-engine`'s public surface
//! does not re-export (`Engine::execute` takes `ludion_protocol::Command` directly). Depending on
//! `ludion-protocol` directly adds no cycle risk (`ludion-engine` already depends on it) and keeps
//! this a thin wrapper.

use ludion_core::time::FrameIndex;
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::{Command, ResponseBody, command};
use ludion_sim::{GamepadAxis, InputEvent};
use serde_json::{Value, json};
use std::path::Path;

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

/// Steps `engine` `ticks` fixed ticks.
///
/// # Panics
/// If the `sim.step` command fails.
///
/// # Example
/// ```
/// use ludion_engine::EngineBuilder;
///
/// let mut engine = ludion_testkit::loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n");
/// ludion_testkit::step(&mut engine, 3);
/// assert_eq!(engine.frame().as_u64(), 3);
/// ```
pub fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks,
            hash: false,
        }))
        .expect("sim steps");
}

/// Injects `event` into `engine` for the next tick (`frame: None`).
///
/// # Panics
/// If the `input.inject` command fails.
///
/// # Example
/// ```
/// use ludion_engine::EngineBuilder;
/// use ludion_sim::{InputEvent, Key};
///
/// let mut engine = ludion_testkit::loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n");
/// ludion_testkit::inject(&mut engine, InputEvent::KeyDown { key: Key::Right });
/// ludion_testkit::step(&mut engine, 1);
/// ```
pub fn inject(engine: &mut Engine, event: InputEvent) {
    engine
        .execute(Command::InputInject(command::InputInjectParams {
            frame: None,
            event: serde_json::to_value(event).expect("an InputEvent always serializes"),
        }))
        .expect("input injects");
}

/// A scripted-input driver over any [`Engine`]: tracks a frame cursor so a test can
/// `hold`/`release`/`tap` keys, send typed [`InputEvent`]s, run until a predicate holds, and
/// `snapshot`/`restore`, without re-deriving which frame each input lands on.
///
/// The cursor is the engine frame the next `inject` targets (read from the engine on construction
/// and after `restore`), not "ticks since construction". Reach the engine through
/// [`engine`](GameplayDriver::engine) / [`engine_mut`](GameplayDriver::engine_mut).
pub struct GameplayDriver {
    engine: Engine,
    frame: u64,
}

impl GameplayDriver {
    /// Builds `builder` and loads `scene` inline (via [`loaded_engine`]).
    ///
    /// # Panics
    /// Same conditions as [`loaded_engine`].
    ///
    /// # Example
    /// ```
    /// use ludion_engine::EngineBuilder;
    /// use ludion_testkit::GameplayDriver;
    ///
    /// let mut driver = GameplayDriver::new(EngineBuilder::new(), "format: 1\nentities: []\n");
    /// driver.step(2);
    /// assert_eq!(driver.frame(), 2);
    /// ```
    pub fn new(builder: EngineBuilder, scene: &str) -> Self {
        Self::from_engine(loaded_engine(builder, scene))
    }

    /// Builds `builder` and loads the scene *file* at `path`; the project roots at the scene's
    /// directory, so data-docs resolve against it.
    ///
    /// # Panics
    /// If the engine fails to build or the scene fails to load (the diagnostic's code and message
    /// are in the panic).
    ///
    /// # Example
    /// ```no_run
    /// use ludion_engine::EngineBuilder;
    /// use ludion_testkit::GameplayDriver;
    ///
    /// let mut driver =
    ///     GameplayDriver::from_scene_file(EngineBuilder::new(), "scenes/main.yaml".as_ref());
    /// driver.step(1);
    /// ```
    pub fn from_scene_file(builder: EngineBuilder, path: &Path) -> Self {
        match builder.load_scene_file(path) {
            Ok(engine) => Self::from_engine(engine),
            Err(d) => panic!(
                "the scene file {} loads: {} {}",
                path.display(),
                d.code,
                d.message
            ),
        }
    }

    /// Wraps an engine the test already built (and possibly stepped); the cursor starts at its frame.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::EngineBuilder;
    /// use ludion_testkit::GameplayDriver;
    ///
    /// let mut engine = ludion_testkit::loaded_engine(EngineBuilder::new(), "format: 1\nentities: []\n");
    /// ludion_testkit::step(&mut engine, 4);
    /// assert_eq!(GameplayDriver::from_engine(engine).frame(), 4);
    /// ```
    pub fn from_engine(engine: Engine) -> Self {
        let frame = engine.frame().as_u64();
        GameplayDriver { engine, frame }
    }

    /// Gives the engine back, e.g. to hand it to a golden check.
    pub fn into_engine(self) -> Engine {
        self.engine
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

    /// The frame cursor: the engine frame the next `inject` targets.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Steps the engine `ticks` fixed ticks, advancing the frame cursor by the same amount.
    ///
    /// # Panics
    /// If the `sim.step` command fails.
    pub fn step(&mut self, ticks: u64) {
        step(&mut self.engine, ticks);
        self.frame += ticks;
    }

    /// Steps one tick at a time, up to `max_ticks`, and returns the cursor after the first tick where
    /// `predicate` holds, or `None` if it never does. A system panic surfaces as the `step` panic.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::EngineBuilder;
    /// use ludion_testkit::GameplayDriver;
    ///
    /// let mut driver = GameplayDriver::new(EngineBuilder::new(), "format: 1\nentities: []\n");
    /// assert_eq!(driver.run_until(10, |e| e.frame().as_u64() >= 5), Some(5));
    /// assert_eq!(driver.run_until(3, |_| false), None);
    /// ```
    pub fn run_until(
        &mut self,
        max_ticks: u64,
        mut predicate: impl FnMut(&mut Engine) -> bool,
    ) -> Option<u64> {
        for _ in 0..max_ticks {
            self.step(1);
            if predicate(&mut self.engine) {
                return Some(self.frame);
            }
        }
        None
    }

    /// The engine's snapshot document, what `engine.snapshot` answers.
    ///
    /// # Panics
    /// If the command fails or answers another body.
    pub fn snapshot(&mut self) -> Value {
        match self
            .engine
            .execute(Command::EngineSnapshot)
            .expect("engine snapshots")
            .body
        {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    /// Restores `snapshot` into this engine and re-syncs the frame cursor to the restored frame.
    ///
    /// # Panics
    /// If the command fails or answers another body.
    pub fn restore(&mut self, snapshot: Value) {
        let params = command::EngineRestoreParams { snapshot };
        match self
            .engine
            .execute(Command::EngineRestore(params))
            .expect("engine restores")
            .body
        {
            ResponseBody::Restored { .. } => {}
            other => panic!("expected a restore, got {other:?}"),
        }
        self.frame = self.engine.frame().as_u64();
    }

    /// Injects one keyboard event (`{"kind": kind, "key": key}`, e.g. `("key_down", "Right")`) at
    /// the current frame cursor.
    ///
    /// # Panics
    /// If the `input.inject` command fails.
    pub fn inject(&mut self, kind: &str, key: &str) {
        self.inject_value(json!({"kind": kind, "key": key}));
    }

    /// Injects a typed [`InputEvent`] at the current frame cursor; analog fields serialize as their
    /// IEEE-754 bits, so they replay bit-exact.
    ///
    /// # Panics
    /// If the `input.inject` command fails.
    pub fn inject_event(&mut self, event: InputEvent) {
        self.inject_value(serde_json::to_value(event).expect("an InputEvent always serializes"));
    }

    /// Sets a gamepad axis to `value` at the current frame cursor.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::EngineBuilder;
    /// use ludion_sim::GamepadAxis;
    /// use ludion_testkit::GameplayDriver;
    ///
    /// let mut driver = GameplayDriver::new(EngineBuilder::new(), "format: 1\nentities: []\n");
    /// driver.set_axis(GamepadAxis::LeftStickX, 0.5);
    /// driver.step(1);
    /// ```
    pub fn set_axis(&mut self, axis: GamepadAxis, value: f32) {
        self.inject_event(InputEvent::GamepadAxis { axis, value });
    }

    /// Injects an already-serialized input event at the current frame cursor — the raw seam under
    /// [`inject_event`](GameplayDriver::inject_event) and [`inject`](GameplayDriver::inject).
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
