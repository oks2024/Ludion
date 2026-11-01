//! Conservative CPU culling: world-space boxes tested against a frustum or a sphere.
//!
//! Generic over the volume so any shadow view (a spot's frustum, a cube face's range sphere, a sun
//! cascade) filters the same instance list. Every test may admit an instance that is not visible and
//! never rejects one that is; an instance whose bounds cannot be known on the CPU is always drawn.

use std::ops::Range;

use ludion_core::math::glam::{Mat4, Vec3, Vec4};

use crate::scene::{MeshInstance, MeshShape};

/// An axis-aligned box.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Aabb {
    pub(crate) min: Vec3,
    pub(crate) max: Vec3,
}

impl Aabb {
    /// The box of a flat `xyz` position list, or `None` when it holds no vertex.
    pub(crate) fn of_positions(positions: &[f32]) -> Option<Aabb> {
        let mut points = positions.chunks_exact(3);
        let first = points.next()?;
        let first = Vec3::new(first[0], first[1], first[2]);
        let (min, max) = points.fold((first, first), |(min, max), p| {
            let p = Vec3::new(p[0], p[1], p[2]);
            (min.min(p), max.max(p))
        });
        Some(Aabb { min, max })
    }

    /// The box of this box's eight corners under `model` (Arvo: the centre through the matrix, the
    /// half-extent through the element-wise absolute upper 3×3).
    pub(crate) fn transformed(&self, model: &Mat4) -> Aabb {
        let center = (self.min + self.max) * 0.5;
        let extent = (self.max - self.min) * 0.5;
        let center = model.transform_point3(center);
        let extent = model.x_axis.truncate().abs() * extent.x
            + model.y_axis.truncate().abs() * extent.y
            + model.z_axis.truncate().abs() * extent.z;
        Aabb {
            min: center - extent,
            max: center + extent,
        }
    }

    pub(crate) fn is_finite(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }
}

/// Six inward-facing, normalized planes `(n, d)`; a point is inside a plane when `n·p + d ≥ 0`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// The planes of a world→clip matrix with wgpu's `[0, 1]` clip depth (Gribb–Hartmann).
    pub(crate) fn from_view_proj(m: Mat4) -> Frustum {
        let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
        let normalize = |p: Vec4| {
            let len = p.truncate().length();
            if len > 0.0 { p / len } else { p }
        };
        Frustum {
            planes: [
                normalize(r3 + r0),
                normalize(r3 - r0),
                normalize(r3 + r1),
                normalize(r3 - r1),
                normalize(r2),
                normalize(r3 - r2),
            ],
        }
    }

    pub(crate) fn intersects_sphere(&self, center: Vec3, radius: f32) -> bool {
        self.planes
            .iter()
            .all(|p| p.truncate().dot(center) + p.w >= -radius)
    }

    pub(crate) fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            let far = Vec3::new(
                if n.x >= 0.0 { aabb.max.x } else { aabb.min.x },
                if n.y >= 0.0 { aabb.max.y } else { aabb.min.y },
                if n.z >= 0.0 { aabb.max.z } else { aabb.min.z },
            );
            n.dot(far) + p.w >= 0.0
        })
    }
}

/// What a shadow view can see.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CullVolume {
    Frustum(Frustum),
}

impl CullVolume {
    pub(crate) fn admits(&self, aabb: &Aabb) -> bool {
        match self {
            CullVolume::Frustum(f) => f.intersects_aabb(aabb),
        }
    }
}

/// World bounds per instance. `None` — always drawn — for a skinned instance (its deformed bounds
/// are not known on the CPU), an asset index outside `asset_bounds`, or non-finite bounds.
/// `asset_bounds` is indexed like `RenderScene::mesh_assets`.
pub(crate) fn instance_bounds(
    meshes: &[MeshInstance],
    asset_bounds: &[Option<Aabb>],
) -> Vec<Option<Aabb>> {
    meshes
        .iter()
        .map(|mesh| {
            if mesh.bone_offset != 0 {
                return None;
            }
            let local = match mesh.shape {
                // Object-space extents of `mesh.rs`'s primitives: cube and sphere span ±0.5, the
                // plane ±0.5 in XZ at y = 0.
                MeshShape::Cube | MeshShape::Sphere => Aabb {
                    min: Vec3::splat(-0.5),
                    max: Vec3::splat(0.5),
                },
                MeshShape::Plane => Aabb {
                    min: Vec3::new(-0.5, 0.0, -0.5),
                    max: Vec3::new(0.5, 0.0, 0.5),
                },
                MeshShape::Asset(i) => (*asset_bounds.get(i)?)?,
            };
            Some(local.transformed(&mesh.model)).filter(Aabb::is_finite)
        })
        .collect()
}

