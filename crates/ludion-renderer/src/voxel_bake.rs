//! GPU holder for the voxelizer's [`WorldVoxelGrid`].
//!
//! [`VoxelBake::resolve`] uploads a finished CPU grid to a pair of 3D textures and stops there — it
//! runs no pass of its own. It mirrors `ibl.rs`'s shape: a `Renderer` field built once in
//! [`crate::render::Renderer::new`], a single-entry content-hash cache consulted every frame inside
//! `Renderer::prepare_frame`. A scene with no `GiVolume` extracts `RenderScene.voxel` as `None`,
//! which costs [`VoxelBake::resolve`] one `Option` test and touches no GPU state.
//!
//! Both textures are handed back from `resolve`: `gi_bake.rs`'s compute pass `textureLoad`s the
//! occupancy channel to march rays through the world, and reads albedo + emissive at a hit to form
//! the bounce's `albedo × (emissive + direct)`. No render pass samples either.
//!
//! **Packing.** [`WorldVoxelGrid`] carries three parallel per-cell fields: RGBA albedo, a scalar
//! emissive, and an occupancy bool. Two textures hold them losslessly rather than folding them into
//! one, so every field survives the upload bit-for-bit with no sentinel value and no dropped channel:
//! `albedo` (`Rgba32Float`, the grid's own `[f32; 4]` verbatim) and `emissive_occupancy`
//! (`Rg32Float`, `r` = emissive, `g` = occupancy as `0.0`/`1.0`). Both carry
//! `STORAGE_BINDING | TEXTURE_BINDING | COPY_DST`; only `TEXTURE_BINDING` has a reader. Full `f32`
//! precision throughout: this is a load-time upload keyed on content, not a per-frame budget, so
//! there is no reason to spend `ibl.rs`'s `f16` working-format compromise here.
//!
//! **Cache key.** [`WorldVoxelGrid::hash`] — the content hash of the grid's own bytes, computed once
//! by the voxelizer that built it. Every input that affects the grid (transforms, palettes, colors)
//! is already resolved into its buffers, so those bytes are a complete key with no per-instance state
//! left to re-gather; and because the producer stamped it, a cache hit costs one `u64` compare rather
//! than a walk of a multi-megabyte grid.
//!
//! **Upload.** `wgpu::Queue::write_texture` (unlike `CommandEncoder::copy_buffer_to_texture`) has no
//! 256-byte `bytes_per_row` alignment requirement — confirmed against the pinned wgpu 30.0.0 source
//! (`TexelCopyBufferLayout::get_buffer_texture_copy_info` only requires `bytes_per_row` to be at
//! least the dense row size), matching how `render.rs::upload_texture` already writes the font atlas
//! at an arbitrary, unpadded width. Both textures upload tightly packed in one `write_texture` call
//! each.

use std::cell::RefCell;

use ludion_render_graph::Gpu;

use crate::scene::{MAX_GI_TEXTURE_AXIS, WorldVoxelGrid};

/// Full `f32` precision RGBA — the grid's own `albedo` verbatim, no channel dropped.
const ALBEDO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// `r` = emissive, `g` = occupancy as `0.0`/`1.0` — a second texture rather than overloading
/// `albedo`'s alpha, so every `WorldVoxelGrid` field keeps its own lossless channel.
const EMISSIVE_OCCUPANCY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;

/// One uploaded grid, keyed by its content hash. The cache's whole purpose is keeping these GPU
/// handles alive across frames so an unchanged grid costs nothing.
struct Cached {
    hash: u64,
    textures: ResolvedVoxel,
}

/// The pair of 3D textures one grid uploads to. Handed out together, because the GI bake reads both
/// off the same cell and a caller holding one without the other has nothing to do with it.
#[derive(Clone)]
pub(crate) struct ResolvedVoxel {
    /// Per-cell RGBA albedo (`Rgba32Float`), the grid's own `albedo` verbatim.
    pub albedo: wgpu::Texture,
    /// Per-cell `r` = emissive, `g` = occupancy (`Rg32Float`).
    pub emissive_occupancy: wgpu::Texture,
}

/// The inert GPU holder for a [`WorldVoxelGrid`]: no pipelines (there is no compute here, only an
/// upload), just the single-entry content-hash cache.
pub(crate) struct VoxelBake {
    cache: RefCell<Option<Cached>>,
    /// **Test-only.** Counts calls to [`upload_grid`] — the proof a cache hit skips the upload
    /// entirely rather than merely reproducing the same hash.
    #[cfg(test)]
    uploads: std::cell::Cell<u32>,
}

impl VoxelBake {
    /// An empty holder — there is nothing to build at renderer start; the first `resolve` call with a
    /// `Some` grid performs the first upload.
    pub(crate) fn new() -> VoxelBake {
        VoxelBake {
            cache: RefCell::new(None),
            #[cfg(test)]
            uploads: std::cell::Cell::new(0),
        }
    }

