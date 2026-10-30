//! The clustered-forward light payload: the frame's light array, the header uniform, and the
//! per-cell index tables the lit shaders walk (`Plan/clustered-forward.md` "The design").
//!
//! The GPU side lives in `shaders/snippets/lights.wgsl` — the grid consts, the cell index, and the
//! `cluster_slice` lookup. This module is its CPU mirror: it decides which lights each cell's list
//! contains, and it is the only place that decision is made.
//!
//! **Conservatism is mandatory, tightness is negotiable.** Every light whose range-sphere can reach
//! a fragment in a cell must appear in that cell's list. A light admitted *spuriously* costs a loop
//! iteration and contributes exactly `+0.0` (the shaders' attenuation is a hard zero at
//! `dist >= range`, and `acc + 0.0` is bit-preserving), so over-inclusion cannot move a pixel; a
//! light *missed* is a visible hole. Every margin below therefore rounds outward, and the randomized
//! oracle in `render.rs` renders the same scene through this build and through the degenerate
//! all-lights-in-every-cell one and byte-compares (decision 7).
//!
//! A corollary worth stating, because it is what keeps this file off the determinism-critical path:
//! since the *rendered result* is independent of over-inclusion, two platforms whose `log2` disagree
//! in the last bit may bin differently and still render identically. The binning must be
//! conservative, not bit-reproducible.

use bytemuck::Zeroable;
use ludion_core::math::glam::{Mat4, Vec3};

use crate::probe::{MAX_PROBES, ProbeData, ProbeVolume};
use crate::scene::{RenderCamera, RenderScene};
use crate::shader_gen::{CLUSTER_SLICES, CLUSTER_TILES_X, CLUSTER_TILES_Y, Light, LightHeader};

/// `Light.axis` for a point light: `w < -1.5` is the shaders' point-light marker, which skips the
/// spot cone factor entirely.
pub(crate) const POINT_LIGHT_AXIS: [f32; 4] = [0.0, 0.0, 0.0, -2.0];
/// The relative widening of a spot's cone bounding sphere — slack for the GPU's rounded `l` and
/// `dot(-l, axis)` putting a fragment a rounding error outside the exact cone yet still lit.
const CONE_SPHERE_MARGIN: f32 = 1e-3;

/// The cluster grid's cell count — the CPU side of the WGSL grid consts, so `cluster_table`'s length
/// and the shader's cell index cannot disagree about the grid's shape.
const CLUSTER_CELLS: usize = (CLUSTER_TILES_X * CLUSTER_TILES_Y * CLUSTER_SLICES) as usize;
/// The radial shell ladder's fixed distance range, in world units: the shells divide
/// `[CLUSTER_SHELL_NEAR, CLUSTER_SHELL_FAR]` geometrically, so each is a constant *fraction* wider
/// than the one before it rather than a constant number of units — the near shells stay fine where
/// point lights crowd, the far ones stay cheap. Both are exact powers of two, which makes the
/// `log2`-derived scale/bias below exact on every platform (`Plan/clustered-forward.md` decision 4;
/// a per-frame fitted range is deferred until occupancy stats measure the waste).
const CLUSTER_SHELL_NEAR: f32 = 1.0;
const CLUSTER_SHELL_FAR: f32 = 512.0;

/// How far, as a fraction, the radial interval `[d - range, d + range]` is widened before it is
/// turned into shell indices.
///
/// The shell a fragment reads comes from a GPU `log2` of a distance the *rasterizer* interpolated;
/// the interval here comes from a CPU `log2` of a distance computed from the light's centre. The two
/// agree to a handful of ulps at worst (WGSL's `log2` is specified to ~2⁻²¹ relative), which is under
/// 1e-5 in ladder units — this margin is ~2.6e-2 in the same units, three orders of magnitude of
/// slack. It is a *relative* widening rather than the blunter "add a shell at each end" because a
/// shell is a factor of ~1.48 in distance: widening by one shell would inflate a typical torch's
/// three-shell interval to five, where 1% inflates it only when an endpoint is already within 1% of
/// a shell boundary.
const SHELL_MARGIN: f32 = 0.01;
/// How far, in pixels, the light's projected screen AABB is grown before it is turned into tile
/// indices. Same job as [`SHELL_MARGIN`] on the other axis: the projection is exact convex-hull
/// arithmetic (see [`tile_range`]) but is computed in `f32` from the light's centre while the shader
/// reads a rasterized pixel centre, so a lit fragment could in principle sit a rounding error
/// outside the computed box. One pixel buys an extra tile only when a box edge lands within a pixel
/// of a tile boundary.
const TILE_PIXEL_MARGIN: f32 = 1.0;
/// The smallest clip-space `w` a projected AABB corner may have before [`tile_range`] gives up and
/// takes the full-tile fallback. `w` is affine in the point, so `w > 0` at all eight corners proves
/// the whole box is in front of the camera — the precondition the convex-hull argument needs. The
/// epsilon only keeps the division away from denormals; a corner just barely in front yields a
/// huge-but-finite box, which clamps to the full tile range anyway.
const MIN_CLIP_W: f32 = 1e-4;

/// How crowded the frame's cluster tables came out — the occupancy panel a light-cost regression is
/// diagnosed from (`Plan/clustered-forward.md`, "Introspection").
///
/// Every field is a **free by-product** of the counting sort in `cull`: the per-cell counts it
/// already builds are reduced once, in the same pass that prefix-sums them. Nothing here is
/// measured, sampled or timed — it is the shape of the tables themselves, so two runs of the same
/// frame produce identical numbers and a change in them is a change in the binning, never noise.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClusterStats {
    /// Lights in the frame's global array — `scene.lights.len()`, before the empty-scene padding
    /// element `build_lights` appends to keep the binding non-zero-sized.
    pub lights: u32,
    /// Cells in the grid. A compile-time constant today; carried anyway so a report reads without
    /// the reader having to know the grid's shape.
    pub cells: u32,
    /// How many cells hold at least one light. The complement is free frame — cells a fragment can
    /// read and pay nothing for.
    pub occupied_cells: u32,
    /// Total index references across every cell — the length of `cluster_lights`, and the honest
    /// measure of what the binning produced. This is the number that grows when conservatism gets
    /// looser, whether or not any single list gets longer.
    pub total_refs: u64,
    /// The longest single cell's list — the worst-case loop a fragment can pay.
    pub max_list_len: u32,
    /// The mean list length **over occupied cells** — the loop length a lit fragment pays where the
    /// loop runs at all. The all-cells mean is `total_refs / cells`, one division away for anyone
    /// who wants it, which is why this one is the occupied mean rather than a second spelling of a
    /// derivable number. `0.0` when nothing is occupied.
    pub mean_list_len: f64,
}

