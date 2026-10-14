//! Connected-component analysis over a raw indexed triangle mesh: two vertices are connected when a
//! triangle spans both of them, and — since a sculpted surface splits a crease into two vertices at
//! the same point with different normals — two vertices at *bit-identical* positions are connected
//! too, so a crease seam never reads as a disconnection. This is the mesh analogue of the voxel
//! mesher's 6-connectivity flood fill (`L1129`, `ludion-scene`'s `voxel::connectivity_warning`);
//! [`connectivity`] raises the sculpt mesher's `L1159` in the same message shape. This module knows
//! nothing about sculpt documents — only `positions`/`indices`.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math;

/// The mesh's connected-component count and, when there is more than one, the `L1159` warning
/// naming the stray pieces and the nearest vertex pair from each to the main body.
///
/// `(0, None)` for a mesh with no triangles (nothing to connect); `(1, None)` for a mesh whose
/// triangles form a single connected patch, whatever the vertex duplication at its creases.
/// Vertices referenced by no triangle are ignored — they cannot be found disconnected or connected,
/// since they contribute no surface at all.
///
/// # Example
/// ```
/// // Two triangles sharing an edge: one component.
/// let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
/// let indices = [0, 1, 2, 0, 2, 3];
/// let (count, warning) = ludion_sculpt::connectivity::connectivity(&positions, &indices);
/// assert_eq!(count, 1);
/// assert!(warning.is_none());
/// ```
pub fn connectivity(positions: &[[f32; 3]], indices: &[u32]) -> (u32, Option<Diagnostic>) {
    let mut union_find = UnionFind::new(positions.len());

    for triangle in indices.chunks_exact(3) {
        union_find.union(triangle[0], triangle[1]);
        union_find.union(triangle[1], triangle[2]);
    }
    merge_coincident_positions(positions, &mut union_find);

    let referenced = referenced_vertices(indices, positions.len());
    if referenced.is_empty() {
        return (0, None);
    }

    let mut components = group_by_root(&referenced, &mut union_find);
    // Size descending, then lowest vertex index — a total order, so the result (and the message
    // built from it) is deterministic regardless of triangle authoring order.
    components.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));

    let count = components.len() as u32;
    if components.len() <= 1 {
        return (count, None);
    }
    (count, Some(warning_of(positions, &components)))
}

/// Sorted, de-duplicated indices of every vertex at least one triangle references. `indices` may
/// name at most `vertex_count` distinct vertices; a trailing partial triangle (a length not a
/// multiple of 3) is ignored, as [`connectivity`]'s union pass already does via `chunks_exact`.
fn referenced_vertices(indices: &[u32], vertex_count: usize) -> Vec<u32> {
    let mut seen = vec![false; vertex_count];
    for &index in indices.chunks_exact(3).flatten() {
        if let Some(slot) = seen.get_mut(index as usize) {
            *slot = true;
        }
    }
    (0..vertex_count as u32)
        .filter(|&index| seen[index as usize])
        .collect()
}

/// Unions every pair of referenced vertices at the same position — sort-based, not a hashing
/// container: [`referenced`] indices are sorted by position with [`f32::total_cmp`] (a total,
/// deterministic order over every representable `f32`, `NaN` included), so vertices at the same
/// point land adjacent and are unioned as one run.
fn merge_coincident_positions(positions: &[[f32; 3]], union_find: &mut UnionFind) {
    let mut by_position: Vec<u32> = (0..positions.len() as u32).collect();
    by_position.sort_by(|&a, &b| cmp_position(&positions[a as usize], &positions[b as usize]));
    for window in by_position.windows(2) {
        let (a, b) = (window[0], window[1]);
        if positions[a as usize] == positions[b as usize] {
            union_find.union(a, b);
        }
    }
}

/// Total order over a position: componentwise [`f32::total_cmp`], so the sort in
/// [`merge_coincident_positions`] is a deterministic total order (bit-identical positions compare
/// equal; every other pair, `NaN` included, compares consistently).
fn cmp_position(a: &[f32; 3], b: &[f32; 3]) -> std::cmp::Ordering {
    a[0].total_cmp(&b[0])
        .then_with(|| a[1].total_cmp(&b[1]))
        .then_with(|| a[2].total_cmp(&b[2]))
}

