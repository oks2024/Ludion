//! An exact Euclidean distance transform of a voxel occupancy grid, signed by inside/outside.
//!
//! Meijster–Roerdink–Hesselink in three separable passes — a 1D scan along `x`, then the lower
//! envelope of a row of parabolas along `y` and along `z` — carried entirely in integer squared
//! distances, so the result is the *exact* Euclidean transform (never the chamfer approximation)
//! and identical on Windows x86-64 and macOS arm64. The only floats are the final `sqrt` of each
//! squared distance and the trilinear sample, both through [`ludion_core::math`]. This module knows
//! nothing about sculpt documents — only an occupancy grid.

use ludion_core::math;

/// A per-voxel signed distance field in voxel units, negative inside the occupied region.
///
/// Voxel `(x, y, z)` is centred at `(x + 0.5, y + 0.5, z + 0.5)`, so the grid occupies the box
/// `[0, dims]` and [`SignedDistanceGrid::sample`] reads the field anywhere in or beyond it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SignedDistanceGrid {
    dims: [usize; 3],
    /// One value per voxel, indexed `x + dims[0] * (y + dims[1] * z)`.
    values: Vec<f32>,
}

impl SignedDistanceGrid {
    /// A grid of values a caller measured itself, indexed `x + dims[0] * (y + dims[1] * z)` — the
    /// same convention and the same [`SignedDistanceGrid::sample`] reconstruction as the transform
    /// below, for a field that is sampled rather than transformed from an occupancy.
    ///
    /// # Panics
    ///
    /// When any dimension is zero, or `values.len()` is not their product.
    pub(crate) fn from_values(dims: [usize; 3], values: Vec<f32>) -> SignedDistanceGrid {
        assert!(
            dims.iter().all(|&n| n > 0),
            "a distance grid needs a non-empty lattice, got {dims:?}"
        );
        assert_eq!(
            values.len(),
            dims[0] * dims[1] * dims[2],
            "{} values do not fill {dims:?}",
            values.len()
        );
        SignedDistanceGrid { dims, values }
    }

    /// Voxels along x, y, z.
    pub(crate) fn dims(&self) -> [usize; 3] {
        self.dims
    }

    /// The field at `p`, in voxel units.
    ///
    /// Inside the grid this is the trilinear interpolation of the per-voxel values about the voxel
    /// centres; within the half-voxel border band the sample clamps to the border voxel. Beyond the
    /// grid's box it is the clamped sample plus the Euclidean distance from `p` to that box, which
    /// keeps the field a valid lower bound on the true distance however far out it is read — what
    /// surface nets and a bounding pass need to step safely toward the surface.
    pub(crate) fn sample(&self, p: [f32; 3]) -> f32 {
        let clamped = [0, 1, 2].map(|axis| p[axis].clamp(0.0, self.dims[axis] as f32));
        let beyond = [0, 1, 2]
            .map(|axis| p[axis] - clamped[axis])
            .iter()
            .map(|d| d * d)
            .sum::<f32>();
        let interior = self.trilinear(clamped);
        if beyond > 0.0 {
            interior + math::sqrt(beyond)
        } else {
            interior
        }
    }

    /// Trilinear interpolation of the voxel values at a point already inside the grid's box, the
    /// lattice coordinate clamped so the half-voxel band outside the outermost centres reads that
    /// border voxel exactly.
    fn trilinear(&self, p: [f32; 3]) -> f32 {
        let last = self.dims.map(|n| n - 1);
        let lattice = [0, 1, 2].map(|axis| (p[axis] - 0.5).clamp(0.0, last[axis] as f32));
        let base = [0, 1, 2].map(|axis| (lattice[axis] as usize).min(last[axis]));
        let next = [0, 1, 2].map(|axis| (base[axis] + 1).min(last[axis]));
        let frac = [0, 1, 2].map(|axis| lattice[axis] - base[axis] as f32);

        let at =
            |x: usize, y: usize, z: usize| self.values[x + self.dims[0] * (y + self.dims[1] * z)];
        // `a + (b - a) * t` returns `a` bit-exactly at `t = 0`, so a sample on a voxel centre is
        // that voxel's stored value and not a rounded copy of it.
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let edge = |y: usize, z: usize| lerp(at(base[0], y, z), at(next[0], y, z), frac[0]);
        let face = |z: usize| lerp(edge(base[1], z), edge(next[1], z), frac[1]);
        lerp(face(base[2]), face(next[2]), frac[2])
    }
}

