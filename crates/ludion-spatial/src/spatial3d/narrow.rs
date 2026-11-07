//! Shape math for the 3D index: inclusive overlap and the swept-sphere cast. Pure `f32` arithmetic
//! with `+ - * /`, comparisons and `ludion_core::math::sqrt`, so every result is bit-exact.

use super::Volume;
use ludion_core::math::sqrt;

pub(super) type V3 = [f32; 3];

const UP: V3 = [0.0, 1.0, 0.0];

pub(super) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(super) fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn at(o: V3, d: V3, t: f32) -> V3 {
    [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]]
}

pub(super) fn is_finite3(v: V3) -> bool {
    v[0].is_finite() && v[1].is_finite() && v[2].is_finite()
}

/// `v / |v|`, or `fallback` when the length is zero.
fn normalize_or(v: V3, fallback: V3) -> V3 {
    let len = sqrt(dot(v, v));
    if len > 0.0 && len.is_finite() {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

pub(super) fn clamp_to_box(p: V3, min: V3, max: V3) -> V3 {
    let mut q = p;
    for i in 0..3 {
        if q[i] < min[i] {
            q[i] = min[i];
        } else if q[i] > max[i] {
            q[i] = max[i];
        }
    }
    q
}

/// Squared distance from `p` to the box (zero inside).
pub(super) fn dist_sq_to_box(p: V3, min: V3, max: V3) -> f32 {
    let mut sum = 0.0;
    for i in 0..3 {
        let d = if p[i] < min[i] {
            min[i] - p[i]
        } else if p[i] > max[i] {
            p[i] - max[i]
        } else {
            0.0
        };
        sum += d * d;
    }
    sum
}

/// Inclusive overlap: touching counts.
pub(super) fn overlap(probe: &Volume, item: &Volume) -> bool {
    match (probe, item) {
        (
            Volume::Sphere {
                center: c1,
                radius: r1,
            },
            Volume::Sphere {
                center: c2,
                radius: r2,
            },
        ) => {
            let d = sub(*c1, *c2);
            let reach = r1 + r2;
            dot(d, d) <= reach * reach
        }
        (Volume::Sphere { center, radius }, Volume::Aabb { min, max })
        | (Volume::Aabb { min, max }, Volume::Sphere { center, radius }) => {
            dist_sq_to_box(*center, *min, *max) <= radius * radius
        }
        (Volume::Aabb { min: a0, max: a1 }, Volume::Aabb { min: b0, max: b1 }) => {
            (0..3).all(|i| a0[i] <= b1[i] && b0[i] <= a1[i])
        }
    }
}

/// Squared distance from `p` to the item's [`Volume::center`].
pub(super) fn center_distance_sq(p: V3, volume: &Volume) -> f32 {
    let d = sub(p, volume.center());
    dot(d, d)
}

/// Slab test of the ray `o + t·d` against a box over `t ∈ [.., t_max]`. Returns the raw entry
/// parameter (negative when `o` is inside) and the axis that set it (ties x < y < z). A box the ray
/// only touches counts as hit; the caller prunes strictly.
pub(super) fn slab(o: V3, d: V3, min: V3, max: V3, t_max: f32) -> Option<(f32, usize)> {
    let mut lo = f32::NEG_INFINITY;
    let mut hi = t_max;
    let mut axis = 0;
    for i in 0..3 {
        if d[i] == 0.0 {
            if o[i] < min[i] || o[i] > max[i] {
                return None;
            }
            continue;
        }
        let mut t1 = (min[i] - o[i]) / d[i];
        let mut t2 = (max[i] - o[i]) / d[i];
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        if t1 > lo {
            lo = t1;
            axis = i;
        }
        if t2 < hi {
            hi = t2;
        }
    }
    if lo > hi || hi < 0.0 {
        None
    } else {
        Some((lo, axis))
    }
}

/// Smallest `t ∈ [0, 1]` where the ray enters the sphere; `0` when `o` is already inside.
fn ray_sphere(o: V3, d: V3, c: V3, radius: f32) -> Option<f32> {
    let m = sub(o, c);
    let cc = dot(m, m) - radius * radius;
    if cc <= 0.0 {
        return Some(0.0);
    }
    let a = dot(d, d);
    let b = dot(m, d);
    if a == 0.0 || b > 0.0 {
        return None;
    }
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - sqrt(disc)) / a;
    if t > 1.0 { None } else { Some(t.max(0.0)) }
}

/// Smallest `t ∈ [0, 1]` where the ray enters the capsule of `radius` around the axis-aligned
/// segment along axis `k` from `lo` to `hi` (the other two coordinates are `fixed`'s).
fn ray_capsule(o: V3, d: V3, radius: f32, k: usize, fixed: V3, lo: f32, hi: f32) -> Option<f32> {
    let mut a_end = fixed;
    a_end[k] = lo;
    let mut b_end = fixed;
    b_end[k] = hi;
    let mut best = ray_sphere(o, d, a_end, radius);
    if let Some(t) = ray_sphere(o, d, b_end, radius) {
        best = Some(best.map_or(t, |b: f32| b.min(t)));
    }
    let mut m = sub(o, fixed);
    let mut d2 = d;
    m[k] = 0.0;
    d2[k] = 0.0;
    let a = dot(d2, d2);
    if a > 0.0 {
        let b = dot(m, d2);
        let c = dot(m, m) - radius * radius;
        if c > 0.0 && b <= 0.0 {
            let disc = b * b - a * c;
            if disc >= 0.0 {
                let t = (-b - sqrt(disc)) / a;
                if (0.0..=1.0).contains(&t) {
                    let along = o[k] + t * d[k];
                    if along >= lo && along <= hi {
                        best = Some(best.map_or(t, |b: f32| b.min(t)));
                    }
                }
            }
        }
    }
    best
}

/// Contact of a probe sphere that already overlaps the item at `o`: the closest surface point and
/// the normal pointing from the item toward the probe.
fn start_contact(o: V3, item: &Volume) -> (V3, V3) {
    match item {
        Volume::Sphere { center, radius } => {
            let n = normalize_or(sub(o, *center), UP);
            let p = [
                center[0] + n[0] * radius,
                center[1] + n[1] * radius,
                center[2] + n[2] * radius,
            ];
            (p, n)
        }
        Volume::Aabb { min, max } => {
            let cp = clamp_to_box(o, *min, *max);
            if cp != o {
                return (cp, normalize_or(sub(o, cp), UP));
            }
            // Origin inside the box: leave through the least-penetration face (ties x < y < z,
            // sign toward the origin's side, `+` on a tie).
            let mut best_axis = 0;
            let mut best_pen = f32::INFINITY;
            let mut best_sign = 1.0;
            for i in 0..3 {
                let to_min = o[i] - min[i];
                let to_max = max[i] - o[i];
                let (pen, sign) = if to_max <= to_min {
                    (to_max, 1.0)
                } else {
                    (to_min, -1.0)
                };
                if pen < best_pen {
                    best_pen = pen;
                    best_axis = i;
                    best_sign = sign;
                }
            }
            let mut n = [0.0; 3];
            n[best_axis] = best_sign;
            let mut p = o;
            p[best_axis] = if best_sign > 0.0 {
                max[best_axis]
            } else {
                min[best_axis]
            };
            (p, n)
        }
    }
}

/// Sweeps a sphere of `radius` from `origin` along `delta` (`t ∈ [0, 1]`) against one item.
/// Returns `(t, point, normal)`: `point` is the item's surface point nearest the probe centre at
/// `t`, `normal` points from the item toward the probe.
pub(super) fn cast(origin: V3, delta: V3, radius: f32, item: &Volume) -> Option<(f32, V3, V3)> {
    if overlap(
        &Volume::Sphere {
            center: origin,
            radius,
        },
        item,
    ) {
        let (point, normal) = start_contact(origin, item);
        return Some((0.0, point, normal));
    }
    if dot(delta, delta) == 0.0 {
        return None;
    }
    match item {
        Volume::Sphere {
            center: c,
            radius: big,
        } => {
            let t = ray_sphere(origin, delta, *c, big + radius)?;
            let n = normalize_or(sub(at(origin, delta, t), *c), UP);
            let p = [c[0] + n[0] * big, c[1] + n[1] * big, c[2] + n[2] * big];
            Some((t, p, n))
        }
        Volume::Aabb { min, max } => cast_box(origin, delta, radius, *min, *max),
    }
}

fn cast_box(o: V3, d: V3, r: f32, min: V3, max: V3) -> Option<(f32, V3, V3)> {
    let emin = [min[0] - r, min[1] - r, min[2] - r];
    let emax = [max[0] + r, max[1] + r, max[2] + r];
    let (enter, entry_axis) = slab(o, d, emin, emax, 1.0)?;
    let t0 = enter.max(0.0);
    let p = at(o, d, t0);

    // Which axes lie outside the original box at the expanded-box entry point.
    let mut side = [0.0f32; 3];
    let mut outside = 0;
    for i in 0..3 {
        if p[i] < min[i] {
            side[i] = -1.0;
            outside += 1;
        } else if p[i] > max[i] {
            side[i] = 1.0;
            outside += 1;
        }
    }

    if outside <= 1 {
        // A face hit: the normal is exactly the unit axis, so a ricochet reflect stays exact.
        let mut n = [0.0; 3];
        if outside == 1 {
            n = side;
        } else {
            n[entry_axis] = if d[entry_axis] > 0.0 { -1.0 } else { 1.0 };
        }
        return Some((t0, clamp_to_box(p, min, max), n));
    }

    // Edge (two axes outside) or corner (three): the edge capsules around the box's corner.
    let corner = [
        if side[0] < 0.0 { min[0] } else { max[0] },
        if side[1] < 0.0 { min[1] } else { max[1] },
        if side[2] < 0.0 { min[2] } else { max[2] },
    ];
    let mut best: Option<f32> = None;
    for k in 0..3 {
        if outside == 2 && side[k] != 0.0 {
            continue;
        }
        if let Some(t) = ray_capsule(o, d, r, k, corner, min[k], max[k]) {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    let t = best?;
    let center = at(o, d, t);
    let cp = clamp_to_box(center, min, max);
    Some((t, cp, normalize_or(sub(center, cp), UP)))
}
