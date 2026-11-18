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
//! - **albedo and emissive are LAST WRITER WINS**, iterating instances in ascending order of their
//!   signature bytes ([`instance_bytes`]: model, colour, emissive, solid) and, within one voxel
//!   model, its cells in ascending flat-index order (an asset mesh's triangles in mesh order).
//!
//! The instance order is a content order, so the grid is a pure function of the *set* of resolved
//! instances: independent of entity slots, spawn history and which instances sit in the kept
//! statics. Same scene ⇒ bit-identical buffers, on any machine, at any thread count.
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
//! - **Asset mesh** — a conservative **surface shell** ([`deposit_triangles`]): each triangle, in
//!   world space, fills every cell its surface overlaps (triangle/box separating-axis test); the
//!   interior is never filled, so a hollow imported building stays hollow. The box is inflated by
//!   `cell_size * 1e-4`, so a surface lying exactly on a cell face fills both neighbours instead of
//!   flickering on rounding. Asset meshes stretch the grid's bounds. Albedo per triangle is
//!   `mean(three vertex colours) × instance colour × MeshData::base_color_mean` — the mean of the
//!   base-colour map's source pixels, stored at decode and at cook (`ludion_assets::srgb_mean`) so
//!   both texture axes voxelize alike; white without a map. Emissive follows
//!   the render path's selection: the mean of the three vertices' authored emissive when the
//!   document authors a material, the `Mesh` component's otherwise; the emissive map is not
//!   sampled. A skinned asset mesh and a `Character` are skipped (the grid is a static baseline),
//!   and so is an `alpha_mode: mask` mesh (a solid shell would block the light its holes pass);
//!   sculpts, assemblies and assembly parts are skipped too — the shell rasterizer does not take
//!   them. [`voxelize`] returns what it skipped as a [`SkipReport`] (counts by reason, the
//!   first entity index of each), the same on a cache hit and a miss.
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
//! a source [`resolve`] discards (a skipped source, an instance whose transform is not finite)
//! contributes nothing to the grid and nothing to the signature. Voxel models and asset meshes enter
//! it by their document's content hash, so a hot-reloaded model invalidates while an unchanged one
//! does not. One conservatism: an asset mesh whose *transformed vertex box* is not finite is skipped
//! on a miss, where the box is first computed — never per frame — but keeps its signature entry, so
//! it costs a needless rebuild and never a stale grid.
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
use ludion_core::math::{mat4_determinant, mat4_inverse};
use ludion_renderer::{MAX_GI_TEXTURE_AXIS, MeshData, MeshShape, WorldVoxelGrid, max_gi_probes};

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

// The ceiling on an irradiance probe lattice's probe count is [`max_gi_probes`], above which
// [`probe_lattice`] reports `L2105` instead of handing the renderer a volume to allocate.
//
// Derived, not chosen: the bounce bake's hit cache is one `u32` per probe for each of the first 16
// rays, 64 B/probe, bound as a single storage buffer — twice the 32 B/probe of the four
// `Rgba16Float` volumes (sky visibility, plus one-bounce indirect's three colour channels when
// `bounce` is authored) — so the device's storage-binding limit is the binding constraint, about
// 2.1 M probes at the 128 MiB the engine requests. A district at the default one-unit spacing is a few
// hundred thousand probes; everything this rejects is a `probe_spacing` typed an order of magnitude
// too small, or a scene whose far geometry stretched the grid.

/// What [`voxelize`] dropped, by reason — the input to a diagnostic that tells an author why a mesh
/// they placed does not occlude. Counts rather than a list: a horde marks thousands of bodies
/// `GiDynamic` and this is built every frame, hit or miss.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkipReport {
    /// Instances carrying `GiDynamic`.
    pub dynamic: SkipCount,
    /// Skinned asset meshes and characters.
    pub skinned: SkipCount,
    /// Instances whose model matrix is not finite.
    pub non_finite: SkipCount,
    /// Sources the voxelizer does not rasterize: sculpts, assemblies and assembly parts, and an
    /// asset mesh with no triangles.
    pub unsupported: SkipCount,
    /// Asset meshes whose material is `alpha_mode: mask`: light passes through their holes, which a
    /// solid shell would block.
    pub cutout: SkipCount,
}

