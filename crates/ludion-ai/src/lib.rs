//! Agent behaviour for the Ludion engine: a hierarchical state machine over a game enum,
//! gameplay-time timers, and squads.
//!
//! A game implements [`Machine`] for an enum (its variants are the states, its `parent` fn the
//! hierarchy, its callbacks plain Rust), registers it with [`register_machine`], and schedules
//! [`ai_tick`] once. Each agent carries a [`Mind`]; [`Timer`] and [`Cooldown`] count whole gameplay frames;
//! [`Group`] and [`GroupMember`] make squads whose members can be sent events with [`send`] or
//! [`Cx::broadcast`]. `ludion docs ai` is the guide.
//!
//! # Boundary
//!
//! Sim tier: no `HashMap`, no wall clock, no threads; time is `ctx.game_time`, so everything holds
//! under a paused game. Only [`Mind`], [`Group`] and [`GroupMember`] (and the game's blackboard) are
//! components, and nothing registers until a game calls [`register_machine`]. Runtime faults queue in
//! [`AiFaults`] for the engine to report; this crate cannot see its `Diagnostics`.
//!
//! # Example
//!
//! ```
//! use ludion_ai::{Cx, Machine, Mind, Step, ai_tick, register_machine};
//! use ludion_core::time::{FrameIndex, TickRate};
//! use ludion_ecs::{Registry, SystemCtx, World};
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
//! enum Lamp { Root, Off, On }
//!
//! impl Machine for Lamp {
//!     type Blackboard = ();
//!     type Event = ();
//!     const NAME: &'static str = "Lamp";
//!     const ROOT: Self = Lamp::Root;
//!     const ALL: &'static [Self] = &[Lamp::Root, Lamp::Off, Lamp::On];
//!     fn parent(s: Self) -> Option<Self> { (s != Lamp::Root).then_some(Lamp::Root) }
//!     fn initial(s: Self) -> Option<Self> { (s == Lamp::Root).then_some(Lamp::Off) }
//!     fn update(cx: &mut Cx<'_, Self>, s: Self) -> Step<Self> {
//!         if s == Lamp::Off && cx.time_in(Lamp::Off) >= 1.0 { Step::Go(Lamp::On) } else { Step::Stay }
//!     }
//! }
//!
//! let mut registry = Registry::new();
//! register_machine::<Lamp>(&mut registry).unwrap();
//! let mut world = World::new();
//! let lamp = world.spawn();
//! world.insert(lamp, Mind::<Lamp>::default()).unwrap();
//! for frame in 0..=60 {
//!     ai_tick::<Lamp>(&mut world, &SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT));
//! }
//! assert_eq!(world.component::<Mind<Lamp>>(lamp).unwrap().state, Lamp::On);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod faults;
mod group;
mod machine;
mod mind;
mod tick;
mod timer;

pub use faults::{AiFault, AiFaults};
pub use group::{Group, GroupMember, members};
pub use machine::{Cx, Machine, Step};
pub use mind::{Mind, active_path, register_machine, send};
pub use tick::{MAX_TRANSITIONS_PER_TICK, ai_tick};
pub use timer::{Cooldown, Timer};
