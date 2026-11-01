#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-core` — the foundational, dependency-light layer every sim-side crate builds on.
//!
//! This crate defines the small, orthogonal primitives that must mean the same thing everywhere
//! in the engine: entity identity, deterministic time, a seeded RNG, order-stable collections,
//! deterministic transcendental math, deterministic data parallelism, and the diagnostic (`L####`)
//! system. It knows nothing about ECS storage, YAML, wgpu, or the wire protocol — those crates
//! depend on `ludion-core`, never the other way around (see `engine-architecture.md` §2).
//!
//! Everything here obeys the determinism rules of §5: no `std` `HashMap`/`HashSet`, no wall clock,
//! one seeded RNG, transcendentals through pure-Rust `libm`. These rules are enforced mechanically
//! by the strict `clippy.toml` denylist shipped in this crate, not by convention.
//!
//! # Example
//!
//! ```
//! use ludion_core::ids::EntityId;
//! use ludion_core::rng::DeterministicRng;
//! use ludion_core::time::{FrameIndex, SimTime, TickRate};
//!
//! let player = EntityId::new(42, 3);
//! assert_eq!(player.to_string(), "e42v3");
//!
//! let mut rng = DeterministicRng::new(1);
//! let _roll = rng.gen_range_u32(1..7);
//!
//! let frame = FrameIndex::ZERO.next();
//! let now = SimTime::from_ticks(frame.as_u64(), TickRate::DEFAULT);
//! assert_eq!(now.ticks(), 1);
//! ```

pub mod atomic;
pub mod audio;
pub mod bless;
pub mod codes;
pub mod collections;
pub mod color;
pub mod diagnostics;
pub mod ids;
pub mod letterbox;
pub mod math;
pub mod number;
pub mod panic_capture;
pub mod par;
pub mod paths;
pub mod require_gpu;
pub mod rng;
pub mod suggest;
pub mod text;
pub mod time;
