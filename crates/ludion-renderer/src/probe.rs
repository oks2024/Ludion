//! Reflection probe plain data and the static rank key (`Plan/reflections.md` decisions 14–15).

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Quat, Vec3};

/// The most probes a scene may hold (decision 21).
pub const MAX_PROBES: usize = 32;

/// A probe's influence volume, which doubles as its parallax proxy.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ProbeVolume {
    /// An oriented box.
    Box {
        /// Half-extents along the probe's local axes.
        extent: Vec3,
    },
    /// A sphere about the probe's position.
    Sphere {
        /// Radius in world units.
        radius: f32,
    },
}

impl ProbeVolume {
    /// Whether this volume is a sphere.
    pub fn is_sphere(&self) -> bool {
        matches!(self, ProbeVolume::Sphere { .. })
    }
}

/// One extracted reflection probe. The pose gives the capture point and the box orientation;
/// entity scale is ignored.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ProbeData {
    /// The owning entity, the rank key's last tie-break.
    pub entity: EntityId,
    /// World-space capture point and volume centre.
    pub position: Vec3,
    /// World-space orientation of a box volume.
    pub rotation: Quat,
    /// Influence and parallax volume.
    pub volume: ProbeVolume,
    /// Outward fade distance: weight 1 inside the volume, 0 at `blend` outside it.
    pub blend: f32,
    /// Higher wins.
    pub priority: i32,
}

impl ProbeData {
    /// The volume's size in cubic world units.
    pub fn size(&self) -> f32 {
        match self.volume {
            ProbeVolume::Box { extent } => 8.0 * extent.x * extent.y * extent.z,
            ProbeVolume::Sphere { radius } => 4.0 / 3.0 * std::f32::consts::PI * radius.powi(3),
        }
    }
}

/// Decision 15's static order: higher `priority` first, then boxes before spheres, then the
/// smaller volume, then `EntityId`.
pub fn probe_rank(a: &ProbeData, b: &ProbeData) -> std::cmp::Ordering {
    b.priority
        .cmp(&a.priority)
        .then_with(|| a.volume.is_sphere().cmp(&b.volume.is_sphere()))
        .then_with(|| a.size().total_cmp(&b.size()))
        .then_with(|| a.entity.cmp(&b.entity))
}

#[cfg(test)]
use ludion_core::math::glam::Vec4;

#[cfg(test)]
/// The most probe contributions one pixel takes.
pub const MAX_TAKEN: usize = 4;