/// Groups `referenced` (already sorted ascending) by union-find root, each group's vertices kept in
/// ascending order — a sort-based grouping, not a hashing container.
fn group_by_root(referenced: &[u32], union_find: &mut UnionFind) -> Vec<Vec<u32>> {
    let mut by_root: Vec<(u32, u32)> = referenced
        .iter()
        .map(|&v| (union_find.find(v), v))
        .collect();
    by_root.sort_unstable();

    let mut components: Vec<Vec<u32>> = Vec::new();
    let mut current_root = None;
    for (root, vertex) in by_root {
        if current_root != Some(root) {
            components.push(Vec::new());
            current_root = Some(root);
        }
        components.last_mut().expect("just pushed").push(vertex);
    }
    components
}

/// The `L1159` warning for `components` (size-descending, main body first): the component count and
/// the main body's vertex count, then one line per stray piece — smallest first — naming its vertex
/// count and the nearest vertex pair between it and the main body, with the gap in world units.
fn warning_of(positions: &[[f32; 3]], components: &[Vec<u32>]) -> Diagnostic {
    let count = components.len();
    let main = &components[0];
    let mut message = format!(
        "this mesh's surface splits into {count} disconnected components (main body {} vertex{}); \
         each stray piece, smallest first:",
        main.len(),
        if main.len() == 1 { "" } else { "s" }
    );
    // `components[1..]` is size-descending; reversed, the smallest stray piece comes first.
    for stray in components[1..].iter().rev() {
        let gap = nearest_pair(positions, stray, main);
        let [sx, sy, sz] = positions[gap.stray as usize];
        let [mx, my, mz] = positions[gap.main as usize];
        message.push_str(&format!(
            "\n  {} vertex{}, nearest pair: stray ({sx:.2}, {sy:.2}, {sz:.2}) vs main ({mx:.2}, \
             {my:.2}, {mz:.2}), gap {:.2} world units",
            stray.len(),
            if stray.len() == 1 { "" } else { "s" },
            gap.distance
        ));
    }
    Diagnostic::new(code::SCULPT_DISCONNECTED, message).with_help(
        "if the piece was meant to attach, move the part toward the named point, grow it, or raise \
         the blend of the part joining it; if the detachment is intentional, no action is needed",
    )
}

/// The nearest pair of vertices between `stray` and `main` — exhaustive over both (fine at authored
/// sculpt scale: a stray piece is a handful of vertices, the main body at most a few thousand).
/// Deterministic: iterating both lists ascending and keeping the first strictly-smaller distance
/// found breaks a tie in favor of the lower `(stray, main)` index pair.
struct Gap {
    stray: u32,
    main: u32,
    distance: f32,
}

fn nearest_pair(positions: &[[f32; 3]], stray: &[u32], main: &[u32]) -> Gap {
    let mut best: Option<(u32, u32, f32)> = None;
    for &s in stray {
        let sp = positions[s as usize];
        for &m in main {
            let mp = positions[m as usize];
            let d = [sp[0] - mp[0], sp[1] - mp[1], sp[2] - mp[2]];
            let squared = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if best.is_none_or(|(_, _, best_squared)| squared < best_squared) {
                best = Some((s, m, squared));
            }
        }
    }
    let (stray, main, squared) = best.expect("stray and main are both non-empty components");
    Gap {
        stray,
        main,
        distance: math::sqrt(squared),
    }
}

/// A deterministic union-find (disjoint-set) over vertex index `0..n`, path-compressed and unioned
/// by size — an array, not a hashing container, so grouping stays reproducible byte-for-byte.
struct UnionFind {
    parent: Vec<u32>,
    size: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind {
            parent: (0..n as u32).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, x: u32) -> u32 {
        let mut root = x;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        let mut cursor = x;
        while self.parent[cursor as usize] != root {
            let next = self.parent[cursor as usize];
            self.parent[cursor as usize] = root;
            cursor = next;
        }
        root
    }

