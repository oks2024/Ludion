//! Quadric-error edge-collapse decimation of a closed, consistently wound indexed triangle mesh.
//!
//! Garland–Heckbert: every vertex accumulates the **area-weighted** plane quadrics of the triangles
//! around it (a face's weight is its area, so the metric does not depend on how finely a flat region
//! was tessellated). An edge's cost is the summed quadric of its endpoints evaluated at the collapse
//! position — the quadric's minimizer, or the edge midpoint when the quadric is singular (relative
//! determinant below `SINGULAR_RELATIVE_DETERMINANT`) or its minimizer lies more than one edge
//! length from the midpoint. Edges collapse cheapest first until the triangle count reaches the
//! target or no legal collapse remains.
//!
//! A collapse is legal when it keeps the surface a closed 2-manifold and does not fold it: the edge
//! has exactly two triangles, the endpoints share no neighbour but those two triangles' opposite
//! vertices (the link condition), neither opposite vertex drops below valence 3, and every surviving
//! triangle around the edge keeps a non-degenerate area and turns its normal by less than
//! `acos(MIN_NORMAL_COSINE)`. Vertices on a boundary, non-manifold or inconsistently wound edge,
//! a non-disc vertex fan, or a triangle repeating an index are locked and never move.
//!
//! Quadrics, placements and the fold tests run in `f64` (plain IEEE arithmetic, square roots through
//! [`ludion_core::math::f64`]); positions and attributes stay `f32`. The heap orders on
//! `(error.to_bits(), edge index, version)` — errors are non-negative, so their bit patterns sort
//! like the values, and edge indices are unique, so no two live entries tie. Output is a pure
//! function of the input bytes.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use ludion_core::math;

/// An indexed triangle mesh with a flat per-vertex attribute blob.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// `attribute_len` values per vertex, vertex `i` at `i * attribute_len..(i + 1) * attribute_len`
    /// (colors, materials, part weights — whatever the caller packs); blended at every collapse.
    pub attributes: Vec<f32>,
    /// Values per vertex in `attributes`; `0` carries none.
    pub attribute_len: usize,
    /// Triangles, three vertex indices each, wound consistently (counter-clockwise seen from outside).
    pub indices: Vec<u32>,
}

impl TriMesh {
    /// Number of triangles: `indices.len() / 3`.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// The smallest cosine between a triangle's normal before and after a collapse; a collapse turning
/// any surviving triangle further (about 78°) is refused as a fold.
const MIN_NORMAL_COSINE: f64 = 0.2;

/// A quadric whose 3×3 determinant is at most this fraction of its diagonal product (the Hadamard
/// bound for a positive semi-definite matrix) is singular; the collapse goes to the edge midpoint.
const SINGULAR_RELATIVE_DETERMINANT: f64 = 1e-6;

/// A triangle whose doubled area is at most this fraction of its longest squared edge is degenerate;
/// a collapse producing one is refused.
const DEGENERATE_RATIO: f64 = 1e-6;

/// Collapses edges of `mesh` cheapest first until it has at most `target_triangles` triangles, or
/// until no legal collapse remains (the result then keeps more; not an error).
///
/// A collapse keeps the lower-indexed endpoint. Its position is the quadric placement; its attributes
/// are the endpoints' blended by `t`, the placement's projection parameter along the edge clamped to
/// `[0, 1]` (`0` at the kept endpoint), each value clamped to the range of its two sources — so
/// every output attribute lies within the range of the inputs it came from.
///
/// The output keeps exactly the vertices referenced by a surviving triangle, in their original
/// relative order, and the surviving triangles in their original order. A mesh already at or under
/// the target is returned as an unchanged clone.
///
/// # Panics
///
/// If `indices.len()` is not a multiple of 3, an index is out of range, `attributes.len()` is not
/// `positions.len() * attribute_len`, a position is not finite, or there are more than `u32::MAX`
/// vertices.
pub fn decimate(mesh: &TriMesh, target_triangles: u32) -> TriMesh {
    validate(mesh);
    if mesh.triangle_count() <= target_triangles as usize {
        return mesh.clone();
    }
    let mut collapser = Collapser::new(mesh);
    collapser.run(target_triangles as usize);
    collapser.compact()
}

fn validate(mesh: &TriMesh) {
    assert!(
        mesh.indices.len().is_multiple_of(3),
        "decimate: {} indices is not a whole number of triangles",
        mesh.indices.len()
    );
    assert!(
        u32::try_from(mesh.positions.len()).is_ok(),
        "decimate: {} vertices exceed u32 indexing",
        mesh.positions.len()
    );
    assert_eq!(
        mesh.attributes.len(),
        mesh.positions.len() * mesh.attribute_len,
        "decimate: attributes must hold attribute_len values per vertex"
    );
    if let Some(&index) = mesh
        .indices
        .iter()
        .find(|&&i| i as usize >= mesh.positions.len())
    {
        panic!(
            "decimate: index {index} out of range for {} vertices",
            mesh.positions.len()
        );
    }
    assert!(
        mesh.positions.iter().flatten().all(|c| c.is_finite()),
        "decimate: positions must be finite"
    );
}

/// A symmetric 4×4 plane quadric, upper triangle row by row:
/// `[a², ab, ac, ad, b², bc, bd, c², cd, d²]`.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    /// `weight · p pᵀ` for the plane `p = (n, d)`, `n` unit length.
    fn plane(n: [f64; 3], d: f64, weight: f64) -> Self {
        let [a, b, c] = n;
        Quadric(
            [
                a * a,
                a * b,
                a * c,
                a * d,
                b * b,
                b * c,
                b * d,
                c * c,
                c * d,
                d * d,
            ]
            .map(|v| v * weight),
        )
    }