    /// [`resolve`](VoxelBake::resolve) for a derived frame that may draw without GI: no grid leaves the
    /// main frame's cached textures alone.
    pub(crate) fn resolve_keeping(
        &self,
        gpu: &Gpu,
        voxel: Option<&WorldVoxelGrid>,
    ) -> Option<ResolvedVoxel> {
        voxel.and_then(|_| self.resolve(gpu, voxel))
    }

    /// Resolves `voxel` for this frame, handing back both textures the GI bake reads (the returned
    /// handles are refcount bumps, not copies). `None` (no `GiVolume`, the overwhelming common case)
    /// drops any cached textures and returns `None`. `Some(grid)` compares the grid's stamped
    /// [`WorldVoxelGrid::hash`]: a cache hit is a `u64` compare and nothing more, a miss (re)creates
    /// and uploads both textures.
    pub(crate) fn resolve(
        &self,
        gpu: &Gpu,
        voxel: Option<&WorldVoxelGrid>,
    ) -> Option<ResolvedVoxel> {
        let Some(grid) = voxel else {
            *self.cache.borrow_mut() = None;
            return None;
        };

        let hash = grid.hash;
        if let Some(cached) = self.cache.borrow().as_ref().filter(|c| c.hash == hash) {
            return Some(cached.textures.clone());
        }

        #[cfg(test)]
        self.uploads.set(self.uploads.get() + 1);
        let (albedo, emissive_occupancy) = upload_grid(gpu, grid);
        let textures = ResolvedVoxel {
            albedo,
            emissive_occupancy,
        };
        *self.cache.borrow_mut() = Some(Cached {
            hash,
            textures: textures.clone(),
        });
        Some(textures)
    }

    /// **Test-only.** The cached grid's hash, or `None` with no cached upload — observes a cache
    /// hit/miss/clear without reaching into GPU state.
    #[cfg(test)]
    pub(crate) fn cached_hash(&self) -> Option<u64> {
        self.cache.borrow().as_ref().map(|c| c.hash)
    }

    /// **Test-only.** How many times [`upload_grid`] has actually run.
    #[cfg(test)]
    pub(crate) fn upload_count(&self) -> u32 {
        self.uploads.get()
    }
}