/// How many instances one skip reason dropped, and the entity index of the first (for a pointer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkipCount {
    /// Instances dropped for this reason.
    pub count: u32,
    /// The extract-walk index of the first one dropped.
    pub first: Option<usize>,
}

impl SkipCount {
    fn note(&mut self, index: usize) {
        self.count += 1;
        self.first.get_or_insert(index);
    }
}

/// [`voxelize`]'s answer: the grid ([`skip_report`] says what was left out of it).
pub(crate) struct Voxelized {
    /// The grid, or `None` when the scene places no geometry the voxelizer rasterizes.
    pub grid: Option<Arc<WorldVoxelGrid>>,
}

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
    statics: &StaticGi,
    meshes: &[PlacedMesh<'_>],
    cell_size: f32,
    assets: &mut dyn SceneAssets,
) -> Result<Voxelized, Vec<Diagnostic>> {
    let (instances, _) = resolve(meshes, assets)?;
    let signature = signature(statics, &instances, cell_size);
    if let Some(cached) = assets.voxelization(signature) {
        return Ok(Voxelized { grid: cached });
    }
    // A held session serves the grid it already has rather than paying the walk for a world whose
    // baseline the host has frozen. The signature above is still computed every frame — that
    // comparison *is* the staleness report, which is why it is never latched: a world that returns
    // to the held baseline hits the line above and is honestly not stale.
    if let Some(held) = assets.held_voxelization() {
        return Ok(Voxelized { grid: held });
    }

    // Object boxes of asset meshes walk every vertex, so they are computed here, on a miss, once
    // per distinct mesh — never in `resolve` or `signature`, which run every frame.
    let mut boxes = MeshBoxes::new();
    let ordered = || statics.instances.iter().chain(&instances);
    let Some(bounds) = scene_bounds(ordered(), &mut boxes) else {
        return Ok(Voxelized {
            grid: assets.set_voxelization(signature, None),
        });
    };
    let mut grid = allocate(bounds, cell_size)?;
    #[cfg(test)]
    RASTERIZATIONS.with(|count| count.set(count.get() + 1));
    rasterize_ordered(&mut grid, ordered(), &mut boxes);
    Ok(Voxelized {
        grid: assets.set_voxelization(signature, Some(Arc::new(grid.sealed()))),
    })
}