    fn add(&self, other: &Quadric) -> Quadric {
        let mut sum = self.0;
        for (s, o) in sum.iter_mut().zip(other.0) {
            *s += o;
        }
        Quadric(sum)
    }

    fn error(&self, [x, y, z]: [f64; 3]) -> f64 {
        let [a2, ab, ac, ad, b2, bc, bd, c2, cd, d2] = self.0;
        a2 * x * x
            + 2.0 * ab * x * y
            + 2.0 * ac * x * z
            + 2.0 * ad * x
            + b2 * y * y
            + 2.0 * bc * y * z
            + 2.0 * bd * y
            + c2 * z * z
            + 2.0 * cd * z
            + d2
    }

    /// The point minimizing the error, `None` when the 3×3 part is singular.
    fn minimizer(&self) -> Option<[f64; 3]> {
        let [a2, ab, ac, ad, b2, bc, bd, c2, cd, _] = self.0;
        let c00 = b2 * c2 - bc * bc;
        let c01 = bc * ac - ab * c2;
        let c02 = ab * bc - b2 * ac;
        let det = a2 * c00 + ab * c01 + ac * c02;
        let diagonal = a2 * b2 * c2;
        if diagonal <= 0.0 || det.abs() <= SINGULAR_RELATIVE_DETERMINANT * diagonal {
            return None;
        }
        let c11 = a2 * c2 - ac * ac;
        let c12 = ab * ac - a2 * bc;
        let c22 = a2 * b2 - ab * ab;
        Some([
            -(c00 * ad + c01 * bd + c02 * cd) / det,
            -(c01 * ad + c11 * bd + c12 * cd) / det,
            -(c02 * ad + c12 * bd + c22 * cd) / det,
        ])
    }
}

/// Heap entry: `(error bits, edge index, edge version)`, popped smallest first.
type Entry = Reverse<(u64, u32, u32)>;

struct Collapser {
    positions: Vec<[f32; 3]>,
    attributes: Vec<f32>,
    attribute_len: usize,
    quadrics: Vec<Quadric>,
    triangles: Vec<[u32; 3]>,
    triangle_alive: Vec<bool>,
    live_triangles: usize,
    /// Live triangles around each vertex, ascending.
    vertex_triangles: Vec<Vec<u32>>,
    /// Live edges at each vertex, ascending.
    vertex_edges: Vec<Vec<u32>>,
    locked: Vec<bool>,
    /// Endpoints of each edge, lower index first.
    edges: Vec<[u32; 2]>,
    edge_alive: Vec<bool>,
    /// Bumped whenever an edge's heap entry is superseded; an entry with an older version is stale.
    edge_version: Vec<u32>,
    edge_cost: Vec<u64>,
    /// Popped and refused; re-queued when its neighbourhood changes.
    edge_refused: Vec<bool>,
    heap: BinaryHeap<Entry>,
    scratch_a: Vec<u32>,
    scratch_b: Vec<u32>,
}