/// The frame's whole `@group(1)` payload: the header uniform plus the three storage arrays the
/// clustered light loop reads (`Plan/clustered-forward.md` "The design").
pub(crate) struct LightUpload {
    pub(crate) header: LightHeader,
    /// Every light in the scene, in `ludion-engine`'s ascending-`EntityId` extract order.
    pub(crate) lights: Vec<Light>,
    /// One `(offset, count)` per cell, in `(shell, tile_y, tile_x)` row-major order — the order the
    /// snippet's `cluster_slice` indexes.
    pub(crate) table: Vec<[u32; 2]>,
    /// The concatenated per-cell index lists; each entry indexes `lights`.
    pub(crate) indices: Vec<u32>,
    /// How crowded `table`/`indices` came out — reduced from the same per-cell counts that built
    /// them, never from a second pass over the tables.
    pub(crate) stats: ClusterStats,
}

/// Which cluster table [`build_lights`] builds.
///
/// The shipping build has exactly one variant: the degenerate oracle is `#[cfg(test)]`, so it is not
/// a mode a running engine can be put into — it exists only so a test can render the same scene both
/// ways through the same pipeline and byte-compare (`Plan/clustered-forward.md` decision 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ClusterBuild {
    /// Per-light conservative culling: a light is filed only into the cells its range-sphere can
    /// reach.
    Culled,
    /// Every light in every cell — the conservative extreme, and therefore the reference the culled
    /// build must render byte-identically to.
    #[cfg(test)]
    Degenerate,
}

/// Builds the clustered-forward light payload from the scene: **every** light, premultiplied, in
/// extract order, plus the cell tables the shader indexes.
///
/// **The order is load-bearing** (`Plan/clustered-forward.md` decision 2 / the identity argument).
/// `scene.lights` arrives in ascending-`EntityId` extract order and is packed verbatim — never
/// sorted, never filtered — followed by `scene.spot_lights` in their own extract order, so a scene
/// without spots packs exactly as a point-only build did (`Plan/spot-light.md` decision 4). A
/// fragment sums its cell's lights in ascending index order, so that order *is* the float summation
/// order of every lit pixel; reordering here churns every lit golden. The binning below preserves it
/// by construction: lights iterate outer, their cells inner, so each cell's list is appended to in
/// ascending light index and needs no sort.
pub(crate) fn build_lights(
    scene: &RenderScene,
    render_extent: (u32, u32),
    build: ClusterBuild,
) -> LightUpload {
    let points = scene.lights.iter().map(|light| {
        let c = light.color * light.intensity;
        Light {
            position: [
                light.position.x,
                light.position.y,
                light.position.z,
                light.range,
            ],
            color: [c.x, c.y, c.z, 0.0],
            axis: POINT_LIGHT_AXIS,
        }
    });
    let spots = scene.spot_lights.iter().map(|spot| {
        let c = spot.color * spot.intensity;
        let a = spot.direction;
        Light {
            position: [
                spot.position.x,
                spot.position.y,
                spot.position.z,
                spot.range,
            ],
            color: [c.x, c.y, c.z, spot.cos_inner],
            axis: [a.x, a.y, a.z, spot.cos_outer],
        }
    });
    let mut lights: Vec<Light> = points.chain(spots).collect();

    let (scale, bias) = shell_ladder();
    let sun = scene.sun.as_ref();
    let header = LightHeader {
        ambient: [scene.ambient[0], scene.ambient[1], scene.ambient[2], 1.0],
        grid: [
            1.0 / render_extent.0.max(1) as f32,
            1.0 / render_extent.1.max(1) as f32,
            scale,
            bias,
        ],
        sun_direction: sun.map_or([0.0; 4], |s| {
            [s.direction.x, s.direction.y, s.direction.z, 0.0]
        }),
        sun_color: sun.map_or([0.0; 4], |s| [s.color.x, s.color.y, s.color.z, 0.0]),
    };

    let (table, mut indices, mut stats) = match build {
        ClusterBuild::Culled => cull(&lights, &scene.camera, render_extent, (scale, bias)),
        #[cfg(test)]
        ClusterBuild::Degenerate => degenerate(lights.len()),
    };
    stats.lights = lights.len() as u32;

    // wgpu rejects a zero-sized buffer binding, and the generated BGL's `min_binding_size` is one
    // element (naga sizes a runtime array as exactly one), so an empty array uploads as a single
    // zeroed element. Every cell's count is 0, so the shader never reads the padding.
    if lights.is_empty() {
        lights.push(Light::zeroed());
    }
    if indices.is_empty() {
        indices.push(0);
    }

    LightUpload {
        header,
        lights,
        table,
        indices,
        stats,
    }
}

/// Builds the frame's cluster tables and returns only how crowded they came out — the occupancy
/// panel, with the tables themselves dropped.
///
/// This is the profile driver's entry point (`ludion_engine::profile`): it does exactly the work
/// the renderer's per-frame prologue does, so timing this call times the real build, and it needs no
/// GPU adapter — the binning is pure CPU arithmetic over the extracted scene. Deliberately a
/// separate call rather than a hook inside [`Renderer::render`](crate::Renderer::render): a
/// profiler must never make the path it measures pay for being measured, and an unprofiled frame
/// keeps its prologue exactly as it was.
pub fn cluster_stats(scene: &RenderScene, render_extent: (u32, u32)) -> ClusterStats {
    build_lights(scene, render_extent, ClusterBuild::Culled).stats
}

/// The radial shell ladder's `(scale, bias)`, the pair the header hands the shader.
///
/// `shell = log2(dist) * scale + bias` maps `[NEAR, FAR]` onto `[0, CLUSTER_SLICES)`. The shader is
/// handed the *squared* distance so it needs no square root, so the `0.5` that `log2(d²) = 2·log2(d)`
/// asks for is folded into `scale` here — one CPU multiply instead of one GPU op per lit fragment.
fn shell_ladder() -> (f32, f32) {
    let scale =
        0.5 * CLUSTER_SLICES as f32 / (CLUSTER_SHELL_FAR.log2() - CLUSTER_SHELL_NEAR.log2());
    (scale, -2.0 * CLUSTER_SHELL_NEAR.log2() * scale)
}

/// The cells one light covers: an inclusive tile rectangle × an inclusive shell interval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Footprint {
    tx: (u32, u32),
    ty: (u32, u32),
    shell: (u32, u32),
}

impl Footprint {
    /// How many cells this footprint spans.
    fn cell_count(&self) -> usize {
        let span = |(lo, hi): (u32, u32)| (hi - lo + 1) as usize;
        span(self.tx) * span(self.ty) * span(self.shell)
    }

    /// Every cell index this footprint covers, in `(shell, tile_y, tile_x)` row-major order.
    fn cells(&self) -> impl Iterator<Item = usize> + '_ {
        (self.shell.0..=self.shell.1).flat_map(move |s| {
            (self.ty.0..=self.ty.1).flat_map(move |ty| {
                (self.tx.0..=self.tx.1)
                    .map(move |tx| ((s * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx) as usize)
            })
        })
    }
}

