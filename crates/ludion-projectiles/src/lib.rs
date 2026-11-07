#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-projectiles` — a fixed-capacity pool of 3D projectiles and the swept step that moves it.
//!
//! [`ProjectilePool`] stores projectiles as structure-of-arrays columns addressed by a generational
//! [`ProjectileId`]. [`step`] integrates the pool for one tick, ages it, sweeps each projectile's
//! segment against the [`PhysicsWorld`](ludion_physics::PhysicsWorld) (`first_hit`) and a
//! [`spatial3d`](ludion_spatial::spatial3d) target index (`cast_skipping`), takes the nearer hit and
//! resolves the projectile's [`Response`] over the time left in the tick. It reports what happened
//! as typed events in [`StepEvents`]. [`ProjectilePool::digest`] is a compact hash of the pool for
//! the engine's world hash.
//!
//! # Boundary
//!
//! Sim-side and GPU-free. The pool is plain data: no ECS storage, no engine, no rendering. Homing,
//! splash and damage belong to the game, which reads [`HitEvent`]s and calls
//! [`ProjectilePool::set_velocity`] / [`ProjectilePool::despawn`]; `tag` is opaque to this crate.
//!
//! # Hit responses
//!
//! - **Despawn** frees the projectile at the contact.
//! - **Reflect** mirrors the velocity about the contact normal (scaled by `restitution`), puts the
//!   centre at the contact plus [`SKIN`] along the normal (scaled up with the position's magnitude so it
//!   survives rounding far from the origin), and never excludes the surface by handle.
//!   A world collider can be a whole room (one trimesh), so excluding it would let the projectile
//!   leave through every other wall; the push-out and the outward velocity keep the next cast off
//!   the surface instead.
//! - **Pierce** continues through *index targets only*, remembering the last [`PIERCE_MEMORY`]
//!   handles it passed so overlapping targets are not re-hit within a sweep or across ticks; physics
//!   hits on a pierced entity are skipped too. Any other physics (world) hit despawns a piercing
//!   projectile.
//! - **Stop** parks the projectile at the contact: live, motionless, still aging.
//!
//! # Determinism
//!
//! The step uses only `+ - * /` (no transcendental, no `sqrt`), integrates semi-implicitly
//! (gravity, then drag as `v *= 1 / (1 + drag·dt)`, then the sweep), fans the per-projectile work
//! out over [`ludion_core::par`] in fixed chunks and applies results serially in ascending slot
//! order. Results, events and ids are therefore identical at any thread count. After warm-up a step at
//! one thread (`LUDION_THREADS=1`) allocates nothing; at more threads `par` spawns scoped workers each
//! tick, which allocate.
//!
//! ```
//! use ludion_core::time::TickRate;
//! use ludion_physics::{PhysicsWorld, QueryFilter};
//! use ludion_projectiles::{ProjectileDesc, ProjectilePool, StepConfig, StepEvents, step};
//! use ludion_spatial::spatial3d::Index3d;
//!
//! let physics = PhysicsWorld::new(TickRate::DEFAULT);
//! let mut index = Index3d::new();
//! index.build();
//! let mut pool = ProjectilePool::with_capacity(8);
//! let id = pool.spawn(ProjectileDesc::new([0.0; 3], [60.0, 0.0, 0.0])).unwrap();
//! let cfg = StepConfig::new(physics.sweep_filter(&QueryFilter::new()).unwrap());
//! let mut events = StepEvents::with_capacity(8, 8);
//! step(&mut pool, 1.0 / 60.0, &physics, &index, &cfg, &mut events);
//! assert_eq!(pool.position(id), Some([1.0, 0.0, 0.0]));
//! ```

mod codec;
mod hash;
mod pool;
mod step;

pub use codec::{DecodeError, MAX_DECODE_CAPACITY};
pub use pool::{PIERCE_MEMORY, ProjectileDesc, ProjectileId, ProjectilePool, Response, SpawnError};
pub use step::{
    Expired, HitEvent, HitOutcome, MAX_HITS_PER_TICK, SKIN, StepConfig, StepEvents, step,
};