/// Rasterizes `instances` into `grid`, last writer winning in ascending signature-byte order, a
/// content order: the grid is a function of the instance set, not of entity slots or of which
/// instances sit in the kept statics. Exact byte ties are the same deposit, so their order cannot
/// matter.
fn rasterize_ordered<'a>(
    grid: &mut WorldVoxelGrid,
    instances: impl Iterator<Item = &'a Instance>,
    boxes: &mut MeshBoxes,
) {
    let mut keyed: Vec<(Vec<u8>, &Instance)> = instances
        .map(|instance| {
            let mut bytes = Vec::with_capacity(96);
            instance_bytes(instance, &mut bytes);
            (bytes, instance)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, instance) in keyed {
        rasterize(grid, instance, boxes);
    }
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
/// resolved order (statics' digest first, then ascending entity slot), each contributing exactly the values [`rasterize`] reads —
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
///
/// With kept statics ([`StaticGi`]) the key hashes their precomputed digest in place of their
/// instances, so a quiet frame never re-walks them; without, the bytes are exactly the historical
/// ones.
fn signature(statics: &StaticGi, instances: &[Instance], cell_size: f32) -> u64 {
    let mut bytes: Vec<u8> = Vec::with_capacity(16 + instances.len() * 96);
    bytes.extend_from_slice(&cell_size.to_le_bytes());
    if !statics.instances.is_empty() {
        bytes.extend_from_slice(b"static");
        bytes.extend_from_slice(&statics.digest.to_le_bytes());
    }
    for instance in instances {
        instance_bytes(instance, &mut bytes);
    }
    ludion_assets::content_hash(&bytes)
}

/// The values [`rasterize`] reads from one instance, as signature bytes.
fn instance_bytes(instance: &Instance, bytes: &mut Vec<u8>) {
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
        Solid::Mesh { mesh } => {
            bytes.push(3);
            bytes.extend_from_slice(&mesh.hash.to_le_bytes());
        }
    }
}

/// The resolved GI instances of the kept statics and a digest of their signature bytes, built once
/// per change to the static set (`StaticScene::ensure_gi`) rather than once per frame.
#[derive(Default)]
pub(crate) struct StaticGi {
    instances: Vec<Instance>,
    digest: u64,
}

/// Resolves the kept statics' placements (hidden ones included, in the flat lists' order) into a
/// [`StaticGi`].
///
/// # Errors
/// As [`voxelize`]: a mesh document that will not load.
pub(crate) fn resolve_static(
    placements: &[PlacedMesh<'_>],
    assets: &mut dyn SceneAssets,
) -> Result<StaticGi, Vec<Diagnostic>> {
    let (instances, _) = resolve(placements, assets)?;
    let mut bytes = Vec::with_capacity(instances.len() * 96);
    for instance in &instances {
        instance_bytes(instance, &mut bytes);
    }
    Ok(StaticGi {
        digest: ludion_assets::content_hash(&bytes),
        instances,
    })
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
    /// An unskinned asset mesh, deposited as a conservative surface shell by [`deposit_triangles`].
    Mesh {
        /// The loaded mesh; its `hash` is the signature entry.
        mesh: Arc<MeshData>,
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

/// Object-space boxes of asset meshes, one per mesh `hash`, built on a miss only.
type MeshBoxes = StableMap<u64, (Vec3, Vec3)>;

impl Solid {
    /// The object-space box this solid occupies — what the scene bounds are unioned from, and the
    /// search region a conservative fill walks. An asset mesh's is its vertex extent, memoized in
    /// `boxes`.
    fn object_box(&self, boxes: &mut MeshBoxes) -> (Vec3, Vec3) {
        match self {
            Solid::Cube => (Vec3::splat(-0.5), Vec3::splat(0.5)),
            Solid::Box { min, max } => (*min, *max),
            Solid::Voxel { grid, .. } => grid_box(grid.extent),
            Solid::Mesh { mesh } => {
                if let Some(known) = boxes.as_inner().get(&mesh.hash) {
                    return *known;
                }
                let computed = mesh_box(mesh);
                boxes.insert(mesh.hash, computed);
                computed
            }
        }
    }
}

/// The vertex extent of `mesh` in object space.
fn mesh_box(mesh: &MeshData) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for vertex in mesh.positions.chunks_exact(3) {
        let p = Vec3::new(vertex[0], vertex[1], vertex[2]);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

/// The instance's world box, or `None` when it is not finite — the one predicate [`scene_bounds`]
/// and [`rasterize`] share, so an instance skipped by one is skipped by the other.
fn world_box(instance: &Instance, boxes: &mut MeshBoxes) -> Option<Aabb> {
    let (lo, hi) = instance.solid.object_box(boxes);
    let world = transformed_box(instance.model, lo, hi);
    (world.min.is_finite() && world.max.is_finite()).then_some(world)
}

/// What [`resolve`] would drop from `meshes`, without rasterizing anything — the dumps' diagnosis of
/// an empty grid.
///
/// # Errors
/// As [`voxelize`]: a mesh document that will not load.
pub(crate) fn skip_report(
    meshes: &[PlacedMesh<'_>],
    assets: &mut dyn SceneAssets,
) -> Result<SkipReport, Vec<Diagnostic>> {
    resolve(meshes, assets).map(|(_, skipped)| skipped)
}

/// Resolves each placed mesh into the geometry the rasterizer needs, keeping the ascending
/// entity-slot order the signature hashes, and counts what it drops. Instances whose
/// transform sends a corner to a non-finite coordinate are dropped: they have no world box the grid
/// can be sized from, and skipping them is what keeps the extent arithmetic finite (the
/// `build_sun` posture — a degenerate input yields an omission, never a NaN).
///
/// Runs every frame: an asset mesh is checked by its model matrix alone, never by walking its
/// vertices. Its transformed box is checked on a miss, by [`world_box`].
fn resolve(
    meshes: &[PlacedMesh<'_>],
    assets: &mut dyn SceneAssets,
) -> Result<(Vec<Instance>, SkipReport), Vec<Diagnostic>> {
    let mut instances = Vec::with_capacity(meshes.len());
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut skipped = SkipReport::default();
    // One entry per distinct asset mesh, for the same reason as `voxels`.
    let mut asset_meshes: StableMap<String, Arc<MeshData>> = StableMap::new();
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
            skipped.dynamic.note(mesh.index);
            continue;
        }
        // A blended mesh is seen through, so it neither occludes probe rays nor deposits albedo.
        if mesh.blend.is_some() {
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
            // `MeshShape::Asset` never reaches `place`; an asset path arrives as `Asset` below.
            // An assembly is skipped because the rasterizer takes one object-space solid per
            // placement, and an assembly is many part grids posed inside one — no single solid
            // describes it. A sculpt and one assembly part on its own entity (a ragdoll switch's
            // part, or a hand-authored one) are triangle surfaces the shell rasterizer could take,
            // but are not wired to it.
            PlacedMeshSource::Primitive(MeshShape::Asset(_))
            | PlacedMeshSource::Assembly(_)
            | PlacedMeshSource::AssemblyPart(_, _)
            | PlacedMeshSource::Sculpt(_) => {
                skipped.unsupported.note(mesh.index);
                continue;
            }
            // A skinned character is posed per frame; the grid is a static baseline.
            PlacedMeshSource::Character(_) => {
                skipped.skinned.note(mesh.index);
                continue;
            }
            PlacedMeshSource::Asset(rel) => {
                let data = match asset_mesh(rel, assets, &mut asset_meshes) {
                    Ok(data) => data,
                    Err(diagnostics) => {
                        let pointer = mesh.site.shape_pointer(mesh.index, "asset");
                        for diagnostic in diagnostics {
                            errors.push(mesh_error(rel, "mesh asset", &pointer, diagnostic));
                        }
                        continue;
                    }
                };
                if !data.skin_joints.is_empty() {
                    skipped.skinned.note(mesh.index);
                    continue;
                }
                if data
                    .textures
                    .as_ref()
                    .is_some_and(|t| t.alpha_cutoff.is_some())
                {
                    skipped.cutout.note(mesh.index);
                    continue;
                }
                if data.positions.is_empty() || data.indices.is_empty() {
                    skipped.unsupported.note(mesh.index);
                    continue;
                }
                if !mesh.model.to_cols_array().iter().all(|v| v.is_finite()) {
                    skipped.non_finite.note(mesh.index);
                    continue;
                }
                instances.push(Instance {
                    model: mesh.model,
                    color: mesh.color,
                    emissive: mesh.material[2],
                    solid: Solid::Mesh { mesh: data },
                });
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

        let (lo, hi) = solid.object_box(&mut MeshBoxes::new());
        let world = transformed_box(mesh.model, lo, hi);
        if !world.min.is_finite() || !world.max.is_finite() {
            skipped.non_finite.note(mesh.index);
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
        Ok((instances, skipped))
    } else {
        Err(errors)
    }
}

/// The asset mesh at `rel`, memoized in `seen` so N instances of one prop cost one loader lookup.
/// Errors are not memoized, like [`voxel_solid`]'s.
fn asset_mesh(
    rel: &str,
    assets: &mut dyn SceneAssets,
    seen: &mut StableMap<String, Arc<MeshData>>,
) -> Result<Arc<MeshData>, Vec<Diagnostic>> {
    if let Some(known) = seen.as_inner().get(rel) {
        return Ok(Arc::clone(known));
    }
    let data = assets.mesh(rel)?;
    seen.insert(rel.to_string(), Arc::clone(&data));
    Ok(data)
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
fn scene_bounds<'a>(
    instances: impl Iterator<Item = &'a Instance>,
    boxes: &mut MeshBoxes,
) -> Option<Aabb> {
    let mut bounds: Option<Aabb> = None;
    for instance in instances {
        if let Some(world) = world_box(instance, boxes) {
            bounds = Some(union(bounds, world));
        }
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
/// `L2105` when the lattice would carry more than [`max_gi_probes`] probes in total, or more than
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
    if total > max_gi_probes() as f64 {
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
             of {}",
            counts[0],
            counts[1],
            counts[2],
            box_span[0],
            box_span[1],
            box_span[2],
            max_gi_probes(),
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
             {} probes, which is within the {}-probe total)",
            counts[axis],
            box_span[0],
            box_span[1],
            box_span[2],
            counts[0],
            counts[1],
            counts[2],
            max_gi_probes(),
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

/// The first repeated doubling of `probe_spacing` whose probe count fits under [`max_gi_probes`],
/// with that count — [`fitting_cell_size`]'s rule for the lattice, and bounded the same way so a
/// degenerate spacing returns `None` instead of looping.
fn fitting_probe_spacing(probe_spacing: f32, total: f64) -> Option<(f32, f64)> {
    let (mut spacing, mut total) = (probe_spacing, total);
    for _ in 0..64 {
        if total <= max_gi_probes() as f64 {
            return Some((spacing, total));
        }
        spacing *= 2.0;
        total /= 8.0;
    }
    None
}

/// Deposits one instance into the grid, per its solid's rule.
fn rasterize(grid: &mut WorldVoxelGrid, instance: &Instance, boxes: &mut MeshBoxes) {
    if world_box(instance, boxes).is_none() {
        return;
    }
    match &instance.solid {
        Solid::Mesh { mesh } => deposit_triangles(grid, instance, mesh),
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
    let determinant = mat4_determinant(instance.model);
    if determinant == 0.0 || !determinant.is_finite() {
        fill_range(grid, world, instance.color, instance.emissive);
        return;
    }
    let inverse = mat4_inverse(instance.model);
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

/// Relative slack on every triangle/cell overlap test, in cell sizes: a surface lying exactly on a
/// cell face fills both neighbours rather than flickering on rounding.
const SHELL_INFLATION: f32 = 1e-4;

/// Deposits an asset mesh as a conservative surface shell: every triangle, in world space, fills
/// every cell its surface overlaps (triangle/box SAT); the interior is never filled.
///
/// Triangles are walked in index order and the cells of one triangle with `z` outer, `x` inner, all
/// through [`write_cell`], so a later triangle overwrites an earlier one's albedo and emissive — the
/// combine rule within one instance; across instances it is ascending signature-byte order. Albedo is `mean(three vertex colours) × instance colour ×
/// MeshData::base_color_mean`; emissive is the mean of the three vertices' authored emissive when the
/// document authors a material (`authored_material`, the render path's selection) and the
/// instance's otherwise. A triangle with an out-of-range index is skipped.
fn deposit_triangles(grid: &mut WorldVoxelGrid, instance: &Instance, mesh: &MeshData) {
    let world: Vec<Vec3> = mesh
        .positions
        .chunks_exact(3)
        .map(|p| instance.model.transform_point3(Vec3::new(p[0], p[1], p[2])))
        .collect();
    let slack = grid.cell_size * SHELL_INFLATION;
    let half = Vec3::splat(grid.cell_size * 0.5 + slack);
    let tint = modulate(instance.color, mesh.base_color_mean);
    for triangle in mesh.indices.chunks_exact(3) {
        let [i0, i1, i2] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let (Some(&a), Some(&b), Some(&c)) = (world.get(i0), world.get(i1), world.get(i2)) else {
            continue;
        };
        let albedo = modulate(mean_color(mesh, [i0, i1, i2]), tint);
        let emissive = if mesh.authored_material {
            let sum: f32 = [i0, i1, i2]
                .iter()
                .map(|&v| mesh.materials[v * 3 + 2])
                .sum();
            sum / 3.0
        } else {
            instance.emissive
        };
        let aabb = Aabb {
            min: a.min(b).min(c) - Vec3::splat(slack),
            max: a.max(b).max(c) + Vec3::splat(slack),
        };
        let Some((lo, hi)) = cell_range(grid, aabb) else {
            continue;
        };
        if lo == hi {
            write_cell(grid, lo, albedo, emissive);
            continue;
        }
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    if triangle_box_overlap(cell_center(grid, [x, y, z]), half, [a, b, c]) {
                        write_cell(grid, [x, y, z], albedo, emissive);
                    }
                }
            }
        }
    }
}

/// The mean RGBA of three vertices' colours.
fn mean_color(mesh: &MeshData, vertices: [usize; 3]) -> [f32; 4] {
    std::array::from_fn(|channel| {
        let sum: f32 = vertices.iter().map(|&v| mesh.colors[v * 4 + channel]).sum();
        sum / 3.0
    })
}

/// Whether the triangle `tri` overlaps the box of half-extents `half` about `center` — the
/// separating-axis test (Akenine-Möller): the three box axes, the triangle's plane, and the nine
/// edge × axis cross products. Only multiply, add, abs, min and max, so it is exact across machines.
/// A degenerate triangle yields zero-length axes, which separate nothing: it fills the cells its box
/// touches.
fn triangle_box_overlap(center: Vec3, half: Vec3, tri: [Vec3; 3]) -> bool {
    let v = [tri[0] - center, tri[1] - center, tri[2] - center];
    for axis in 0..3 {
        let lo = v[0][axis].min(v[1][axis]).min(v[2][axis]);
        let hi = v[0][axis].max(v[1][axis]).max(v[2][axis]);
        if lo > half[axis] || hi < -half[axis] {
            return false;
        }
    }
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let normal = edges[0].cross(edges[1]);
    if normal.dot(v[0]).abs() > half.dot(normal.abs()) {
        return false;
    }
    for edge in edges {
        for unit in [Vec3::X, Vec3::Y, Vec3::Z] {
            let axis = unit.cross(edge);
            let p = [axis.dot(v[0]), axis.dot(v[1]), axis.dot(v[2])];
            let radius = half.dot(axis.abs());
            if p[0].min(p[1]).min(p[2]) > radius || p[0].max(p[1]).max(p[2]) < -radius {
                return false;
            }
        }
    }
    true
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
/// rule, in the one place it happens. Which writer is last is [`voxelize`]'s instance order.
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

    fn cube(x: f32, color: [f32; 4], emissive: f32) -> Instance {
        Instance {
            model: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            color,
            emissive,
            solid: Solid::Cube,
        }
    }

    /// Builds the grid `voxelize` would for these kept statics and dynamic instances.
    fn grid_of(statics: &StaticGi, instances: &[Instance]) -> WorldVoxelGrid {
        let mut boxes = MeshBoxes::new();
        let all = || statics.instances.iter().chain(instances);
        let bounds = scene_bounds(all(), &mut boxes).expect("the cubes have bounds");
        let mut grid = allocate(bounds, 1.0).expect("the grid fits");
        rasterize_ordered(&mut grid, all(), &mut boxes);
        grid.sealed()
    }

    fn signature_bytes(instance: &Instance) -> Vec<u8> {
        let mut bytes = Vec::new();
        instance_bytes(instance, &mut bytes);
        bytes
    }

    /// Two cubes cover the same cells with different colour and emissive; a third sits apart. Any
    /// order of the instances gives one grid, and the shared cells hold the cube whose signature
    /// bytes sort last. Equal signature bytes are one deposit, so a tie cannot change the grid.
    #[test]
    fn a_permuted_instance_order_voxelizes_identically() {
        let red = || cube(0.0, [1.0, 0.0, 0.0, 1.0], 0.0);
        let blue = || cube(0.0, [0.0, 0.0, 1.0, 1.0], 2.0);
        let far = || cube(6.0, [0.0, 1.0, 0.0, 1.0], 0.0);
        let statics = StaticGi::default();
        let forward = grid_of(&statics, &[red(), blue(), far()]);
        let backward = grid_of(&statics, &[far(), blue(), red()]);
        assert_eq!(forward.albedo, backward.albedo);
        assert_eq!(forward.emissive, backward.emissive);
        assert_eq!(forward.occupancy, backward.occupancy);
        assert_eq!(forward.hash, backward.hash);

        let (last, first) = if signature_bytes(&red()) > signature_bytes(&blue()) {
            (red(), blue())
        } else {
            (blue(), red())
        };
        let holding = |who: &Instance| {
            (0..forward.occupancy.len())
                .filter(|&i| forward.albedo[i] == who.color && forward.emissive[i] == who.emissive)
                .count()
        };
        assert!(
            holding(&last) > 0,
            "the later signature owns the shared cells"
        );
        assert_eq!(
            holding(&first),
            0,
            "the earlier signature is overwritten everywhere"
        );
    }

    /// Which instances sit in the kept statics does not change the grid.
    #[test]
    fn a_static_set_does_not_change_the_grid() {
        let make = || {
            vec![
                cube(0.0, [1.0, 0.0, 0.0, 1.0], 0.0),
                cube(0.0, [0.0, 0.0, 1.0, 1.0], 2.0),
                cube(4.0, [0.0, 1.0, 0.0, 1.0], 0.0),
            ]
        };
        let dynamic = grid_of(&StaticGi::default(), &make());
        let mut kept = make();
        let rest = kept.split_off(1);
        let split = grid_of(
            &StaticGi {
                instances: kept,
                digest: 0,
            },
            &rest,
        );
        assert_eq!(dynamic.hash, split.hash);
        assert_eq!(dynamic.albedo, split.albedo);
    }

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
        let over = max_gi_probes() as f64 * 8.0;
        let (spacing, total) = fitting_probe_spacing(1.0, over).expect("one doubling fits");
        assert_eq!(spacing, 2.0);
        assert_eq!(total, max_gi_probes() as f64);
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

    /// A long, thin grid: 4001 x 5 x 5 probes is 100_025, comfortably inside [`max_gi_probes`], so
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

    /// The overlap test against a unit box about the origin (half-extent 0.5).
    fn overlaps(tri: [Vec3; 3]) -> bool {
        triangle_box_overlap(Vec3::ZERO, Vec3::splat(0.5), tri)
    }

    /// A triangle through the box overlaps it; one whose bounding box touches the box but whose
    /// surface passes the far side of a corner does not (the case an AABB-only fill gets wrong).
    #[test]
    fn a_triangle_overlaps_a_box_only_where_its_surface_does() {
        let through = [
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::new(2.0, -1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
        ];
        assert!(overlaps(through));

        // Its AABB spans [0.4, 2] on x and y and covers the box corner at (0.5, 0.5), but the
        // hypotenuse x + y = 2.5 stays well clear of the box.
        let clear_of_corner = [
            Vec3::new(2.0, 0.4, 0.0),
            Vec3::new(0.4, 2.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
        ];
        assert!(!overlaps(clear_of_corner));
        let grazing_corner = [
            Vec3::new(0.5, 1.0, 0.0),
            Vec3::new(1.0, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
        ];
        assert!(
            overlaps(grazing_corner),
            "touching at a point is an overlap"
        );
        let past_corner = [
            Vec3::new(0.2, 1.2, 0.0),
            Vec3::new(1.2, 0.2, 0.0),
            Vec3::new(1.2, 1.2, 0.0),
        ];
        assert!(
            !overlaps(past_corner),
            "x + y = 1.4 clears the corner sum of 1.0"
        );
    }

    /// A triangle lying exactly on a box face overlaps it, and a sliver crossing the box does too.
    #[test]
    fn a_triangle_on_a_face_or_a_sliver_overlaps() {
        let on_face = [
            Vec3::new(0.5, -1.0, -1.0),
            Vec3::new(0.5, 1.0, -1.0),
            Vec3::new(0.5, 0.0, 1.0),
        ];
        assert!(overlaps(on_face));
        let sliver = [
            Vec3::new(-2.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 1e-6, 0.0),
        ];
        assert!(overlaps(sliver));
        let degenerate = [Vec3::ZERO; 3];
        assert!(overlaps(degenerate));
    }

    /// One triangle spanning a 2 x 2 cell patch, with distinct vertex colours and a non-white
    /// `base_color_mean`: the deposited albedo is mean(vertex) x instance x mean.
    #[test]
    fn deposited_albedo_applies_the_base_colour_mean() {
        let mut mesh = MeshData {
            positions: vec![0.0, 0.5, 0.0, 1.0, 0.5, 0.0, 0.0, 0.5, 1.0],
            normals: vec![0.0; 9],
            colors: vec![
                1.0, 0.0, 0.0, 1.0, //
                0.0, 1.0, 0.0, 1.0, //
                0.0, 0.0, 1.0, 1.0,
            ],
            materials: [1.0, 0.0, 0.0].repeat(3),
            uvs: vec![0.0; 6],
            tangents: [1.0, 0.0, 0.0, 1.0].repeat(3),
            weights: [1.0, 0.0, 0.0, 0.0].repeat(3),
            bone_indices: vec![0; 12],
            skin_joints: Vec::new(),
            textures: None,
            authored_material: false,
            base_color_mean: [1.0; 4],
            rim: None,
            double_sided: false,
            indices: vec![0, 1, 2],
            hash: 1,
            lods: Vec::new(),
        };
        let instance = |mesh: &MeshData| Instance {
            model: Mat4::IDENTITY,
            color: [0.5, 1.0, 1.0, 1.0],
            emissive: 3.0,
            solid: Solid::Mesh {
                mesh: Arc::new(mesh.clone()),
            },
        };
        let deposit = |mesh: &MeshData| {
            let mut grid = empty_grid([2, 2, 2], 1.0);
            deposit_triangles(&mut grid, &instance(mesh), mesh);
            grid
        };

        let white = deposit(&mesh);
        let third = 1.0_f32 / 3.0;
        let cell = white.index([0, 0, 0]);
        assert!(white.occupancy[cell]);
        assert_eq!(white.albedo[cell], [0.5 * third * 1.0, third, third, 1.0]);
        assert_eq!(
            white.emissive[cell], 3.0,
            "no authored material: the component's"
        );

        mesh.base_color_mean = [0.5, 0.25, 1.0, 1.0];
        let tinted = deposit(&mesh);
        assert_eq!(
            tinted.albedo[cell],
            [0.5 * third * 0.5, third * 0.25, third, 1.0]
        );

        mesh.authored_material = true;
        mesh.materials = vec![1.0, 0.0, 3.0, 1.0, 0.0, 6.0, 1.0, 0.0, 0.0];
        assert_eq!(deposit(&mesh).emissive[cell], 3.0, "mean of 3, 6, 0");
    }

    /// A lattice exactly at the derived cap passes; one that is over it is `L2105`.
    #[test]
    fn the_derived_probe_cap_is_the_edge_of_l2105() {
        let cap = max_gi_probes();
        // 127 cells of 1.0 at spacing 1.0 is 128 probes an axis: 128^3 = 2^21.
        let at_cap = empty_grid([1, 1, 1], 127.0);
        let lattice = probe_lattice(&at_cap, 1.0).expect("a lattice at the cap fits");
        assert_eq!(lattice.count, [128, 128, 128]);
        assert_eq!(
            cap,
            128 * 128 * 128,
            "the engine's 128 MiB binding at 64 B/probe"
        );

        let over = empty_grid([1, 1, 1], 127.5);
        let error = probe_lattice(&over, 1.0).expect_err("129 x 129 x 129 probes is over the cap");
        assert_eq!(error.code.as_str(), "L2105");
        assert!(
            error.message.contains(&cap.to_string()),
            "{}",
            error.message
        );
    }
}