    fn union(&mut self, a: u32, b: u32) {
        let (mut ra, mut rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        if self.size[ra as usize] < self.size[rb as usize] {
            std::mem::swap(&mut ra, &mut rb);
        }
        self.parent[rb as usize] = ra;
        self.size[ra as usize] += self.size[rb as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned unit cube's 8 corners and 12 triangles (2 per face, outward winding not
    /// significant here), offset by `at`.
    fn cube(at: [f32; 3]) -> (Vec<[f32; 3]>, Vec<u32>) {
        let [ox, oy, oz] = at;
        let positions = vec![
            [ox, oy, oz],
            [ox + 1.0, oy, oz],
            [ox + 1.0, oy + 1.0, oz],
            [ox, oy + 1.0, oz],
            [ox, oy, oz + 1.0],
            [ox + 1.0, oy, oz + 1.0],
            [ox + 1.0, oy + 1.0, oz + 1.0],
            [ox, oy + 1.0, oz + 1.0],
        ];
        #[rustfmt::skip]
        let indices = vec![
            0, 1, 2, 0, 2, 3, // -Z
            4, 6, 5, 4, 7, 6, // +Z
            0, 4, 5, 0, 5, 1, // -Y
            3, 2, 6, 3, 6, 7, // +Y
            0, 3, 7, 0, 7, 4, // -X
            1, 5, 6, 1, 6, 2, // +X
        ];
        (positions, indices)
    }

    /// A cube with every face's two triangles given their own, position-duplicated vertices (the
    /// flat-shaded crease split a real sculpt mesher emits): 36 distinct vertex slots, 8 distinct
    /// points, still one closed surface.
    fn crease_split_cube(at: [f32; 3]) -> (Vec<[f32; 3]>, Vec<u32>) {
        let (corners, corner_indices) = cube(at);
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for triangle in corner_indices.chunks_exact(3) {
            let base = positions.len() as u32;
            for &corner in triangle {
                positions.push(corners[corner as usize]);
            }
            indices.extend([base, base + 1, base + 2]);
        }
        (positions, indices)
    }

    #[test]
    fn empty_mesh_has_no_components() {
        let (count, warning) = connectivity(&[], &[]);
        assert_eq!(count, 0);
        assert!(warning.is_none());
    }

    #[test]
    fn a_closed_cube_is_one_component() {
        let (positions, indices) = cube([0.0; 3]);
        let (count, warning) = connectivity(&positions, &indices);
        assert_eq!(count, 1);
        assert!(warning.is_none());
    }

    #[test]
    fn a_crease_split_cube_is_still_one_component() {
        let (positions, indices) = crease_split_cube([0.0; 3]);
        let (count, warning) = connectivity(&positions, &indices);
        assert_eq!(
            count, 1,
            "bit-identical positions at a crease must not read as disconnected"
        );
        assert!(warning.is_none());
    }

    #[test]
    fn two_cubes_half_a_unit_apart_are_two_components_with_the_gap_in_the_message() {
        let (mut positions, mut indices) = cube([0.0; 3]);
        let (far_positions, far_indices) = cube([1.5, 0.0, 0.0]);
        let offset = positions.len() as u32;
        positions.extend(far_positions);
        indices.extend(far_indices.iter().map(|i| i + offset));

        let (count, warning) = connectivity(&positions, &indices);
        assert_eq!(count, 2);
        let warning = warning.expect("two disconnected cubes must warn");
        assert_eq!(warning.code.as_str(), "L1159");
        assert!(
            warning.message.contains("2 disconnected components"),
            "message: {}",
            warning.message
        );
        assert!(
            warning.message.contains("gap 0.50 world units"),
            "message: {}",
            warning.message
        );
    }

    #[test]
    fn a_vertex_no_triangle_references_is_ignored() {
        let (mut positions, indices) = cube([0.0; 3]);
        positions.push([50.0, 50.0, 50.0]);
        let (count, warning) = connectivity(&positions, &indices);
        assert_eq!(
            count, 1,
            "the stray unreferenced vertex must not count as a component"
        );
        assert!(warning.is_none());
    }

    #[test]
    fn connectivity_is_deterministic() {
        let (mut positions, mut indices) = cube([0.0; 3]);
        let (far_positions, far_indices) = cube([2.0, 0.0, 0.0]);
        let offset = positions.len() as u32;
        positions.extend(far_positions);
        indices.extend(far_indices.iter().map(|i| i + offset));

        let first = connectivity(&positions, &indices);
        let second = connectivity(&positions, &indices);
        assert_eq!(first.0, second.0);
        assert_eq!(first.1.map(|d| d.message), second.1.map(|d| d.message));
    }
}
