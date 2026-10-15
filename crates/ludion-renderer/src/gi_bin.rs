//! The GI light bin: a world-space counting sort of point lights over the voxel grid's own box, so
//! a shader evaluating direct light at a point loops that point's cell list instead of every light
//! in the scene (`Plan/gi-wave3-dynamic-injection.md` §4.3).
//!
//! **`cluster.rs`'s shape, one seam inward.** Three sequential passes — count, prefix-sum, scatter —
//! keyed by the *light* index with lights outer and cells inner, so every cell's slice comes out
//! ascending by construction and needs no sort. The domain is what differs: a fixed grid over the
//! voxel grid's **world** box, not the view frustum, because both consumers ask a world-space
//! question and neither has a camera in it.
//!
//! **Two consumers, one build.** The static (unmarked) light subset is binned once per bake and
//! uploaded beside `gi_bake.wgsl`'s light buffer; the dynamic (marked) subset is binned per frame
//! for the injection pass. Same code, same table/index/light-buffer layout, different inputs — which
//! is why [`LightBin`] hands back the three pieces a shader binds ([`GiBinParams`], the
//! `(offset, count)` table, the concatenated index list) rather than a bake-shaped bundle.
//!
//! **Conservatism is mandatory, tightness is negotiable** — the `cluster.rs` rule, and here it is
//! what makes the bake's adoption of the bin *byte-identity-provable*. Every light whose range
//! sphere can reach any evaluation point in a cell must appear in that cell's list; a light filed
//! spuriously costs a loop iteration and contributes exactly `+0.0` (both consumers' attenuation is
//! a hard zero at `dist >= range`, and adding an exact `+0.0` to an accumulator that is itself a sum
//! of non-negatives is bit-preserving), so over-inclusion cannot move a value. A light *missed* is a
//! wrong number. The margin therefore rounds outward by a whole voxel cell — far more than the ulps
//! between the CPU's cell arithmetic here and the shader's evaluation at cell centres — and the
//! degenerate build below is the oracle a binned bake is byte-compared against.
//!
//! **This bin does NOT inherit `cluster.rs`'s "deliberately not bit-reproducible" stance, and the
//! reason must survive.** `cluster.rs` may bin loosely and differ between platforms because its
//! consumer re-tests range exactly, so its *assignment* never reaches a value. The same is true here
//! today. But the moment anyone sums lights into per-cell virtual sources (the stage-1 aggregation
//! `Plan/gi-wave3-dynamic-injection.md` §16 defers), bin assignment becomes part of the radiance
//! arithmetic and lands in pixels. The rule this module ships, from §4.3: **the build is
//! deterministic and fixed-order as written — additions, multiplications and `floor`, no
//! transcendentals, no `par`, no atomics — and any future change that makes bin assignment affect a
//! value rather than only a loop bound must carry a cross-process fixture as its gate.** The
//! determinism is nearly free today; keeping it is what keeps that door open.

use crate::scene::WorldVoxelGrid;
use crate::shader_gen::Light;

pub(crate) use crate::shader_gen::GiBinParams;

/// The bin grid's cell counts per axis — a **fixed count**, so the cell size is a pure function of
/// the authored grid's bounds (the `CLUSTER_TILES` posture).
///
/// `32 × 16 × 32` because content is wide and flat: a district's box at these dims comes out cells
/// of roughly the same order as a light's range, which is the regime where a cell list is short
/// without the table itself dominating. The vertical axis is halved for the same reason vigil's
/// districts are: a city block is far wider than it is tall, and 16 rows of thin bins over a
/// two-metre interior would buy nothing.
pub(crate) const BIN_DIMS: [u32; 3] = [32, 16, 32];

/// How many cells [`BIN_DIMS`] describes — the length of every table this module builds.
pub(crate) const BIN_CELLS: usize = (BIN_DIMS[0] * BIN_DIMS[1] * BIN_DIMS[2]) as usize;