/// Indices (ascending) of the instances a volume can see: those admitted, plus every `None`.
pub(crate) fn cull(bounds: &[Option<Aabb>], volume: &CullVolume) -> Vec<u32> {
    bounds
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_none_or(|b| volume.admits(&b)))
        .map(|(i, _)| i as u32)
        .collect()
}

/// The instance-position ranges of a mesh draw whose `sources` (ascending `scene.meshes` indices,
/// one per instance) are `visible`, adjacent positions coalesced into one range.
pub(crate) fn visible_ranges(sources: &[u32], visible: &[bool]) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    for (position, &source) in sources.iter().enumerate() {
        if !visible[source as usize] {
            continue;
        }
        let position = position as u32;
        match ranges.last_mut() {
            Some(last) if last.end == position => last.end += 1,
            _ => ranges.push(position..position + 1),
        }
    }
    ranges
}

/// `ranges` (instance positions of a draw whose `sources` are ascending `scene.meshes` indices) with
/// every position whose source does not cast (`casts[source]` false) removed, survivors re-coalesced.
pub(crate) fn casting_ranges(
    ranges: Vec<Range<u32>>,
    sources: &[u32],
    casts: &[bool],
) -> Vec<Range<u32>> {
    let mut kept: Vec<Range<u32>> = Vec::new();
    for position in ranges.into_iter().flatten() {
        if !casts[sources[position as usize] as usize] {
            continue;
        }
        match kept.last_mut() {
            Some(last) if last.end == position => last.end += 1,
            _ => kept.push(position..position + 1),
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::{Mat3, Quat};

    #[test]
    fn casting_ranges_split_drop_and_recoalesce() {
        let sources = [0, 1, 2, 3, 4, 5];
        let all: Vec<Range<u32>> = std::iter::once(0..6).collect();
        assert_eq!(
            casting_ranges(all.clone(), &sources, &[true; 6]),
            vec![0..6]
        );
        assert!(casting_ranges(all.clone(), &sources, &[false; 6]).is_empty());
        let alternating = [true, false, true, false, true, false];
        assert_eq!(
            casting_ranges(all, &sources, &alternating),
            vec![0..1, 2..3, 4..5]
        );
        // Multi-range input: a gap between ranges stays a gap, a dropped middle splits a range.
        let casts = [true, true, false, true, true, true];
        assert_eq!(
            casting_ranges(vec![0..3, 4..6], &sources, &casts),
            vec![0..2, 4..6]
        );
    }

    #[test]
    fn visible_ranges_coalesce_runs_and_skip_gaps() {
        let sources = [0, 2, 3, 5, 6, 7];
        let visible = [true, false, true, false, false, true, true, false];
        assert_eq!(visible_ranges(&sources, &visible), vec![0..2, 3..5]);
        assert_eq!(visible_ranges(&sources, &[true; 8]), vec![0..6]);
        assert!(visible_ranges(&sources, &[false; 8]).is_empty());
    }

    fn instance(shape: MeshShape, model: Mat4) -> MeshInstance {
        MeshInstance {
            shape,
            model,
            normal_matrix: Mat3::IDENTITY,
            color: [1.0; 4],
            material: [1.0, 0.0, 0.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
        }
    }

    fn cube_at(center: Vec3) -> Aabb {
        Aabb {
            min: center - Vec3::splat(0.5),
            max: center + Vec3::splat(0.5),
        }
    }

    /// Camera at the origin looking down -Z, near 1, far 100.
    fn frustum() -> Frustum {
        let view = Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        let proj = Mat4::perspective_rh(90.0_f32.to_radians(), 1.0, 1.0, 100.0);
        Frustum::from_view_proj(proj * view)
    }

    #[test]
    fn frustum_admits_in_front_and_rejects_behind_and_beyond_far() {
        let f = frustum();
        assert!(f.intersects_aabb(&cube_at(Vec3::new(0.0, 0.0, -10.0))));
        assert!(!f.intersects_aabb(&cube_at(Vec3::new(0.0, 0.0, 10.0))));
        assert!(!f.intersects_aabb(&cube_at(Vec3::new(0.0, 0.0, -200.0))));
        assert!(!f.intersects_aabb(&cube_at(Vec3::new(50.0, 0.0, -10.0))));
    }

    #[test]
    fn a_box_straddling_a_plane_is_admitted() {
        let f = frustum();
        // Across the near plane (z = -1) and across the right plane (x = -z) at depth 10.
        assert!(f.intersects_aabb(&cube_at(Vec3::new(0.0, 0.0, -1.0))));
        assert!(f.intersects_aabb(&cube_at(Vec3::new(10.0, 0.0, -10.0))));
        assert!(f.intersects_sphere(Vec3::new(10.4, 0.0, -10.0), 1.0));
        assert!(!f.intersects_sphere(Vec3::new(20.0, 0.0, -10.0), 1.0));
    }

    #[test]
    fn unknown_bounds_are_always_drawn_and_output_ascends() {
        let far = Mat4::from_translation(Vec3::new(1000.0, 0.0, 0.0));
        let mut skinned = instance(MeshShape::Cube, far);
        skinned.bone_offset = 3;
        let nan = instance(MeshShape::Cube, Mat4::from_translation(Vec3::NAN));
        let missing = instance(MeshShape::Asset(7), far);
        let culled = instance(MeshShape::Cube, far);
        let seen = instance(
            MeshShape::Sphere,
            Mat4::from_translation(Vec3::new(0.0, 0.0, -5.0)),
        );
        let meshes = [culled, skinned, seen, nan, missing];
        let bounds = instance_bounds(&meshes, &[]);
        assert_eq!(bounds[0].map(|b| b.is_finite()), Some(true));
        assert_eq!(bounds[1], None);
        assert_eq!(bounds[3], None);
        assert_eq!(bounds[4], None);
        let out = cull(&bounds, &CullVolume::Frustum(frustum()));
        assert_eq!(out, vec![1, 2, 3, 4]);
    }

    #[test]
    fn asset_bounds_are_used_and_planes_are_flat() {
        let asset = Aabb {
            min: Vec3::new(-1.0, 0.0, -1.0),
            max: Vec3::new(1.0, 4.0, 1.0),
        };
        let m = Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0));
        let meshes = [
            instance(MeshShape::Asset(0), m),
            instance(MeshShape::Plane, m),
        ];
        let bounds = instance_bounds(&meshes, &[Some(asset)]);
        assert_eq!(bounds[0].unwrap().max.y, 4.0);
        let plane = bounds[1].unwrap();
        assert_eq!((plane.min.y, plane.max.y), (0.0, 0.0));
    }

    #[test]
    fn of_positions_spans_the_vertices() {
        assert_eq!(Aabb::of_positions(&[]), None);
        let b = Aabb::of_positions(&[1.0, 2.0, 3.0, -1.0, 5.0, 0.0]).unwrap();
        assert_eq!(b.min, Vec3::new(-1.0, 2.0, 0.0));
        assert_eq!(b.max, Vec3::new(1.0, 5.0, 3.0));
    }

    #[test]
    fn transformed_box_contains_every_transformed_corner() {
        let model = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 0.5, 3.0),
            Quat::from_euler(ludion_core::math::glam::EulerRot::XYZ, 0.4, 1.1, -0.7),
            Vec3::new(3.0, -2.0, 1.0),
        );
        let local = cube_at(Vec3::ZERO);
        let world = local.transformed(&model);
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { local.min.x } else { local.max.x },
                if i & 2 == 0 { local.min.y } else { local.max.y },
                if i & 4 == 0 { local.min.z } else { local.max.z },
            );
            let p = model.transform_point3(corner);
            assert!(p.cmpge(world.min - 1e-4).all() && p.cmple(world.max + 1e-4).all());
        }
    }
}