/// The exact signed Euclidean distance transform of `occupancy`, indexed
/// `x + dims[0] * (y + dims[1] * z)`.
///
/// Two transforms run: the outside distance (from each empty voxel to the nearest occupied one) and
/// the inside distance (from each occupied voxel to the nearest empty one, the region beyond the
/// grid counting as empty). Since one of the pair is zero at every voxel, the field is
/// `sqrt(outside_sq) - sqrt(inside_sq)`, shifted half a voxel so the zero level sits on the voxel
/// faces rather than on the centres: an occupied voxel gets `-(sqrt(inside_sq) - 0.5)`, an empty one
/// `sqrt(outside_sq) - 0.5`. A voxel on either side of a flat face therefore reads exactly ∓0.5.
///
/// An all-empty grid has no occupied voxel to measure from at all: every value is the grid's box
/// diagonal, positive — never an infinity, so the field stays finite and comparable wherever it is
/// folded. An all-full grid is negative everywhere, its magnitude the distance to just beyond the
/// nearest face, since the region beyond the grid is empty.
///
/// # Panics
///
/// When any dimension is zero, or `occupancy.len()` is not their product.
pub(crate) fn signed_distance(occupancy: &[bool], dims: [usize; 3]) -> SignedDistanceGrid {
    assert!(
        dims.iter().all(|&n| n > 0),
        "a distance transform needs a non-empty grid, got {dims:?}"
    );
    assert_eq!(
        occupancy.len(),
        dims[0] * dims[1] * dims[2],
        "occupancy of {} voxels does not fill {dims:?}",
        occupancy.len()
    );

    let outside = squared_edt(occupancy, dims);
    let padded = [dims[0] + 2, dims[1] + 2, dims[2] + 2];
    let inside = squared_edt(&padded_empty(occupancy, dims), padded);

    let span: u64 = dims.iter().map(|&n| (n as u64) * (n as u64)).sum();
    let diagonal = math::sqrt(span as f32);
    let (outside_unreachable, inside_unreachable) = (far(dims).pow(2), far(padded).pow(2));

    let mut values = Vec::with_capacity(occupancy.len());
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let at = x + dims[0] * (y + dims[1] * z);
                values.push(if occupancy[at] {
                    let shell = (x + 1) + padded[0] * ((y + 1) + padded[1] * (z + 1));
                    -(resolve(inside[shell], inside_unreachable, diagonal) - 0.5)
                } else {
                    resolve(outside[at], outside_unreachable, diagonal) - 0.5
                });
            }
        }
    }
    SignedDistanceGrid { dims, values }
}

/// A squared distance as a distance in voxel units; a voxel the transform never reached (no source
/// anywhere in the grid) falls back to the box diagonal.
fn resolve(squared: u64, unreachable: u64, diagonal: f32) -> f32 {
    if squared >= unreachable {
        diagonal
    } else {
        math::sqrt(squared as f32)
    }
}

/// The inside transform's source set: every empty voxel, on a grid padded with one voxel of empty on
/// each side. The nearest point of the region beyond the grid is always the axis-aligned projection
/// onto the nearest face, so that one-voxel shell carries it exactly — no wider pad is needed.
fn padded_empty(occupancy: &[bool], dims: [usize; 3]) -> Vec<bool> {
    let padded = [dims[0] + 2, dims[1] + 2, dims[2] + 2];
    let mut source = vec![true; padded[0] * padded[1] * padded[2]];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let shell = (x + 1) + padded[0] * ((y + 1) + padded[1] * (z + 1));
                source[shell] = !occupancy[x + dims[0] * (y + dims[1] * z)];
            }
        }
    }
    source
}

/// The clamp the 1D pass uses where a row holds no source. It strictly exceeds every distance
/// realizable inside the grid, so a source-free row never wins the lower envelope against a real
/// source, and its square stays far inside `u64`.
fn far(dims: [usize; 3]) -> u64 {
    (dims[0] + dims[1] + dims[2]) as u64
}

/// The squared Euclidean distance from every voxel to the nearest `true` voxel of `source`. A voxel
/// with no source anywhere reads at least `far(dims).pow(2)`, which no reachable voxel can.
fn squared_edt(source: &[bool], dims: [usize; 3]) -> Vec<u64> {
    let ceiling = far(dims);
    let mut values = vec![0u64; source.len()];

    for z in 0..dims[2] {
        for y in 0..dims[1] {
            let row = (y + dims[1] * z) * dims[0];
            let mut run = ceiling;
            for x in 0..dims[0] {
                run = if source[row + x] {
                    0
                } else {
                    (run + 1).min(ceiling)
                };
                values[row + x] = run;
            }
            run = ceiling;
            for x in (0..dims[0]).rev() {
                run = if source[row + x] {
                    0
                } else {
                    (run + 1).min(ceiling)
                };
                values[row + x] = values[row + x].min(run);
                let d = values[row + x];
                values[row + x] = d * d;
            }
        }
    }

    envelope_axis(&mut values, dims, 1);
    envelope_axis(&mut values, dims, 2);
    values
}

