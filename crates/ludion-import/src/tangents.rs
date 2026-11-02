//! A deterministic per-vertex tangent generator (Lengyel's method), in-house rather than a new
//! dependency (`Plan/textured-pbr.md` G1) — the mesh pipeline already imports vertex data as plain
//! `f64` (the f32 round-trip rule in `convert.rs`), and the algorithm is a fixed, short arithmetic
//! chain that a crate would only wrap. Computed only when a material has a `normalTexture` but no
//! `TANGENT` accessor (a glTF document supplying `TANGENT` is imported verbatim instead); the
//! caller decides which case applies.
//!
//! Every input and intermediate value is `f64`, and every operation is plain arithmetic plus
//! `f64::sqrt` — no `glam`, no trigonometry — in a fixed association order, so the result is
//! bit-identical on x86_64 and arm64: the same property `ludion-sculpt`'s `edt` module leans on.

use ludion_scene::DEGENERATE_VECTOR_LENGTH_SQUARED;

/// Per-vertex tangents (xyz + handedness `w` in `{-1.0, 1.0}`), 4 values per vertex, from flat
/// positions (3/vertex), normals (3/vertex), uvs (2/vertex) and a triangle index list. The frame
/// follows glTF: `cross(n, t) * w` points toward decreasing `v` (image-up), as Blender's exporter
/// writes it.
///
/// Per triangle: the edge vectors and UV deltas give a determinant `r = 1 / (du1*dv2 - du2*dv1)`;
/// an exactly-zero or non-finite determinant (a degenerate UV triangle) contributes nothing. The
/// per-vertex accumulators are then reduced to a normal-orthogonal unit tangent (Gram–Schmidt) and
/// a handedness sign from the bitangent accumulator; a vertex touched by no non-degenerate
/// triangle, or whose accumulated tangent collapses under Gram–Schmidt, falls back to an axis
/// orthogonal to its normal.
pub fn compute_tangents(
    positions: &[f64],
    normals: &[f64],
    uvs: &[f64],
    indices: &[u32],
) -> Vec<f64> {
    let vertex_count = positions.len() / 3;
    let mut tan1 = vec![[0.0_f64; 3]; vertex_count];
    let mut tan2 = vec![[0.0_f64; 3]; vertex_count];

    for triangle in indices.chunks_exact(3) {
        let (i0, i1, i2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        let (p0, p1, p2) = (
            vec3_at(positions, i0),
            vec3_at(positions, i1),
            vec3_at(positions, i2),
        );
        let (uv0, uv1, uv2) = (vec2_at(uvs, i0), vec2_at(uvs, i1), vec2_at(uvs, i2));

        let e1 = sub3(p1, p0);
        let e2 = sub3(p2, p0);
        let (du1, dv1) = (uv1[0] - uv0[0], uv1[1] - uv0[1]);
        let (du2, dv2) = (uv2[0] - uv0[0], uv2[1] - uv0[1]);

        let det = du1 * dv2 - du2 * dv1;
        if det == 0.0 || !det.is_finite() {
            continue;
        }
        let r = 1.0 / det;
        let sdir = [
            (e1[0] * dv2 - e2[0] * dv1) * r,
            (e1[1] * dv2 - e2[1] * dv1) * r,
            (e1[2] * dv2 - e2[2] * dv1) * r,
        ];
        let tdir = [
            (e2[0] * du1 - e1[0] * du2) * r,
            (e2[1] * du1 - e1[1] * du2) * r,
            (e2[2] * du1 - e1[2] * du2) * r,
        ];
        for &v in &[i0, i1, i2] {
            tan1[v] = add3(tan1[v], sdir);
            tan2[v] = add3(tan2[v], tdir);
        }
    }

    let mut out = Vec::with_capacity(vertex_count * 4);
    for v in 0..vertex_count {
        let n = normalize3(vec3_at(normals, v)).unwrap_or([0.0, 0.0, 1.0]);
        let t = tan1[v];
        let tangent = if dot3(t, t) == 0.0 {
            orthogonal_axis(n)
        } else {
            let projected = sub3(t, scale3(n, dot3(n, t)));
            normalize3(projected).unwrap_or_else(|| orthogonal_axis(n))
        };
        // `tan2` is dPos/dv, toward *increasing* v; glTF's bitangent points the other way.
        let w = if dot3(cross3(n, tangent), tan2[v]) > 0.0 {
            -1.0
        } else {
            1.0
        };
        out.extend_from_slice(&[tangent[0], tangent[1], tangent[2], w]);
    }
    out
}

/// Replaces every authored tangent whose xyz is near zero (the mesh validator refuses those) with
/// the same normal-orthogonal axis [`compute_tangents`] falls back to, keeping its handedness `w`.
/// A verbatim `TANGENT` is otherwise untouched.
pub(crate) fn repair_degenerate_tangents(normals: &[f64], tangents: &mut [f64]) {
    for (v, t) in tangents.chunks_exact_mut(4).enumerate() {
        if dot3([t[0], t[1], t[2]], [t[0], t[1], t[2]]) < DEGENERATE_VECTOR_LENGTH_SQUARED {
            let n = normalize3(vec3_at(normals, v)).unwrap_or([0.0, 0.0, 1.0]);
            let axis = orthogonal_axis(n);
            t[..3].copy_from_slice(&axis);
        }
    }
}

/// Per-vertex faceted normals (3/vertex) from flat positions and a triangle index list: the
/// area-weighted sum of the incident face normals, normalized (a vertex on no triangle, or whose
/// faces cancel, gets +z, so the committed mesh never carries a zero-length normal).
///
/// Mirrors `ludion_engine::mesh_asset::compute_faceted_normals` (which the loader runs when a mesh
/// document omits `normals`) in `f64`, so a primitive that carries a normal map but no `NORMAL` can
/// be given a tangent frame orthogonal to the normals the importer commits alongside it.
pub(crate) fn faceted_normals(positions: &[f64], indices: &[u32]) -> Vec<f64> {
    let vertex_count = positions.len() / 3;
    let mut accumulated = vec![[0.0_f64; 3]; vertex_count];
    for triangle in indices.chunks_exact(3) {
        let (a, b, c) = (
            vec3_at(positions, triangle[0] as usize),
            vec3_at(positions, triangle[1] as usize),
            vec3_at(positions, triangle[2] as usize),
        );
        let face = cross3(sub3(b, a), sub3(c, a));
        for &vertex in triangle {
            accumulated[vertex as usize] = add3(accumulated[vertex as usize], face);
        }
    }
    let mut out = Vec::with_capacity(vertex_count * 3);
    for normal in &accumulated {
        out.extend_from_slice(&normalize3(*normal).unwrap_or([0.0, 0.0, 1.0]));
    }
    out
}

/// An arbitrary unit vector orthogonal to `n`: the axis of `n`'s smallest-magnitude component,
/// crossed with `n`. That component axis is never parallel to `n` (a vector parallel to an axis has
/// its *other* two components at the minimum, zero), so the cross product is never degenerate.
fn orthogonal_axis(n: [f64; 3]) -> [f64; 3] {
    let magnitudes = [n[0].abs(), n[1].abs(), n[2].abs()];
    let mut smallest = 0;
    for i in 1..3 {
        if magnitudes[i] < magnitudes[smallest] {
            smallest = i;
        }
    }
    let mut axis = [0.0_f64; 3];
    axis[smallest] = 1.0;
    // Never zero-length: see the doc comment above.
    normalize3(cross3(axis, n)).expect("the chosen axis is never parallel to n")
}

fn vec3_at(data: &[f64], i: usize) -> [f64; 3] {
    [data[i * 3], data[i * 3 + 1], data[i * 3 + 2]]
}

fn vec2_at(data: &[f64], i: usize) -> [f64; 2] {
    [data[i * 2], data[i * 2 + 1]]
}

fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `None` for a zero-length (or non-finite-length) vector.
fn normalize3(v: [f64; 3]) -> Option<[f64; 3]> {
    let len = f64::sqrt(dot3(v, v));
    if len == 0.0 || !len.is_finite() {
        None
    } else {
        Some(scale3(v, 1.0 / len))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_zero_length_authored_tangent_takes_the_collapse_fallback() {
        let normals = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let mut tangents = [0.0, 0.0, 0.0, -1.0, 1.0, 0.0, 0.0, 1.0];
        super::repair_degenerate_tangents(&normals, &mut tangents);
        let t = &tangents[..3];
        assert!((t.iter().map(|c| c * c).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(t[2].abs() < 1e-12, "orthogonal to the normal");
        assert_eq!(tangents[3], -1.0, "handedness is kept");
        assert_eq!(
            &tangents[4..],
            &[1.0, 0.0, 0.0, 1.0],
            "a good tangent is verbatim"
        );
    }

    use super::*;

    /// Two triangles forming a unit quad in the XY plane, UVs mapped 1:1 to x/y, normal +z.
    fn unit_quad(uvs: [[f64; 2]; 4]) -> ([f64; 12], [f64; 12], [f64; 8], [u32; 6]) {
        let positions = [
            0.0, 0.0, 0.0, // 0
            1.0, 0.0, 0.0, // 1
            1.0, 1.0, 0.0, // 2
            0.0, 1.0, 0.0, // 3
        ];
        let normals = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let uvs_flat = [
            uvs[0][0], uvs[0][1], uvs[1][0], uvs[1][1], uvs[2][0], uvs[2][1], uvs[3][0], uvs[3][1],
        ];
        let indices = [0, 1, 2, 0, 2, 3];
        (positions, normals, uvs_flat, indices)
    }

    #[test]
    fn a_unit_quad_yields_the_u_axis_tangent_with_negative_handedness() {
        let (positions, normals, uvs, indices) =
            unit_quad([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);
        assert_eq!(tangents.len(), 16);
        for v in 0..4 {
            let t = &tangents[v * 4..v * 4 + 4];
            assert_eq!(t, &[1.0, 0.0, 0.0, -1.0], "vertex {v}");
        }
    }

    #[test]
    fn mirrored_u_flips_handedness() {
        // u flipped relative to the previous test's mapping.
        let (positions, normals, uvs, indices) =
            unit_quad([[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);
        for v in 0..4 {
            assert_eq!(tangents[v * 4 + 3], 1.0, "vertex {v} handedness");
        }
    }

    #[test]
    fn a_degenerate_uv_triangle_falls_back_to_a_unit_orthogonal_axis_with_no_nan() {
        let positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let normals = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        // All three UVs identical: every determinant is exactly zero.
        let uvs = [0.25, 0.25, 0.25, 0.25, 0.25, 0.25];
        let indices = [0, 1, 2];

        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);
        for v in 0..3 {
            let t = [tangents[v * 4], tangents[v * 4 + 1], tangents[v * 4 + 2]];
            let w = tangents[v * 4 + 3];
            assert!(t.iter().all(|c| c.is_finite()), "vertex {v}: {t:?}");
            assert!(w.is_finite());
            let len = f64::sqrt(dot3(t, t));
            assert!((len - 1.0).abs() < 1e-12, "vertex {v}: length {len}");
            assert_eq!(w, 1.0, "no bitangent contribution defaults to +1");
        }
    }

    /// A small procedural cube (24 vertices, one normal per face, non-degenerate per-face UVs).
    fn procedural_cube() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<u32>) {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();

        // Each face: four corners (a right-handed quad), its outward normal, and an axis-aligned
        // UV square slightly offset per face so no two faces share identical UV deltas.
        type Face = ([[f64; 3]; 4], [f64; 3], [f64; 2]);
        let faces: [Face; 6] = [
            (
                [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
                [0., 0., 1.],
                [0.0, 0.0],
            ),
            (
                [
                    [1., -1., -1.],
                    [-1., -1., -1.],
                    [-1., 1., -1.],
                    [1., 1., -1.],
                ],
                [0., 0., -1.],
                [0.1, 0.0],
            ),
            (
                [[-1., 1., 1.], [1., 1., 1.], [1., 1., -1.], [-1., 1., -1.]],
                [0., 1., 0.],
                [0.2, 0.0],
            ),
            (
                [
                    [-1., -1., -1.],
                    [1., -1., -1.],
                    [1., -1., 1.],
                    [-1., -1., 1.],
                ],
                [0., -1., 0.],
                [0.3, 0.0],
            ),
            (
                [[1., -1., 1.], [1., -1., -1.], [1., 1., -1.], [1., 1., 1.]],
                [1., 0., 0.],
                [0.4, 0.0],
            ),
            (
                [
                    [-1., -1., -1.],
                    [-1., -1., 1.],
                    [-1., 1., 1.],
                    [-1., 1., -1.],
                ],
                [-1., 0., 0.],
                [0.5, 0.0],
            ),
        ];

        for (corners, normal, uv_offset) in faces {
            let base = (positions.len() / 3) as u32;
            let face_uvs = [[0.0, 0.0], [1.3, 0.0], [1.3, 0.7], [0.0, 0.7]];
            for (corner, uv) in corners.iter().zip(face_uvs.iter()) {
                positions.extend_from_slice(corner);
                normals.extend_from_slice(&normal);
                uvs.push(uv_offset[0] + uv[0]);
                uvs.push(uv_offset[1] + uv[1]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (positions, normals, uvs, indices)
    }

    /// Blender's glTF exporter writes `TANGENT` (MikkTSpace) for three quads with identity,
    /// u-mirrored and rotated UVs; the generator must reproduce its frame, handedness included.
    #[test]
    fn blender_exported_tangents_match_the_generated_frame() {
        let gltf::Gltf { document, blob } =
            gltf::Gltf::from_slice(include_bytes!("../tests/fixtures/tangent-quad.glb")).unwrap();
        let blob = blob.unwrap();
        let primitive = document
            .meshes()
            .next()
            .unwrap()
            .primitives()
            .next()
            .unwrap();
        let reader = primitive.reader(|_| Some(&blob[..]));
        let widen3 =
            |v: Vec<[f32; 3]>| -> Vec<f64> { v.iter().flatten().map(|c| f64::from(*c)).collect() };
        let positions = widen3(reader.read_positions().unwrap().collect());
        let normals = widen3(reader.read_normals().unwrap().collect());
        let uvs: Vec<f64> = reader
            .read_tex_coords(0)
            .unwrap()
            .into_f32()
            .flatten()
            .map(f64::from)
            .collect();
        let indices: Vec<u32> = reader.read_indices().unwrap().into_u32().collect();
        let exported: Vec<[f32; 4]> = reader.read_tangents().unwrap().collect();

        let generated = compute_tangents(&positions, &normals, &uvs, &indices);
        assert_eq!(generated.len(), exported.len() * 4);
        assert!(exported.iter().any(|t| t[3] > 0.0) && exported.iter().any(|t| t[3] < 0.0));
        for (v, blender) in exported.iter().enumerate() {
            let ours = &generated[v * 4..v * 4 + 4];
            assert_eq!(ours[3], f64::from(blender[3]), "vertex {v} handedness");
            for axis in 0..3 {
                assert!(
                    (ours[axis] - f64::from(blender[axis])).abs() < 1e-5,
                    "vertex {v} axis {axis}: ours {ours:?}, Blender {blender:?}"
                );
            }
        }
    }

    /// glTF's rule: `cross(n, t) * w` points toward decreasing `v` (image-up, UV origin top-left).
    #[test]
    fn the_bitangent_points_toward_decreasing_v() {
        let (positions, normals, uvs, indices) = procedural_cube();
        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);
        for triangle in indices.chunks_exact(3) {
            let [i0, i1, i2] = [triangle[0], triangle[1], triangle[2]].map(|i| i as usize);
            let (e1, e2) = (
                sub3(vec3_at(&positions, i1), vec3_at(&positions, i0)),
                sub3(vec3_at(&positions, i2), vec3_at(&positions, i0)),
            );
            let (du1, dv1) = (uvs[i1 * 2] - uvs[i0 * 2], uvs[i1 * 2 + 1] - uvs[i0 * 2 + 1]);
            let (du2, dv2) = (uvs[i2 * 2] - uvs[i0 * 2], uvs[i2 * 2 + 1] - uvs[i0 * 2 + 1]);
            let r = 1.0 / (du1 * dv2 - du2 * dv1);
            let d_pos_d_v = scale3(sub3(scale3(e2, du1), scale3(e1, du2)), r);
            let t = &tangents[i0 * 4..i0 * 4 + 4];
            let bitangent = scale3(cross3(vec3_at(&normals, i0), [t[0], t[1], t[2]]), t[3]);
            assert!(dot3(bitangent, d_pos_d_v) < 0.0, "triangle at vertex {i0}");
        }
    }

    #[test]
    fn every_tangent_on_a_cube_is_unit_length_and_orthogonal_to_its_normal() {
        let (positions, normals, uvs, indices) = procedural_cube();
        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);
        let vertex_count = positions.len() / 3;
        for v in 0..vertex_count {
            let n = vec3_at(&normals, v);
            let t = [tangents[v * 4], tangents[v * 4 + 1], tangents[v * 4 + 2]];
            let len = f64::sqrt(dot3(t, t));
            assert!((len - 1.0).abs() < 1e-12, "vertex {v}: length {len}");
            assert!(
                dot3(n, t).abs() < 1e-12,
                "vertex {v}: dot(n, t) = {}",
                dot3(n, t)
            );
        }
    }

    /// The committed tangent text is a bit-exact CPU artifact of a fixed algorithm, like the
    /// greedy voxel mesher's output — this hash pins it so any change to the arithmetic (order,
    /// fallback behavior, degenerate handling) shows up as a mismatch rather than a silently
    /// different tangent basis.
    #[test]
    fn a_fixed_cube_input_hashes_to_a_pinned_value() {
        let (positions, normals, uvs, indices) = procedural_cube();
        let tangents = compute_tangents(&positions, &normals, &uvs, &indices);

        let mut buffer = ryu::Buffer::new();
        let mut text = String::new();
        for value in &tangents {
            text.push_str(buffer.format(*value));
            text.push('\n');
        }
        let hash = ludion_assets::content_hash(text.as_bytes());
        assert_eq!(hash, 3_211_472_064_622_795_512);
    }
}
