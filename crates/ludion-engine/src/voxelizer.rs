//! `voxelize()` — the scene's placed geometry → a world-space [`WorldVoxelGrid`].
//!
//! The global-illumination arc's foundation (`Plan/gi-wave0-voxelizer.md`): a pure CPU
//! rasterization of every placed mesh instance into an occupancy / albedo / emissive grid covering
//! the scene's world bounds. **No render pass consumes the grid and no shader binds it** — its
//! product is the input to the GI bake, and a scene that authors no `GiVolume` never runs this code
//! at all.
//!
//! # Why CPU, and why that makes it deterministic
//!
//! Occupancy is an order-independent OR, but albedo and emissive are not: when several model cells
//! or several instances land in one coarser world cell, the surviving value depends on the order
//! they were written in, and GPU float scatter has no such order. So the combine rule is stated
//! rather than emergent:
//!
//! - **occupancy is OR** — any contribution marks the cell filled;
//! - **albedo and emissive are LAST WRITER WINS**, iterating instances in ascending entity-slot
//!   order (exactly the order [`place`](crate::extract::place) walks, which is the `WorldHash`
//!   order) and, within one voxel model, its cells in ascending flat-index order.
//!
//! That rule is what makes the grid a pure function of the world: same scene ⇒ bit-identical
//! buffers, on any machine, at any thread count.
//!
//! # What each source contributes (`Plan/gi-wave0-voxelizer.md` §8)
//!
//! - **Cube** — the object box `[-0.5, 0.5]³` transformed by the instance model; a cell is filled
//!   when its **center** lies inside that box. Exact for the axis-aligned cubes every committed
//!   scene is built from.
//! - **Plane** — the flat object box `[-0.5, 0.5] × {0} × [-0.5, 0.5]`, filled conservatively; for
//!   an unrotated ground plane its transformed box has no thickness, so it fills exactly the one
//!   cell row its world Y lands in.
//! - **Sphere** — the transformed object box filled conservatively. The per-cell ellipsoid test is
//!   not built: spheres are rare in the content that drives this, and a conservative fill only ever
//!   over-reports occupancy.
//! - **Voxel model** — each filled authored cell's object-space center is transformed by the
//!   instance model and deposited into the world cell it lands in, taking the palette's albedo
//!   modulated by the instance color (the same product the mesh shader forms) and whichever
//!   emissive the *render* path would use for that instance: the palette's when the document
//!   authors a material, the `Mesh` component's when it does not ([`deposit_voxels`]).
//! - **Asset mesh** — contributes nothing, and does not stretch the grid's bounds either. Its
//!   triangles are not rasterized, and filling its bounding box instead would mark a hollow
//!   imported building solid, which is a worse answer than its absence.
//!
//! Primitives take their albedo from the instance color and their emissive from the `Mesh`
//! component's `material[2]` — the same two channels the mesh path carries to the shader.
//!
//! # The instance signature, and why rasterization is load-time work
//!
//! Rasterizing a district is a multi-million-cell walk, and `extract` runs every frame. So
//! [`voxelize`] hashes an **`O(instances)` signature** of exactly the inputs it reads — the authored
//! `cell_size`, and per instance in the pinned order its model matrix, color, emissive, and a
//! discriminant for what it rasterizes from — and asks the loader whether that signature already has
//! a grid ([`SceneAssets::voxelization`]). A hit hands back the same `Arc` and touches no cell.
//!
//! The signature covers the inputs [`resolve`] gathers, so it moves if and only if the grid would:
//! a source [`resolve`] discards (an asset mesh, an instance whose transform is not finite)
//! contributes nothing to the grid and nothing to the signature. Voxel models enter it by their
//! document's content hash, so a hot-reloaded model invalidates while an unchanged one does not.
//! Adding a source the rasterizer reads means adding it here too — the two are one contract.
//!
//! The documented consequence: any instance a system animates misses every frame and pays the full
//! rasterization. That is not only the moving case — the signature covers every input, so a tweened
//! color or a pulsing emissive invalidates it exactly as a moved transform does. The `GiDynamic`
//! marker component (`Plan/gi-wave3-dynamic-injection.md` §3) is the answer: [`resolve`] drops a
//! marked instance before it ever reaches `instances`, so its motion cannot reach the signature at
//! all — a game marks every entity a system moves, and that entity's rasterization cost goes to
//! zero regardless of how often it changes.
//!
//! For a world that has *not* marked its movers, a host can hold the baseline instead
//! ([`Engine::set_gi_rebake`](crate::Engine::set_gi_rebake)): on a miss the loader serves whatever
//! grid it retains ([`SceneAssets::held_voxelization`]) and records staleness, leaving the held key
//! in place. The signature is still hashed every frame — it is the staleness comparison — but the
//! per-cell walk below it is not.

use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_renderer::{MAX_GI_TEXTURE_AXIS, MeshShape, WorldVoxelGrid};

use crate::bounds::{Aabb, transformed_box, union};
use crate::extract::{PlacedMesh, PlacedMeshSource, SceneAssets, mesh_error};
use crate::voxel_mesh::{VoxelStaticGrid, grid_box};

/// The ceiling on a GI grid's cell count, above which [`voxelize`] reports `L2103` instead of
/// allocating.
///
/// A guardrail, not a resolution policy: at ~21 bytes per cell (16 albedo + 4 emissive + 1
/// occupancy) this is roughly 0.7 GB of grid, about seventeen times what a shipped district costs at
/// half-unit cells and comfortably above a whole district at quarter-unit cells. Everything it
/// rejects is an authoring mistake — a `cell_size` typed an order of magnitude too small, or a stray
/// entity that stretched the bounds around empty space.
pub(crate) const MAX_GI_CELLS: u64 = 32_000_000;

/// The ceiling on an irradiance probe lattice's probe count, above which [`probe_lattice`] reports
/// `L2105` instead of handing the renderer a volume to allocate.
///
/// A guardrail, not a resolution policy: the cap bounds probe *count* — the thing an authoring typo
/// gets wrong — not a fixed byte total, and the byte cost is per volume. Each `Rgba16Float` probe
/// volume costs 8 bytes per probe; the shipped layout is four of them (sky visibility, plus
/// one-bounce indirect's three colour channels when `bounce` is authored), so 32 B/probe, ~128 MB at
/// the cap. **Plus the bounce bake's hit cache**, which is a buffer rather than a volume: one `u32`
/// per probe for each of the first 16 rays, 64 B/probe — ~30 MB at a district's ~460 k probes, and
/// 256 MB at this cap, a number to state rather than to reach. A shipped district's box at the
/// default one-unit spacing is a few hundred thousand probes, so this leaves an order of magnitude
/// of headroom; everything it rejects is a `probe_spacing` typed an order of magnitude too small.
pub(crate) const MAX_GI_PROBES: u64 = 4_000_000;