/// Creates and uploads both 3D textures for `grid`, tightly packed (see the module doc's alignment
/// note) — one `write_texture` call each.
fn upload_grid(gpu: &Gpu, grid: &WorldVoxelGrid) -> (wgpu::Texture, wgpu::Texture) {
    let device = gpu.device();
    let [ex, ey, ez] = grid.extent;
    let size = wgpu::Extent3d {
        width: ex,
        height: ey,
        depth_or_array_layers: ez,
    };

    let albedo = texture_3d(device, "voxel bake albedo", size, ALBEDO_FORMAT);
    let mut albedo_bytes = Vec::with_capacity(grid.albedo.len() * 16);
    for c in &grid.albedo {
        for v in c {
            albedo_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    write_3d(gpu, &albedo, &albedo_bytes, ex * 16, ey, size);

    let emissive_occupancy = texture_3d(
        device,
        "voxel bake emissive/occupancy",
        size,
        EMISSIVE_OCCUPANCY_FORMAT,
    );
    let mut eo_bytes = Vec::with_capacity(grid.emissive.len() * 8);
    for (i, &e) in grid.emissive.iter().enumerate() {
        let occ: f32 = if grid.occupancy[i] { 1.0 } else { 0.0 };
        eo_bytes.extend_from_slice(&e.to_le_bytes());
        eo_bytes.extend_from_slice(&occ.to_le_bytes());
    }
    write_3d(gpu, &emissive_occupancy, &eo_bytes, ex * 8, ey, size);

    (albedo, emissive_occupancy)
}

/// A 3D texture sized `size` in `format`, `STORAGE_BINDING | TEXTURE_BINDING | COPY_DST`. Only
/// `TEXTURE_BINDING` has a reader (the GI bake's `textureLoad`); the other two are declared
/// capability.
///
/// The size is an invariant, not a runtime branch: `ludion-engine`'s voxelizer caps every grid axis
/// at [`MAX_GI_TEXTURE_AXIS`] and reports `L2103` before it allocates, so an oversized grid cannot
/// reach here. Clamping or skipping instead would hand back a silently wrong volume, which is the
/// "authored, validated, inert" failure the GI arc exists to avoid.
fn texture_3d(
    device: &wgpu::Device,
    label: &str,
    size: wgpu::Extent3d,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    debug_assert!(
        size.width.max(size.height).max(size.depth_or_array_layers) <= MAX_GI_TEXTURE_AXIS,
        "voxel grid {}x{}x{} exceeds the {MAX_GI_TEXTURE_AXIS}-texel GI axis cap that \
         ludion-engine's `allocate` enforces as L2103",
        size.width,
        size.height,
        size.depth_or_array_layers,
    );
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Writes `data` to all of `texture` in one call: tightly packed rows (`bytes_per_row`, no 256-byte
/// pad — see the module doc), `rows` rows per z-layer, `size.depth_or_array_layers` layers.
fn write_3d(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    data: &[u8],
    bytes_per_row: u32,
    rows: u32,
    size: wgpu::Extent3d,
) {
    gpu.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(rows),
        },
        size,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::Vec3;

    fn skip_or_gpu(name: &str) -> Option<Gpu> {
        match Gpu::request() {
            Ok(gpu) => Some(gpu),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED {name}: no GPU adapter");
                None
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        }
    }

    /// A small fully-occupied grid, every cell filled with `fill`, sealed the way the voxelizer
    /// seals one.
    fn small_grid(fill: f32) -> WorldVoxelGrid {
        let extent = [2u32, 2, 2];
        let cells = 8usize;
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size: 0.5,
            extent,
            occupancy: vec![true; cells],
            albedo: vec![[fill, fill, fill, 1.0]; cells],
            emissive: vec![fill; cells],
            hash: 0,
        }
        .sealed()
    }

    /// Resolving the same grid twice uploads once: the second call is a cache hit, not a second
    /// `write_texture` round trip.
    #[test]
    fn needs_gpu_resolve_twice_with_the_same_grid_is_one_upload() {
        let Some(gpu) = skip_or_gpu("resolve_twice_with_the_same_grid_is_one_upload") else {
            return;
        };
        let bake = VoxelBake::new();
        let grid = small_grid(0.5);

        bake.resolve(&gpu, Some(&grid));
        assert_eq!(bake.upload_count(), 1, "the first resolve uploads");

        bake.resolve(&gpu, Some(&grid));
        assert_eq!(
            bake.upload_count(),
            1,
            "an unchanged grid is a cache hit, not a second upload"
        );
    }

    /// `None` after `Some` drops the cached textures rather than leaving the last upload's hash
    /// resident — a scene that stops authoring `GiVolume` (or a following env-less scene) must not
    /// find a stale cache hit.
    #[test]
    fn needs_gpu_resolving_none_after_some_clears_the_cache() {
        let Some(gpu) = skip_or_gpu("resolving_none_after_some_clears_the_cache") else {
            return;
        };
        let bake = VoxelBake::new();
        let grid = small_grid(0.5);

        bake.resolve(&gpu, Some(&grid));
        assert!(bake.cached_hash().is_some(), "a Some grid caches an upload");

        bake.resolve(&gpu, None);
        assert!(
            bake.cached_hash().is_none(),
            "None drops the cached textures"
        );
    }

    /// A single changed cell in any one per-cell buffer moves the cache key — the combine rule's
    /// last-writer value is part of what the key must cover, not just the grid's shape. Each buffer
    /// is changed in its own clone, so a key that skips one of them cannot hide behind the others.
    #[test]
    fn every_per_cell_buffer_moves_the_hash() {
        let grid = small_grid(0.5);
        let base = grid.hash;

        let mut albedo = grid.clone();
        albedo.albedo[0][0] += 0.25;
        let mut emissive = grid.clone();
        emissive.emissive[0] = 9.0;
        let mut occupancy = grid.clone();
        occupancy.occupancy[0] = !occupancy.occupancy[0];

        for (buffer, changed) in [
            ("albedo", albedo),
            ("emissive", emissive),
            ("occupancy", occupancy),
        ] {
            assert_ne!(
                base,
                changed.content_hash(),
                "a single cell's {buffer} value must move the cache key"
            );
        }
    }

    /// Hashing the same grid twice, with no GPU or global state involved, is bit-identical — the
    /// mesher's `double_run_is_bit_identical` precedent applied to the cache key.
    #[test]
    fn grid_hash_is_deterministic_across_runs() {
        let grid = small_grid(0.25);
        assert_eq!(
            grid.content_hash(),
            grid.content_hash(),
            "hashing the same grid twice must be bit-identical"
        );
    }

    /// The stamped field is the grid's own bytes — a sealed grid's `hash` re-derives exactly, so the
    /// value `resolve` keys on cannot silently drift from the buffers it stands for.
    #[test]
    fn the_stamped_hash_equals_a_freshly_computed_one() {
        let grid = small_grid(0.75);
        assert_eq!(
            grid.hash,
            grid.content_hash(),
            "`sealed` must stamp exactly what `content_hash` computes"
        );
    }
}
