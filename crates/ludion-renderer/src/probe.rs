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
    /// Inward fade distance: weight 1 at `blend` inside the boundary.
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
/// A probe's weight at world point `p`: 0 outside its volume, ramping to 1 over `blend`.
/// The WGSL twin is `probe_weight` in `snippets/probes.wgsl`.
pub fn probe_weight(probe: &ProbeData, p: Vec3) -> f32 {
    let inside = match probe.volume {
        ProbeVolume::Box { extent } => {
            let q = probe.rotation.inverse() * (p - probe.position);
            (extent - q.abs()).min_element()
        }
        ProbeVolume::Sphere { radius } => radius - (p - probe.position).length(),
    };
    if inside <= 0.0 {
        0.0
    } else if probe.blend <= 0.0 {
        1.0
    } else {
        (inside / probe.blend).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
/// The world direction from the capture point to sample for a reflection ray `r` from `p`,
/// parallax-corrected against the probe's volume. The WGSL twin is `probe_lookup`.
pub fn probe_lookup(probe: &ProbeData, p: Vec3, r: Vec3) -> Vec3 {
    match probe.volume {
        ProbeVolume::Box { extent } => {
            let inv_rot = probe.rotation.inverse();
            let q = inv_rot * (p - probe.position);
            let d = inv_rot * r;
            let guard = |v: f32| if v < 0.0 { v.min(-1e-6) } else { v.max(1e-6) };
            let inv = Vec3::new(guard(d.x), guard(d.y), guard(d.z)).recip();
            let a = (extent - q) * inv;
            let b = (-extent - q) * inv;
            let t = a.max(b).min_element();
            probe.rotation * (q + t * d)
        }
        ProbeVolume::Sphere { radius } => {
            let q = p - probe.position;
            let b = q.dot(r);
            let c = q.dot(q) - radius * radius;
            let t = -b + (b * b - c).max(0.0).sqrt();
            q + t * r
        }
    }
}

#[cfg(test)]
/// A prefiltered probe sample with `ibl_intensity` `k` applied to its sky-visible fraction
/// (alpha) only. The WGSL twin is `probe_sky_scaled`.
pub fn sky_scaled(sample: Vec4, k: f32) -> Vec3 {
    sample.truncate() * (1.0 - sample.w + sample.w * k)
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
    fn weight_is_zero_outside_ramps_in_the_band_and_one_deep_inside() {
        let b = boxed(Vec3::splat(2.0), Quat::IDENTITY, 1.0);
        assert_eq!(probe_weight(&b, Vec3::new(2.5, 0.0, 0.0)), 0.0);
        assert!((probe_weight(&b, Vec3::new(1.5, 0.0, 0.0)) - 0.5).abs() < 1e-6);
        assert_eq!(probe_weight(&b, Vec3::ZERO), 1.0);
        let mut s = probe(0, 0, ProbeVolume::Sphere { radius: 2.0 });
        s.blend = 1.0;
        assert_eq!(probe_weight(&s, Vec3::new(3.0, 0.0, 0.0)), 0.0);
        assert!((probe_weight(&s, Vec3::new(1.5, 0.0, 0.0)) - 0.5).abs() < 1e-6);
        assert_eq!(probe_weight(&s, Vec3::ZERO), 1.0);
        s.blend = 0.0;
        assert_eq!(probe_weight(&s, Vec3::new(1.9, 0.0, 0.0)), 1.0);
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
    fn sky_scaled_scales_only_the_sky_visible_fraction() {
        let rgb = Vec3::new(0.2, 0.4, 0.8);
        let at = |a: f32, k: f32| sky_scaled(rgb.extend(a), k);
        assert_eq!(at(0.0, 0.9), rgb);
        assert!((at(1.0, 0.9) - rgb * 0.9).length() < 1e-6);
        assert!((at(0.5, 0.9) - rgb * 0.95).length() < 1e-6);
        assert!((at(0.37, 1.0) - rgb).length() < 1e-6);
    }
}
