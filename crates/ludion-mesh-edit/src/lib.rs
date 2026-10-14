#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-mesh-edit` — deliberate, replayable edits to a mesh's texture maps.
//!
//! A generated or imported mesh is bought surface: its geometry and its maps arrive finished, and
//! the one thing an author must still be able to say is **what glows**. This crate is how a
//! `kind: mesh-edit` document — an ordered list of paint ops, each a colour and a 3D region — turns
//! into a derived copy of that mesh: the same geometry, one new content-hash-named PNG per edited
//! map, and every untouched map referenced where it already lives. The source mesh and its
//! committed maps are never rewritten, so one can always go back to the asset as it arrived and
//! branch another version from it.
//!
//! Six modules, in pipeline order:
//!
//! 1. [`region`] — the signed distance of a `sphere`/`capsule`/`box`, and the weight its `feather`
//!    turns that distance into.
//! 2. [`raster`] — which texel of a map the surface reaches, and where on the surface.
//! 3. [`fill`] — a `fill` region's seeded flood over the surface, across UV seams.
//! 4. [`paint`] — one op blended into a map, in linear light, through a region's optional `key`
//!    colour filter.
//! 5. [`ray`] — the geometry half of a pick: one ray against the triangle list.
//! 6. [`derive()`] — the whole derived tree, written or byte-compared.
//!
//! # Verifying an edit without looking at the screen
//!
//! Every derive carries a [`Report`]: per op the texels painted, their share of the map, the
//! model-space box of the surface they sit on and how many triangles it spans; per written map the
//! non-black texel count. An op whose region misses the surface is an **error** (`L1177`), not a
//! quiet no-op, because that is the failure a blind author is most likely to make. And a map is
//! only ever the glow's shape and colour — its intensity is the scene's `Mesh.emissive`, which is
//! what every report's `next` line says.
//!
//! # Bit-reproducibility
//!
//! A derived PNG is a **committed** artifact that CI regenerates on Windows x86-64 and macOS arm64
//! and byte-compares. Texel math is therefore `f64`, rounded to bytes exactly once after the last
//! op; every irrational operation goes through [`ludion_core::math`]; a texel covered by more than
//! one UV triangle resolves to the lower triangle index; and every path a derived document carries
//! is computed lexically rather than through the filesystem.
//!
//! # Example
//!
//! ```
//! // One triangle in the y = 0 plane, its UVs filling the map.
//! let mesh = "\
//! format: 1
//! kind: mesh
//! positions:
//!   - 0.0
//!   - 0.0
//!   - 0.0
//!   - 1.0
//!   - 0.0
//!   - 0.0
//!   - 0.0
//!   - 0.0
//!   - 1.0
//! uvs:
//!   - 0.0
//!   - 0.0
//!   - 1.0
//!   - 0.0
//!   - 0.0
//!   - 1.0
//! indices:
//!   - 0
//!   - 1
//!   - 2
//! ";
//! // A green blob around one corner of it, on an emissive map the mesh does not have yet.
//! let edit = "\
//! format: 1
//! kind: mesh-edit
//! source: plate.mesh.yaml
//! ops:
//!   - op: paint
//!     map: emissive
//!     size: 32
//!     color:
//!       - 0.0
//!       - 1.0
//!       - 0.0
//!     region:
//!       sphere:
//!         at:
//!           - 0.0
//!           - 0.0
//!           - 0.0
//!         radius: 0.25
//! ";
//!
//! let dir = std::env::temp_dir().join(format!("ludion-mesh-edit-{}", std::process::id()));
//! std::fs::create_dir_all(&dir)?;
//! std::fs::write(dir.join("plate.mesh.yaml"), mesh)?;
//! std::fs::write(dir.join("lit.mesh-edit.yaml"), edit)?;
//!
//! let derived = ludion_mesh_edit::derive(&dir.join("lit.mesh-edit.yaml")).expect("it derives");
//! let report = derived.report();
//! assert!(report.ops[0].texels > 0, "the region reached the surface");
//! assert_eq!(report.maps[0].map, "emissive");
//!
//! derived.write().expect("the tree is written");
//! assert!(derived.check().is_empty(), "a freshly written tree never drifts");
//!
//! std::fs::remove_dir_all(&dir)?;
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod derive;
pub mod fill;
pub mod paint;
pub mod raster;
pub mod ray;
pub mod region;

pub use derive::{
    DERIVED_DOCUMENT, DERIVED_TEXTURE_DIR, Derived, MapReport, OpReport, Report, derive,
};
pub use paint::{DILATE_PAD, WorkingMap};
pub use raster::Coverage;
pub use ray::{RayHit, cast};
pub use region::Aabb;
