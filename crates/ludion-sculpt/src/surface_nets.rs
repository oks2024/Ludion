//! Naive surface nets: one vertex per cell the surface crosses, one quad per lattice edge it crosses.
//!
//! A corner is inside when its value is `< 0`. A crossed cell's vertex is the mean of the linear
//! sign-change points on its crossed edges; vertices are numbered in `z, y, x` discovery order. A
//! crossed lattice edge joins the four cells around it into a quad wound counter-clockwise seen from
//! the outside (the side the field increases toward), split along its shorter diagonal with ties
//! to the `(0, 2)` diagonal.

use crate::grid::Grid;

/// The extracted surface: positions and a counter-clockwise triangle list.
pub(crate) struct SurfaceNet {
    pub(crate) positions: Vec<[f32; 3]>,
    pub(crate) indices: Vec<u32>,
}

const NO_VERTEX: u32 = u32::MAX;

pub(crate) fn extract(grid: &Grid) -> SurfaceNet {
    let [nx, ny, nz] = grid.cells;
    let cell_index = |c: [usize; 3]| (c[2] * ny + c[1]) * nx + c[0];
    let mut cell_vertex = vec![NO_VERTEX; nx * ny * nz];
    let mut positions: Vec<[f32; 3]> = Vec::new();

    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let corner = |bits: usize| [x + (bits & 1), y + ((bits >> 1) & 1), z + (bits >> 2)];
                let values: [f32; 8] = core::array::from_fn(|bits| grid.value(corner(bits)));
                let inside = values.map(|v| v < 0.0);
                if inside.iter().all(|&i| i) || inside.iter().all(|&i| !i) {
                    continue;
                }
                let mut sum = [0.0f32; 3];
                let mut count = 0u32;
                for axis_bit in [1, 2, 4] {
                    for a in 0..8 {
                        let b = a | axis_bit;
                        if a & axis_bit != 0 || inside[a] == inside[b] {
                            continue;
                        }
                        let point = crossing(
                            grid.corner_position(corner(a)),
                            grid.corner_position(corner(b)),
                            values[a],
                            values[b],
                        );
                        for axis in 0..3 {
                            sum[axis] += point[axis];
                        }
                        count += 1;
                    }
                }
                cell_vertex[cell_index([x, y, z])] = positions.len() as u32;
                positions.push(sum.map(|s| s / count as f32));
            }
        }
    }

    let mut indices: Vec<u32> = Vec::new();
    for z in 0..=nz {
        for y in 0..=ny {
            for x in 0..=nx {
                let c = [x, y, z];
                for a in 0..3 {
                    if c[a] >= grid.cells[a] {
                        continue;
                    }
                    let (b, d) = ((a + 1) % 3, (a + 2) % 3);
                    if c[b] == 0 || c[b] >= grid.cells[b] || c[d] == 0 || c[d] >= grid.cells[d] {
                        continue;
                    }
                    let mut far = c;
                    far[a] += 1;
                    let near_inside = grid.value(c) < 0.0;
                    if near_inside == (grid.value(far) < 0.0) {
                        continue;
                    }
                    let around = |db: usize, dd: usize| {
                        let mut cell = c;
                        cell[b] = c[b] + db - 1;
                        cell[d] = c[d] + dd - 1;
                        cell_vertex[cell_index(cell)]
                    };
                    let ring = [around(0, 0), around(1, 0), around(1, 1), around(0, 1)];
                    let quad = if near_inside {
                        ring
                    } else {
                        [ring[0], ring[3], ring[2], ring[1]]
                    };
                    push_quad(&mut indices, &positions, quad);
                }
            }
        }
    }

    SurfaceNet { positions, indices }
}

/// The point on the corner-to-corner segment where the linearly interpolated field is zero.
fn crossing(pa: [f32; 3], pb: [f32; 3], va: f32, vb: f32) -> [f32; 3] {
    let t = (va / (va - vb)).clamp(0.0, 1.0);
    [0, 1, 2].map(|axis| pa[axis] + (pb[axis] - pa[axis]) * t)
}

fn push_quad(indices: &mut Vec<u32>, positions: &[[f32; 3]], q: [u32; 4]) {
    let p = q.map(|v| positions[v as usize]);
    if distance_squared(p[1], p[3]) < distance_squared(p[0], p[2]) {
        indices.extend_from_slice(&[q[0], q[1], q[3], q[1], q[2], q[3]]);
    } else {
        indices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    }
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}