impl Collapser {
    fn new(mesh: &TriMesh) -> Self {
        let vertex_count = mesh.positions.len();
        let triangles: Vec<[u32; 3]> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        let mut locked = vec![false; vertex_count];
        let mut vertex_triangles = vec![Vec::new(); vertex_count];
        let mut quadrics = vec![Quadric::default(); vertex_count];

        for (index, &[i0, i1, i2]) in triangles.iter().enumerate() {
            if i0 == i1 || i1 == i2 || i0 == i2 {
                for v in [i0, i1, i2] {
                    locked[v as usize] = true;
                }
            }
            for (k, v) in [i0, i1, i2].into_iter().enumerate() {
                if !(k == 1 && v == i0 || k == 2 && (v == i0 || v == i1)) {
                    vertex_triangles[v as usize].push(index as u32);
                }
            }
            let [p0, p1, p2] = [i0, i1, i2].map(|v| to_f64(mesh.positions[v as usize]));
            let normal = cross(sub(p1, p0), sub(p2, p0));
            let doubled_area = math::f64::sqrt(dot(normal, normal));
            if doubled_area > 0.0 {
                let n = scale(normal, 1.0 / doubled_area);
                let plane = Quadric::plane(n, -dot(n, p0), 0.5 * doubled_area);
                for v in [i0, i1, i2] {
                    quadrics[v as usize] = quadrics[v as usize].add(&plane);
                }
            }
        }

        // Every directed edge as (lower, upper, runs lower→upper); a manifold, consistently wound
        // edge appears exactly twice, once each way.
        let mut directed: Vec<(u32, u32, bool)> = Vec::with_capacity(triangles.len() * 3);
        for &[i0, i1, i2] in &triangles {
            for (from, to) in [(i0, i1), (i1, i2), (i2, i0)] {
                if from != to {
                    directed.push((from.min(to), from.max(to), from < to));
                }
            }
        }
        directed.sort_unstable();
        let mut edges = Vec::new();
        let mut vertex_edges = vec![Vec::new(); vertex_count];
        let mut run_start = 0;
        while run_start < directed.len() {
            let (lo, hi, forward) = directed[run_start];
            let mut run_end = run_start + 1;
            while run_end < directed.len() && (directed[run_end].0, directed[run_end].1) == (lo, hi)
            {
                run_end += 1;
            }
            let manifold = run_end - run_start == 2 && !forward && directed[run_start + 1].2;
            if !manifold {
                locked[lo as usize] = true;
                locked[hi as usize] = true;
            }
            let edge = edges.len() as u32;
            edges.push([lo, hi]);
            vertex_edges[lo as usize].push(edge);
            vertex_edges[hi as usize].push(edge);
            run_start = run_end;
        }

        let mut fan = Vec::new();
        for (v, around) in vertex_triangles.iter().enumerate() {
            if !locked[v] && !is_disc_fan(v as u32, around, &triangles, &mut fan) {
                locked[v] = true;
            }
        }

        let edge_count = edges.len();
        let mut collapser = Collapser {
            positions: mesh.positions.clone(),
            attributes: mesh.attributes.clone(),
            attribute_len: mesh.attribute_len,
            quadrics,
            triangle_alive: vec![true; triangles.len()],
            live_triangles: triangles.len(),
            triangles,
            vertex_triangles,
            vertex_edges,
            locked,
            edges,
            edge_alive: vec![true; edge_count],
            edge_version: vec![0; edge_count],
            edge_cost: vec![0; edge_count],
            edge_refused: vec![false; edge_count],
            heap: BinaryHeap::with_capacity(edge_count),
            scratch_a: Vec::new(),
            scratch_b: Vec::new(),
        };
        for edge in 0..edge_count as u32 {
            collapser.requeue(edge);
        }
        collapser
    }