/// Runs the lower envelope along `axis` over every line of the grid, replacing each value with
/// `min over i of (u - i)^2 + value[i]`. Given the exact transform of the preceding axes this yields
/// the exact transform of one axis more; the lines are walked in a fixed order, though the result is
/// independent of it.
fn envelope_axis(values: &mut [u64], dims: [usize; 3], axis: usize) {
    let strides = [1, dims[0], dims[0] * dims[1]];
    let others = match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let length = dims[axis];
    let (mut line, mut out) = (vec![0u64; length], vec![0u64; length]);
    let (mut vertices, mut breaks) = (vec![0usize; length], vec![0i64; length]);

    for outer in 0..dims[others[1]] {
        for inner in 0..dims[others[0]] {
            let base = outer * strides[others[1]] + inner * strides[others[0]];
            let stride = strides[axis];
            for (u, slot) in line.iter_mut().enumerate() {
                *slot = values[base + u * stride];
            }
            lower_envelope(&line, &mut vertices, &mut breaks, &mut out);
            for (u, &value) in out.iter().enumerate() {
                values[base + u * stride] = value;
            }
        }
    }
}

/// The lower envelope of the parabolas `f(x, i) = (x - i)^2 + g[i]`, evaluated at every integer `x`
/// of the line. `vertices` and `breaks` are scratch: the stack of parabolas currently on the
/// envelope and the `x` at which each takes over.
fn lower_envelope(g: &[u64], vertices: &mut [usize], breaks: &mut [i64], out: &mut [u64]) {
    let length = g.len();
    let f = |x: i64, i: usize| {
        let d = x - i as i64;
        (d * d) as u64 + g[i]
    };

    let mut top: isize = 0;
    vertices[0] = 0;
    breaks[0] = 0;
    for u in 1..length {
        while top >= 0
            && f(breaks[top as usize], vertices[top as usize]) > f(breaks[top as usize], u)
        {
            top -= 1;
        }
        if top < 0 {
            top = 0;
            vertices[0] = u;
        } else {
            let takeover = 1 + separation(g, vertices[top as usize], u);
            if takeover < length as i64 {
                top += 1;
                vertices[top as usize] = u;
                breaks[top as usize] = takeover;
            }
        }
    }
    for u in (0..length).rev() {
        out[u] = f(u as i64, vertices[top as usize]);
        if u as i64 == breaks[top as usize] {
            top -= 1;
        }
    }
}