/// The floor on a light's range, mirroring both shaders' own `max(light.position.w, 1e-3)`: a light
/// authored at range 0 still lights a hair of geometry GPU-side, so the bin must file it as if it
/// did.
const MIN_RANGE: f32 = 1e-3;

/// The smallest bin cell size any axis takes, as a multiple of the voxel grid's cell size.
///
/// Below one voxel cell the bin would be subdividing space finer than the geometry it bins against —
/// more references, more table, and never a shorter list, since a light's range is at least an
/// occluder thick in any scene the GI arc's authoring rules admit. A degenerately thin axis (a
/// two-cell-tall interior) therefore gets *fewer effective* rows rather than a stack of slivers;
/// the lookup clamps into the grid, so the cells past the domain are simply empty.
const MIN_CELL_IN_VOXELS: f32 = 1.0;

// `GiBinParams` (generated from the WGSL) is the bin's placement and shape as one shader-visible
// POD: `origin` is the bin box's world minimum corner, `inv_cell` the **reciprocal** cell size (both
// sides multiply by it, so neither divides), and `dims` is [`BIN_DIMS`], carried in the uniform
// rather than duplicated in WGSL so `BIN_DIMS` is the *only* place the grid's shape is written down.
// Each has a pad lane in `w`. It is embedded in the bake's and the injection's uniforms rather than
// given a binding of its own.

/// One built bin: where it sits, and which lights each cell holds.
///
/// `table[c]` is `(offset, count)` into [`indices`](LightBin::indices), and each index addresses the
/// light array the caller binned — the same array it binds beside these two, in the same order.
pub(crate) struct LightBin {
    /// The placement uniform a shader needs to find its own cell.
    pub params: GiBinParams,
    /// One `(offset, count)` per cell, in `(z, y, x)` row-major order — the order [`cell_index`]
    /// produces and `gi_bake.wgsl`'s `bin_cell_of` indexes.
    pub table: Vec<[u32; 2]>,
    /// The concatenated per-cell index lists, each cell's slice ascending.
    pub indices: Vec<u32>,
}

/// Which table [`build_bin`] builds.
///
/// The shipping build has exactly one variant: the degenerate oracle is `#[cfg(test)]`, so it is not
/// a mode a running engine can be put into — it exists only so a test can bake the same scene both
/// ways through the same shader and byte-compare (the `cluster.rs` decision-7 discipline).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BinBuild {
    /// Per-light conservative filing: a light lands only in the cells its range sphere, expanded by
    /// one voxel cell, can reach.
    Culled,
    /// Every light in every cell — the conservative extreme, and therefore the reference the culled
    /// build must produce byte-identical output against.
    #[cfg(test)]
    Degenerate,
}

/// Bins `lights` over `grid`'s world box.
///
/// `lights` is the caller's already-premultiplied array in its own pinned order (the bake's static
/// subset, or the injection's dynamic one); the indices this returns address it directly, so the
/// caller must upload the very slice it passed here.
///
/// Sequential on the calling thread by design: at most a few hundred lights over 16 384 cells is
/// microseconds, `ludion_core::par` is deliberately unused, and thread-count invariance is therefore
/// trivial rather than argued.
pub(crate) fn build_bin(grid: &WorldVoxelGrid, lights: &[Light], build: BinBuild) -> LightBin {
    let params = params_for(grid);
    let (table, indices) = match build {
        BinBuild::Culled => cull(&params, lights, voxel_cell(grid)),
        #[cfg(test)]
        BinBuild::Degenerate => degenerate(lights.len()),
    };
    LightBin {
        params,
        table,
        indices,
    }
}

/// The grid's cell size, floored off zero so the reciprocals below stay finite. The authored
/// `GiVolume.cell_size` is `exclusiveMinimum: 0` in the schema; a game constructing the component in
/// Rust bypasses that, and a zero-sized cell must produce a degenerate-but-finite bin rather than an
/// infinity.
fn voxel_cell(grid: &WorldVoxelGrid) -> f32 {
    grid.cell_size.max(f32::MIN_POSITIVE)
}

