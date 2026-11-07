//! The read-only traversals: explicit-stack, allocation-free once the caller's buffer has grown.

use std::cmp::Ordering;

use super::narrow::{self, V3};
use super::{CastHit, Filter, Handle, Index3d, Neighbor, STACK, Volume};

/// `(a_distance, a_handle) < (b_distance, b_handle)` in the pinned total order.
fn key_less(a: (f32, Handle), b: (f32, Handle)) -> bool {
    a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)) == Ordering::Less
}

fn valid_radius(r: f32) -> bool {
    r.is_finite() && r >= 0.0
}

impl Index3d {
    /// Handles of items overlapping the sphere (touching counts), sorted by handle. Clears `out`
    /// first; a non-finite probe finds nothing.
    pub fn overlap_sphere(
        &self,
        center: [f32; 3],
        radius: f32,
        filter: &Filter,
        out: &mut Vec<Handle>,
    ) {
        out.clear();
        if !narrow::is_finite3(center) || !valid_radius(radius) {
            return;
        }
        self.overlap_volume(&Volume::Sphere { center, radius }, filter, out);
    }

    /// Handles of items overlapping the box (touching counts), sorted by handle. Clears `out`
    /// first; a non-finite or inverted probe finds nothing.
    pub fn overlap_aabb(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        filter: &Filter,
        out: &mut Vec<Handle>,
    ) {
        out.clear();
        if !narrow::is_finite3(min) || !narrow::is_finite3(max) || !(0..3).all(|i| min[i] <= max[i])
        {
            return;
        }
        self.overlap_volume(&Volume::Aabb { min, max }, filter, out);
    }