/// The real build: file each light into exactly the cells its range-sphere can reach.
///
/// Three sequential passes — count, prefix-sum, scatter — which is a counting sort with the *light*
/// index as the key, so each cell's slice comes out ascending with no sort and no per-cell `Vec`.
/// Deliberately sequential: `ludion_core::par` is available, but decision 3's trigger for reaching
/// for it (or for a GPU compute build) is a *measured* 0.3 ms at the stress population, and the
/// build measures inside that budget — `cluster_build_cost_at_the_stress_population` is the
/// measurement, and it is what to re-run before assuming otherwise.
fn cull(
    lights: &[Light],
    camera: &RenderCamera,
    render_extent: (u32, u32),
    ladder: (f32, f32),
) -> (Vec<[u32; 2]>, Vec<u32>, ClusterStats) {
    let view_proj = camera.projection * camera.view;

    let mut counts = vec![0u32; CLUSTER_CELLS];
    let mut footprints: Vec<Footprint> = Vec::with_capacity(lights.len());
    let mut total = 0usize;
    for light in lights {
        let footprint = footprint_of(light, camera.position, &view_proj, render_extent, ladder);
        total += footprint.cell_count();
        for cell in footprint.cells() {
            counts[cell] += 1;
        }
        footprints.push(footprint);
    }

    // `table[c]` becomes `(offset, cursor)`; the cursor is the count the shader wants once every
    // light has been scattered, so the running write position needs no second array. The occupancy
    // panel is reduced out of the same walk — the counts are already in cache and already being
    // read, so the stats cost a compare and a max per cell, not a pass.
    let mut table = vec![[0u32; 2]; CLUSTER_CELLS];
    let mut offset = 0u32;
    let mut occupied = 0u32;
    let mut max_list_len = 0u32;
    for (slot, &count) in table.iter_mut().zip(counts.iter()) {
        slot[0] = offset;
        offset += count;
        occupied += u32::from(count > 0);
        max_list_len = max_list_len.max(count);
    }
    let stats = ClusterStats {
        // Filled by `build_lights`, which owns the light array this build was handed.
        lights: 0,
        cells: CLUSTER_CELLS as u32,
        occupied_cells: occupied,
        total_refs: total as u64,
        max_list_len,
        mean_list_len: if occupied == 0 {
            0.0
        } else {
            total as f64 / f64::from(occupied)
        },
    };

    let mut indices = vec![0u32; total];
    for (index, footprint) in footprints.iter().enumerate() {
        for cell in footprint.cells() {
            let slot = &mut table[cell];
            indices[(slot[0] + slot[1]) as usize] = index as u32;
            slot[1] += 1;
        }
    }

    (table, indices, stats)
}

/// The conformance oracle (`Plan/clustered-forward.md` decisions 6–7): every light in every cell.
///
/// Correct by definition — no sphere test can be more conservative than admitting everything — so
/// any frame the culled build renders differently is a hole in that build, not a difference of
/// opinion. The compact form is exact: one shared index list `[0..N)` that every cell's `(0, N)`
/// entry points at, rather than [`CLUSTER_CELLS`] copies of it.
#[cfg(test)]
fn degenerate(count: usize) -> (Vec<[u32; 2]>, Vec<u32>, ClusterStats) {
    let count = count as u32;
    (
        vec![[0, count]; CLUSTER_CELLS],
        (0..count).collect::<Vec<u32>>(),
        ClusterStats {
            lights: 0,
            cells: CLUSTER_CELLS as u32,
            // The stats describe what the *tables* say, not how they are stored: every cell holds
            // every light, however compactly the shared index list encodes that.
            occupied_cells: if count == 0 { 0 } else { CLUSTER_CELLS as u32 },
            total_refs: count as u64 * CLUSTER_CELLS as u64,
            max_list_len: count,
            mean_list_len: if count == 0 { 0.0 } else { f64::from(count) },
        },
    )
}

/// The cells one light's bounding sphere can reach: a point light's range sphere, or a spot's
/// [`bounding_sphere`].
fn footprint_of(
    light: &Light,
    cam_pos: Vec3,
    view_proj: &Mat4,
    render_extent: (u32, u32),
    ladder: (f32, f32),
) -> Footprint {
    let (center, range) = bounding_sphere(light);
    sphere_footprint(center, range, cam_pos, view_proj, render_extent, ladder)
}

/// The cells a bounding sphere can reach, rounded outward.
fn sphere_footprint(
    center: Vec3,
    range: f32,
    cam_pos: Vec3,
    view_proj: &Mat4,
    render_extent: (u32, u32),
    ladder: (f32, f32),
) -> Footprint {
    // The scalar-f32 discipline the deleted `dist2` documented: three multiply-adds written out, no
    // glam reducing op, no fused intrinsic — one value, computed the same way everywhere.
    let dx = center.x - cam_pos.x;
    let dy = center.y - cam_pos.y;
    let dz = center.z - cam_pos.z;
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();

    // The radial interval every point of the sphere lies in, by the triangle inequality: a fragment
    // the light reaches is within `range` of its centre, so its own camera distance is within
    // `[d - range, d + range]`. Widened by SHELL_MARGIN and clamped at zero.
    let near = (dist - range).max(0.0) * (1.0 - SHELL_MARGIN);
    let far = (dist + range) * (1.0 + SHELL_MARGIN);
    let shell_lo = shell_of(near * near, ladder);
    let shell = (shell_lo, shell_of(far * far, ladder).max(shell_lo));

    let (tx, ty) = tile_range(center, range, view_proj, render_extent)
        .unwrap_or(((0, CLUSTER_TILES_X - 1), (0, CLUSTER_TILES_Y - 1)));

    Footprint { tx, ty, shell }
}

/// Per-cell reflection-probe bitmasks: bit `k` of cell `c` is set when the `k`th probe of `ranked`
/// (rank order) can reach a fragment in `c`. Same conservatism as the light tables — a probe's
/// weight is an exact 0 outside its volume grown by `blend`, so over-inclusion cannot move a pixel.
pub(crate) fn probe_masks(
    ranked: &[ProbeData],
    camera: &RenderCamera,
    render_extent: (u32, u32),
) -> Vec<u32> {
    const _: () = assert!(MAX_PROBES <= 32);
    let view_proj = camera.projection * camera.view;
    let ladder = shell_ladder();
    let mut masks = vec![0u32; CLUSTER_CELLS];
    for (k, probe) in ranked.iter().take(MAX_PROBES).enumerate() {
        let radius = probe.blend
            + match probe.volume {
                ProbeVolume::Box { extent } => extent.length(),
                ProbeVolume::Sphere { radius } => radius,
            };
        let footprint = sphere_footprint(
            probe.position,
            radius,
            camera.position,
            &view_proj,
            render_extent,
            ladder,
        );
        for cell in footprint.cells() {
            masks[cell] |= 1 << k;
        }
    }
    masks
}