    fn run(&mut self, target: usize) {
        while self.live_triangles > target {
            let Some(Reverse((_, edge, version))) = self.heap.pop() else {
                break;
            };
            let e = edge as usize;
            if !self.edge_alive[e] || self.edge_version[e] != version {
                continue;
            }
            if !self.try_collapse(edge) {
                self.edge_refused[e] = true;
            }
        }
    }

    /// Recomputes `edge`'s cost and queues it under a new version, unless an endpoint is locked.
    fn requeue(&mut self, edge: u32) {
        let e = edge as usize;
        let [a, b] = self.edges[e];
        self.edge_refused[e] = false;
        self.edge_version[e] = self.edge_version[e].wrapping_add(1);
        if self.locked[a as usize] || self.locked[b as usize] {
            return;
        }
        let (_, error) = self.placement(a, b);
        self.edge_cost[e] = error.to_bits();
        self.heap
            .push(Reverse((self.edge_cost[e], edge, self.edge_version[e])));
    }

    /// Where the edge `a`–`b` collapses to and the quadric error there (non-negative).
    fn placement(&self, a: u32, b: u32) -> ([f32; 3], f64) {
        let quadric = self.quadrics[a as usize].add(&self.quadrics[b as usize]);
        let pa = to_f64(self.positions[a as usize]);
        let pb = to_f64(self.positions[b as usize]);
        let midpoint = scale(add(pa, pb), 0.5);
        let reach = sub(pb, pa);
        let position = match quadric.minimizer() {
            Some(x) if dot(sub(x, midpoint), sub(x, midpoint)) <= dot(reach, reach) => x,
            _ => midpoint,
        };
        let position = position.map(|c| c as f32);
        let error = quadric.error(to_f64(position));
        let error = if error.is_nan() {
            f64::INFINITY
        } else if error > 0.0 {
            error
        } else {
            0.0
        };
        (position, error)
    }

    fn try_collapse(&mut self, edge: u32) -> bool {
        let [a, b] = self.edges[edge as usize];
        let mut shared = [0u32; 2];
        let mut shared_count = 0;
        for &t in &self.vertex_triangles[a as usize] {
            if self.vertex_triangles[b as usize].binary_search(&t).is_ok() {
                if shared_count == 2 {
                    return false;
                }
                shared[shared_count] = t;
                shared_count += 1;
            }
        }
        if shared_count != 2 {
            return false;
        }
        let opposite = shared.map(|t| {
            let tri = self.triangles[t as usize];
            tri[(0..3).find(|&k| tri[k] != a && tri[k] != b).unwrap_or(0)]
        });
        if opposite[0] == opposite[1]
            || opposite
                .iter()
                .any(|&o| self.vertex_triangles[o as usize].len() <= 3)
        {
            return false;
        }

        let mut ring_a = std::mem::take(&mut self.scratch_a);
        let mut ring_b = std::mem::take(&mut self.scratch_b);
        self.ring(a, &mut ring_a);
        self.ring(b, &mut ring_b);
        let expected = [opposite[0].min(opposite[1]), opposite[0].max(opposite[1])];
        let mut common = 0;
        let mut link_holds = true;
        for &w in &ring_a {
            if w != b && ring_b.binary_search(&w).is_ok() {
                if common == 2 || expected[common] != w {
                    link_holds = false;
                    break;
                }
                common += 1;
            }
        }
        self.scratch_a = ring_a;
        self.scratch_b = ring_b;
        if !link_holds || common != 2 {
            return false;
        }

        let (position, _) = self.placement(a, b);
        if !self.keeps_triangles_sound(a, b, shared, position) {
            return false;
        }
        self.apply(edge, a, b, shared, opposite, position);
        true
    }

    /// The distinct neighbours of `v`, ascending, into `out`.
    fn ring(&self, v: u32, out: &mut Vec<u32>) {
        out.clear();
        for &t in &self.vertex_triangles[v as usize] {
            out.extend(self.triangles[t as usize].iter().filter(|&&w| w != v));
        }
        out.sort_unstable();
        out.dedup();
    }

