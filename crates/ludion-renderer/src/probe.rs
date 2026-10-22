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
}