/// The sphere every fragment a light can reach lies in, as `(centre, radius)`.
///
/// A point light's is its range sphere. A spot reaches only the cone ∩ range sphere (its cone factor
/// is an exact 0.0 outside the outer cone): for an outer half-angle θ ≤ 45° that sector's minimal
/// bounding sphere passes through the apex and the rim circle — centre `apex + axis·R/(2cosθ)`,
/// radius `R/(2cosθ)` — widened by [`CONE_SPHERE_MARGIN`]; a wider (or malformed) cone takes the
/// range sphere at the apex.
fn bounding_sphere(light: &Light) -> (Vec3, f32) {
    let apex = Vec3::new(light.position[0], light.position[1], light.position[2]);
    let axis = Vec3::new(light.axis[0], light.axis[1], light.axis[2]);
    light_sphere(apex, axis, light.position[3], light.axis[3])
}

/// [`bounding_sphere`] over unpacked fields; a point light passes `cos_outer = -2.0` (see
/// [`POINT_LIGHT_AXIS`]) for its range sphere.
pub(crate) fn light_sphere(apex: Vec3, axis: Vec3, range: f32, cos_outer: f32) -> (Vec3, f32) {
    // Mirrors both consumers' `max(light.position.w, 1e-3)`: a light authored at range 0 still lights
    // a hair of geometry GPU-side, so the CPU must bin it as if it did.
    let range = range.max(1e-3);
    if cos_outer < std::f32::consts::FRAC_1_SQRT_2 || !axis.is_finite() {
        return (apex, range);
    }
    let radius = range / (2.0 * cos_outer);
    (apex + axis * radius, radius * (1.0 + CONE_SPHERE_MARGIN))
}

/// Which shell a squared camera distance falls in — the CPU mirror of the snippet's ladder lookup,
/// down to the `max(_, 1e-12)` guard, the clamp on the float side, and the truncating cast.
fn shell_of(dist_sq: f32, (scale, bias): (f32, f32)) -> u32 {
    let ladder = dist_sq.max(1e-12).log2() * scale + bias;
    ladder.clamp(0.0, (CLUSTER_SLICES - 1) as f32) as u32
}

/// The inclusive tile rectangle a light's range-sphere can touch, or `None` when the conservative
/// full-tile fallback is required.
///
/// **The conservatism argument.** The sphere sits inside its own axis-aligned bounding box, which is
/// the convex hull of eight corners. Clip-space `w` is an *affine* function of a world point, so
/// `w > 0` at all eight corners proves `w > 0` everywhere on the hull; on that halfspace the
/// perspective divide is a projective map, which carries the hull of the corners onto the hull of
/// the projected corners. The screen AABB of the eight projected corners therefore contains the
/// projected box, which contains the projected sphere. Any corner at or behind the camera breaks
/// that precondition — that is the `None` the plan's "behind-camera / near-plane-spanning lights
/// take the full-tile fallback" rule names.
///
/// Tightness is the negotiable half: an exact sphere-frustum refinement is deferred behind the
/// plan's occupancy-stats trigger.
fn tile_range(
    center: Vec3,
    range: f32,
    view_proj: &Mat4,
    render_extent: (u32, u32),
) -> Option<((u32, u32), (u32, u32))> {
    if !center.is_finite() || !range.is_finite() {
        return None;
    }
    let width = render_extent.0.max(1) as f32;
    let height = render_extent.1.max(1) as f32;

    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for corner in 0..8u32 {
        let sign = |bit: u32| if corner & bit == 0 { -range } else { range };
        let point = center + Vec3::new(sign(1), sign(2), sign(4));
        let clip = *view_proj * point.extend(1.0);
        if !clip.is_finite() || clip.w <= MIN_CLIP_W {
            return None;
        }
        // NDC → framebuffer pixels: x right from 0, y **down** from 0 (wgpu's +Y-up clip space
        // against `@builtin(position)`'s top-left origin).
        let px = (clip.x / clip.w * 0.5 + 0.5) * width;
        let py = (0.5 - clip.y / clip.w * 0.5) * height;
        min_x = min_x.min(px);
        max_x = max_x.max(px);
        min_y = min_y.min(py);
        max_y = max_y.max(py);
    }

    Some((
        (
            tile_of(min_x - TILE_PIXEL_MARGIN, 1.0 / width, CLUSTER_TILES_X),
            tile_of(max_x + TILE_PIXEL_MARGIN, 1.0 / width, CLUSTER_TILES_X),
        ),
        (
            tile_of(min_y - TILE_PIXEL_MARGIN, 1.0 / height, CLUSTER_TILES_Y),
            tile_of(max_y + TILE_PIXEL_MARGIN, 1.0 / height, CLUSTER_TILES_Y),
        ),
    ))
}