    /// Whether every triangle around `a` or `b` but the two `shared` ones stays non-degenerate and
    /// unfolded once its `a`/`b` corner moves to `position`.
    fn keeps_triangles_sound(&self, a: u32, b: u32, shared: [u32; 2], position: [f32; 3]) -> bool {
        let moved = to_f64(position);
        let around = self.vertex_triangles[a as usize]
            .iter()
            .chain(&self.vertex_triangles[b as usize]);
        for &t in around {
            if shared.contains(&t) {
                continue;
            }
            let tri = self.triangles[t as usize];
            let before = tri.map(|v| to_f64(self.positions[v as usize]));
            let after = tri.map(|v| {
                if v == a || v == b {
                    moved
                } else {
                    to_f64(self.positions[v as usize])
                }
            });
            let normal_before = cross(sub(before[1], before[0]), sub(before[2], before[0]));
            let normal_after = cross(sub(after[1], after[0]), sub(after[2], after[0]));
            let area_after = dot(normal_after, normal_after);
            let longest = [(0, 1), (1, 2), (2, 0)]
                .map(|(i, j)| {
                    let e = sub(after[j], after[i]);
                    dot(e, e)
                })
                .into_iter()
                .fold(0.0, f64::max);
            if area_after <= DEGENERATE_RATIO * DEGENERATE_RATIO * longest * longest {
                return false;
            }
            let area_before = dot(normal_before, normal_before);
            if dot(normal_after, normal_before)
                <= MIN_NORMAL_COSINE * math::f64::sqrt(area_after * area_before)
            {
                return false;
            }
        }
        true
    }

