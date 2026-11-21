//! Navigation for the Ludion engine: a layered 2.5D walkability grid and shared, bounded flow fields.
//!
//! A [`ColumnSource`] reports the floors above each grid column centre; [`NavGrid`] turns them into
//! nodes (several per column for bridges and overhangs) and links them under a height, step and slope
//! rule. A [`FlowField`] is a radius-capped Dijkstra from one target node plus a unit flow direction per
//! node; a [`FlowCache`] keeps one field per target node, so every agent chasing the same cell shares it.
//! [`find_path`] is point-to-point A* over the same links, with [`string_pull`] and [`walkable_line`] cutting a
//! node path to waypoints; [`find_path_linked`] also crosses [`NavLinks`], the off-mesh edges (ladders, vaults).
//!
//! # Boundary
//!
//! Pure and GPU-free: this crate knows neither physics nor the ECS (a source adapts either). Everything
//! here is derived state, a pure function of the samples, the parameters and the target; there is no
//! serde and no hashing, so none of it enters `WorldHash` or a snapshot. Sim tier: no `HashMap`, no
//! wall clock, transcendentals through `ludion_core::math`.
//!
//! # Example
//!
//! ```
//! use ludion_nav::{ColumnSource, FlowField, FloorSample, GridParams, NavGrid};
//!
//! struct Flat;
//! impl ColumnSource for Flat {
//!     fn column(&self, _i: u32, _j: u32, _center: [f32; 2], out: &mut Vec<FloorSample>) {
//!         out.push(FloorSample { y: 0.0, normal: [0.0, 1.0, 0.0], headroom: f32::INFINITY });
//!     }
//! }
//!
//! let params = GridParams {
//!     origin: [0.0, 0.0],
//!     cell_size: 1.0,
//!     nx: 4,
//!     nz: 4,
//!     agent_height: 1.8,
//!     max_step: 0.3,
//!     max_slope_deg: 45.0,
//! };
//! let grid = NavGrid::build(params, &Flat);
//! let target = grid.node_at([0.5, 0.0, 0.5]).unwrap();
//! let mut field = FlowField::default();
//! field.compute(&grid, target, f32::INFINITY);
//! let dir = field.flow_at(&grid, [3.5, 0.0, 3.5]).unwrap();
//! assert!(dir[0] < 0.0 && dir[1] < 0.0, "flows toward the target corner");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cache;
mod field;
mod grid;
mod links;
mod path;

pub use cache::FlowCache;
pub use field::FlowField;
pub use grid::{ColumnSource, FloorSample, GridParams, NEIGHBORS, NO_LINK, NavGrid, NodeId};
pub use links::{LinkEdge, NavLinks, OffMeshLink};
pub use path::{
    PathOptions, PathResult, PathStatus, find_path, find_path_linked, line_exit, nearest_node,
    reachable_within, string_pull, string_pull_linked, walkable_line,
};