/// Where the bin sits and how big its cells are: [`BIN_DIMS`] cells over the grid's world box, with
/// each axis's cell size floored at [`MIN_CELL_IN_VOXELS`] voxel cells.
fn params_for(grid: &WorldVoxelGrid) -> GiBinParams {
    let voxel = voxel_cell(grid);
    let inv_cell: [f32; 3] = std::array::from_fn(|axis| {
        let span = grid.extent[axis] as f32 * voxel;
        let cell = (span / BIN_DIMS[axis] as f32).max(voxel * MIN_CELL_IN_VOXELS);
        1.0 / cell
    });
    GiBinParams {
        origin: [grid.origin.x, grid.origin.y, grid.origin.z, 0.0],
        inv_cell: [inv_cell[0], inv_cell[1], inv_cell[2], 0.0],
        dims: [BIN_DIMS[0], BIN_DIMS[1], BIN_DIMS[2], 0],
    }
}

/// Which bin cell a world coordinate falls in on one axis.
///
/// **This is the arithmetic**, and `gi_bake.wgsl`'s `bin_cell_of` mirrors it operation for
/// operation: subtract the origin, multiply by the stored reciprocal, `floor`, clamp *in float*, and
/// only then truncate. Clamping before the cast is what keeps a position far outside the box from
/// depending on an out-of-range float→int conversion, and the whole chain is monotone
/// non-decreasing in `p` — which is precisely what the conservatism argument needs: if a hit's index
/// falls below a light's low corner index, the hit is genuinely below that corner in world space.
fn axis_index(params: &GiBinParams, axis: usize, p: f32) -> u32 {
    let f = ((p - params.origin[axis]) * params.inv_cell[axis]).floor();
    f.clamp(0.0, (params.dims[axis] - 1) as f32) as u32
}

/// The flat index of bin cell `(x, y, z)` — `(z·dy + y)·dx + x`, the one cell ordering both sides
/// index.
fn cell_index(params: &GiBinParams, cell: [u32; 3]) -> usize {
    let [dx, dy, _, _] = params.dims;
    (cell[2] as usize * dy as usize + cell[1] as usize) * dx as usize + cell[0] as usize
}

/// **Test-only.** Which bin cell a shader evaluating at `p` reads — the CPU mirror of
/// `gi_bake.wgsl`'s `bin_cell_of`, composed from the very [`axis_index`]/[`cell_index`] the filing
/// uses.
///
/// It lives under `cfg(test)` on purpose (the `cluster.rs` precedent): the shipping build files
/// footprints and never asks this question, so a production copy of the lookup would be a second way
/// to compute it. Two test modules share this one instead of writing it twice.
#[cfg(test)]
pub(crate) fn bin_cell_of(params: &GiBinParams, p: ludion_core::math::glam::Vec3) -> usize {
    cell_index(
        params,
        std::array::from_fn(|axis| axis_index(params, axis, [p.x, p.y, p.z][axis])),
    )
}

/// The inclusive cell box one light covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Footprint {
    lo: [u32; 3],
    hi: [u32; 3],
}

impl Footprint {
    /// Every cell of the whole grid — the fallback a non-finite light takes, since no filing rule
    /// can be more conservative than admitting everything.
    fn everything(params: &GiBinParams) -> Footprint {
        Footprint {
            lo: [0, 0, 0],
            hi: std::array::from_fn(|axis| params.dims[axis] - 1),
        }
    }

    /// How many cells this footprint spans.
    fn cell_count(&self) -> usize {
        (0..3)
            .map(|axis| (self.hi[axis] - self.lo[axis] + 1) as usize)
            .product()
    }

    /// Every cell index this footprint covers, in `(z, y, x)` row-major order.
    fn cells(&self, params: &GiBinParams) -> impl Iterator<Item = usize> + '_ {
        let params = *params;
        (self.lo[2]..=self.hi[2]).flat_map(move |z| {
            (self.lo[1]..=self.hi[1]).flat_map(move |y| {
                (self.lo[0]..=self.hi[0]).map(move |x| cell_index(&params, [x, y, z]))
            })
        })
    }
}