#[cfg(test)]
/// A probe's weight at world point `p`: 1 inside and on its volume, ramping to 0 over `blend`
/// outside it. The WGSL twin is `probe_weight` in `snippets/probes.wgsl`.
pub fn probe_weight(probe: &ProbeData, p: Vec3) -> f32 {
    let d = match probe.volume {
        ProbeVolume::Box { extent } => {
            let q = probe.rotation.inverse() * (p - probe.position);
            (q.abs() - extent).max(Vec3::ZERO).length()
        }
        ProbeVolume::Sphere { radius } => ((p - probe.position).length() - radius).max(0.0),
    };
    if d <= 0.0 {
        1.0
    } else if probe.blend <= 0.0 {
        0.0
    } else {
        (1.0 - d / probe.blend).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
/// The world direction from the capture point to sample for a reflection ray `r` from `p`,
/// parallax-corrected against the probe's volume. A point outside the volume is clamped onto it
/// first. The WGSL twin is `probe_lookup`.
pub fn probe_lookup(probe: &ProbeData, p: Vec3, r: Vec3) -> Vec3 {
    match probe.volume {
        ProbeVolume::Box { extent } => {
            let inv_rot = probe.rotation.inverse();
            let q = (inv_rot * (p - probe.position)).clamp(-extent, extent);
            let d = inv_rot * r;
            let guard = |v: f32| if v < 0.0 { v.min(-1e-6) } else { v.max(1e-6) };
            let inv = Vec3::new(guard(d.x), guard(d.y), guard(d.z)).recip();
            let a = (extent - q) * inv;
            let b = (-extent - q) * inv;
            let t = a.max(b).min_element();
            probe.rotation * (q + t * d)
        }
        ProbeVolume::Sphere { radius } => {
            let mut q = p - probe.position;
            let len = q.length();
            if len > radius {
                q *= radius / len;
            }
            let b = q.dot(r);
            let c = q.dot(q) - radius * radius;
            let t = -b + (b * b - c).max(0.0).sqrt();
            q + t * r
        }
    }
}

#[cfg(test)]
/// The relight ratio's ceiling; the WGSL twin is `MAX_RELIGHT_RATIO`.
const MAX_RELIGHT_RATIO: f32 = 16.0;

#[cfg(test)]
/// The live-to-baked irradiance ratio per channel, at most [`MAX_RELIGHT_RATIO`]; 1 where the baked value is too small to divide
/// by. The WGSL twin is `probe_ratio`.
pub fn ratio(baked: Vec3, live: Vec3) -> Vec3 {
    let channel = |b: f32, l: f32| {
        if b > 1e-6 {
            (l / b).min(MAX_RELIGHT_RATIO)
        } else {
            1.0
        }
    };
    Vec3::new(
        channel(baked.x, live.x),
        channel(baked.y, live.y),
        channel(baked.z, live.z),
    )
}

#[cfg(test)]
/// The ambient-cube rows (+X −X +Y −Y +Z −Z = 0..5) a unit direction `n` weights: the signed axis
/// per component, zero counting as positive. The WGSL twin is `probe_axis_rows`.
pub fn axis_rows(n: Vec3) -> [usize; 3] {
    [
        if n.x >= 0.0 { 0 } else { 1 },
        if n.y >= 0.0 { 2 } else { 3 },
        if n.z >= 0.0 { 4 } else { 5 },
    ]
}

#[cfg(test)]
/// The relight ratio along the facing `n`: ambient-cube weights `n * n` over the per-axis baked and
/// live irradiance, then [`ratio`]. Both sums spell the same operations, so equal bits give exactly
/// 1. The WGSL twin is `probe_axis_ratio`.
pub fn axis_ratio(baked: [Vec3; 6], live: [Vec3; 6], n: Vec3) -> Vec3 {
    let [rx, ry, rz] = axis_rows(n);
    let w = n * n;
    ratio(
        w.x * baked[rx] + w.y * baked[ry] + w.z * baked[rz],
        w.x * live[rx] + w.y * live[ry] + w.z * live[rz],
    )
}

#[cfg(test)]
/// A prefiltered probe sample relit: the geometry radiance (rgb) scaled by `ratio` except its
/// emissive part `em`, the sky fraction (alpha) replaced by the live sky `sky` times `k`. The WGSL
/// twin is `probe_relit`.
pub fn relit(sample: Vec4, em: Vec3, ratio: Vec3, sky: Vec3, k: f32) -> Vec3 {
    (sample.truncate() * ratio + em * (Vec3::ONE - ratio)).max(Vec3::ZERO) + sample.w * sky * k
}

#[cfg(test)]
/// Rank-ordered weights in, per-probe contributions and final coverage out: each positive
/// weight contributes `w * (1 - coverage)`, at most [`MAX_TAKEN`] of them, stopping once the
/// coverage reaches 1.
pub fn accumulate(weights: impl IntoIterator<Item = f32>) -> (Vec<f32>, f32) {
    let mut out = Vec::new();
    let mut coverage = 0.0_f32;
    for w in weights {
        if out.len() == MAX_TAKEN || coverage >= 1.0 {
            break;
        }
        if w <= 0.0 {
            continue;
        }
        out.push(w * (1.0 - coverage));
        coverage += w * (1.0 - coverage);
    }
    (out, coverage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn probe(entity: u32, priority: i32, volume: ProbeVolume) -> ProbeData {
        ProbeData {
            entity: EntityId::new(entity, 0),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            volume,
            blend: 0.5,
            priority,
        }
    }

    fn cube(half: f32) -> ProbeVolume {
        ProbeVolume::Box {
            extent: Vec3::splat(half),
        }
    }

    #[test]
    fn size_of_both_volumes() {
        let b = probe(
            0,
            0,
            ProbeVolume::Box {
                extent: Vec3::new(1.0, 2.0, 3.0),
            },
        );
        assert_eq!(b.size(), 48.0);
        let s = probe(0, 0, ProbeVolume::Sphere { radius: 1.0 });
        assert!((s.size() - 4.188_79).abs() < 1e-4);
    }

    #[test]
    fn priority_decides_first() {
        let big_high = probe(1, 1, cube(5.0));
        let small_low = probe(0, 0, cube(1.0));
        assert_eq!(probe_rank(&big_high, &small_low), Ordering::Less);
    }

    #[test]
    fn a_box_ranks_before_a_smaller_sphere_at_equal_priority() {
        let b = probe(1, 0, cube(5.0));
        let s = probe(0, 0, ProbeVolume::Sphere { radius: 0.5 });
        assert_eq!(probe_rank(&b, &s), Ordering::Less);
    }

    #[test]
    fn the_smaller_volume_ranks_first_then_the_entity() {
        let small = probe(5, 0, cube(1.0));
        let big = probe(0, 0, cube(2.0));
        assert_eq!(probe_rank(&small, &big), Ordering::Less);
        let a = probe(1, 0, cube(1.0));
        let b = probe(2, 0, cube(1.0));
        assert_eq!(probe_rank(&a, &b), Ordering::Less);
    }

    fn boxed(extent: Vec3, rotation: Quat, blend: f32) -> ProbeData {
        ProbeData {
            rotation,
            blend,
            volume: ProbeVolume::Box { extent },
            ..probe(0, 0, cube(1.0))
        }
    }

    #[test]
    fn a_rotated_box_lookup_hits_the_world_face() {
        // 90 degrees about Y maps local +Z to world +X.
        let p = boxed(
            Vec3::new(1.0, 1.0, 3.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            0.0,
        );
        let v = probe_lookup(&p, Vec3::ZERO, Vec3::X);
        assert!((v - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4, "{v:?}");
    }

    #[test]
    fn a_box_lookup_is_the_hit_relative_to_the_capture_point() {
        // A room [-2, 2] x [-1, 1] x [-4, 4] about a capture point at its centre; the ray from
        // (1, 0, 0) toward (+1, 0, +1) reaches the x = 2 wall first.
        let p = boxed(Vec3::new(2.0, 1.0, 4.0), Quat::IDENTITY, 0.0);
        let r = Vec3::new(1.0, 0.0, 1.0).normalize();
        let v = probe_lookup(&p, Vec3::new(1.0, 0.0, 0.0), r);
        // Hit at x = 2: t = 1/r.x, so hit = (2, 0, 1/r.x * r.z) = (2, 0, 1).
        assert!((v - Vec3::new(2.0, 0.0, 1.0)).length() < 1e-4, "{v:?}");
    }

    #[test]
    fn a_sphere_lookup_has_the_radius_from_the_centre_at_the_hit() {
        let mut p = probe(0, 0, ProbeVolume::Sphere { radius: 3.0 });
        p.position = Vec3::new(1.0, 2.0, 3.0);
        let at = p.position + Vec3::new(0.5, -0.25, 0.75);
        let r = Vec3::new(0.3, 0.9, -0.2).normalize();
        let v = probe_lookup(&p, at, r);
        // The returned direction is the hit point relative to the centre.
        assert!((v.length() - 3.0).abs() < 1e-4, "{}", v.length());
    }

    #[test]
    fn weight_is_one_inside_ramps_to_zero_outside_the_volume() {
        let b = boxed(Vec3::splat(2.0), Quat::IDENTITY, 1.0);
        assert_eq!(probe_weight(&b, Vec3::ZERO), 1.0);
        assert_eq!(probe_weight(&b, Vec3::new(2.0, 0.0, 0.0)), 1.0);
        assert!((probe_weight(&b, Vec3::new(2.5, 0.0, 0.0)) - 0.5).abs() < 1e-6);
        assert_eq!(probe_weight(&b, Vec3::new(3.5, 0.0, 0.0)), 0.0);
        // A corner is measured as the Euclidean distance to the box.
        let corner = Vec3::splat(2.0) + Vec3::splat(0.5 / 3.0_f32.sqrt());
        assert!((probe_weight(&b, corner) - 0.5).abs() < 1e-5);
        let mut s = probe(0, 0, ProbeVolume::Sphere { radius: 2.0 });
        s.blend = 1.0;
        assert_eq!(probe_weight(&s, Vec3::ZERO), 1.0);
        assert_eq!(probe_weight(&s, Vec3::new(2.0, 0.0, 0.0)), 1.0);
        assert!((probe_weight(&s, Vec3::new(2.5, 0.0, 0.0)) - 0.5).abs() < 1e-6);
        assert_eq!(probe_weight(&s, Vec3::new(3.5, 0.0, 0.0)), 0.0);
        s.blend = 0.0;
        assert_eq!(probe_weight(&s, Vec3::new(2.0, 0.0, 0.0)), 1.0);
        assert_eq!(probe_weight(&s, Vec3::new(2.01, 0.0, 0.0)), 0.0);
        let mut b0 = b;
        b0.blend = 0.0;
        assert_eq!(probe_weight(&b0, Vec3::new(2.0, 0.0, 0.0)), 1.0);
        assert_eq!(probe_weight(&b0, Vec3::new(2.01, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn a_lookup_from_the_band_equals_the_lookup_from_the_surface() {
        let r = Vec3::new(0.3, 0.9, -0.2).normalize();
        let b = boxed(Vec3::new(2.0, 1.0, 4.0), Quat::from_rotation_y(0.4), 1.0);
        let on_face = Vec3::new(2.0, 0.5, 1.0);
        let outside = b.rotation * (on_face + Vec3::new(0.5, 0.0, 0.0));
        let on_face = b.rotation * on_face;
        let a = probe_lookup(&b, outside, r);
        let c = probe_lookup(&b, on_face, r);
        assert!(a.is_finite() && (a - c).length() < 1e-4, "{a:?} {c:?}");
        let s = probe(0, 0, ProbeVolume::Sphere { radius: 3.0 });
        let dir = Vec3::new(1.0, 0.5, -0.5).normalize();
        let a = probe_lookup(&s, dir * 3.6, r);
        let c = probe_lookup(&s, dir * 3.0, r);
        assert!(a.is_finite() && (a - c).length() < 1e-4, "{a:?} {c:?}");
    }

    #[test]
    fn accumulate_fills_coverage_in_rank_order() {
        assert_eq!(accumulate([1.0, 0.5]), (vec![1.0], 1.0));
        let (c, cov) = accumulate([0.5, 0.5]);
        assert_eq!(c, vec![0.5, 0.25]);
        assert_eq!(cov, 0.75);
        let (c, _) = accumulate([0.1; 5]);
        assert_eq!(c.len(), 4);
        let (c, _) = accumulate([0.0, 0.0, 0.3, 0.0, 0.3, 0.0, 0.3, 0.3, 0.3]);
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn ratio_divides_per_channel_and_guards_a_dark_capture() {
        let r = ratio(Vec3::new(0.5, 0.0, 2.0), Vec3::new(1.0, 3.0, 1.0));
        assert_eq!(r, Vec3::new(2.0, 1.0, 0.5));
        let same = Vec3::new(0.3, 0.07, 1.9);
        assert_eq!(ratio(same, same), Vec3::ONE);
        assert_eq!(
            ratio(Vec3::splat(0.01), Vec3::splat(5.0)),
            Vec3::splat(16.0)
        );
    }

    #[test]
    fn relit_follows_the_ratio_for_geometry_and_the_live_sky_for_the_rest() {
        let rgb = Vec3::new(0.2, 0.4, 0.8);
        let sky = Vec3::new(0.1, 0.2, 0.4);
        // Geometry only: the ratio scales it, the sky and k do not matter.
        assert_eq!(
            relit(rgb.extend(0.0), Vec3::ZERO, Vec3::splat(2.0), sky, 0.9),
            rgb * 2.0
        );
        // Sky only (captured rgb is 0): the live sky times k.
        let at = relit(
            Vec4::new(0.0, 0.0, 0.0, 1.0),
            Vec3::ZERO,
            Vec3::splat(5.0),
            sky,
            0.9,
        );
        assert!((at - sky * 0.9).length() < 1e-6);
        // A mix adds the two parts; with no Environment (k = 0) only the geometry remains.
        let mixed = relit(rgb.extend(0.5), Vec3::ZERO, Vec3::ONE, sky, 0.0);
        assert_eq!(mixed, rgb);
        let mixed = relit(rgb.extend(0.5), Vec3::ZERO, Vec3::ONE, sky, 1.0);
        assert!((mixed - (rgb + sky * 0.5)).length() < 1e-6);
    }

    #[test]
    fn axis_ratio_is_exactly_one_when_live_equals_baked() {
        let e: [Vec3; 6] =
            std::array::from_fn(|i| Vec3::new(0.3 + i as f32, 0.07 * (i + 1) as f32, 1.9));
        for n in [Vec3::new(0.3, -0.5, 0.2).normalize(), Vec3::X, -Vec3::Z] {
            assert_eq!(axis_ratio(e, e, n), Vec3::ONE);
        }
    }

    #[test]
    fn axis_ratio_along_an_axis_reads_that_row_only() {
        let baked = [Vec3::ONE; 6];
        let mut live = [Vec3::splat(100.0); 6];
        live[2] = Vec3::new(2.0, 3.0, 4.0);
        assert_eq!(axis_rows(Vec3::Y), [0, 2, 4]);
        assert_eq!(axis_rows(-Vec3::Y), [0, 3, 4]);
        assert_eq!(axis_ratio(baked, live, Vec3::Y), Vec3::new(2.0, 3.0, 4.0));
    }

    #[test]
    fn the_ambient_cube_weights_sum_to_one() {
        for n in [
            Vec3::new(1.0, 2.0, -3.0).normalize(),
            Vec3::new(-0.2, 0.9, 0.4).normalize(),
        ] {
            let w = n * n;
            assert!((w.x + w.y + w.z - 1.0).abs() < 1e-6);
        }
    }

    /// The WGSL `probe_axis_rows` + `probe_axis_ratio` and [`axis_rows`] + [`axis_ratio`] are one
    /// formula in two languages.
    #[test]
    fn needs_gpu_the_axis_ratio_snippet_matches_its_rust_twin() {
        let gpu = match ludion_render_graph::Gpu::request() {
            Ok(gpu) => gpu,
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED the axis ratio snippet test: no GPU adapter");
                return;
            }
            Err(d) => panic!("gpu request failed: {d:?}"),
        };
        // A small LCG keeps the inputs fixed without a dependency.
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut dirs = Vec::new();
        let mut input = Vec::new();
        for _ in 0..50 {
            let n =
                Vec3::new(next() * 2.0 - 1.0, next() * 2.0 - 1.0, next() * 2.0 - 1.0).normalize();
            dirs.push(n);
            input.extend([n.x, n.y, n.z]);
            for _ in 0..12 {
                input.extend([
                    0.05 + next() * 4.0,
                    0.05 + next() * 4.0,
                    0.05 + next() * 4.0,
                ]);
            }
        }
        let src = format!(
            "{}\n@group(0) @binding(0) var<storage, read> inp: array<f32>;\n\
             @group(0) @binding(1) var<storage, read_write> out: array<f32>;\n\
             fn row(base: u32, i: u32) -> vec3<f32> {{\n\
             let at = base + i * 3u;\n return vec3<f32>(inp[at], inp[at + 1u], inp[at + 2u]);\n}}\n\
             @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{\n\
             if (id.x >= 50u) {{ return; }}\n\
             let b = id.x * 39u;\n\
             let n = vec3<f32>(inp[b], inp[b + 1u], inp[b + 2u]);\n\
             let r = probe_axis_rows(n);\n\
             let q = probe_axis_ratio(row(b + 3u, r.x), row(b + 3u, r.y), row(b + 3u, r.z),\n\
             row(b + 21u, r.x), row(b + 21u, r.y), row(b + 21u, r.z), n * n);\n\
             out[id.x * 3u] = q.x; out[id.x * 3u + 1u] = q.y; out[id.x * 3u + 2u] = q.z;\n}}\n",
            include_str!("shaders/snippets/probes.wgsl"),
        );
        let got = crate::lobe::run_f32_kernel(&gpu, &src, &input, dirs.len() * 3);
        for (k, (&n, row)) in dirs.iter().zip(input.chunks(39)).enumerate() {
            let e = |at: usize| -> [Vec3; 6] {
                std::array::from_fn(|i| Vec3::from_slice(&row[at + i * 3..at + i * 3 + 3]))
            };
            let want = axis_ratio(e(3), e(21), n);
            for c in 0..3 {
                let g = got[k * 3 + c];
                assert!(
                    (g - want[c]).abs() <= 1e-6 * want[c].abs().max(1.0),
                    "k {k} direction {n:?} channel {c}: gpu {g} vs {} (rows {:?}, baked {:?}, live {:?})",
                    want[c],
                    axis_rows(n),
                    e(3),
                    e(21)
                );
            }
        }
    }
}