/// Which tile a framebuffer coordinate falls in — the CPU mirror of the snippet's tile lookup,
/// including its clamp-then-truncate-then-`min` shape, so an edge coordinate lands in the same tile
/// on both sides.
fn tile_of(pixel: f32, inv_extent: f32, tiles: u32) -> u32 {
    let uv = (pixel * inv_extent).clamp(0.0, 1.0);
    ((uv * tiles as f32) as u32).min(tiles - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{PointLightData, SpotLightData, aspect_ratio};
    use ludion_core::rng::DeterministicRng;

    const EXTENT: (u32, u32) = (192, 108);

    fn camera_at(position: Vec3, target: Vec3) -> RenderCamera {
        RenderCamera {
            view: Mat4::look_at_rh(position, target, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                aspect_ratio(EXTENT),
                0.1,
                1000.0,
            ),
            position,
        }
    }

    fn scene_with(camera: RenderCamera, lights: Vec<PointLightData>) -> RenderScene {
        let mut scene = RenderScene::empty(EXTENT);
        scene.camera = camera;
        scene.lights = lights;
        scene
    }

    fn light(position: Vec3, range: f32) -> PointLightData {
        PointLightData {
            position,
            color: Vec3::ONE,
            intensity: 1.0,
            range,
            dynamic: false,
            shadow: None,
        }
    }

    /// The cells a given light index was filed into.
    fn cells_of(upload: &LightUpload, index: u32) -> Vec<usize> {
        (0..CLUSTER_CELLS)
            .filter(|&cell| {
                let [offset, count] = upload.table[cell];
                upload.indices[offset as usize..(offset + count) as usize].contains(&index)
            })
            .collect()
    }

    /// The CPU mirror of `snippets/lights.wgsl`'s `cluster_slice`: which cell a fragment at
    /// framebuffer position `frag_pos` with squared camera distance `dist_sq` reads. Kept here, in
    /// the test module, deliberately — the shipping build never needs to answer this question, and a
    /// second production copy of the shader's lookup would be a second way to compute it.
    fn cell_read_by(frag_pos: (f32, f32), dist_sq: f32, header: &LightHeader) -> usize {
        let tx = tile_of(frag_pos.0, header.grid[0], CLUSTER_TILES_X);
        let ty = tile_of(frag_pos.1, header.grid[1], CLUSTER_TILES_Y);
        let shell = shell_of(dist_sq, (header.grid[2], header.grid[3]));
        ((shell * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx) as usize
    }

    /// A light that occupies a small patch of screen at a settled distance bins into a handful of
    /// cells, not the whole grid — the culling actually culls. The claim is deliberately loose on
    /// tightness (conservatism is what must hold) but tight enough that a build that quietly fell
    /// back to full coverage would fail it.
    #[test]
    fn a_small_distant_light_bins_into_a_handful_of_cells() {
        let camera = camera_at(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO);
        let scene = scene_with(camera, vec![light(Vec3::ZERO, 1.0)]);
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);

        let cells = cells_of(&upload, 0);
        assert!(
            !cells.is_empty(),
            "a light in front of the camera must reach at least one cell",
        );
        assert!(
            cells.len() < CLUSTER_CELLS / 8,
            "a unit light 20 units out must not cover {} of {CLUSTER_CELLS} cells",
            cells.len(),
        );
        // It is centred, so the centre tile at its own shell is one of them.
        let centre = cell_read_by(
            (EXTENT.0 as f32 * 0.5, EXTENT.1 as f32 * 0.5),
            20.0 * 20.0,
            &upload.header,
        );
        assert!(
            cells.contains(&centre),
            "the light's own screen centre / shell cell {centre} must be covered: {cells:?}",
        );
    }

    /// A light behind the camera breaks the projection's convex-hull precondition, so it takes the
    /// full-tile fallback — every tile, at its own shells. It is still shell-culled: a light behind
    /// the camera can only light fragments at its own radial distance band.
    #[test]
    fn a_light_behind_the_camera_takes_the_full_tile_fallback() {
        let camera = camera_at(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO);
        let scene = scene_with(camera, vec![light(Vec3::new(0.0, 0.0, 40.0), 3.0)]);
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);

        let cells = cells_of(&upload, 0);
        let tiles = (CLUSTER_TILES_X * CLUSTER_TILES_Y) as usize;
        assert_eq!(
            cells.len() % tiles,
            0,
            "the fallback covers whole tile planes: {} cells",
            cells.len(),
        );
        let shells = cells.len() / tiles;
        assert!(
            (1..CLUSTER_SLICES as usize).contains(&shells),
            "a behind-camera light keeps its shell interval ({shells} shells of {CLUSTER_SLICES})",
        );
        // Every tile of the light's own shell is present, which is what "full-tile" means.
        let own_shell = shell_of(20.0 * 20.0, (upload.header.grid[2], upload.header.grid[3]));
        for ty in 0..CLUSTER_TILES_Y {
            for tx in 0..CLUSTER_TILES_X {
                let cell = ((own_shell * CLUSTER_TILES_Y + ty) * CLUSTER_TILES_X + tx) as usize;
                assert!(
                    cells.contains(&cell),
                    "tile ({tx}, {ty}) missing from fallback"
                );
            }
        }
    }

    /// The ladder clamps at both ends rather than indexing out of the table: a light sitting on the
    /// camera reaches shell 0, and one far past `CLUSTER_SHELL_FAR` reaches the last shell. Both
    /// mirror the shader's own float-side clamp.
    #[test]
    fn the_shell_ladder_clamps_at_both_ends() {
        let camera = camera_at(Vec3::new(0.0, 0.0, 20.0), Vec3::ZERO);
        let last = CLUSTER_SLICES - 1;

        // On top of the camera: `d - range` is negative, so the near end pins to shell 0.
        let near = scene_with(camera, vec![light(camera.position, 2.0)]);
        let near = build_lights(&near, EXTENT, ClusterBuild::Culled);
        let near_cells = cells_of(&near, 0);
        assert!(
            near_cells
                .iter()
                .any(|&cell| cell < (CLUSTER_TILES_X * CLUSTER_TILES_Y) as usize),
            "a light at the camera must occupy shell 0",
        );

        // Far past FAR (and past the far plane): the interval pins to the last shell, and nothing
        // indexes past the table.
        let far = scene_with(camera, vec![light(Vec3::new(0.0, 0.0, -4000.0), 50.0)]);
        let far = build_lights(&far, EXTENT, ClusterBuild::Culled);
        let far_cells = cells_of(&far, 0);
        assert!(
            !far_cells.is_empty(),
            "a light past the ladder must still be filed somewhere, or the check below is vacuous",
        );
        assert!(
            far_cells
                .iter()
                .all(|&cell| cell >= (last * CLUSTER_TILES_Y * CLUSTER_TILES_X) as usize),
            "a light past CLUSTER_SHELL_FAR must pin to the last shell",
        );
        assert!(
            far_cells.iter().all(|&cell| cell < CLUSTER_CELLS),
            "no cell index may leave the table",
        );
    }

    /// Within a cell the indices ascend — the half of the identity argument that fixes the float
    /// summation order of every lit pixel (`Plan/clustered-forward.md` "The design"). It holds by
    /// construction (lights outer, cells inner), and this pins it against anyone who reorders the
    /// scatter. The table is also checked to be a partition: contiguous, gapless, in cell order.
    #[test]
    fn each_cells_index_list_ascends_and_the_table_partitions_the_indices() {
        let camera = camera_at(Vec3::new(6.0, 5.0, 14.0), Vec3::ZERO);
        let mut rng = DeterministicRng::new(0x51CE_D0FF);
        let lights: Vec<PointLightData> = (0..120)
            .map(|_| {
                let p = |rng: &mut DeterministicRng| (rng.next_f32() - 0.5) * 40.0;
                light(
                    Vec3::new(p(&mut rng), p(&mut rng), p(&mut rng)),
                    1.0 + rng.next_f32() * 12.0,
                )
            })
            .collect();
        let scene = scene_with(camera, lights);
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);

        let mut expected_offset = 0u32;
        for (cell, &[offset, count]) in upload.table.iter().enumerate() {
            assert_eq!(offset, expected_offset, "cell {cell} is not contiguous");
            expected_offset += count;
            let slice = &upload.indices[offset as usize..(offset + count) as usize];
            assert!(
                slice.windows(2).all(|w| w[0] < w[1]),
                "cell {cell}'s list must ascend strictly: {slice:?}",
            );
            assert!(
                slice.iter().all(|&i| (i as usize) < upload.lights.len()),
                "cell {cell} indexes past the light array",
            );
        }
        assert_eq!(
            expected_offset as usize,
            upload.indices.len(),
            "the table must account for every index",
        );
    }

    /// The conservatism gate, brute-forced on the CPU with no GPU in sight: for a spread of random
    /// scenes and thousands of random sample points, **every** light whose range reaches a sample
    /// appears in the cell that sample's fragment would read. This is the same claim the rendered
    /// oracle in `render.rs` makes; making it here too localizes a hole to the binning instead of to
    /// a moved pixel, and it runs on a machine with no adapter.
    #[test]
    fn the_binning_never_misses_a_light_that_reaches_a_sample_point() {
        let mut rng = DeterministicRng::new(0xC1_05_7E_5D);
        let mut checked = 0usize;
        for case in 0..12 {
            let eye = Vec3::new(
                (rng.next_f32() - 0.5) * 30.0,
                (rng.next_f32() - 0.5) * 20.0,
                (rng.next_f32() - 0.5) * 30.0,
            );
            let camera = camera_at(eye, Vec3::new(rng.next_f32() * 4.0, 0.0, 0.0));
            let lights: Vec<PointLightData> = (0..80)
                .map(|_| {
                    light(
                        Vec3::new(
                            (rng.next_f32() - 0.5) * 60.0,
                            (rng.next_f32() - 0.5) * 30.0,
                            (rng.next_f32() - 0.5) * 60.0,
                        ),
                        0.5 + rng.next_f32() * 14.0,
                    )
                })
                .collect();
            let scene = scene_with(camera, lights);
            let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);
            let view_proj = camera.projection * camera.view;

            for _ in 0..4000 {
                let sample = Vec3::new(
                    (rng.next_f32() - 0.5) * 70.0,
                    (rng.next_f32() - 0.5) * 40.0,
                    (rng.next_f32() - 0.5) * 70.0,
                );
                let clip = view_proj * sample.extend(1.0);
                // Only fragments the rasterizer could actually produce: in front of the camera and
                // inside the frustum.
                if clip.w <= 0.0
                    || clip.x.abs() > clip.w
                    || clip.y.abs() > clip.w
                    || clip.z < 0.0
                    || clip.z > clip.w
                {
                    continue;
                }
                let frag = (
                    (clip.x / clip.w * 0.5 + 0.5) * EXTENT.0 as f32,
                    (0.5 - clip.y / clip.w * 0.5) * EXTENT.1 as f32,
                );
                let cell = cell_read_by(
                    frag,
                    (camera.position - sample).length_squared(),
                    &upload.header,
                );
                let [offset, count] = upload.table[cell];
                let slice = &upload.indices[offset as usize..(offset + count) as usize];
                for (index, light) in upload.lights.iter().enumerate() {
                    let centre = Vec3::new(light.position[0], light.position[1], light.position[2]);
                    if (centre - sample).length() < light.position[3].max(1e-3) {
                        assert!(
                            slice.contains(&(index as u32)),
                            "case {case}: light {index} reaches {sample:?} but is absent from \
                             cell {cell} — a conservatism hole",
                        );
                    }
                }
                checked += 1;
            }
        }
        assert!(
            checked > 2_000,
            "only {checked} in-frustum samples were checked — the fixture stopped covering the frame",
        );
    }

    #[test]
    fn a_probe_mask_covers_every_in_volume_point_in_view() {
        use ludion_core::ids::EntityId;
        use ludion_core::math::glam::Quat;

        let camera = camera_at(Vec3::new(0.0, 2.0, 12.0), Vec3::ZERO);
        let probe = |entity, position, rotation, volume| ProbeData {
            entity: EntityId::new(entity, 0),
            position,
            rotation,
            volume,
            blend: 0.5,
            priority: 0,
        };
        let ranked = [
            probe(
                0,
                Vec3::new(-3.0, 0.0, 0.0),
                Quat::from_rotation_y(0.7),
                ProbeVolume::Box {
                    extent: Vec3::new(2.0, 1.0, 3.0),
                },
            ),
            probe(
                1,
                Vec3::new(4.0, 1.0, -2.0),
                Quat::IDENTITY,
                ProbeVolume::Sphere { radius: 2.5 },
            ),
            probe(
                2,
                Vec3::new(0.0, 0.0, 6.0),
                Quat::IDENTITY,
                ProbeVolume::Box {
                    extent: Vec3::splat(1.5),
                },
            ),
        ];
        let masks = probe_masks(&ranked, &camera, EXTENT);
        assert_eq!(masks.len(), CLUSTER_CELLS);
        let header = build_lights(
            &scene_with(camera, Vec::new()),
            EXTENT,
            ClusterBuild::Culled,
        )
        .header;
        let view_proj = camera.projection * camera.view;
        let mut rng = DeterministicRng::new(0x0094_0BE5);
        let mut checked = 0;
        for (k, probe) in ranked.iter().enumerate() {
            for _ in 0..1500 {
                let mut unit = || (rng.next_f32() - 0.5) * 2.0;
                let (x, y, z) = (unit(), unit(), unit());
                let local = match probe.volume {
                    ProbeVolume::Box { extent } => Vec3::new(x, y, z) * extent,
                    ProbeVolume::Sphere { radius } => {
                        let v = Vec3::new(x, y, z);
                        if v.length() > 1.0 {
                            continue;
                        }
                        v * radius
                    }
                };
                // The in-volume point, then the same point pushed out into the blend shell.
                let pushed = local + local.normalize_or_zero() * probe.blend * 0.9;
                for local in [local, pushed] {
                    let sample = probe.position + probe.rotation * local;
                    let clip = view_proj * sample.extend(1.0);
                    if clip.w <= 0.0
                        || clip.x.abs() > clip.w
                        || clip.y.abs() > clip.w
                        || clip.z < 0.0
                        || clip.z > clip.w
                    {
                        continue;
                    }
                    let frag = (
                        (clip.x / clip.w * 0.5 + 0.5) * EXTENT.0 as f32,
                        (0.5 - clip.y / clip.w * 0.5) * EXTENT.1 as f32,
                    );
                    let cell =
                        cell_read_by(frag, (camera.position - sample).length_squared(), &header);
                    assert!(
                        masks[cell] & (1 << k) != 0,
                        "probe {k} holds {sample:?} but is absent from cell {cell}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1_000, "only {checked} samples were in view");
    }

    fn spot(
        position: Vec3,
        direction: Vec3,
        range: f32,
        inner_deg: f32,
        outer_deg: f32,
    ) -> SpotLightData {
        SpotLightData {
            position,
            direction: direction.normalize(),
            color: Vec3::ONE,
            intensity: 1.0,
            range,
            cos_inner: inner_deg.to_radians().cos(),
            cos_outer: outer_deg.to_radians().cos(),
            shadow: None,
        }
    }

    /// A unit vector within `max_angle` radians of `axis` (not uniform; coverage, not statistics).
    fn direction_within(rng: &mut DeterministicRng, axis: Vec3, max_angle: f32) -> Vec3 {
        let (u, v) = axis.any_orthonormal_pair();
        let tilt = rng.next_f32() * max_angle;
        let spin = rng.next_f32() * std::f32::consts::TAU;
        (axis * tilt.cos() + (u * spin.cos() + v * spin.sin()) * tilt.sin()).normalize()
    }

    /// Points then spots, each in extract order; a point carries the `w = -2` marker, a spot its two
    /// cosines and axis. A scene without spots packs exactly the point-only array.
    #[test]
    fn points_pack_first_then_spots_each_in_extract_order() {
        let camera = camera_at(Vec3::new(0.0, 3.0, 12.0), Vec3::ZERO);
        let mut scene = scene_with(camera, vec![light(Vec3::X, 2.0), light(Vec3::Y, 3.0)]);
        let point_only = build_lights(&scene, EXTENT, ClusterBuild::Culled);
        assert_eq!(point_only.lights.len(), 2);
        for packed in &point_only.lights {
            assert_eq!(packed.axis, POINT_LIGHT_AXIS);
            assert_eq!(packed.color[3], 0.0);
        }

        scene.spot_lights = vec![
            spot(Vec3::Z, Vec3::NEG_Y, 5.0, 10.0, 20.0),
            spot(Vec3::NEG_Z, Vec3::X, 6.0, 30.0, 30.0),
        ];
        let mixed = build_lights(&scene, EXTENT, ClusterBuild::Culled);
        assert_eq!(mixed.lights.len(), 4);
        assert_eq!(
            bytemuck::cast_slice::<Light, u8>(&mixed.lights[..2]),
            bytemuck::cast_slice::<Light, u8>(&point_only.lights),
            "the points must pack exactly as in a spot-free scene",
        );
        for (packed, spot) in mixed.lights[2..].iter().zip(&scene.spot_lights) {
            assert_eq!(packed.position[..3], spot.position.to_array());
            assert_eq!(packed.position[3], spot.range);
            assert_eq!(packed.color[3], spot.cos_inner);
            assert_eq!(packed.axis[..3], spot.direction.to_array());
            assert_eq!(packed.axis[3], spot.cos_outer);
        }
    }

    /// A narrow spot's cone sphere is far smaller than its range sphere, so it bins into fewer
    /// cells than a point light of the same range at its apex.
    #[test]
    fn a_narrow_spot_bins_into_fewer_cells_than_its_range_sphere() {
        let camera = camera_at(Vec3::new(0.0, 4.0, 18.0), Vec3::ZERO);
        let apex = Vec3::new(0.0, 6.0, 0.0);
        let mut scene = scene_with(camera, vec![light(apex, 8.0)]);
        scene.spot_lights = vec![spot(apex, Vec3::NEG_Y, 8.0, 8.0, 12.0)];
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);
        let sphere = cells_of(&upload, 0).len();
        let cone = cells_of(&upload, 1).len();
        assert!(cone > 0, "the spot must be filed somewhere");
        assert!(
            cone < sphere,
            "a 12-degree spot ({cone} cells) must bin tighter than its range sphere ({sphere})",
        );
    }

    /// The never-misses gate for spots: narrow, just under the 45-degree switch, wide (the
    /// range-sphere fallback), hard-edged, and with the apex behind the camera. Half the samples are
    /// drawn inside some spot's cone, since a uniform sample would rarely land in a narrow one. A
    /// spot reaches a sample within range and strictly inside its outer cone — where the shader's
    /// factor is non-zero.
    #[test]
    fn the_binning_never_misses_a_spot_that_reaches_a_sample_point() {
        let mut rng = DeterministicRng::new(0x5B07_C0DE);
        let mut checked = 0usize;
        let mut lit = 0usize;
        for case in 0..12 {
            let eye = Vec3::new(
                (rng.next_f32() - 0.5) * 30.0,
                (rng.next_f32() - 0.5) * 20.0,
                (rng.next_f32() - 0.5) * 30.0,
            );
            let target = Vec3::new(rng.next_f32() * 4.0, 0.0, 0.0);
            let camera = camera_at(eye, target);
            let forward = (target - eye).normalize();
            let spots: Vec<SpotLightData> = (0..48)
                .map(|i| {
                    let (inner, outer) = match i % 4 {
                        0 => (rng.next_f32() * 5.0, 5.0 + rng.next_f32() * 15.0),
                        1 => (30.0, 40.0 + rng.next_f32() * 5.0),
                        2 => (40.0, 46.0 + rng.next_f32() * 43.0),
                        _ => {
                            let edge = 2.0 + rng.next_f32() * 60.0;
                            (edge, edge)
                        }
                    };
                    let range = 0.5 + rng.next_f32() * 14.0;
                    let (apex, dir) = if i % 6 == 0 {
                        let apex = eye - forward * (0.5 + rng.next_f32() * 4.0);
                        (apex, direction_within(&mut rng, forward, 0.6))
                    } else {
                        let apex = Vec3::new(
                            (rng.next_f32() - 0.5) * 60.0,
                            (rng.next_f32() - 0.5) * 30.0,
                            (rng.next_f32() - 0.5) * 60.0,
                        );
                        let dir = direction_within(&mut rng, Vec3::NEG_Y, std::f32::consts::PI);
                        (apex, dir)
                    };
                    spot(apex, dir, range, inner, outer)
                })
                .collect();
            let mut scene = scene_with(camera, vec![light(Vec3::ZERO, 3.0)]);
            scene.spot_lights = spots.clone();
            let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);
            let view_proj = camera.projection * camera.view;

            for n in 0..6000 {
                let sample = if n % 2 == 0 {
                    let s = &spots[(rng.next_f32() * spots.len() as f32) as usize % spots.len()];
                    let dir = direction_within(&mut rng, s.direction, s.cos_outer.acos());
                    s.position + dir * (rng.next_f32() * s.range)
                } else {
                    Vec3::new(
                        (rng.next_f32() - 0.5) * 70.0,
                        (rng.next_f32() - 0.5) * 40.0,
                        (rng.next_f32() - 0.5) * 70.0,
                    )
                };
                let clip = view_proj * sample.extend(1.0);
                if clip.w <= 0.0
                    || clip.x.abs() > clip.w
                    || clip.y.abs() > clip.w
                    || clip.z < 0.0
                    || clip.z > clip.w
                {
                    continue;
                }
                let frag = (
                    (clip.x / clip.w * 0.5 + 0.5) * EXTENT.0 as f32,
                    (0.5 - clip.y / clip.w * 0.5) * EXTENT.1 as f32,
                );
                let cell = cell_read_by(
                    frag,
                    (camera.position - sample).length_squared(),
                    &upload.header,
                );
                let [offset, count] = upload.table[cell];
                let slice = &upload.indices[offset as usize..(offset + count) as usize];
                // Index 0 is the point light; every later entry is a spot.
                for (index, packed) in upload.lights.iter().enumerate().skip(1) {
                    let apex =
                        Vec3::new(packed.position[0], packed.position[1], packed.position[2]);
                    let axis = Vec3::new(packed.axis[0], packed.axis[1], packed.axis[2]);
                    let to_sample = sample - apex;
                    let in_range = to_sample.length() < packed.position[3].max(1e-3);
                    if in_range && to_sample.normalize().dot(axis) > packed.axis[3] {
                        lit += 1;
                        assert!(
                            slice.contains(&(index as u32)),
                            "case {case}: spot {index} reaches {sample:?} but is absent from \
                             cell {cell} — a conservatism hole",
                        );
                    }
                }
                checked += 1;
            }
        }
        assert!(
            checked > 2_000 && lit > 2_000,
            "only {checked} in-frustum samples and {lit} lit pairs — the fixture stopped covering \
             the cones",
        );
    }

    /// The degenerate oracle is one shared `[0..N)` list every cell
    /// points at, and every count `N`. If it ever drifted, the byte-comparison it anchors would be
    /// comparing the culled build against something other than "admit everything".
    #[test]
    fn the_degenerate_oracle_files_every_light_into_every_cell() {
        let camera = camera_at(Vec3::new(0.0, 3.0, 12.0), Vec3::ZERO);
        let lights: Vec<PointLightData> = (0..7)
            .map(|i| light(Vec3::new(i as f32 * 3.0, 1.0, 0.0), 2.0))
            .collect();
        let scene = scene_with(camera, lights);
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Degenerate);

        assert_eq!(upload.table.len(), CLUSTER_CELLS);
        assert!(
            upload
                .table
                .iter()
                .all(|&[offset, count]| offset == 0 && count == 7)
        );
        assert_eq!(upload.indices, (0..7).collect::<Vec<u32>>());
        // The occupancy panel describes what the tables *say*, not how compactly they are stored:
        // every cell holds all seven, however few index words that takes.
        assert_eq!(upload.stats.occupied_cells, CLUSTER_CELLS as u32);
        assert_eq!(upload.stats.total_refs, 7 * CLUSTER_CELLS as u64);
        assert_eq!(upload.stats.max_list_len, 7);
        assert_eq!(upload.stats.mean_list_len, 7.0);
    }

    /// The occupancy panel is a reduction of the very tables it describes, not a second opinion
    /// about them (`Plan/clustered-forward.md`, "Introspection"). Every field is re-derived
    /// here from `table`/`indices` the slow, obvious way and must agree exactly — which is what
    /// makes the panel usable as evidence when a `scene`-pass regression needs a cause named.
    #[test]
    fn the_occupancy_panel_agrees_with_the_tables_it_summarizes() {
        let camera = camera_at(Vec3::new(4.0, 9.0, 22.0), Vec3::ZERO);
        let mut rng = DeterministicRng::new(0x0CC0_9A47);
        let lights: Vec<PointLightData> = (0..180)
            .map(|i| {
                let p = |rng: &mut DeterministicRng| (rng.next_f32() - 0.5) * 80.0;
                let position = Vec3::new(p(&mut rng), rng.next_f32() * 5.0, p(&mut rng));
                // Both populations the stress scene authors: torch statics, then projectile movers.
                let range = if i < 80 {
                    8.0 + rng.next_f32() * 7.0
                } else {
                    1.5 + rng.next_f32() * 1.5
                };
                light(position, range)
            })
            .collect();
        let scene = scene_with(camera, lights);
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);
        let stats = upload.stats;

        assert_eq!(stats.lights, 180);
        assert_eq!(stats.cells, CLUSTER_CELLS as u32);
        assert_eq!(stats.total_refs, upload.indices.len() as u64);
        let counts: Vec<u32> = upload.table.iter().map(|&[_, count]| count).collect();
        assert_eq!(
            stats.occupied_cells,
            counts.iter().filter(|&&c| c > 0).count() as u32
        );
        assert_eq!(stats.max_list_len, counts.iter().copied().max().unwrap());
        assert_eq!(
            stats.mean_list_len,
            stats.total_refs as f64 / f64::from(stats.occupied_cells),
        );
        // The fixture must actually exercise the culling, or every assertion above is about the
        // degenerate extreme wearing the culled build's name.
        assert!(
            stats.occupied_cells < stats.cells,
            "180 small lights must leave some cell empty: {stats:?}",
        );
        assert!(
            stats.max_list_len < stats.lights,
            "no cell should hold every light: {stats:?}",
        );

        // The public entry point the profile driver times returns the same panel — one build, one
        // set of numbers, no second implementation to drift.
        assert_eq!(cluster_stats(&scene, EXTENT), stats);
    }

    /// An empty scene's panel reads as empty rather than as the one padding element the buffers
    /// carry — the padding exists for wgpu, and a reported `1 light` would be a lie about content.
    #[test]
    fn an_empty_scene_reports_an_empty_panel() {
        let scene = scene_with(camera_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO), Vec::new());
        let stats = cluster_stats(&scene, EXTENT);
        assert_eq!(stats.lights, 0);
        assert_eq!(stats.occupied_cells, 0);
        assert_eq!(stats.total_refs, 0);
        assert_eq!(stats.max_list_len, 0);
        assert_eq!(stats.mean_list_len, 0.0);
    }

    /// An empty scene still uploads a bindable payload (wgpu rejects a zero-sized storage binding),
    /// with every cell empty so the padding is never read.
    #[test]
    fn an_empty_scene_uploads_one_padded_element_and_no_references() {
        let scene = scene_with(camera_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO), Vec::new());
        let upload = build_lights(&scene, EXTENT, ClusterBuild::Culled);
        assert_eq!(upload.lights.len(), 1);
        assert_eq!(upload.indices.len(), 1);
        assert!(upload.table.iter().all(|&[_, count]| count == 0));
    }

    /// The measurement decision 3 gates the GPU compute build on: the CPU build's wall cost at the
    /// stress-scene population (~500 lights — ~200 torch-density statics plus ~300 projectile-like
    /// movers). `#[ignore]`d because a timing assertion in the default lane is a flake waiting to
    /// happen; run it with `--release --ignored --nocapture` and read the number, which must stay
    /// under 0.3 ms for the CPU build to keep its ground.
    #[test]
    #[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
    fn cluster_build_cost_at_the_stress_population() {
        let camera = camera_at(Vec3::new(0.0, 18.0, 40.0), Vec3::ZERO);
        let mut rng = DeterministicRng::new(0x5735_5000);
        let lights: Vec<PointLightData> = (0..500)
            .map(|i| {
                let position = Vec3::new(
                    (rng.next_f32() - 0.5) * 130.0,
                    rng.next_f32() * 6.0,
                    (rng.next_f32() - 0.5) * 130.0,
                );
                // ~200 torch statics at vigil's ranges, then ~300 small projectile lights.
                let range = if i < 200 {
                    8.0 + rng.next_f32() * 7.0
                } else {
                    1.5 + rng.next_f32() * 1.5
                };
                light(position, range)
            })
            .collect();
        let scene = scene_with(camera, lights);

        // One warm-up, then a run long enough that the clock's resolution is not the measurement.
        let mut stats = cluster_stats(&scene, (1920, 1080));
        const RUNS: u32 = 200;
        let start = std::time::Instant::now();
        for _ in 0..RUNS {
            stats = cluster_stats(&scene, (1920, 1080));
        }
        let per_build = start.elapsed().as_secs_f64() * 1000.0 / f64::from(RUNS);
        eprintln!(
            "cluster build: {per_build:.4} ms for {} lights\n  \
             refs {} | occupied {}/{} cells | max list {} | mean list (occupied) {:.2}",
            stats.lights,
            stats.total_refs,
            stats.occupied_cells,
            stats.cells,
            stats.max_list_len,
            stats.mean_list_len,
        );
    }
}