/// The cells one light's range sphere can reach, expanded by `margin` world units on every side.
///
/// The expansion is what covers the gap between this arithmetic and the shader's: the CPU files from
/// the light's centre, the shader looks up from a hit cell's *centre*, and the two are computed by
/// different hardware. One whole voxel cell is orders of magnitude more slack than the ulps at
/// stake — and it costs, at most, one extra ring of cells around a light.
fn footprint_of(params: &GiBinParams, light: &Light, margin: f32) -> Footprint {
    let center = [light.position[0], light.position[1], light.position[2]];
    let reach = light.position[3].max(MIN_RANGE) + margin;
    if !center.iter().all(|v| v.is_finite()) || !reach.is_finite() {
        return Footprint::everything(params);
    }
    Footprint {
        lo: std::array::from_fn(|axis| axis_index(params, axis, center[axis] - reach)),
        hi: std::array::from_fn(|axis| axis_index(params, axis, center[axis] + reach)),
    }
}

/// The real build: file each light into exactly the cells its expanded range sphere can reach.
///
/// Count, prefix-sum, scatter — a counting sort with the *light* index as the key, so each cell's
/// slice comes out ascending with no sort and no per-cell `Vec`.
fn cull(params: &GiBinParams, lights: &[Light], margin: f32) -> (Vec<[u32; 2]>, Vec<u32>) {
    let mut counts = vec![0u32; BIN_CELLS];
    let mut footprints: Vec<Footprint> = Vec::with_capacity(lights.len());
    let mut total = 0usize;
    for light in lights {
        let footprint = footprint_of(params, light, margin);
        total += footprint.cell_count();
        for cell in footprint.cells(params) {
            counts[cell] += 1;
        }
        footprints.push(footprint);
    }

    // `table[c]` becomes `(offset, cursor)`; the cursor is the count the shader wants once every
    // light has been scattered, so the running write position needs no second array.
    let mut table = vec![[0u32; 2]; BIN_CELLS];
    let mut offset = 0u32;
    for (slot, &count) in table.iter_mut().zip(counts.iter()) {
        slot[0] = offset;
        offset += count;
    }

    let mut indices = vec![0u32; total];
    for (index, footprint) in footprints.iter().enumerate() {
        for cell in footprint.cells(params) {
            let slot = &mut table[cell];
            indices[(slot[0] + slot[1]) as usize] = index as u32;
            slot[1] += 1;
        }
    }

    (table, indices)
}