    /// Collapses `b` into `a` at `position`, removing the `shared` triangles.
    fn apply(
        &mut self,
        edge: u32,
        a: u32,
        b: u32,
        shared: [u32; 2],
        opposite: [u32; 2],
        position: [f32; 3],
    ) {
        let (ai, bi) = (a as usize, b as usize);
        let pa = self.positions[ai];
        let pb = self.positions[bi];
        let reach = sub32(pb, pa);
        let reach_squared = dot32(reach, reach);
        let t = if reach_squared > 0.0 {
            (dot32(sub32(position, pa), reach) / reach_squared).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let len = self.attribute_len;
        for k in 0..len {
            let x = self.attributes[ai * len + k];
            let y = self.attributes[bi * len + k];
            self.attributes[ai * len + k] = (x + t * (y - x)).clamp(x.min(y), x.max(y));
        }
        self.positions[ai] = position;
        self.quadrics[ai] = self.quadrics[ai].add(&self.quadrics[bi]);

        for (&triangle, &o) in shared.iter().zip(&opposite) {
            self.triangle_alive[triangle as usize] = false;
            self.live_triangles -= 1;
            remove_sorted(&mut self.vertex_triangles[o as usize], triangle);
        }
        let from_b = std::mem::take(&mut self.vertex_triangles[bi]);
        let mut merged = std::mem::take(&mut self.vertex_triangles[ai]);
        merged.retain(|t| !shared.contains(t));
        for &triangle in from_b.iter().filter(|t| !shared.contains(t)) {
            for corner in &mut self.triangles[triangle as usize] {
                if *corner == b {
                    *corner = a;
                }
            }
            merged.push(triangle);
        }
        merged.sort_unstable();
        self.vertex_triangles[ai] = merged;

        self.edge_alive[edge as usize] = false;
        let edges_b = std::mem::take(&mut self.vertex_edges[bi]);
        let mut edges_a = std::mem::take(&mut self.vertex_edges[ai]);
        remove_sorted(&mut edges_a, edge);
        for e in edges_b {
            if e == edge {
                continue;
            }
            let [lo, hi] = self.edges[e as usize];
            let w = if lo == b { hi } else { lo };
            if opposite.contains(&w) {
                self.edge_alive[e as usize] = false;
                remove_sorted(&mut self.vertex_edges[w as usize], e);
            } else {
                self.edges[e as usize] = [a.min(w), a.max(w)];
                edges_a.push(e);
            }
        }
        edges_a.sort_unstable();

        for &e in &edges_a {
            self.requeue(e);
        }
        self.vertex_edges[ai] = edges_a;

        let mut ring = std::mem::take(&mut self.scratch_a);
        self.ring(a, &mut ring);
        for &w in &ring {
            for k in 0..self.vertex_edges[w as usize].len() {
                let e = self.vertex_edges[w as usize][k];
                if self.edge_refused[e as usize] {
                    self.requeue_refused(e);
                }
            }
        }
        self.scratch_a = ring;
    }

    /// Queues a refused edge again at its stored cost: its endpoints' quadrics are unchanged, only
    /// the triangles around it moved.
    fn requeue_refused(&mut self, edge: u32) {
        let e = edge as usize;
        self.edge_refused[e] = false;
        self.edge_version[e] = self.edge_version[e].wrapping_add(1);
        self.heap
            .push(Reverse((self.edge_cost[e], edge, self.edge_version[e])));
    }

    fn compact(self) -> TriMesh {
        let len = self.attribute_len;
        let mut remap = vec![u32::MAX; self.positions.len()];
        let mut positions = Vec::new();
        let mut attributes = Vec::new();
        for (v, around) in self.vertex_triangles.iter().enumerate() {
            if !around.is_empty() {
                remap[v] = positions.len() as u32;
                positions.push(self.positions[v]);
                attributes.extend_from_slice(&self.attributes[v * len..(v + 1) * len]);
            }
        }
        let mut indices = Vec::with_capacity(self.live_triangles * 3);
        for (tri, _) in self
            .triangles
            .iter()
            .zip(&self.triangle_alive)
            .filter(|(_, alive)| **alive)
        {
            indices.extend(tri.map(|v| remap[v as usize]));
        }
        TriMesh {
            positions,
            attributes,
            attribute_len: len,
            indices,
        }
    }
}

/// Whether the triangles `around` vertex `v` form one closed disc: each neighbour starts exactly one
/// corner wedge and ends exactly one, and following the wedges from any neighbour visits them all.
fn is_disc_fan(
    v: u32,
    around: &[u32],
    triangles: &[[u32; 3]],
    wedges: &mut Vec<(u32, u32)>,
) -> bool {
    wedges.clear();
    for &t in around {
        let tri = triangles[t as usize];
        let Some(k) = (0..3).find(|&k| tri[k] == v) else {
            return false;
        };
        wedges.push((tri[(k + 1) % 3], tri[(k + 2) % 3]));
    }
    wedges.sort_unstable();
    if wedges.windows(2).any(|w| w[0].0 == w[1].0) {
        return false;
    }
    let Some(&(start, mut next)) = wedges.first() else {
        return true;
    };
    for _ in 1..wedges.len() {
        if next == start {
            return false;
        }
        match wedges.binary_search_by_key(&next, |w| w.0) {
            Ok(i) => next = wedges[i].1,
            Err(_) => return false,
        }
    }
    next == start
}

fn remove_sorted(list: &mut Vec<u32>, value: u32) {
    if let Ok(i) = list.binary_search(&value) {
        list.remove(i);
    }
}

fn to_f64(p: [f32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub32(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot32(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A closed box of full extents `size` centered on the origin, each face a grid of `cells`
    /// squares split into two triangles, wound outward. Attributes: `[x / size.x + 0.5, 0.25]`.
    fn box_mesh(size: [f32; 3], cells: [u32; 3]) -> TriMesh {
        let mut index_of: BTreeMap<[u32; 3], u32> = BTreeMap::new();
        let mut mesh = TriMesh {
            attribute_len: 2,
            ..TriMesh::default()
        };
        let mut vertex = |lattice: [u32; 3], mesh: &mut TriMesh| -> u32 {
            *index_of.entry(lattice).or_insert_with(|| {
                let p = [0, 1, 2]
                    .map(|axis| (lattice[axis] as f32 / cells[axis] as f32 - 0.5) * size[axis]);
                mesh.positions.push(p);
                mesh.attributes.extend([p[0] / size[0] + 0.5, 0.25]);
                (mesh.positions.len() - 1) as u32
            })
        };
        for axis in 0..3 {
            let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
            for side in [0, cells[axis]] {
                for j in 0..cells[w] {
                    for i in 0..cells[u] {
                        let mut corners = [0u32; 4];
                        for (c, (di, dj)) in
                            [(0, 0), (1, 0), (1, 1), (0, 1)].into_iter().enumerate()
                        {
                            let mut lattice = [0u32; 3];
                            lattice[axis] = side;
                            lattice[u] = i + di;
                            lattice[w] = j + dj;
                            corners[c] = vertex(lattice, &mut mesh);
                        }
                        let [p00, p10, p11, p01] = corners;
                        if side == 0 {
                            mesh.indices.extend([p00, p11, p10, p00, p01, p11]);
                        } else {
                            mesh.indices.extend([p00, p10, p11, p00, p11, p01]);
                        }
                    }
                }
            }
        }
        mesh
    }

    /// A unit sphere: the `n`-cell cube with every vertex pushed onto the sphere.
    fn sphere_mesh(n: u32) -> TriMesh {
        let mut mesh = box_mesh([2.0; 3], [n; 3]);
        for p in &mut mesh.positions {
            let length = math::sqrt(dot32(*p, *p));
            *p = p.map(|c| c / length);
        }
        mesh
    }

    fn position_f64(mesh: &TriMesh, v: u32) -> [f64; 3] {
        to_f64(mesh.positions[v as usize])
    }

    /// Closed and consistently wound: each directed edge appears once and its reverse once. Also no
    /// degenerate triangle, no unreferenced vertex, Euler characteristic 2 (one sphere-like shell).
    fn assert_closed_manifold(mesh: &TriMesh) {
        let mut directed = Vec::new();
        let mut referenced = vec![false; mesh.positions.len()];
        for tri in mesh.indices.chunks_exact(3) {
            let [p0, p1, p2] = [tri[0], tri[1], tri[2]].map(|v| position_f64(mesh, v));
            let normal = cross(sub(p1, p0), sub(p2, p0));
            assert!(dot(normal, normal) > 0.0, "degenerate triangle {tri:?}");
            for (from, to) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                assert_ne!(from, to, "triangle {tri:?} repeats a vertex");
                directed.push((from, to));
                referenced[from as usize] = true;
            }
        }
        assert!(referenced.iter().all(|&r| r), "unreferenced vertex");
        directed.sort_unstable();
        assert!(
            directed.windows(2).all(|w| w[0] != w[1]),
            "a directed edge appears twice: inconsistent winding or a non-manifold edge"
        );
        for &(from, to) in &directed {
            assert!(
                directed.binary_search(&(to, from)).is_ok(),
                "edge {from}->{to} has no twin: the mesh is open"
            );
        }
        let edges = directed.len() / 2;
        let euler = mesh.positions.len() as i64 - edges as i64 + mesh.triangle_count() as i64;
        assert_eq!(euler, 2, "Euler characteristic");
    }

    fn assert_attributes_in_input_range(input: &TriMesh, output: &TriMesh) {
        let len = input.attribute_len;
        assert_eq!(output.attributes.len(), output.positions.len() * len);
        for k in 0..len {
            let column = input.attributes.iter().skip(k).step_by(len);
            let (lo, hi) = column.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &x| {
                (lo.min(x), hi.max(x))
            });
            for &x in output.attributes.iter().skip(k).step_by(len) {
                assert!(lo <= x && x <= hi, "attribute {k} = {x} escapes {lo}..{hi}");
            }
        }
        for &x in output.attributes.iter().skip(1).step_by(len) {
            assert_eq!(x, 0.25, "a constant attribute must blend to itself");
        }
    }

    #[test]
    fn sphere_reaches_the_target_and_stays_closed_and_round() {
        let input = sphere_mesh(30);
        assert_eq!(input.triangle_count(), 10_800);
        assert_closed_manifold(&input);
        let output = decimate(&input, 1_000);
        assert_eq!(output.triangle_count(), 1_000);
        assert_closed_manifold(&output);
        assert_attributes_in_input_range(&input, &output);
        for p in &output.positions {
            let radius = math::sqrt(dot32(*p, *p));
            assert!(
                (radius - 1.0).abs() < 0.02,
                "vertex {p:?} drifted to radius {radius}"
            );
        }
    }

    #[test]
    fn elongated_box_reaches_the_target_and_keeps_its_faces() {
        let size = [4.0, 1.0, 1.0];
        let input = box_mesh(size, [80, 20, 20]);
        assert_eq!(input.triangle_count(), 14_400);
        let output = decimate(&input, 500);
        assert!(output.triangle_count() <= 500);
        assert_closed_manifold(&output);
        assert_attributes_in_input_range(&input, &output);
        for p in &output.positions {
            let off_surface = (0..3)
                .map(|axis| p[axis].abs() / (0.5 * size[axis]))
                .fold(0.0, f32::max);
            assert!(
                (off_surface - 1.0).abs() < 1e-3,
                "vertex {p:?} left the box surface"
            );
        }
    }

    #[test]
    fn flat_faces_collapse_to_the_eight_corner_cube() {
        let input = box_mesh([1.0; 3], [16; 3]);
        assert_eq!(input.triangle_count(), 3_072);
        let output = decimate(&input, 12);
        assert_eq!(output.triangle_count(), 12);
        assert_eq!(output.positions.len(), 8);
        assert_closed_manifold(&output);
        for p in &output.positions {
            for c in p {
                assert!((c.abs() - 0.5).abs() < 1e-4, "vertex {p:?} is not a corner");
            }
        }
    }

    #[test]
    fn decimation_is_bit_identical_across_runs() {
        let input = sphere_mesh(20);
        let first = decimate(&input, 700);
        let second = decimate(&input, 700);
        assert_eq!(first.indices, second.indices);
        let bits = |mesh: &TriMesh| {
            (
                mesh.positions
                    .iter()
                    .flatten()
                    .map(|c| c.to_bits())
                    .collect::<Vec<_>>(),
                mesh.attributes
                    .iter()
                    .map(|c| c.to_bits())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(bits(&first), bits(&second));
    }

    #[test]
    fn a_mesh_at_or_under_the_target_is_returned_unchanged() {
        let input = sphere_mesh(4);
        assert_eq!(decimate(&input, input.triangle_count() as u32), input);
    }

    #[test]
    fn a_tetrahedron_has_no_legal_collapse() {
        let input = TriMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            attributes: Vec::new(),
            attribute_len: 0,
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
        };
        assert_closed_manifold(&input);
        assert_eq!(decimate(&input, 0), input);
    }

    #[test]
    fn exhausting_legal_collapses_stops_at_a_closed_minimum() {
        let input = sphere_mesh(6);
        let output = decimate(&input, 0);
        assert!(output.triangle_count() >= 4);
        assert_closed_manifold(&output);
    }

    #[test]
    fn boundary_vertices_are_locked() {
        let mut input = box_mesh([1.0; 3], [4; 3]);
        input.indices.truncate(input.indices.len() - 6);
        let output = decimate(&input, 0);
        let boundary_before: Vec<[u32; 3]> = {
            let mut directed = Vec::new();
            for tri in input.indices.chunks_exact(3) {
                directed.extend([(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])]);
            }
            directed.sort_unstable();
            directed
                .iter()
                .filter(|&&(a, b)| directed.binary_search(&(b, a)).is_err())
                .map(|&(a, _)| input.positions[a as usize].map(f32::to_bits))
                .collect()
        };
        for bits in boundary_before {
            assert!(
                output.positions.iter().any(|p| p.map(f32::to_bits) == bits),
                "a boundary vertex moved or vanished"
            );
        }
    }

    #[test]
    #[ignore = "timing probe: cargo test -p ludion-sculpt --release -- --ignored --nocapture"]
    #[allow(clippy::disallowed_methods)]
    fn large_sphere_timing() {
        let input = sphere_mesh(90);
        let start = std::time::Instant::now();
        let output = decimate(&input, 10_000);
        let elapsed = start.elapsed();
        eprintln!(
            "decimated {} -> {} triangles in {elapsed:?}",
            input.triangle_count(),
            output.triangle_count()
        );
        assert_closed_manifold(&output);
    }
}
