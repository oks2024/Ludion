#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-sim` — the fixed-timestep tick loop and the engine's determinism harness.
//!
//! This crate is the credibility core: everything the engine later claims (replay, golden tests,
//! snapshots, editor parity) rests on the guarantee proven here — **the same initial world, seed,
//! and input script produce bit-identical simulation state, tick for tick, on every platform.**
//!
//! Four pieces, one per module:
//!
//! - [`SimDriver`] ([`driver`]) — owns the [`World`](ludion_ecs::World) outright (no
//!   `Arc<Mutex<_>>`, ever) and advances it one fixed tick at a time. Time enters systems only as
//!   `SimTime`/`FrameIndex` parameters; the crate contains no wall clock (the clippy denylist
//!   makes one a build error). Wall-clock pacing of a live run belongs to the embedder.
//! - [`InputScript`] / [`InputState`] ([`input`]) — input as serializable data addressed to frame
//!   indices ("frame 10: key `W` down"), applied at tick boundaries. Scripted gameplay tests and
//!   `ludion replay` are this, verbatim.
//! - [`RunState`] / [`Speed`] ([`run_state`]) — run/pause/step-N/set-speed as explicit driver
//!   state. Speed scales wall-clock pacing only; it never changes `dt` or what a tick computes.
//! - [`WorldHash`] ([`hash`]) — a stable 64-bit digest of the world's entities and component data
//!   under byte-exact canonical rules. Same state ⇒ same hash across processes and platforms;
//!   it is the agent's first-class verification primitive (`ludion sim-hash`, `sim.hash`).
//!
//! # Example: a scripted, verifiable run
//!
//! ```
//! use ludion_core::time::FrameIndex;
//! use ludion_ecs::{Registry, Schedule, SystemCtx, World};
//! use ludion_sim::{InputEvent, InputScript, InputState, Key, SimConfig, SimDriver, WorldHash};
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, JsonSchema, Clone)]
//! struct Paddle { x: f32 }
//!
//! /// Move the paddle right while `D` is held.
//! fn steer(world: &mut World, ctx: &SystemCtx) {
//!     let right = world.resource::<InputState>().unwrap().is_held(Key::D);
//!     if right {
//!         for (paddle,) in world.query::<(&mut Paddle,)>() {
//!             paddle.x += 4.0 * ctx.dt;
//!         }
//!     }
//! }
//!
//! let mut registry = Registry::new();
//! registry.register::<Paddle>("Paddle").unwrap();
//!
//! let mut world = World::new();
//! let paddle = world.spawn();
//! world.insert(paddle, Paddle { x: 0.0 }).unwrap();
//!
//! let mut schedule = Schedule::new();
//! schedule.add_system("steer", steer);
//!
//! // "Frame 0: press D" — input is data, not OS events.
//! let mut input = InputScript::new();
//! input.push(FrameIndex::new(0), InputEvent::KeyDown { key: Key::D });
//!
//! let config = SimConfig { seed: 7, input, ..SimConfig::default() };
//! let mut driver = SimDriver::new(world, schedule, config);
//! driver.run_ticks(30); // headless: as fast as possible, wall clock uninvolved
//!
//! // 30 ticks × 4.0/60 per tick = 2.0 units of travel.
//! let x = driver.world().get::<Paddle>(paddle).unwrap().unwrap().x;
//! assert!((x - 2.0).abs() < 1e-4);
//!
//! // The same state always digests to the same hash — the verification primitive.
//! let hash = WorldHash::compute(driver.world(), &registry).unwrap();
//! assert_eq!(hash, WorldHash::compute(driver.world(), &registry).unwrap());
//! ```

pub mod actions;
pub mod driver;
pub mod hash;
pub mod input;
pub mod rebind;
pub mod run_state;

pub use actions::{
    ActionMap, ActionName, ActionState, Actions, InputActionsDoc, InputBindingsDoc,
    unknown_action_diagnostic,
};
pub use driver::{DriverRestore, PastFrameInject, SimConfig, SimDriver};
pub use hash::{HashReport, NonFinite, WorldHash, non_finite_count};
pub use input::{
    Action, Device, DeviceFacts, GamepadAxis, GamepadButton, GamepadFamily, GamepadState,
    InputEvent, InputScript, InputState, Key, MOUSE_DEVICE_THRESHOLD, MouseButton, MouseState,
    STICK_DEADZONE, ScriptedInput, TRIGGER_THRESHOLD,
};
pub use run_state::{RunState, Speed};