/// The conformance oracle: every light in every cell.
///
/// Correct by definition — no filing rule can be more conservative than admitting everything — so
/// any bake the culled build produces differently is a hole in that build, not a difference of
/// opinion. The compact form is exact: one shared index list `[0..N)` that every cell's `(0, N)`
/// entry points at, rather than [`BIN_CELLS`] copies of it.
#[cfg(test)]
fn degenerate(count: usize) -> (Vec<[u32; 2]>, Vec<u32>) {
    let count = count as u32;
    (
        vec![[0, count]; BIN_CELLS],
        (0..count).collect::<Vec<u32>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::Vec3;
    use ludion_core::rng::DeterministicRng;

    /// A grid with no content — the bin reads only its origin, cell size and extent, never a cell.
    fn grid(origin: Vec3, cell_size: f32, extent: [u32; 3]) -> WorldVoxelGrid {
        let total = (extent[0] * extent[1] * extent[2]) as usize;
        WorldVoxelGrid {
            origin,
            cell_size,
            extent,
            occupancy: vec![false; total],
            albedo: vec![[1.0, 1.0, 1.0, 1.0]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
        .sealed()
    }

    fn light(position: Vec3, range: f32) -> Light {
        Light {
            position: [position.x, position.y, position.z, range],
            color: [1.0, 1.0, 1.0, 0.0],
            axis: crate::cluster::POINT_LIGHT_AXIS,
        }
    }

    /// The cells a given light index was filed into.
    fn cells_of(bin: &LightBin, index: u32) -> Vec<usize> {
        (0..BIN_CELLS)
            .filter(|&cell| {
                let [offset, count] = bin.table[cell];
                bin.indices[offset as usize..(offset + count) as usize].contains(&index)
            })
            .collect()
    }

    /// The bin's cells are a pure function of the authored bounds: a fixed count, so a bigger box
    /// gets bigger cells rather than more of them, and a thin axis floors at one voxel cell instead
    /// of stacking slivers.
    #[test]
    fn the_cell_size_is_the_box_over_a_fixed_count_with_a_one_voxel_floor() {
        let wide = params_for(&grid(Vec3::ZERO, 0.5, [128, 64, 128]));
        // 128 cells × 0.5 = 64 world units over 32 bins ⇒ 2.0 units per bin cell.
        assert_eq!(wide.inv_cell[0], 0.5);
        // 64 × 0.5 = 32 over 16 ⇒ 2.0.
        assert_eq!(wide.inv_cell[1], 0.5);

        // A two-cell-tall interior: 2 × 0.5 = 1.0 world unit over 16 bins would be 0.0625 — under
        // the voxel cell, so the floor takes it to 0.5.
        let thin = params_for(&grid(Vec3::ZERO, 0.5, [128, 2, 128]));
        assert_eq!(thin.inv_cell[1], 2.0);
    }

    /// Within a cell the indices ascend — the half of the identity argument that fixes the float
    /// summation order of every binned evaluation. It holds by construction (lights outer, cells
    /// inner), and this pins it against anyone who reorders the scatter. The table is also checked
    /// to be a partition: contiguous, gapless, in cell order.
    #[test]
    fn each_cells_index_list_ascends_and_the_table_partitions_the_indices() {
        let world = grid(Vec3::new(-20.0, 0.0, -20.0), 0.5, [80, 24, 80]);
        let mut rng = DeterministicRng::new(0x6117_B14E);
        let lights: Vec<Light> = (0..140)
            .map(|_| {
                let p = |rng: &mut DeterministicRng| (rng.next_f32() - 0.5) * 44.0;
                light(
                    Vec3::new(p(&mut rng), rng.next_f32() * 12.0, p(&mut rng)),
                    0.5 + rng.next_f32() * 8.0,
                )
            })
            .collect();
        let bin = build_bin(&world, &lights, BinBuild::Culled);

        let mut expected_offset = 0u32;
        for (cell, &[offset, count]) in bin.table.iter().enumerate() {
            assert_eq!(offset, expected_offset, "cell {cell} is not contiguous");
            expected_offset += count;
            let slice = &bin.indices[offset as usize..(offset + count) as usize];
            assert!(
                slice.windows(2).all(|w| w[0] < w[1]),
                "cell {cell}'s list must ascend strictly: {slice:?}",
            );
            assert!(
                slice.iter().all(|&i| (i as usize) < lights.len()),
                "cell {cell} indexes past the light array",
            );
        }
        assert_eq!(
            expected_offset as usize,
            bin.indices.len(),
            "the table must account for every index",
        );
        // The fixture must actually exercise the filing, or every assertion above is about the
        // degenerate extreme wearing the culled build's name.
        let occupied = bin.table.iter().filter(|&&[_, c]| c > 0).count();
        assert!(
            occupied < BIN_CELLS,
            "140 small lights must leave some cell empty: {occupied} of {BIN_CELLS} occupied",
        );
    }

    /// The filing is conservative and the margin is real: a light whose range sphere reaches into a
    /// cell is in that cell's list, and one comfortably outside the expanded sphere is not. The
    /// absence half is what makes the presence half worth asserting — a build that filed everything
    /// everywhere would pass the first and fail the second.
    #[test]
    fn a_light_is_filed_where_it_can_reach_and_absent_where_it_cannot() {
        let world = grid(Vec3::ZERO, 1.0, [64, 16, 64]);
        let params = params_for(&world);
        // 64 units over 32 bins ⇒ 2-unit cells on x/z; 16 over 16 ⇒ 1-unit on y.
        assert_eq!(params.inv_cell, [0.5, 1.0, 0.5, 0.0]);

        let bin = build_bin(
            &world,
            &[light(Vec3::new(32.0, 8.0, 32.0), 3.0)],
            BinBuild::Culled,
        );
        let cells = cells_of(&bin, 0);

        // Its own cell, and a point just inside the range sphere, are both covered.
        assert!(cells.contains(&bin_cell_of(&params, Vec3::new(32.0, 8.0, 32.0))));
        assert!(cells.contains(&bin_cell_of(&params, Vec3::new(34.9, 8.0, 32.0))));
        // Range 3 + one voxel cell of margin = 4, so a point 4.1 units out is outside the expanded
        // box on x — and every cell it reads is at least a cell beyond the footprint.
        assert!(!cells.contains(&bin_cell_of(&params, Vec3::new(38.5, 8.0, 32.0))));
        assert!(!cells.contains(&bin_cell_of(&params, Vec3::new(32.0, 8.0, 38.5))));
        assert!(
            cells.len() < BIN_CELLS / 8,
            "a 3-unit light must not reach {} of {BIN_CELLS} cells",
            cells.len(),
        );
    }

    /// **The conservatism gate**, brute-forced: for a spread of random light sets and thousands of
    /// random evaluation points, every light whose range reaches a point appears in the cell that
    /// point reads. This is the claim the bake's byte-identity argument rests on — a light the bin
    /// drops must have been out of range, so its term was exactly `+0.0`.
    #[test]
    fn the_bin_never_drops_a_light_that_reaches_an_evaluation_point() {
        let mut rng = DeterministicRng::new(0x81_00_D1_CE);
        let mut checked = 0usize;
        for case in 0..10 {
            let world = grid(
                Vec3::new(
                    (rng.next_f32() - 0.5) * 20.0,
                    0.0,
                    (rng.next_f32() - 0.5) * 20.0,
                ),
                0.25 + rng.next_f32(),
                [40 + (rng.next_f32() * 60.0) as u32, 12, 40],
            );
            let params = params_for(&world);
            let span = Vec3::new(
                world.extent[0] as f32 * world.cell_size,
                world.extent[1] as f32 * world.cell_size,
                world.extent[2] as f32 * world.cell_size,
            );
            let lights: Vec<Light> = (0..60)
                .map(|_| {
                    let at = |rng: &mut DeterministicRng, axis: f32| rng.next_f32() * axis;
                    light(
                        world.origin
                            + Vec3::new(
                                at(&mut rng, span.x),
                                at(&mut rng, span.y),
                                at(&mut rng, span.z),
                            ),
                        0.5 + rng.next_f32() * 9.0,
                    )
                })
                .collect();
            let bin = build_bin(&world, &lights, BinBuild::Culled);

            for _ in 0..2000 {
                // Evaluation points are voxel cell centres, exactly what both consumers look up.
                let cell = [
                    (rng.next_f32() * world.extent[0] as f32) as u32,
                    (rng.next_f32() * world.extent[1] as f32) as u32,
                    (rng.next_f32() * world.extent[2] as f32) as u32,
                ];
                let point = world.origin
                    + Vec3::new(
                        (cell[0] as f32 + 0.5) * world.cell_size,
                        (cell[1] as f32 + 0.5) * world.cell_size,
                        (cell[2] as f32 + 0.5) * world.cell_size,
                    );
                let read = bin_cell_of(&params, point);
                let [offset, count] = bin.table[read];
                let slice = &bin.indices[offset as usize..(offset + count) as usize];
                for (index, light) in lights.iter().enumerate() {
                    let centre = Vec3::new(light.position[0], light.position[1], light.position[2]);
                    if (centre - point).length() < light.position[3].max(MIN_RANGE) {
                        assert!(
                            slice.contains(&(index as u32)),
                            "case {case}: light {index} reaches {point:?} but is absent from cell \
                             {read} — a conservatism hole",
                        );
                    }
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 20_000);
    }

    /// The build is deterministic and fixed-order: the same inputs produce byte-equal tables, run to
    /// run. Cheap today, and the property `Plan/gi-wave3-dynamic-injection.md` §4.3 requires be kept
    /// so the deferred stage-1 aggregation can be added without redesigning the bin.
    #[test]
    fn two_builds_of_the_same_inputs_are_byte_equal() {
        let world = grid(Vec3::new(3.5, -1.0, 2.25), 0.75, [50, 20, 70]);
        let mut rng = DeterministicRng::new(0xDE7E_4D14);
        let lights: Vec<Light> = (0..90)
            .map(|_| {
                let p = |rng: &mut DeterministicRng| (rng.next_f32() - 0.5) * 40.0;
                light(
                    Vec3::new(p(&mut rng), rng.next_f32() * 10.0, p(&mut rng)),
                    1.0 + rng.next_f32() * 6.0,
                )
            })
            .collect();

        let first = build_bin(&world, &lights, BinBuild::Culled);
        let second = build_bin(&world, &lights, BinBuild::Culled);
        assert_eq!(
            bytemuck::bytes_of(&first.params),
            bytemuck::bytes_of(&second.params)
        );
        assert_eq!(first.table, second.table);
        assert_eq!(first.indices, second.indices);
    }

    /// The degenerate oracle files every light into every cell, compactly: one shared `[0..N)` list
    /// every cell's `(0, N)` entry points at. If it ever drifted, the byte comparison it anchors
    /// would be comparing the culled build against something other than "admit everything".
    #[test]
    fn the_degenerate_oracle_files_every_light_into_every_cell() {
        let world = grid(Vec3::ZERO, 1.0, [32, 8, 32]);
        let lights: Vec<Light> = (0..5)
            .map(|i| light(Vec3::new(i as f32 * 4.0, 2.0, 0.0), 1.5))
            .collect();
        let bin = build_bin(&world, &lights, BinBuild::Degenerate);

        assert_eq!(bin.table.len(), BIN_CELLS);
        assert!(
            bin.table
                .iter()
                .all(|&[offset, count]| offset == 0 && count == 5)
        );
        assert_eq!(bin.indices, (0..5).collect::<Vec<u32>>());
        // And it is genuinely a superset of the culled build, cell for cell.
        let culled = build_bin(&world, &lights, BinBuild::Culled);
        for index in 0..5u32 {
            for cell in cells_of(&culled, index) {
                assert!(cells_of(&bin, index).contains(&cell));
            }
        }
    }

    /// A light whose position or range is not finite takes the whole-grid fallback rather than a
    /// footprint derived from a NaN — the same posture `cluster.rs` takes for a light it cannot
    /// project. Conservative by definition, so no evaluation point can miss it.
    #[test]
    fn a_non_finite_light_is_filed_everywhere() {
        let world = grid(Vec3::ZERO, 1.0, [32, 8, 32]);
        let bin = build_bin(
            &world,
            &[light(Vec3::new(f32::NAN, 0.0, 0.0), 2.0)],
            BinBuild::Culled,
        );
        assert_eq!(cells_of(&bin, 0).len(), BIN_CELLS);
    }

    /// An empty light set builds an empty-but-well-formed bin: every cell zero-count, no indices.
    /// The caller pads the buffers for wgpu; the tables say the padding is never read.
    #[test]
    fn an_empty_light_set_bins_to_an_empty_table() {
        let bin = build_bin(&grid(Vec3::ZERO, 1.0, [16, 8, 16]), &[], BinBuild::Culled);
        assert_eq!(bin.table.len(), BIN_CELLS);
        assert!(bin.table.iter().all(|&[_, count]| count == 0));
        assert!(bin.indices.is_empty());
    }
}