/// The largest integer `x` at which parabola `i` is still at or below parabola `u` (`i < u`): the
/// floor of their real crossing point. The division is Euclidean, not truncating — the numerator is
/// negative whenever `g[i]` far exceeds `g[u]`, and rounding it toward zero would keep `i` on the
/// envelope one step too far and return a distance that is merely close.
fn separation(g: &[u64], i: usize, u: usize) -> i64 {
    let (i, u) = (i as i64, u as i64);
    let numerator = u * u - i * i + g[u as usize] as i64 - g[i as usize] as i64;
    numerator.div_euclid(2 * (u - i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(dims: [usize; 3], x: usize, y: usize, z: usize) -> usize {
        x + dims[0] * (y + dims[1] * z)
    }

    /// The same field by exhaustive search over every source voxel, the region beyond the grid
    /// included as empty — the definition [`signed_distance`] is an optimization of.
    fn brute_force(occupancy: &[bool], dims: [usize; 3]) -> Vec<f32> {
        let padded = [dims[0] + 2, dims[1] + 2, dims[2] + 2];
        let mut out = Vec::with_capacity(occupancy.len());
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let solid = occupancy[index(dims, x, y, z)];
                    let mut best = u64::MAX;
                    for qz in 0..padded[2] {
                        for qy in 0..padded[1] {
                            for qx in 0..padded[0] {
                                let within = (1..=dims[0]).contains(&qx)
                                    && (1..=dims[1]).contains(&qy)
                                    && (1..=dims[2]).contains(&qz);
                                let occupied =
                                    within && occupancy[index(dims, qx - 1, qy - 1, qz - 1)];
                                if occupied == solid {
                                    continue;
                                }
                                let d = [
                                    qx as i64 - 1 - x as i64,
                                    qy as i64 - 1 - y as i64,
                                    qz as i64 - 1 - z as i64,
                                ];
                                best = best.min((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as u64);
                            }
                        }
                    }
                    let distance = math::sqrt(best as f32);
                    out.push(if solid {
                        -(distance - 0.5)
                    } else {
                        distance - 0.5
                    });
                }
            }
        }
        out
    }

    #[test]
    fn one_voxel_gives_the_analytic_distance_to_its_centre() {
        let dims = [5, 5, 5];
        let centre = index(dims, 2, 2, 2);
        let mut occupancy = vec![false; dims[0] * dims[1] * dims[2]];
        occupancy[centre] = true;

        let grid = signed_distance(&occupancy, dims);
        assert_eq!(grid.dims(), dims);
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let at = index(dims, x, y, z);
                    let d = [x, y, z].map(|c| c as f32 - 2.0);
                    let analytic = math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                    // The one occupied voxel is a voxel away from empty in every direction.
                    let expected = if at == centre { -0.5 } else { analytic - 0.5 };
                    assert!(
                        (grid.values[at] - expected).abs() <= 1e-6,
                        "({x}, {y}, {z}): {} is not {expected}",
                        grid.values[at]
                    );
                }
            }
        }
        assert!(grid.values[centre] < 0.0);
    }

    #[test]
    fn a_solid_cube_is_deepest_at_its_centre_and_half_a_voxel_at_its_faces() {
        let dims = [8, 8, 8];
        let mut occupancy = vec![false; dims[0] * dims[1] * dims[2]];
        for z in 2..6 {
            for y in 2..6 {
                for x in 2..6 {
                    occupancy[index(dims, x, y, z)] = true;
                }
            }
        }

        let grid = signed_distance(&occupancy, dims);
        let deepest = grid.values.iter().copied().fold(f32::INFINITY, f32::min);
        assert_eq!(deepest, -1.5, "the 4-voxel cube's core is two voxels deep");
        for z in 3..5 {
            for y in 3..5 {
                for x in 3..5 {
                    assert_eq!(grid.values[index(dims, x, y, z)], deepest);
                }
            }
        }
        assert_eq!(grid.values[index(dims, 2, 3, 3)], -0.5, "a face voxel");
        assert_eq!(
            grid.values[index(dims, 1, 3, 3)],
            0.5,
            "just outside a face"
        );
    }

    #[test]
    fn an_asymmetric_shape_matches_the_exhaustive_reference() {
        let dims = [6, 5, 4];
        let mut occupancy = vec![false; dims[0] * dims[1] * dims[2]];
        for z in 1..3 {
            for x in 0..4 {
                occupancy[index(dims, x, 1, z)] = true;
            }
            for y in 1..5 {
                occupancy[index(dims, 0, y, z)] = true;
            }
        }

        let grid = signed_distance(&occupancy, dims);
        assert_eq!(
            grid.values,
            brute_force(&occupancy, dims),
            "the separable transform must be exact, not merely close"
        );
    }

    #[test]
    fn sampling_reads_voxel_centres_exactly_and_grows_outside_the_box() {
        let dims = [5, 4, 3];
        let mut occupancy = vec![false; dims[0] * dims[1] * dims[2]];
        occupancy[index(dims, 1, 2, 1)] = true;
        occupancy[index(dims, 2, 2, 1)] = true;

        let grid = signed_distance(&occupancy, dims);
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let at = index(dims, x, y, z);
                    let centre = [x, y, z].map(|c| c as f32 + 0.5);
                    assert_eq!(grid.sample(centre), grid.values[at], "({x}, {y}, {z})");
                }
            }
        }

        let border = grid.values[index(dims, 0, 2, 1)];
        assert_eq!(grid.sample([-1.0, 2.5, 1.5]), border + 1.0);
        let far_corner = grid.values[index(dims, 4, 3, 2)];
        assert_eq!(
            grid.sample([dims[0] as f32 + 1.0, 3.5, 2.5]),
            far_corner + 1.0
        );
    }

    /// Pins the field of a fixed pseudo-random occupancy, so any change to the transform's
    /// arithmetic shows up as a hash mismatch rather than as a silently different mesh.
    #[test]
    fn a_pseudo_random_occupancy_hashes_to_a_pinned_value() {
        let dims = [12, 10, 9];
        let count = dims[0] * dims[1] * dims[2];
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut occupancy = Vec::with_capacity(count);
        for _ in 0..count {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            occupancy.push((state >> 33) % 5 < 2);
        }

        let grid = signed_distance(&occupancy, dims);
        let bytes: Vec<u8> = grid
            .values
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect();
        assert_eq!(
            ludion_assets::content_hash(&bytes),
            9_152_167_254_837_221_700
        );
    }
}
