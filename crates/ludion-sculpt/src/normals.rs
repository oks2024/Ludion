//! Vertex normals: the field gradient everywhere, split into faceted normals across creases on
//! surfaces owned by `sharp` parts.
//!
//! A triangle is sharp when all three of its vertices carry a `sharp` part as their dominant
//! material weight. Around each vertex the sharp triangles group edge-to-edge while their face
//! normals stay within [`CREASE_ANGLE_DEGREES`]; each group shares one vertex whose normal is the
//! area-weighted mean of its faces. The first group (lowest triangle index) keeps the original vertex
//! when no smooth triangle uses it; every other group gets a duplicate appended in vertex, then group,
//! order — the only place the vertex count grows.

use ludion_core::math;

use crate::grid::SculptField;

/// Faces around a sharp vertex whose normals differ by more than this angle get separate vertices.
pub(crate) const CREASE_ANGLE_DEGREES: f32 = 60.0;

/// Half the central-difference span of the gradient, in cells.
const GRADIENT_STEP_CELLS: f32 = 0.5;

/// The normal where the gradient vanishes.
const FALLBACK_NORMAL: [f32; 3] = [0.0, 1.0, 0.0];

/// The normalized field gradient at every position, by central differences.
pub(crate) fn gradient_normals(
    field: &SculptField,
    positions: &[[f32; 3]],
    spacing: f32,
) -> Vec<[f32; 3]> {
    let step = GRADIENT_STEP_CELLS * spacing;
    positions
        .iter()
        .map(|&p| {
            let gradient = [0, 1, 2].map(|axis| {
                let (mut ahead, mut behind) = (p, p);
                ahead[axis] += step;
                behind[axis] -= step;
                field.value(ahead) - field.value(behind)
            });
            normalize(gradient).unwrap_or(FALLBACK_NORMAL)
        })
        .collect()
}

/// A crease-split vertex stream.
pub(crate) struct Split {
    /// For every output vertex, the input vertex it copies.
    pub(crate) source: Vec<u32>,
    pub(crate) normals: Vec<[f32; 3]>,
    pub(crate) indices: Vec<u32>,
}