/// Builds the world-space voxelization of `meshes` at `cell_size` world units per cell, or hands
/// back the one `assets` already holds for this instance set (see the module doc's signature
/// section).
///
/// Returns `Ok(None)` when the scene places no geometry the voxelizer rasterizes — there is no
/// world box to bound, so there is no grid, and a zero-cell grid would only be a shape every
/// consumer has to special-case.
///
/// # Errors
/// - `L2103` if the scene's bounds at `cell_size` imply more than [`MAX_GI_CELLS`] cells. Checked
///   on the computed extent, before any allocation. A cache hit skips the check because the very
///   same inputs already passed it to produce the cached grid.
/// - `L1101`/`L1113`/`L1114` if a referenced voxel document cannot be read or validated. Extract
///   resolves and reports every such document before it calls this, so the path is defensive.
pub(crate) fn voxelize(
    meshes: &[PlacedMesh<'_>],
    cell_size: f32,
    assets: &mut dyn SceneAssets,
) -> Result<Option<Arc<WorldVoxelGrid>>, Vec<Diagnostic>> {
    let instances = resolve(meshes, assets)?;
    let signature = signature(&instances, cell_size);
    if let Some(cached) = assets.voxelization(signature) {
        return Ok(cached);
    }
    // A held session serves the grid it already has rather than paying the walk for a world whose
    // baseline the host has frozen. The signature above is still computed every frame — that
    // comparison *is* the staleness report, which is why it is never latched: a world that returns
    // to the held baseline hits the line above and is honestly not stale.
    if let Some(held) = assets.held_voxelization() {
        return Ok(held);
    }

    let Some(bounds) = scene_bounds(&instances) else {
        return Ok(assets.set_voxelization(signature, None));
    };
    let mut grid = allocate(bounds, cell_size)?;
    #[cfg(test)]
    RASTERIZATIONS.with(|count| count.set(count.get() + 1));
    for instance in &instances {
        rasterize(&mut grid, instance);
    }
    Ok(assets.set_voxelization(signature, Some(Arc::new(grid.sealed()))))
}

// Test-only. Counts completed rasterizations — the proof a signature hit skips the whole per-cell
// walk rather than merely reproducing the same grid (`voxel_bake.rs`'s `uploads` precedent).
// Thread-local so tests sharing a process cannot see each other's count.
#[cfg(test)]
thread_local! {
    static RASTERIZATIONS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// **Test-only.** How many times the per-cell rasterization walk has actually run on this thread.
#[cfg(test)]
pub(crate) fn rasterization_count() -> u32 {
    RASTERIZATIONS.with(|count| count.get())
}

/// The `O(instances)` cache key: the authored `cell_size`, then every resolved instance in the
/// pinned ascending-entity-slot order, each contributing exactly the values [`rasterize`] reads —
/// its model matrix, its color, its emissive, and a tag for the solid it fills from (a voxel model
/// by its document's content hash, so the model's *contents* are covered without walking them).
///
/// Never `O(cells)`: that is the cost this key exists to avoid paying per frame.
///
/// One deliberate conservatism, which can only cost a needless rebuild and never a stale grid: a
/// voxel model enters by its *document's* hash, so two different documents that happen to build one
/// grid are two keys. That hash also covers whether the document authors a material, which is what
/// picks a voxel model's emissive source — and `emissive` is load-bearing for every instance, since
/// a model whose palette authors none deposits the component's value.
fn signature(instances: &[Instance], cell_size: f32) -> u64 {
    let mut bytes: Vec<u8> = Vec::with_capacity(4 + instances.len() * 96);
    bytes.extend_from_slice(&cell_size.to_le_bytes());
    for instance in instances {
        for v in instance.model.to_cols_array() {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in instance.color {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&instance.emissive.to_le_bytes());
        match &instance.solid {
            Solid::Cube => bytes.push(0),
            Solid::Box { min, max } => {
                bytes.push(1);
                for v in [min.x, min.y, min.z, max.x, max.y, max.z] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            Solid::Voxel { grid, .. } => {
                bytes.push(2);
                bytes.extend_from_slice(&grid.hash.to_le_bytes());
            }
        }
    }
    ludion_assets::content_hash(&bytes)
}

/// One instance's resolved contribution: its world transform, the two shading channels the grid
/// stores, and the object-space geometry to rasterize.
struct Instance {
    /// Local→world transform, verbatim from [`PlacedMesh::model`].
    model: Mat4,
    /// Linear RGBA the instance contributes — for a voxel model this is the *tint* its palette
    /// colors are modulated by, matching `mesh.wgsl`'s `vertex_color * instance_color`.
    color: [f32; 4],
    /// Emissive strength from the `Mesh` component — what a primitive deposits, and what a voxel
    /// model deposits too whenever its document authors no material (the selection `extract` makes
    /// through `MeshInstance::use_vertex_material`).
    emissive: f32,
    /// What to rasterize.
    solid: Solid,
}

/// The object-space geometry an [`Instance`] fills from.
enum Solid {
    /// The unit cube `[-0.5, 0.5]³`, filled exactly by a cell-center-inside test.
    Cube,
    /// An object-space box filled conservatively — every cell its transformed box touches.
    Box {
        /// Minimum corner in object space.
        min: Vec3,
        /// Maximum corner in object space.
        max: Vec3,
    },
    /// An authored voxel model, deposited one filled cell at a time.
    Voxel {
        /// The model's authored occupancy and per-cell palette channels.
        grid: Arc<VoxelStaticGrid>,
        /// Whether the model's document authors a material — the render path's
        /// `MeshData::authored_material`, which decides which emissive [`deposit_voxels`] writes.
        palette_material: bool,
    },
}

impl Solid {
    /// The object-space box this solid occupies — what the scene bounds are unioned from, and the
    /// search region a conservative fill walks.
    fn object_box(&self) -> (Vec3, Vec3) {
        match self {
            Solid::Cube => (Vec3::splat(-0.5), Vec3::splat(0.5)),
            Solid::Box { min, max } => (*min, *max),
            Solid::Voxel { grid, .. } => grid_box(grid.extent),
        }
    }
}

/// Resolves each placed mesh into the geometry the rasterizer needs, keeping the ascending
/// entity-slot order the combine rule depends on. Asset meshes and instances whose transform sends
/// a corner to a non-finite coordinate are dropped: neither has a world box the grid can be sized
/// from, and skipping them is what keeps the extent arithmetic finite (the `build_sun` posture — a
/// degenerate input yields an omission, never a NaN).
fn resolve(
    meshes: &[PlacedMesh<'_>],
    assets: &mut dyn SceneAssets,
) -> Result<Vec<Instance>, Vec<Diagnostic>> {
    let mut instances = Vec::with_capacity(meshes.len());
    let mut errors: Vec<Diagnostic> = Vec::new();
    // One entry per distinct voxel document. A district places the same prop hundreds of times and
    // the one-shot loader holds no cache of its own, so without this every instance re-reads and
    // re-meshes the same file.
    let mut voxels: StableMap<String, (Arc<VoxelStaticGrid>, bool)> = StableMap::new();

    for mesh in meshes {
        // `GiDynamic` (`Plan/gi-wave3-dynamic-injection.md` §3.2): a marked instance neither
        // occludes the probe rays nor deposits albedo/emissive, and its motion must not reach the
        // signature at all. Skipping it here, before any geometry is resolved or the signature
        // hashed, is what makes both true at once — the excluded instance never enters `instances`,
        // so `signature` (which walks exactly this list) cannot see it move, recolor, or repaint.
        if mesh.dynamic {
            continue;
        }
        let solid = match mesh.source {
            PlacedMeshSource::Primitive(MeshShape::Cube) => Solid::Cube,
            PlacedMeshSource::Primitive(MeshShape::Plane) => Solid::Box {
                min: Vec3::new(-0.5, 0.0, -0.5),
                max: Vec3::new(0.5, 0.0, 0.5),
            },
            PlacedMeshSource::Primitive(MeshShape::Sphere) => Solid::Box {
                min: Vec3::splat(-0.5),
                max: Vec3::splat(0.5),
            },
            // `MeshShape::Asset` never reaches `place`; an asset path arrives as the variant below.
            // An assembly is skipped for the same reason an asset mesh is: the rasterizer takes one
            // object-space solid per placement, and an assembly is many part grids posed inside one —
            // no single solid describes it. Characters (the models assemblies exist for) carry
            // `GiDynamic` and are excluded above anyway. A sculpt is a triangle surface with no
            // authored occupancy grid, so it is skipped like an asset mesh — and so is one assembly
            // part on its own entity (a ragdoll switch's part, or a hand-authored one): a triangle
            // mesh with no authored occupancy grid of its own.
            PlacedMeshSource::Primitive(MeshShape::Asset(_))
            | PlacedMeshSource::Asset(_)
            | PlacedMeshSource::Character(_)
            | PlacedMeshSource::Assembly(_)
            | PlacedMeshSource::AssemblyPart(_, _)
            | PlacedMeshSource::Sculpt(_) => {
                continue;
            }
            PlacedMeshSource::Voxel(rel) => match voxel_solid(rel, assets, &mut voxels) {
                Ok(solid) => solid,
                Err(diagnostics) => {
                    let pointer = mesh.site.shape_pointer(mesh.index, "voxel");
                    for diagnostic in diagnostics {
                        errors.push(mesh_error(rel, "voxel model", &pointer, diagnostic));
                    }
                    continue;
                }
            },
        };

        let (lo, hi) = solid.object_box();
        let world = transformed_box(mesh.model, lo, hi);
        if !world.min.is_finite() || !world.max.is_finite() {
            continue;
        }
        instances.push(Instance {
            model: mesh.model,
            color: mesh.color,
            emissive: mesh.material[2],
            solid,
        });
    }

    if errors.is_empty() {
        Ok(instances)
    } else {
        Err(errors)
    }
}

/// The voxel model at `rel` as a [`Solid`]: its authored occupancy grid, and whether its document
/// authors a material.
///
/// That flag is read from the *meshed* form of the very same document rather than derived here,
/// because the meshed form is where the render path reads it: `extract` copies
/// `MeshData::authored_material` into `MeshInstance::use_vertex_material`, and the mesh shader
/// selects the whole material vector on it. Reading the same field is what keeps the grid a
/// voxelization of what renders.
///
/// `seen` memoizes both, one entry per distinct path. Errors are not memoized, so a failing
/// document reports once per instance exactly as it did before — extract has already resolved and
/// reported every voxel document by the time the voxelizer runs, so that path is defensive anyway.
fn voxel_solid(
    rel: &str,
    assets: &mut dyn SceneAssets,
    seen: &mut StableMap<String, (Arc<VoxelStaticGrid>, bool)>,
) -> Result<Solid, Vec<Diagnostic>> {
    if let Some((grid, palette_material)) = seen.as_inner().get(rel) {
        return Ok(Solid::Voxel {
            grid: Arc::clone(grid),
            palette_material: *palette_material,
        });
    }
    let grid = assets.voxel_grid(rel)?;
    let palette_material = assets.voxel(rel)?.authored_material;
    seen.insert(rel.to_string(), (Arc::clone(&grid), palette_material));
    Ok(Solid::Voxel {
        grid,
        palette_material,
    })
}

/// The union of every instance's transformed object box. A voxel model's box is `render.bounds`'
/// own (both call `grid_box`); the other solids are the primitive boxes the query reports too. `None`
/// for a scene with nothing to bound.
fn scene_bounds(instances: &[Instance]) -> Option<Aabb> {
    let mut bounds: Option<Aabb> = None;
    for instance in instances {
        let (lo, hi) = instance.solid.object_box();
        bounds = Some(union(bounds, transformed_box(instance.model, lo, hi)));
    }
    bounds
}

/// Sizes and allocates the (empty) grid: the world bounds snapped **outward** to the `cell_size`
/// lattice, so the grid's own faces sit on whole-cell boundaries and a hair of jitter in the bounds
/// cannot shift every cell. An axis whose bounds are flat (a ground plane) still gets one cell row.
///
/// The snapping arithmetic runs in `f64` ([`axis_cells`]) rather than in the `f32` the bounds
/// arrive in: a scene reaching far from the origin at a fine `cell_size` divides to an `f32`
/// infinity on both faces, and `inf - inf` is a `NaN` span that the `max(1.0)` floor would silently
/// turn into a one-cell axis — a sealed, cached, empty grid instead of a diagnostic.
///
/// # Errors
/// `L2103`, in all three of its shapes and each reported from the computed extent alone, so an
/// unbuildable grid costs an error rather than an allocation:
/// - a `cell_size` that is not a positive, finite length;
/// - bounds that are not finite, which imply no lattice to snap to;
/// - more than [`MAX_GI_CELLS`] cells in total, or more than [`MAX_GI_TEXTURE_AXIS`] on any one
///   axis (the two constraints the grid's 3D texture is under).
fn allocate(bounds: Aabb, cell_size: f32) -> Result<WorldVoxelGrid, Vec<Diagnostic>> {
    // The component's schema forbids this (`exclusiveMinimum: 0`); a game constructing `GiVolume`
    // in Rust bypasses that, and a zero or non-finite cell has no volume, so the extent it implies
    // is not a number of cells. Reported rather than allowed to produce a NaN-cornered grid.
    if !cell_size.is_finite() || cell_size <= 0.0 {
        return Err(vec![degenerate_cell_size(cell_size)]);
    }
    // `resolve` drops any instance whose transformed box has a non-finite corner, so a scene that
    // reaches here has finite bounds. Checked anyway rather than trusted: the alternative to an
    // error is a grid whose origin is an infinity, which no cell ever falls inside.
    if !bounds.min.is_finite() || !bounds.max.is_finite() {
        return Err(vec![nonfinite_bounds(bounds)]);
    }

    let lo: [f64; 3] = std::array::from_fn(|axis| axis_floor(bounds, cell_size, axis));
    let cells: [f64; 3] = std::array::from_fn(|axis| axis_cells(bounds, cell_size, axis));

    let count = cells[0] * cells[1] * cells[2];
    if count > MAX_GI_CELLS as f64 {
        return Err(vec![too_many_cells(bounds, cell_size, cells, count)]);
    }
    if let Some(axis) = breaching_axis(cells) {
        return Err(vec![too_many_cells_on_an_axis(
            bounds, cell_size, cells, axis,
        )]);
    }

    let extent = [cells[0] as u32, cells[1] as u32, cells[2] as u32];
    let total = extent[0] as usize * extent[1] as usize * extent[2] as usize;
    Ok(WorldVoxelGrid {
        origin: Vec3::new(
            (lo[0] * cell_size as f64) as f32,
            (lo[1] * cell_size as f64) as f32,
            (lo[2] * cell_size as f64) as f32,
        ),
        cell_size,
        extent,
        occupancy: vec![false; total],
        albedo: vec![[0.0; 4]; total],
        emissive: vec![0.0; total],
        // Unstamped: rasterization is still to come, and the content hash is only meaningful once
        // no buffer will change again. `voxelize` seals the grid as its last step.
        hash: 0,
    })
}

/// The lattice index of the cell `bounds`' minimum corner falls in along `axis` — the grid's own
/// first cell, in `cell_size` units from the world origin.
fn axis_floor(bounds: Aabb, cell_size: f32, axis: usize) -> f64 {
    (bounds.min[axis] as f64 / cell_size as f64).floor()
}

/// How many cells `axis` carries: the outward-snapped bounds in `cell_size` units, floored at the
/// one row a flat axis (a ground plane) still needs. The one sizing rule, shared by [`allocate`]
/// and by the suggestion its diagnostics carry, so the two can never disagree.
fn axis_cells(bounds: Aabb, cell_size: f32, axis: usize) -> f64 {
    let hi = (bounds.max[axis] as f64 / cell_size as f64).ceil();
    (hi - axis_floor(bounds, cell_size, axis)).max(1.0)
}

/// The first axis of `counts` past [`MAX_GI_TEXTURE_AXIS`], or `None` when every axis fits.
fn breaching_axis(counts: [f64; 3]) -> Option<usize> {
    (0..3).find(|&axis| counts[axis] > MAX_GI_TEXTURE_AXIS as f64)
}

/// `axis`'s name in the message — the world axis an author reads their scene in, not an index.
fn axis_name(axis: usize) -> &'static str {
    ["X", "Y", "Z"][axis]
}

/// The `L2103` diagnostic for a `cell_size` that is not a positive, finite length — the case where
/// there is no cell count to report, only an unusable resolution.
fn degenerate_cell_size(cell_size: f32) -> Diagnostic {
    Diagnostic::new(
        code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.cell_size` {cell_size} is not a usable global-illumination grid resolution, \
             so the scene's bounds imply no finite number of cells"
        ),
    )
    .with_help("`cell_size` is a length in world units and must be positive and finite")
}

/// The `L2103` diagnostic for world bounds that are not finite — the case where there is no lattice
/// to snap to, only a box with no location.
fn nonfinite_bounds(bounds: Aabb) -> Diagnostic {
    Diagnostic::new(
        code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        format!(
            "the scene's world bounds are not finite ({:?} to {:?}), so they imply no \
             global-illumination grid — a grid sized from them would have an infinite origin and no \
             cell any geometry falls inside",
            bounds.min, bounds.max,
        ),
    )
    .with_help(
        "an entity's `Transform` carries a non-finite position or scale, or a scale so large that \
         transforming its mesh's corners overflows; fix that entity, or remove the `GiVolume` \
         component to turn the voxelizer off",
    )
}

/// The `L2103` diagnostic: what was authored, what it implies, what the ceiling is, and a
/// `cell_size` that would fit.
fn too_many_cells(bounds: Aabb, cell_size: f32, cells: [f64; 3], count: f64) -> Diagnostic {
    let size = bounds.max - bounds.min;
    let mut diagnostic = Diagnostic::new(
        code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.cell_size` {cell_size} splits the scene's {:.1} x {:.1} x {:.1} world-unit \
             bounds into {} x {} x {} = {count} global-illumination grid cells, past the ceiling \
             of {MAX_GI_CELLS}",
            size.x, size.y, size.z, cells[0], cells[1], cells[2],
        ),
    );
    diagnostic = match fitting_cell_size(cell_size, count) {
        Some((fits, fitted)) => diagnostic.with_help(format!(
            "raise `cell_size` — cell count grows as its reciprocal cubed, so `{fits}` brings the \
             scene to about {fitted:.0} cells; or shrink the scene's world bounds, which one entity \
             placed far from the rest can stretch around empty space"
        )),
        None => diagnostic.with_help(
            "`cell_size` must be a positive, finite number of world units; cell count grows as its \
             reciprocal cubed",
        ),
    };
    diagnostic
}

/// The `L2103` diagnostic for a grid whose *total* fits but whose `axis` is longer than one 3D
/// texture may be: which axis, how many cells it carries, the limit, and a `cell_size` that fits it.
///
/// A different constraint from [`too_many_cells`] and so a different message, but the same code:
/// same component, same authored field, same set of fixes. `L1014` covering every schema range
/// breach is the precedent — a code names a problem, not a formula.
fn too_many_cells_on_an_axis(
    bounds: Aabb,
    cell_size: f32,
    cells: [f64; 3],
    axis: usize,
) -> Diagnostic {
    let name = axis_name(axis);
    let size = bounds.max - bounds.min;
    let mut diagnostic = Diagnostic::new(
        code::GI_VOLUME_CELL_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.cell_size` {cell_size} splits the scene's {:.1} x {:.1} x {:.1} world-unit \
             bounds into {} cells along {name}, past the limit of {MAX_GI_TEXTURE_AXIS} cells on a \
             single global-illumination grid axis (the whole grid is {} x {} x {} cells, which is \
             within the {MAX_GI_CELLS}-cell total)",
            size.x, size.y, size.z, cells[axis], cells[0], cells[1], cells[2],
        ),
    );
    diagnostic = match fitting_axis_cell_size(bounds, cell_size, axis) {
        Some((fits, fitted)) => diagnostic.with_help(format!(
            "raise `cell_size` — an axis's cell count grows as its reciprocal, so `{fits}` brings \
             {name} to {fitted:.0} cells; or shrink the scene along {name}, which one entity placed \
             far from the rest can stretch around empty space. The limit is WebGPU's guaranteed \
             minimum 3D texture dimension, so a grid that fits it builds on every conformant GPU"
        )),
        None => diagnostic.with_help(
            "`cell_size` must be a positive, finite number of world units; an axis's cell count \
             grows as its reciprocal",
        ),
    };
    diagnostic
}

/// The first repeated doubling of `cell_size` at which `axis` fits under [`MAX_GI_TEXTURE_AXIS`],
/// with the cell count it then carries — [`fitting_cell_size`]'s rule applied to one axis, and
/// recomputed through [`axis_cells`] at each step rather than estimated, so the number the help
/// names is the number the next run will size. Raising `cell_size` only ever lowers the total too,
/// so a suggestion from here cannot trade this breach for [`too_many_cells`].
fn fitting_axis_cell_size(bounds: Aabb, cell_size: f32, axis: usize) -> Option<(f32, f64)> {
    let mut size = cell_size;
    for _ in 0..64 {
        let cells = axis_cells(bounds, size, axis);
        if cells <= MAX_GI_TEXTURE_AXIS as f64 {
            return Some((size, cells));
        }
        size *= 2.0;
        if !size.is_finite() {
            return None;
        }
    }
    None
}

/// The first repeated doubling of `cell_size` whose cell count fits under [`MAX_GI_CELLS`], with
/// that count. Doubling rather than the exact cube root keeps the suggestion free of a
/// transcendental and each step exact in `f32`; the iteration bound is what makes a degenerate
/// `cell_size` (zero, or non-finite) return `None` instead of looping.
fn fitting_cell_size(cell_size: f32, count: f64) -> Option<(f32, f64)> {
    let (mut size, mut count) = (cell_size, count);
    for _ in 0..64 {
        if count <= MAX_GI_CELLS as f64 {
            return Some((size, count));
        }
        size *= 2.0;
        count /= 8.0;
    }
    None
}

/// The irradiance probe lattice laid over a finished [`WorldVoxelGrid`]: how many probes each axis
/// carries and the world-space span they cover. The lattice's origin is the grid's own, so only
/// these two need carrying.
#[derive(Debug)]
pub(crate) struct ProbeLattice {
    /// Probes per axis, at least 2 everywhere so trilinear interpolation between them is always
    /// defined.
    pub count: [u32; 3],
    /// World-space extent the lattice covers, `(count - 1) · probe_spacing` per axis — at least the
    /// grid's own box, since the count rounds up.
    pub span: Vec3,
}

/// Sizes the probe lattice covering `grid`'s world box at `probe_spacing` world units between
/// probes.
///
/// Probes sit on a lattice aligned to the grid's origin rather than at cell centres — probe
/// `(i, j, k)` is at `grid.origin + (i, j, k) · probe_spacing` — with one closing probe past the
/// box's far face on each axis, so the lattice covers the whole grid and a sample anywhere inside it
/// has eight probes around it.
///
/// # Errors
/// `L2105` when the lattice would carry more than [`MAX_GI_PROBES`] probes in total, or more than
/// [`MAX_GI_TEXTURE_AXIS`] on any one axis (the two constraints the probe volume's 3D texture is
/// under). Both checked on the computed counts, before anything is allocated from them.
pub(crate) fn probe_lattice(
    grid: &WorldVoxelGrid,
    probe_spacing: f32,
) -> Result<ProbeLattice, Diagnostic> {
    // The component's schema forbids this (`exclusiveMinimum: 0`); a game constructing `GiVolume`
    // in Rust bypasses that, and a zero or non-finite spacing places no lattice at all.
    if !probe_spacing.is_finite() || probe_spacing <= 0.0 {
        return Err(degenerate_probe_spacing(probe_spacing));
    }

    let box_span: [f64; 3] =
        std::array::from_fn(|axis| grid.extent[axis] as f64 * grid.cell_size as f64);
    let counts: [f64; 3] = std::array::from_fn(|axis| axis_probes(box_span[axis], probe_spacing));

    let total = counts[0] * counts[1] * counts[2];
    if total > MAX_GI_PROBES as f64 {
        return Err(too_many_probes(box_span, probe_spacing, counts, total));
    }
    if let Some(axis) = breaching_axis(counts) {
        return Err(too_many_probes_on_an_axis(
            box_span,
            probe_spacing,
            counts,
            axis,
        ));
    }

    let count = [counts[0] as u32, counts[1] as u32, counts[2] as u32];
    Ok(ProbeLattice {
        count,
        span: Vec3::new(
            (count[0] - 1) as f32 * probe_spacing,
            (count[1] - 1) as f32 * probe_spacing,
            (count[2] - 1) as f32 * probe_spacing,
        ),
    })
}

/// How many probes `axis` carries over a world box `box_span` units long: one probe per whole
/// spacing plus the closing probe on the far face, floored at the two probes trilinear interpolation
/// always needs. The one lattice rule, shared by [`probe_lattice`] and by the suggestion its
/// diagnostics carry.
fn axis_probes(box_span: f64, probe_spacing: f32) -> f64 {
    (box_span / probe_spacing as f64).ceil().max(1.0) + 1.0
}

/// The `L2105` diagnostic for a `probe_spacing` that is not a positive, finite length — the case
/// where there is no probe count to report, only an unusable lattice.
fn degenerate_probe_spacing(probe_spacing: f32) -> Diagnostic {
    Diagnostic::new(
        code::GI_VOLUME_PROBE_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.probe_spacing` {probe_spacing} is not a usable distance between irradiance \
             probes, so the grid's box implies no finite lattice"
        ),
    )
    .with_help("`probe_spacing` is a length in world units and must be positive and finite")
}

/// The `L2105` diagnostic: what was authored, what it implies, what the ceiling is, and a
/// `probe_spacing` that would fit.
fn too_many_probes(
    box_span: [f64; 3],
    probe_spacing: f32,
    counts: [f64; 3],
    total: f64,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        code::GI_VOLUME_PROBE_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.probe_spacing` {probe_spacing} lays {} x {} x {} = {total} irradiance probes \
             over the {:.1} x {:.1} x {:.1} world-unit global-illumination grid, past the ceiling \
             of {MAX_GI_PROBES}",
            counts[0], counts[1], counts[2], box_span[0], box_span[1], box_span[2],
        ),
    );
    diagnostic = match fitting_probe_spacing(probe_spacing, total) {
        Some((fits, fitted)) => diagnostic.with_help(format!(
            "raise `probe_spacing` — probe count grows as its reciprocal cubed, so `{fits}` brings \
             the volume to about {fitted:.0} probes; the volume is interpolated between probes, so \
             a spacing near `cell_size` is asking a coarse irradiance lattice to act as a second \
             voxel grid"
        )),
        None => diagnostic.with_help(
            "`probe_spacing` must be a positive, finite number of world units; probe count grows \
             as its reciprocal cubed",
        ),
    };
    diagnostic
}

/// The `L2105` diagnostic for a lattice whose *total* fits but whose `axis` is longer than one 3D
/// texture may be: which axis, how many probes it carries, the limit, and a `probe_spacing` that
/// fits it. [`too_many_cells_on_an_axis`]'s shape at the lattice, sharing `L2105` for the same
/// reason it shares `L2103`.
fn too_many_probes_on_an_axis(
    box_span: [f64; 3],
    probe_spacing: f32,
    counts: [f64; 3],
    axis: usize,
) -> Diagnostic {
    let name = axis_name(axis);
    let mut diagnostic = Diagnostic::new(
        code::GI_VOLUME_PROBE_BUDGET_EXCEEDED,
        format!(
            "`GiVolume.probe_spacing` {probe_spacing} lays {} irradiance probes along {name} over \
             the {:.1} x {:.1} x {:.1} world-unit global-illumination grid, past the limit of \
             {MAX_GI_TEXTURE_AXIS} probes on a single volume axis (the whole lattice is {} x {} x \
             {} probes, which is within the {MAX_GI_PROBES}-probe total)",
            counts[axis], box_span[0], box_span[1], box_span[2], counts[0], counts[1], counts[2],
        ),
    );
    diagnostic = match fitting_axis_probe_spacing(box_span[axis], probe_spacing) {
        Some((fits, fitted)) => diagnostic.with_help(format!(
            "raise `probe_spacing` — an axis's probe count grows as its reciprocal, so `{fits}` \
             brings {name} to {fitted:.0} probes; or raise `cell_size` / shrink the scene along \
             {name}, since the grid's box is what the lattice covers. The limit is WebGPU's \
             guaranteed minimum 3D texture dimension, so a volume that fits it bakes on every \
             conformant GPU"
        )),
        None => diagnostic.with_help(
            "`probe_spacing` must be a positive, finite number of world units; an axis's probe \
             count grows as its reciprocal",
        ),
    };
    diagnostic
}

/// The first repeated doubling of `probe_spacing` at which an axis spanning `box_span` world units
/// fits under [`MAX_GI_TEXTURE_AXIS`], with the probe count it then carries — [`fitting_axis_cell_size`]'s
/// rule at the lattice, recomputed through [`axis_probes`] at each step so the number the help names
/// is the number the next run will lay.
fn fitting_axis_probe_spacing(box_span: f64, probe_spacing: f32) -> Option<(f32, f64)> {
    let mut spacing = probe_spacing;
    for _ in 0..64 {
        let probes = axis_probes(box_span, spacing);
        if probes <= MAX_GI_TEXTURE_AXIS as f64 {
            return Some((spacing, probes));
        }
        spacing *= 2.0;
        if !spacing.is_finite() {
            return None;
        }
    }
    None
}

/// The first repeated doubling of `probe_spacing` whose probe count fits under [`MAX_GI_PROBES`],
/// with that count — [`fitting_cell_size`]'s rule for the lattice, and bounded the same way so a
/// degenerate spacing returns `None` instead of looping.
fn fitting_probe_spacing(probe_spacing: f32, total: f64) -> Option<(f32, f64)> {
    let (mut spacing, mut total) = (probe_spacing, total);
    for _ in 0..64 {
        if total <= MAX_GI_PROBES as f64 {
            return Some((spacing, total));
        }
        spacing *= 2.0;
        total /= 8.0;
    }
    None
}

/// Deposits one instance into the grid, per its solid's rule.
fn rasterize(grid: &mut WorldVoxelGrid, instance: &Instance) {
    match &instance.solid {
        Solid::Cube => fill_cube(grid, instance),
        Solid::Box { min, max } => {
            let world = transformed_box(instance.model, *min, *max);
            fill_range(grid, world, instance.color, instance.emissive);
        }
        Solid::Voxel {
            grid: model,
            palette_material,
        } => deposit_voxels(grid, instance, model, *palette_material),
    }
}

/// Fills every cell whose **center** lies inside the transformed unit cube, by carrying the center
/// back into object space and testing it against `[-0.5, 0.5]³` (inclusive, so a center exactly on
/// a face counts as inside — a rule, not a tolerance). Exact for any invertible transform. A
/// singular transform has no inverse to test through, so it falls back to the conservative box
/// fill.
fn fill_cube(grid: &mut WorldVoxelGrid, instance: &Instance) {
    let world = transformed_box(instance.model, Vec3::splat(-0.5), Vec3::splat(0.5));
    let determinant = instance.model.determinant();
    if determinant == 0.0 || !determinant.is_finite() {
        fill_range(grid, world, instance.color, instance.emissive);
        return;
    }
    let inverse = instance.model.inverse();
    let Some((lo, hi)) = cell_range(grid, world) else {
        return;
    };
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let local = inverse.transform_point3(cell_center(grid, [x, y, z]));
                if local.abs().cmple(Vec3::splat(0.5)).all() {
                    write_cell(grid, [x, y, z], instance.color, instance.emissive);
                }
            }
        }
    }
}

/// Fills every cell the world box `aabb` touches — the conservative rule, used for planes, spheres,
/// and a cube whose transform cannot be inverted.
fn fill_range(grid: &mut WorldVoxelGrid, aabb: Aabb, albedo: [f32; 4], emissive: f32) {
    let Some((lo, hi)) = cell_range(grid, aabb) else {
        return;
    };
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                write_cell(grid, [x, y, z], albedo, emissive);
            }
        }
    }
}

/// Deposits a voxel model: every filled authored cell's object-space center, transformed into the
/// world and dropped into the cell it lands in. Walked in ascending flat index (`z` outer, `x`
/// inner), which is what fixes the last-writer order when several model cells land in one world
/// cell — a 16-voxel-per-unit prop is far finer than a half-unit world cell, so that is the normal
/// case, not the exception.
///
/// `palette_material` is the render path's own material selection, mirrored: `extract` sets
/// `use_vertex_material` from the document's `MeshData::authored_material`, and `mesh.wgsl` then
/// takes the whole material vector from the palette when it is set and from the instance's scalars
/// when it is not. So a lamp whose palette authors no material renders emissive from
/// `Mesh.emissive`, and voxelizes as an emitter for the same reason. Albedo needs no such
/// selection — the shader always forms `vertex_color * instance_color`.
fn deposit_voxels(
    grid: &mut WorldVoxelGrid,
    instance: &Instance,
    model: &VoxelStaticGrid,
    palette_material: bool,
) {
    let [sx, sy, sz] = model.extent;
    // The voxel-model origin convention: X and Z centered on the extent, Y sitting on zero.
    let offset = Vec3::new(sx as f32 * 0.5, 0.0, sz as f32 * 0.5);
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let flat = x as usize + sx as usize * (y as usize + sy as usize * z as usize);
                let Some(cell) = model.cells[flat] else {
                    continue;
                };
                // Point-sampled: one world cell per authored voxel, taken at its center. Dense only
                // while an authored voxel is no larger than a world cell — `Transform.scale` <=
                // `cell_size`, the convention being one authored voxel per object-space unit. Past
                // that the deposit thins into a lattice and the model reads as a hollow shell, and
                // refining `cell_size` makes that worse rather than better.
                let center = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) - offset;
                let world = instance.model.transform_point3(center);
                let Some(target) = cell_at(grid, world) else {
                    continue;
                };
                let emissive = if palette_material {
                    cell.emissive
                } else {
                    instance.emissive
                };
                write_cell(
                    grid,
                    target,
                    modulate(cell.albedo, instance.color),
                    emissive,
                );
            }
        }
    }
}

/// The inclusive cell index range the world box `aabb` covers, clamped to the grid, or `None` when
/// the box lies wholly outside it. Cells are half-open at their upper face, so a coordinate exactly
/// on a boundary belongs to the cell above it — except at the grid's own far face, where the clamp
/// keeps it in the last cell.
fn cell_range(grid: &WorldVoxelGrid, aabb: Aabb) -> Option<([u32; 3], [u32; 3])> {
    let mut lo = [0u32; 3];
    let mut hi = [0u32; 3];
    for axis in 0..3 {
        let last = grid.extent[axis] as i64 - 1;
        let first = cell_index(grid, aabb.min[axis], axis);
        let final_ = cell_index(grid, aabb.max[axis], axis);
        if final_ < 0 || first > last {
            return None;
        }
        lo[axis] = first.max(0) as u32;
        hi[axis] = final_.min(last) as u32;
    }
    Some((lo, hi))
}

/// The (unclamped, possibly out-of-range) cell index a world coordinate falls in along `axis`.
fn cell_index(grid: &WorldVoxelGrid, coord: f32, axis: usize) -> i64 {
    ((coord - grid.origin[axis]) / grid.cell_size).floor() as i64
}

/// The cell a world point falls in, or `None` if it lies outside the grid.
fn cell_at(grid: &WorldVoxelGrid, point: Vec3) -> Option<[u32; 3]> {
    let mut cell = [0u32; 3];
    for axis in 0..3 {
        let index = cell_index(grid, point[axis], axis);
        if index < 0 || index >= grid.extent[axis] as i64 {
            return None;
        }
        cell[axis] = index as u32;
    }
    Some(cell)
}

/// The world-space center of cell `(x, y, z)`.
fn cell_center(grid: &WorldVoxelGrid, cell: [u32; 3]) -> Vec3 {
    grid.origin
        + Vec3::new(
            cell[0] as f32 + 0.5,
            cell[1] as f32 + 0.5,
            cell[2] as f32 + 0.5,
        ) * grid.cell_size
}

/// Marks a cell filled and overwrites its shading channels — the last-writer half of the combine
/// rule, in the one place it happens.
fn write_cell(grid: &mut WorldVoxelGrid, cell: [u32; 3], albedo: [f32; 4], emissive: f32) {
    let index = grid.index(cell);
    grid.occupancy[index] = true;
    grid.albedo[index] = albedo;
    grid.emissive[index] = emissive;
}

/// Component-wise RGBA product — a voxel palette color under its instance's tint, the same product
/// `mesh.wgsl` forms from `vertex_color * instance_color`.
fn modulate(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The suggestion walks whole doublings until the count fits: eight times too many cells is one
    /// doubling (each divides the count by eight).
    #[test]
    fn fitting_cell_size_doubles_until_the_count_fits() {
        let over = MAX_GI_CELLS as f64 * 8.0;
        let (size, count) = fitting_cell_size(0.5, over).expect("one doubling fits");
        assert_eq!(size, 1.0);
        assert_eq!(count, MAX_GI_CELLS as f64);
    }

    /// A degenerate `cell_size` (zero — reachable only by a game constructing `GiVolume` in Rust and
    /// skipping validation) yields no suggestion instead of looping forever on `0 * 2`.
    #[test]
    fn fitting_cell_size_gives_up_on_a_degenerate_size() {
        assert!(fitting_cell_size(0.0, f64::INFINITY).is_none());
    }

    /// The scene the axis cap exists for: 4000 x 4 x 2000 cells is *exactly* [`MAX_GI_CELLS`], so
    /// the product check passes and the axis check is the only thing standing between the author and
    /// a `max_texture_dimension_3d` validation panic inside `prepare_frame`. One entity placed a
    /// couple of thousand units from the rest produces this shape.
    ///
    /// Reported before allocation: the grid this rejects is 32M cells, over half a gigabyte of
    /// buffers, and the test allocates none of it.
    #[test]
    fn an_axis_past_the_texture_cap_is_l2103_naming_the_axis() {
        let bounds = Aabb {
            min: Vec3::new(-2000.0, -2.0, -1000.0),
            max: Vec3::new(2000.0, 2.0, 1000.0),
        };
        let cells: [f64; 3] = std::array::from_fn(|axis| axis_cells(bounds, 1.0, axis));
        assert_eq!(cells, [4000.0, 4.0, 2000.0]);
        assert_eq!(
            cells[0] * cells[1] * cells[2],
            MAX_GI_CELLS as f64,
            "the fixture sits exactly on the total cap, so only the axis check can fire"
        );

        let errors = allocate(bounds, 1.0).expect_err("4000 cells on X is past the axis cap");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.as_str(), "L2103");
        assert!(
            errors[0].message.contains("4000 cells along X")
                && errors[0].message.contains(&MAX_GI_TEXTURE_AXIS.to_string()),
            "the message names the breaching axis, its extent, and the limit: {}",
            errors[0].message,
        );
        assert!(
            errors[0]
                .help
                .as_deref()
                .is_some_and(|help| help.contains('2') && help.contains("cell_size")),
            "the help suggests a cell size that fits the axis: {:?}",
            errors[0].help,
        );
    }

    /// The axis suggestion halves the breaching axis per doubling (where the total's suggestion
    /// divides by eight), and is recomputed from the bounds rather than estimated: a 4000-cell axis
    /// takes one doubling to reach 2000.
    #[test]
    fn fitting_axis_cell_size_doubles_until_the_axis_fits() {
        let bounds = Aabb {
            min: Vec3::new(-2000.0, -2.0, -1000.0),
            max: Vec3::new(2000.0, 2.0, 1000.0),
        };
        let (size, cells) = fitting_axis_cell_size(bounds, 1.0, 0).expect("one doubling fits");
        assert_eq!(size, 2.0);
        assert_eq!(cells, 2000.0);
    }

    /// A `cell_size` that is not a positive, finite length (reachable only by a game constructing
    /// `GiVolume` in Rust and skipping schema validation) is `L2103` rather than a grid with no
    /// cells in it — [`a_degenerate_probe_spacing_is_l2105`]'s twin at the grid.
    #[test]
    fn a_degenerate_cell_size_is_l2103() {
        let bounds = Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        };
        let errors = allocate(bounds, 0.0).expect_err("a zero cell has no volume");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.as_str(), "L2103");
    }

    /// Bounds that are not finite imply no lattice to snap to, so they are `L2103` rather than a
    /// grid whose origin is an infinity and whose every cell is empty.
    #[test]
    fn non_finite_bounds_are_l2103_rather_than_a_degenerate_grid() {
        let bounds = Aabb {
            min: Vec3::splat(f32::NEG_INFINITY),
            max: Vec3::splat(f32::INFINITY),
        };
        let errors = allocate(bounds, 0.5).expect_err("infinite bounds imply no grid");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.as_str(), "L2103");
    }

    /// The `f32` overflow the sizing arithmetic used to take: a finite box far enough from the
    /// origin divides to `+inf` on *both* faces at a fine `cell_size`, and `inf - inf` is a `NaN`
    /// span that the `max(1.0)` floor turns into a one-cell axis — a sealed, hashed, cached grid
    /// with an infinite origin that nothing ever fills. In `f64` the same bounds are an honest,
    /// enormous cell count, which is a diagnostic.
    #[test]
    fn a_box_that_overflows_f32_cell_units_is_a_diagnostic_not_a_degenerate_grid() {
        let bounds = Aabb {
            min: Vec3::splat(1e38 - 5e31),
            max: Vec3::splat(1e38 + 5e31),
        };
        assert!(bounds.min.is_finite() && bounds.max.is_finite());
        assert!(
            ((bounds.max.x / 0.1).ceil() - (bounds.min.x / 0.1).floor()).is_nan(),
            "the fixture is the overflow itself: in f32 both faces divide to the same infinity"
        );

        let errors = allocate(bounds, 0.1).expect_err("a box this large implies no buildable grid");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code.as_str(), "L2103");
    }

    /// The lattice's suggestion walks doublings the same way the grid's does — a spacing eight times
    /// too dense is one doubling away from fitting.
    #[test]
    fn fitting_probe_spacing_doubles_until_the_count_fits() {
        let over = MAX_GI_PROBES as f64 * 8.0;
        let (spacing, total) = fitting_probe_spacing(1.0, over).expect("one doubling fits");
        assert_eq!(spacing, 2.0);
        assert_eq!(total, MAX_GI_PROBES as f64);
    }

    /// A degenerate `probe_spacing` yields no suggestion instead of looping, `fitting_cell_size`'s
    /// rule at the lattice.
    #[test]
    fn fitting_probe_spacing_gives_up_on_a_degenerate_spacing() {
        assert!(fitting_probe_spacing(0.0, f64::INFINITY).is_none());
    }

    /// The lattice's axis suggestion halves the breaching axis per doubling, recomputed through
    /// [`axis_probes`]: a 4000-unit axis at unit spacing carries 4001 probes and takes one doubling
    /// to reach 2001.
    #[test]
    fn fitting_axis_probe_spacing_doubles_until_the_axis_fits() {
        let (spacing, probes) = fitting_axis_probe_spacing(4000.0, 1.0).expect("one doubling fits");
        assert_eq!(spacing, 2.0);
        assert_eq!(probes, 2001.0);
    }

    /// An empty grid of the shape the tests need — occupancy is irrelevant to sizing.
    fn empty_grid(extent: [u32; 3], cell_size: f32) -> WorldVoxelGrid {
        let total = extent[0] as usize * extent[1] as usize * extent[2] as usize;
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size,
            extent,
            occupancy: vec![false; total],
            albedo: vec![[0.0; 4]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
    }

    /// A `probe_spacing` that is not a positive, finite length (reachable only by a game
    /// constructing `GiVolume` in Rust) is `L2105` rather than a lattice with no probes in it.
    #[test]
    fn a_degenerate_probe_spacing_is_l2105() {
        let grid = empty_grid([4, 4, 4], 1.0);
        let error = probe_lattice(&grid, 0.0).expect_err("a zero spacing places no lattice");
        assert_eq!(error.code.as_str(), "L2105");
    }

    /// A long, thin grid: 4001 x 5 x 5 probes is 100_025, comfortably inside [`MAX_GI_PROBES`], so
    /// only the axis check can fire — and it must, because 4001 probes on X is a 3D texture no
    /// conformant adapter is required to create. Reported before the volume is sized.
    #[test]
    fn a_probe_axis_past_the_texture_cap_is_l2105_naming_the_axis() {
        let grid = empty_grid([2000, 2, 2], 2.0);
        let error = probe_lattice(&grid, 1.0).expect_err("4001 probes on X is past the axis cap");
        assert_eq!(error.code.as_str(), "L2105");
        assert!(
            error.message.contains("4001 irradiance probes along X")
                && error.message.contains(&MAX_GI_TEXTURE_AXIS.to_string()),
            "the message names the breaching axis, its probe count, and the limit: {}",
            error.message,
        );
        assert!(
            error
                .help
                .as_deref()
                .is_some_and(|help| help.contains('2') && help.contains("probe_spacing")),
            "the help suggests a spacing that fits the axis: {:?}",
            error.help,
        );
    }

    /// The same grid at the suggested spacing sizes cleanly — the help names a value that works,
    /// not merely a larger one.
    #[test]
    fn the_suggested_probe_spacing_actually_fits() {
        let grid = empty_grid([2000, 2, 2], 2.0);
        let lattice = probe_lattice(&grid, 2.0).expect("the suggested spacing fits");
        assert_eq!(lattice.count, [2001, 3, 3]);
    }
}