    fn overlap_volume(&self, probe: &Volume, filter: &Filter, out: &mut Vec<Handle>) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = [0u32; STACK];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let index = stack[sp];
            let node = &self.nodes[index as usize];
            if node.layers & filter.layers == 0
                || !narrow::overlap(
                    probe,
                    &Volume::Aabb {
                        min: node.min,
                        max: node.max,
                    },
                )
            {
                continue;
            }
            if node.count > 0 {
                let range = node.first as usize..(node.first + node.count) as usize;
                for item in &self.items[range] {
                    if filter.passes(item) && narrow::overlap(probe, &item.volume) {
                        out.push(item.handle);
                    }
                }
            } else {
                stack[sp] = node.first;
                stack[sp + 1] = index + 1;
                sp += 2;
            }
        }
        out.sort_unstable();
    }

    /// Up to `k` items nearest `point`, measured to each [`Volume::center`], within `max_radius`
    /// (inclusive), ordered by `(distance_sq, handle)`. Clears `out` first.
    pub fn k_nearest(
        &self,
        point: [f32; 3],
        k: usize,
        max_radius: f32,
        filter: &Filter,
        out: &mut Vec<Neighbor>,
    ) {
        out.clear();
        if k == 0
            || self.nodes.is_empty()
            || !narrow::is_finite3(point)
            || max_radius.is_nan()
            || max_radius < 0.0
        {
            return;
        }
        let max_sq = max_radius * max_radius;
        out.reserve(k.min(self.items.len()));
        let mut stack = [(0u32, 0.0f32); STACK];
        let root = &self.nodes[0];
        stack[0] = (0, narrow::dist_sq_to_box(point, root.min, root.max));
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let (index, bound) = stack[sp];
            // Strict: a node whose bound equals the worst kept may hold an equal-distance item
            // with a smaller handle.
            if bound > max_sq || (out.len() == k && bound > out[k - 1].distance_sq) {
                continue;
            }
            let node = &self.nodes[index as usize];
            if node.layers & filter.layers == 0 {
                continue;
            }
            if node.count > 0 {
                let range = node.first as usize..(node.first + node.count) as usize;
                for item in &self.items[range] {
                    if !filter.passes(item) {
                        continue;
                    }
                    let d2 = narrow::center_distance_sq(point, &item.volume);
                    if d2 > max_sq {
                        continue;
                    }
                    let key = (d2, item.handle);
                    if out.len() == k {
                        let worst = &out[k - 1];
                        if !key_less(key, (worst.distance_sq, worst.handle)) {
                            continue;
                        }
                        out.pop();
                    }
                    let at = out.partition_point(|n| key_less((n.distance_sq, n.handle), key));
                    out.insert(
                        at,
                        Neighbor {
                            handle: item.handle,
                            distance_sq: d2,
                            center: item.volume.center(),
                        },
                    );
                }
            } else {
                let left = &self.nodes[index as usize + 1];
                let right = &self.nodes[node.first as usize];
                let lb = narrow::dist_sq_to_box(point, left.min, left.max);
                let rb = narrow::dist_sq_to_box(point, right.min, right.max);
                let (near, far) = if rb < lb {
                    ((node.first, rb), (index + 1, lb))
                } else {
                    ((index + 1, lb), (node.first, rb))
                };
                stack[sp] = far;
                stack[sp + 1] = near;
                sp += 2;
            }
        }
    }

    /// The first item hit by a sphere of `radius` swept from `origin` along `delta`
    /// (`t ∈ [0, 1]`; radius 0 is a segment). The minimum by `(t, handle)`. An item the probe
    /// already overlaps hits at `t = 0`. A non-finite probe or a negative radius hits nothing.
    pub fn cast(
        &self,
        origin: [f32; 3],
        delta: [f32; 3],
        radius: f32,
        filter: &Filter,
    ) -> Option<CastHit> {
        if self.nodes.is_empty()
            || !narrow::is_finite3(origin)
            || !narrow::is_finite3(delta)
            || !valid_radius(radius)
        {
            return None;
        }
        let mut best: Option<CastHit> = None;
        let mut best_t = 1.0f32;
        let mut stack = [(0u32, 0.0f32); STACK];
        let root = self.enter(0, origin, delta, radius, best_t)?;
        stack[0] = (0, root);
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let (index, enter) = stack[sp];
            // Strict: an equal-`t` node may hold a smaller handle.
            if enter > best_t {
                continue;
            }
            let node = &self.nodes[index as usize];
            if node.layers & filter.layers == 0 {
                continue;
            }
            if node.count > 0 {
                let range = node.first as usize..(node.first + node.count) as usize;
                for item in &self.items[range] {
                    if !filter.passes(item) {
                        continue;
                    }
                    let Some((t, point, normal)) =
                        narrow::cast(origin, delta, radius, &item.volume)
                    else {
                        continue;
                    };
                    let better = best.is_none_or(|b| key_less((t, item.handle), (b.t, b.handle)));
                    if better {
                        best_t = t;
                        best = Some(CastHit {
                            handle: item.handle,
                            t,
                            point,
                            normal,
                        });
                    }
                }
            } else {
                let left = self.enter(index + 1, origin, delta, radius, best_t);
                let right = self.enter(node.first, origin, delta, radius, best_t);
                let mut push = |entry: Option<f32>, child: u32| {
                    if let Some(t) = entry {
                        stack[sp] = (child, t);
                        sp += 1;
                    }
                };
                // Farther child first, so the nearer pops first (left on a tie).
                match (left, right) {
                    (Some(l), Some(r)) if r < l => {
                        push(left, index + 1);
                        push(right, node.first);
                    }
                    _ => {
                        push(right, node.first);
                        push(left, index + 1);
                    }
                }
            }
        }
        best
    }

    /// Entry parameter of the probe's swept volume into a node's box inflated by `radius`, or
    /// `None` when it cannot reach it within `t_max`.
    fn enter(&self, node: u32, origin: V3, delta: V3, radius: f32, t_max: f32) -> Option<f32> {
        let n = &self.nodes[node as usize];
        let min = [n.min[0] - radius, n.min[1] - radius, n.min[2] - radius];
        let max = [n.max[0] + radius, n.max[1] + radius, n.max[2] + radius];
        narrow::slab(origin, delta, min, max, t_max).map(|(t, _)| t.max(0.0))
    }
}