/// Splits `normals` across creases on sharp triangles; `sharp[v]` says vertex `v`'s dominant part is
/// `sharp`.
pub(crate) fn crease_split(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    indices: &[u32],
    sharp: &[bool],
) -> Split {
    let vertex_count = positions.len();
    let triangle_count = indices.len() / 3;
    let corners = |t: usize| [indices[3 * t], indices[3 * t + 1], indices[3 * t + 2]];
    let sharp_triangle: Vec<bool> = (0..triangle_count)
        .map(|t| corners(t).iter().all(|&v| sharp[v as usize]))
        .collect();

    let mut split = Split {
        source: (0..vertex_count as u32).collect(),
        normals: normals.to_vec(),
        indices: indices.to_vec(),
    };
    if !sharp_triangle.iter().any(|&s| s) {
        return split;
    }

    let crease_cosine = math::cos(CREASE_ANGLE_DEGREES.to_radians());
    let face_normals: Vec<[f32; 3]> = (0..triangle_count)
        .map(|t| {
            let [a, b, c] = corners(t).map(|v| positions[v as usize]);
            cross(sub(b, a), sub(c, a))
        })
        .collect();
    let face_units: Vec<[f32; 3]> = face_normals
        .iter()
        .map(|&n| normalize(n).unwrap_or([0.0; 3]))
        .collect();

    let mut used_smooth = vec![false; vertex_count];
    let mut offsets = vec![0usize; vertex_count + 1];
    for (t, &is_sharp) in sharp_triangle.iter().enumerate() {
        for v in corners(t) {
            if is_sharp {
                offsets[v as usize + 1] += 1;
            } else {
                used_smooth[v as usize] = true;
            }
        }
    }
    for v in 0..vertex_count {
        offsets[v + 1] += offsets[v];
    }
    let mut fill = offsets.clone();
    let mut incident = vec![0u32; offsets[vertex_count]];
    for t in (0..triangle_count).filter(|&t| sharp_triangle[t]) {
        for v in corners(t) {
            incident[fill[v as usize]] = t as u32;
            fill[v as usize] += 1;
        }
    }

    for v in 0..vertex_count {
        let mut fan: Vec<u32> = incident[offsets[v]..offsets[v + 1]].to_vec();
        fan.dedup();
        if fan.is_empty() {
            continue;
        }
        let mut root: Vec<usize> = (0..fan.len()).collect();
        for i in 0..fan.len() {
            for j in i + 1..fan.len() {
                let (ti, tj) = (fan[i] as usize, fan[j] as usize);
                if shares_edge_through(corners(ti), corners(tj), v as u32)
                    && dot(face_units[ti], face_units[tj]) >= crease_cosine
                {
                    let (ri, rj) = (find(&mut root, i), find(&mut root, j));
                    root[ri.max(rj)] = ri.min(rj);
                }
            }
        }
        let mut first_group = true;
        for i in 0..fan.len() {
            if find(&mut root, i) != i {
                continue;
            }
            let mut sum = [0.0f32; 3];
            for (j, &t) in fan.iter().enumerate() {
                if find(&mut root, j) == i {
                    sum = add(sum, face_normals[t as usize]);
                }
            }
            let normal = normalize(sum).unwrap_or(normals[v]);
            let target = if first_group && !used_smooth[v] {
                split.normals[v] = normal;
                v as u32
            } else {
                split.source.push(v as u32);
                split.normals.push(normal);
                (split.source.len() - 1) as u32
            };
            first_group = false;
            for (j, &t) in fan.iter().enumerate() {
                if find(&mut root, j) == i {
                    let slots = 3 * t as usize..3 * t as usize + 3;
                    for (slot, &index) in
                        split.indices[slots.clone()].iter_mut().zip(&indices[slots])
                    {
                        if index == v as u32 {
                            *slot = target;
                        }
                    }
                }
            }
        }
    }
    split
}

fn find(root: &mut [usize], mut i: usize) -> usize {
    while root[i] != i {
        root[i] = root[root[i]];
        i = root[i];
    }
    i
}

/// Whether two triangles around `v` share one of their other vertices, i.e. an edge through `v`.
fn shares_edge_through(a: [u32; 3], b: [u32; 3], v: u32) -> bool {
    a.iter().any(|&u| u != v && b.contains(&u))
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = math::sqrt(dot(v, v));
    (length > 0.0 && length.is_finite()).then(|| v.map(|c| c / length))
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two faces of a tent meeting at 90° along the edge 0–1.
    fn tent() -> (Vec<[f32; 3]>, Vec<u32>) {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, -1.0, 0.5],
            [-1.0, -1.0, 0.5],
        ];
        (positions, vec![0, 1, 2, 1, 0, 3])
    }

    #[test]
    fn a_crease_on_sharp_triangles_duplicates_its_vertices() {
        let (positions, indices) = tent();
        let smooth = vec![[0.0, 1.0, 0.0]; 4];
        let split = crease_split(&positions, &smooth, &indices, &[true; 4]);
        assert_eq!(split.source, vec![0, 1, 2, 3, 0, 1]);
        assert_eq!(split.indices, vec![0, 1, 2, 5, 4, 3]);
        assert!(dot(split.normals[0], split.normals[4]) < 0.5);
    }

    #[test]
    fn smooth_triangles_are_left_alone() {
        let (positions, indices) = tent();
        let smooth = vec![[0.0, 1.0, 0.0]; 4];
        let split = crease_split(&positions, &smooth, &indices, &[true, true, false, true]);
        assert_eq!(split.source, vec![0, 1, 2, 3, 0, 1]);
        assert_eq!(split.indices, vec![0, 1, 2, 5, 4, 3]);
        assert_eq!(split.normals[0], [0.0, 1.0, 0.0]);
        let split = crease_split(&positions, &smooth, &indices, &[false; 4]);
        assert_eq!(split.indices, indices);
        assert_eq!(split.normals, smooth);
    }
}
