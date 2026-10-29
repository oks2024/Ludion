#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-ecs` — the custom, milestone-minimal ECS at the heart of the engine.
//!
//! This crate is deliberately small and regular (architecture doc §4 freeze list): generational
//! entities, per-component sorted dense-column storage, typed queries, a
//! component registry (name + JSON Schema + erased serde), a plain `Parent` hierarchy component
//! (child lists derived, never stored), sequential system scheduling, and typed resources. There
//! are no archetypes, no change detection, **no parallel scheduler**, and **no `unsafe`** — every
//! storage is two parallel `Vec`s sorted by entity index (keys, values) plus a slot table from
//! entity index to row.
//!
//! The one sanctioned parallelism is *within* a single system: [`World::par_for_each`] and
//! [`World::par_join_fold`] chunk one query's rows across [`ludion_core::par`] workers with
//! bit-identical results at any thread count (see [`par_query`] for the region model). Systems
//! themselves still run one at a time, in registration order.
//!
//! # Determinism contracts (enforced, not aspirational)
//!
//! - **Free-list order is LIFO** — the most recently despawned slot is reused first
//!   ([`World::spawn`]/[`World::despawn`]).
//! - **Storage and query iteration is ascending entity index** ([`World::entities`],
//!   [`World::query`]).
//! - **Registry iteration is registration order** — the canonical key order for scenes and the
//!   world hash ([`Registry::iter`]).
//! - **Resources are accessed by type, never iterated**, so `TypeId` ordering is never observable
//!   ([`World`] resource methods).
//! - **Parallel iteration is thread-count-invariant** — [`World::par_for_each`] joins sequentially,
//!   partitions on fixed row boundaries, and concatenates results in chunk order, so it is
//!   `to_bits`-identical at 1, 2, or 8 threads ([`par_query`]).
//!
//! Every method that takes an [`EntityId`](ludion_core::ids::EntityId) rejects a stale handle with
//! an `L0002` diagnostic instead of silently hitting a recycled entity.
//!
//! # Components
//!
//! A [`Component`] is any type that is `Send + Sync + 'static` and derives `Serialize`,
//! `Deserialize`, and `schemars::JsonSchema`. The blanket impl means you never write `impl Component`
//! — deriving the three traits is enough.
//!
//! # Example
//!
//! ```
//! use ludion_ecs::{Registry, Schedule, SystemCtx, World};
//! use ludion_core::time::{FrameIndex, TickRate};
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
//! struct Position { x: f32 }
//! #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
//! struct Velocity { dx: f32 }
//!
//! fn integrate(world: &mut World, ctx: &SystemCtx) {
//!     for (pos, vel) in world.query::<(&mut Position, &Velocity)>() {
//!         pos.x += vel.dx * ctx.dt;
//!     }
//! }
//!
//! let mut world = World::new();
//! let e = world.spawn();
//! world.insert(e, Position { x: 0.0 }).unwrap();
//! world.insert(e, Velocity { dx: 60.0 }).unwrap();
//!
//! let mut schedule = Schedule::new();
//! schedule.add_system("integrate", integrate);
//! let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);
//! schedule.run(&mut world, &ctx);
//! assert!((world.get::<Position>(e).unwrap().unwrap().x - 1.0).abs() < 1e-6);
//! ```

use serde::Serialize;
use serde::de::DeserializeOwned;

mod asset_path;
mod entity;
mod entity_ref;
mod hierarchy;
pub mod par_query;
mod query;
mod registry;
mod schedule;
mod storage;
mod world;

pub use asset_path::{ASSET_PATH_KEYWORD, AssetPathSchema, asset_path_fields, for_each_asset_path};
pub use entity::{AllocatorState, MAX_ENTITY_SLOTS, PRESENTATION_BASE, Phase, is_presentation};
pub use entity_ref::{
    ENTITY_REF_KEYWORD, EntityRefField, EntityRefSchema, EntityRefSelfSchema, entity_ref_fields,
};
pub use hierarchy::{ChildMap, Parent};
pub use par_query::ParCtx;
pub use query::QueryElemShape;
pub use registry::{ComponentInfo, Registry, SemanticValidate};
pub use schedule::payload_text as panic_payload_text;
pub use schedule::{Schedule, ScheduleObserver, SystemCtx, SystemFn, SystemPanic};
pub use world::{QueryObservations, World};

// The query traits and join iterators are `#[doc(hidden)]` implementation detail, but must be
// nameable because they appear in `World::query`'s signature and its return type.
#[doc(hidden)]
pub use query::{
    Column, IndexedJoin, Join1, Join2, Join3, Join4, MutCol, Query, QueryIds, QueryParam, RefCol,
};
// Likewise for the parallel join: `par_for_each`/`par_join_fold` name these in their signatures.
#[doc(hidden)]
pub use par_query::{
    ParParam, ParQuery, ParReadQuery, ParRows, ParTask1, ParTask2, ParTask3, ParTask4,
};
#[doc(hidden)]
pub use storage::AnyStorage;

/// A type that can live on an entity as a component.
///
/// This is a marker with a blanket impl: any `Send + Sync + 'static` type that is serde-serializable
/// and has a JSON Schema is automatically a `Component`. You never implement this by hand — derive
/// `Serialize`, `Deserialize`, and `schemars::JsonSchema` on your type and it qualifies.
///
/// The bounds exist because every component must round-trip through the scene format, snapshots, and
/// the wire protocol, and must publish a schema for validation and the inspector.
pub trait Component:
    'static + Send + Sync + Serialize + DeserializeOwned + schemars::JsonSchema
{
}

impl<T> Component for T where
    T: 'static + Send + Sync + Serialize + DeserializeOwned + schemars::JsonSchema
{
}
