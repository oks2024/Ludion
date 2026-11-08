//! Conservative CPU culling: world-space boxes tested against a frustum or a sphere.
//!
//! Generic over the volume so any shadow view (a spot's frustum, a cube face's range sphere, a sun
//! cascade) filters the same instance list. Every test may admit an instance that is not visible and
//! never rejects one that is; an instance whose bounds cannot be known on the CPU is always drawn.
//! A skinned instance is bounded by its palette's posed joints ([`SkinBounds`]).

use std::ops::Range;
use std::sync::Arc;

use ludion_core::math::glam::{Mat4, Vec3, Vec4};

use crate::scene::{DebugSegment, MeshData, MeshInstance, MeshShape, RenderScene};

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

    pub(crate) fn planes(&self) -> [Vec4; 6] {
        self.planes
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

/// The fraction of viewport height an instance's bounding sphere covers (`Plan/gpu-driven-culling.md`
/// LOD decision): `radius` is the half-diagonal of its world box, `distance` the span from the LOD eye
/// to the box centre and `k` the projection's `proj[1][1]` scaled by the bias. Orthographic ignores
/// distance. Rotation-invariant, so turning the camera never changes a level.
pub fn lod_size(radius: f32, distance: f32, k: f32, ortho: bool) -> f32 {
    if ortho {
        radius * k
    } else {
        radius * k / distance.max(radius)
    }
}

/// The level a `size` selects from a chain whose `thresholds[i]` is the screen size under which level
/// `i + 1` takes over (strictly decreasing): the count of thresholds above `size`, plus `offset`,
/// clamped to the last level. The one selection function; `cull.wgsl`'s `select_level` mirrors it.
///
/// ```
/// assert_eq!(ludion_renderer::lod_level(0.2, &[0.5, 0.1], 0), 1);
/// assert_eq!(ludion_renderer::lod_level(0.2, &[0.5, 0.1], 5), 2);
/// ```
pub fn lod_level(size: f32, thresholds: &[f32], offset: u32) -> u32 {
    let base = thresholds.iter().filter(|&&t| size < t).count() as u32;
    (base + offset).min(thresholds.len() as u32)
}

/// One view's LOD inputs: the eye, `k` (see [`lod_size`]), whether the projection is orthographic and
/// the level offset (`shadow_levels` for a shadow view, 0 for the camera).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct LodView {
    pub(crate) eye: Vec3,
    pub(crate) k: f32,
    pub(crate) ortho: bool,
    pub(crate) offset: u32,
}

impl LodView {
    /// The camera's LOD inputs: `projection[1][1] * exp2(-bias)` and the eye of `position`; a shadow
    /// view copies them with its own `offset`.
    pub(crate) fn of_camera(camera: &crate::scene::RenderCamera, bias: f32) -> LodView {
        LodView {
            eye: camera.position,
            k: camera.projection.y_axis.y * ludion_core::math::exp2(-bias),
            ortho: camera.projection.w_axis.w == 1.0,
            offset: 0,
        }
    }
}

/// The level of an instance with world box `bound` in `view`, chosen from `thresholds`; 0 for an
/// instance whose bounds are unknown.
pub(crate) fn instance_level(bound: Option<Aabb>, thresholds: &[f32], view: &LodView) -> u32 {
    let Some(b) = bound else {
        return 0;
    };
    let center = (b.min + b.max) * 0.5;
    let radius = ((b.max - b.min) * 0.5).length();
    let size = lod_size(radius, (center - view.eye).length(), view.k, view.ortho);
    lod_level(size, thresholds, view.offset)
}

#[cfg(test)]
/// The CPU oracle of the GPU level selection: each instance's level in `view`, where `chains[i]` is
/// instance `i`'s thresholds (empty for an instance with no chain, which is always level 0).
pub(crate) fn instance_levels(
    bounds: &[Option<Aabb>],
    chains: &[&[f32]],
    view: &LodView,
) -> Vec<u32> {
    bounds
        .iter()
        .zip(chains)
        .map(|(&bound, thresholds)| instance_level(bound, thresholds, view))
        .collect()
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

/// What bounds a skinned asset's posed vertices from its palette slice alone.
///
/// Palette entries are model-space skin matrices `P_j`, and a vertex is `Σ w_j P_j v` with a convex
/// blend. Pick any reference point `b_j` per joint and let `radius = max |v − b_j|` over every
/// vertex and every influence with `w > 0`: then `P_j v` stays within `radius · ‖row_k(P_j)‖` of
/// `P_j b_j` on axis `k`, and a convex blend of points inside a box stays inside it. Correctness
/// does not depend on `b_j` being the true joint position; the bind joint origin is just tight.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SkinBounds {
    /// The influencing joints only, ascending by index: `(index, b_j)`.
    joints: Vec<(u32, Vec3)>,
    /// The asset's whole joint list, which is how many palette entries an instance owns.
    joint_count: usize,
    radius: f32,
}

impl SkinBounds {
    /// `None` for an unskinned mesh or a skin whose weights or bone indices cannot be trusted
    /// (a bone outside the joint list, a negative or non-finite weight, a vertex not summing to 1).
    pub(crate) fn of_mesh(mesh: &MeshData) -> Option<SkinBounds> {
        let count = mesh.skin_joints.len();
        if count == 0 {
            return None;
        }
        let vertices = mesh.positions.len() / 3;
        if mesh.weights.len() < vertices * 4 || mesh.bone_indices.len() < vertices * 4 {
            return None;
        }
        let origins: Vec<Vec3> = mesh
            .skin_joints
            .iter()
            .map(|joint| joint.inverse_bind.inverse().w_axis.truncate())
            .collect();
        let mut used = vec![false; count];
        for v in 0..vertices {
            let weights = &mesh.weights[v * 4..v * 4 + 4];
            let bones = &mesh.bone_indices[v * 4..v * 4 + 4];
            let mut sum = 0.0_f32;
            for (&w, &bone) in weights.iter().zip(bones) {
                if !w.is_finite() || w < 0.0 {
                    return None;
                }
                sum += w;
                if w > 0.0 {
                    *used.get_mut(bone as usize)? = true;
                }
            }
            if (sum - 1.0).abs() > 1e-5 {
                return None;
            }
        }
        let mut radius = 0.0_f32;
        for v in 0..vertices {
            let p = Vec3::new(
                mesh.positions[v * 3],
                mesh.positions[v * 3 + 1],
                mesh.positions[v * 3 + 2],
            );
            for k in 0..4 {
                if mesh.weights[v * 4 + k] > 0.0 {
                    let origin = origins[mesh.bone_indices[v * 4 + k] as usize];
                    radius = radius.max((p - origin).length());
                }
            }
        }
        if !radius.is_finite() {
            return None;
        }
        let joints = used
            .iter()
            .enumerate()
            .filter(|(_, used)| **used)
            .map(|(j, _)| (j as u32, origins[j]))
            .collect();
        Some(SkinBounds {
            joints,
            joint_count: count,
            radius,
        })
    }

    /// The world box of an instance posed by `palette[offset - 1 ..][..joint_count]`
    /// (`palette` is `RenderScene::bone_palette`, without the identity prefix) under `model`.
    /// `None` for a slice outside the palette or a non-finite result.
    pub(crate) fn world_box(&self, palette: &[Mat4], offset: u32, model: &Mat4) -> Option<Aabb> {
        let start = (offset as usize).checked_sub(1)?;
        let slice = palette.get(start..start.checked_add(self.joint_count)?)?;
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for &(joint, origin) in &self.joints {
            let p = slice.get(joint as usize)?;
            let center = p.transform_point3(origin);
            let reach = Vec3::new(
                p.x_axis.x.hypot(p.y_axis.x).hypot(p.z_axis.x),
                p.x_axis.y.hypot(p.y_axis.y).hypot(p.z_axis.y),
                p.x_axis.z.hypot(p.y_axis.z).hypot(p.z_axis.z),
            ) * self.radius;
            min = min.min(center - reach);
            max = max.max(center + reach);
        }
        let margin = 1e-3 * (1.0 + min.abs().max(max.abs()).max_element());
        let model_box = Aabb {
            min: min - margin,
            max: max + margin,
        };
        if !model_box.is_finite() {
            return None;
        }
        Some(model_box.transformed(model)).filter(Aabb::is_finite)
    }
}

/// An asset's cull bounds: the object-space box of its vertices (`None` for a mesh with no vertex)
/// and, for a skinned asset, what bounds it under a posed palette.
#[derive(Clone, Default)]
pub(crate) struct AssetBounds {
    pub(crate) local: Option<Aabb>,
    pub(crate) skin: Option<Arc<SkinBounds>>,
}

impl AssetBounds {
    pub(crate) fn of_mesh(mesh: &MeshData) -> AssetBounds {
        AssetBounds {
            local: Aabb::of_positions(&mesh.positions),
            skin: SkinBounds::of_mesh(mesh).map(Arc::new),
        }
    }
}

/// World bounds per instance. `None` — always drawn — for an asset index outside `assets`,
/// non-finite bounds, or a skinned instance whose asset has no [`SkinBounds`] or whose palette
/// slice is missing. A skinned instance with a skin is bounded by its posed joints.
/// `assets` is indexed like `RenderScene::mesh_assets`; `palette` is `RenderScene::bone_palette`.
pub(crate) fn instance_bounds(
    meshes: &[MeshInstance],
    assets: &[AssetBounds],
    palette: &[Mat4],
) -> Vec<Option<Aabb>> {
    meshes
        .iter()
        .map(|mesh| {
            if mesh.bone_offset != 0 {
                let MeshShape::Asset(i) = mesh.shape else {
                    return None;
                };
                let skin = assets.get(i)?.skin.as_ref()?;
                return skin.world_box(palette, mesh.bone_offset, &mesh.model);
            }
            let local = local_bounds(mesh.shape, assets)?;
            Some(local.transformed(&mesh.model)).filter(Aabb::is_finite)
        })
        .collect()
}

/// A shape's object-space bounds; `None` for an asset index outside `assets` or an asset with
/// unknown bounds.
pub(crate) fn local_bounds(shape: MeshShape, assets: &[AssetBounds]) -> Option<Aabb> {
    Some(match shape {
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
        MeshShape::Asset(i) => assets.get(i)?.local?,
    })
}

/// One object-space box of the GPU bounds table. `known == 0` marks an unknown box (zeros elsewhere),
/// which a cull must treat as always visible.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CullBoundsRaw {
    pub(crate) min: [f32; 3],
    pub(crate) known: u32,
    pub(crate) max: [f32; 3],
    pub(crate) _pad: u32,
}

/// A shape's row in [`bounds_table`]: the three primitives first, then each mesh asset.
pub(crate) fn bounds_slot(shape: MeshShape) -> u32 {
    match shape {
        MeshShape::Cube => 0,
        MeshShape::Plane => 1,
        MeshShape::Sphere => 2,
        MeshShape::Asset(i) => 3 + i as u32,
    }
}

/// The object-space box of every [`bounds_slot`], for the GPU cull to transform per instance.
pub(crate) fn bounds_table(assets: &[AssetBounds]) -> Vec<CullBoundsRaw> {
    let shapes = [MeshShape::Cube, MeshShape::Plane, MeshShape::Sphere]
        .into_iter()
        .chain((0..assets.len()).map(MeshShape::Asset));
    shapes
        .map(|shape| match local_bounds(shape, assets) {
            Some(b) => CullBoundsRaw {
                min: b.min.to_array(),
                known: 1,
                max: b.max.to_array(),
                _pad: 0,
            },
            None => CullBoundsRaw::default(),
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

const KEPT: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
const CULLED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
/// The tint of a kept mesh by the LOD level the camera view selects: level 0 is [`KEPT`] green, then
/// yellow, orange, magenta.
const LEVEL_TINTS: [[f32; 4]; 4] = [
    KEPT,
    [1.0, 1.0, 0.2, 1.0],
    [1.0, 0.55, 0.1, 1.0],
    [1.0, 0.2, 1.0, 1.0],
];
const CULL_FRUSTUM: [f32; 4] = [1.0, 0.85, 0.1, 1.0];
const UNBOUNDED_HALF: f32 = 0.25;

/// The 12 edges of a box, as index pairs into [`box_corners`].
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// Corner `i` has x from bit 0, y from bit 1, z from bit 2 (`0` = min/near, `1` = max/far).
fn box_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

fn box_edges(corners: &[Vec3; 8], color: [f32; 4], out: &mut Vec<DebugSegment>) {
    out.extend(BOX_EDGES.iter().map(|&(a, b)| DebugSegment {
        a: corners[a].to_array(),
        b: corners[b].to_array(),
        color,
    }));
}

/// The camera cull view's verdict on every world mesh, as debug lines: the mesh's world box in green
/// when the cull keeps it and red when it culls it, a green cross for a mesh whose bounds are
/// unknown (always drawn). Viewmodel meshes never enter the world cull and are not marked. When the
/// frame culls against a `cull_camera`, that camera's frustum is outlined in yellow. A kept mesh
/// whose asset carries a `lods:` chain is tinted by the level the camera view selects
/// ([`LEVEL_TINTS`]); `quality_lod_bias` is the machine's `QualityParams::lod_bias`.
///
/// Computed by the CPU oracle over the same bounds the GPU table is built from, so the marks are
/// the set the GPU keeps. It is the source of the `cull` debug-draw category: the verdict depends on
/// the camera, so the engine appends these to the extracted scene's lines.
pub fn cull_marks(scene: &RenderScene, quality_lod_bias: f32) -> Vec<DebugSegment> {
    let view_proj = scene.cull_view_proj();
    let frustum = Frustum::from_view_proj(view_proj);
    let assets: Vec<AssetBounds> = scene
        .mesh_assets
        .iter()
        .map(|mesh| AssetBounds::of_mesh(mesh))
        .collect();
    let bounds = instance_bounds(&scene.meshes, &assets, &scene.bone_palette);
    let lod_view = LodView::of_camera(
        scene.cull_camera.as_ref().unwrap_or(&scene.camera),
        scene.lod.bias + quality_lod_bias,
    );
    let mut out = Vec::new();
    for (mesh, bound) in scene.meshes.iter().zip(&bounds) {
        if mesh.viewmodel {
            continue;
        }
        match bound {
            Some(b) => {
                let color = if frustum.intersects_aabb(b) {
                    let thresholds: Vec<f32> = match mesh.shape {
                        MeshShape::Asset(i) => {
                            scene.mesh_assets.get(i).map_or_else(Vec::new, |m| {
                                m.lods.iter().map(|lod| lod.screen_size).collect()
                            })
                        }
                        _ => Vec::new(),
                    };
                    let level = instance_level(Some(*b), &thresholds, &lod_view);
                    LEVEL_TINTS[(level as usize).min(LEVEL_TINTS.len() - 1)]
                } else {
                    CULLED
                };
                box_edges(&box_corners(b.min, b.max), color, &mut out);
            }
            None => {
                let at = mesh.model.w_axis.truncate();
                for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                    out.push(DebugSegment {
                        a: (at - axis * UNBOUNDED_HALF).to_array(),
                        b: (at + axis * UNBOUNDED_HALF).to_array(),
                        color: KEPT,
                    });
                }
            }
        }
    }
    if scene.cull_camera.is_some() {
        let inverse = view_proj.inverse();
        let corners: [Vec3; 8] = std::array::from_fn(|i| {
            let ndc = Vec4::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { 0.0 } else { 1.0 },
                1.0,
            );
            let world = inverse * ndc;
            world.truncate() / world.w
        });
        box_edges(&corners, CULL_FRUSTUM, &mut out);
    }
    out
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

    /// The machine's quality bias coarsens a pick: `LOW`'s bias selects a coarser level than `FULL`'s
    /// for the same instance, and the camera's own bias adds to it.
    #[test]
    fn a_cheaper_quality_bias_selects_a_coarser_level() {
        use crate::quality::QualityParams;
        let thresholds = [0.5, 0.25, 0.125];
        let (radius, k) = (1.0_f32, 2.0_f32);
        let level = |bias: f32| {
            let size = lod_size(radius, 6.0, k * ludion_core::math::exp2(-bias), false);
            lod_level(size, &thresholds, 0)
        };
        let full = level(QualityParams::FULL.lod_bias);
        let low = level(QualityParams::LOW.lod_bias);
        assert!(low > full, "low {low} vs full {full}");
        assert!(level(QualityParams::LOW.lod_bias + 1.0) >= low);
    }

    #[test]
    fn lod_level_walks_the_thresholds_and_clamps_the_offset() {
        let t = [0.5, 0.25, 0.1];
        assert_eq!(lod_level(0.9, &t, 0), 0);
        assert_eq!(
            lod_level(0.5, &t, 0),
            0,
            "a size equal to a threshold stays finer"
        );
        assert_eq!(lod_level(0.4, &t, 0), 1);
        assert_eq!(lod_level(0.2, &t, 0), 2);
        assert_eq!(lod_level(0.01, &t, 0), 3);
        assert_eq!(lod_level(0.9, &t, 1), 1);
        assert_eq!(
            lod_level(0.2, &t, 9),
            3,
            "the offset clamps to the last level"
        );
        assert_eq!(lod_level(0.0, &[], 3), 0, "no chain is level 0");
    }

    #[test]
    fn lod_size_is_inside_sphere_stable_and_scales_with_bias() {
        // Inside the bounding sphere the distance floors at the radius: size is `k`.
        assert_eq!(lod_size(2.0, 0.5, 1.5, false), 1.5);
        assert_eq!(lod_size(2.0, 2.0, 1.5, false), 1.5);
        // Beyond it, size falls as 1/distance.
        assert_eq!(lod_size(1.0, 10.0, 2.0, false), 0.2);
        assert_eq!(lod_size(1.0, 20.0, 2.0, false), 0.1);
        // Orthographic ignores distance.
        assert_eq!(lod_size(1.0, 99.0, 2.0, true), 2.0);
        // A bias of 1 halves k, which halves every switch distance.
        let camera = |bias| {
            let projection = Mat4::perspective_rh(90.0_f32.to_radians(), 1.0, 0.1, 100.0);
            LodView::of_camera(
                &crate::scene::RenderCamera {
                    view: Mat4::IDENTITY,
                    projection,
                    position: Vec3::ZERO,
                },
                bias,
            )
        };
        assert!((camera(1.0).k * 2.0 - camera(0.0).k).abs() < 1e-6);
        assert!(!camera(0.0).ortho);
    }

    #[test]
    fn instance_levels_recede_and_unknown_bounds_stay_fine() {
        let view = LodView {
            eye: Vec3::ZERO,
            k: 1.0,
            ortho: false,
            offset: 0,
        };
        let thresholds: &[f32] = &[0.25, 0.1];
        let at = |z: f32| Some(cube_at(Vec3::new(0.0, 0.0, -z)));
        let bounds = [at(3.0), at(5.0), at(40.0), None];
        let chains = [thresholds; 4];
        let levels = instance_levels(&bounds, &chains, &view);
        assert_eq!(levels, vec![0, 1, 2, 0]);
        let shadow = LodView { offset: 1, ..view };
        assert_eq!(instance_levels(&bounds, &chains, &shadow), vec![1, 2, 2, 0]);
    }

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

    fn instance(shape: MeshShape, model: Mat4) -> MeshInstance {
        MeshInstance {
            medium: 1.0,
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
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
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

    fn marks_scene(meshes: Vec<MeshInstance>) -> RenderScene {
        let mut scene = RenderScene::empty((160, 90));
        scene.camera.view = Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        scene.camera.projection = Mat4::perspective_rh(90.0_f32.to_radians(), 1.0, 1.0, 100.0);
        scene.meshes = meshes;
        scene
    }

    fn count(marks: &[DebugSegment], color: [f32; 4]) -> usize {
        marks.iter().filter(|m| m.color == color).count()
    }

    fn cube_instance(z: f32) -> MeshInstance {
        instance(
            MeshShape::Cube,
            Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
        )
    }

    #[test]
    fn cull_marks_paint_kept_green_and_culled_red_and_skip_the_viewmodel() {
        let scene = marks_scene(vec![cube_instance(-10.0), cube_instance(10.0)]);
        let marks = cull_marks(&scene, 0.0);
        assert_eq!((count(&marks, KEPT), count(&marks, CULLED)), (12, 12));

        let mut gun = cube_instance(-2.0);
        gun.viewmodel = true;
        let scene = marks_scene(vec![gun]);
        assert!(cull_marks(&scene, 0.0).is_empty());
    }

    /// A kept mesh with a chain is tinted by its level (green, then yellow, orange, magenta); a
    /// mesh without one stays green at any distance, and the machine's bias coarsens the tint.
    #[test]
    fn cull_marks_tint_kept_meshes_by_lod_level() {
        use crate::scene::MeshLod;
        let level_mesh = |hash: u64| {
            let mut mesh = skin_mesh(vec![0.0; 3], vec![], vec![], &[]);
            mesh.hash = hash;
            Arc::new(mesh)
        };
        let mut base = skin_mesh(vec![-0.5, -0.5, -0.5, 0.5, 0.5, 0.5], vec![], vec![], &[]);
        base.lods = [0.3, 0.1, 0.03]
            .iter()
            .enumerate()
            .map(|(k, &screen_size)| MeshLod {
                mesh: level_mesh(k as u64 + 1),
                screen_size,
            })
            .collect();
        let asset = |z: f32| {
            instance(
                MeshShape::Asset(0),
                Mat4::from_translation(Vec3::new(0.0, 0.0, z)),
            )
        };
        let mut scene = marks_scene(vec![asset(-2.0), asset(-8.0), asset(-20.0), asset(-60.0)]);
        scene.mesh_assets = vec![Arc::new(base)];
        scene.camera.position = Vec3::ZERO;
        let marks = cull_marks(&scene, 0.0);
        for (tint, expected) in LEVEL_TINTS.iter().enumerate() {
            assert_eq!(count(&marks, *expected), 12, "level {tint}");
        }
        // A bias of 1 halves every switch distance: each instance moves one level coarser.
        let marks = cull_marks(&scene, 1.0);
        assert_eq!(count(&marks, KEPT), 0);
        assert_eq!(count(&marks, LEVEL_TINTS[3]), 24);
        // No chain: green however far.
        let mut plain = marks_scene(vec![cube_instance(-90.0)]);
        plain.lod.bias = 3.0;
        assert_eq!(count(&cull_marks(&plain, 0.0), KEPT), 12);
    }

    #[test]
    fn a_cull_camera_decides_admission_and_outlines_its_frustum() {
        let mut scene = marks_scene(vec![cube_instance(-10.0)]);
        assert_eq!(count(&cull_marks(&scene, 0.0), CULL_FRUSTUM), 0);
        // Looking the other way, the cube is behind the cull camera.
        let mut turned = scene.camera;
        turned.view = Mat4::look_at_rh(Vec3::ZERO, Vec3::Z, Vec3::Y);
        scene.cull_camera = Some(turned);
        let marks = cull_marks(&scene, 0.0);
        assert_eq!((count(&marks, KEPT), count(&marks, CULLED)), (0, 12));
        assert_eq!(count(&marks, CULL_FRUSTUM), 12);
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
        let bounds = instance_bounds(&meshes, &[], &[]);
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
        let bounds = instance_bounds(
            &meshes,
            &[AssetBounds {
                local: Some(asset),
                skin: None,
            }],
            &[],
        );
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

    // --- skinned bounds and the GPU table ------------------------------------------------------

    use crate::scene::SkinJoint;
    use ludion_core::math::glam::EulerRot;

    /// A deterministic LCG in `[0, 1)`; no dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
        fn vec3(&mut self, lo: f32, hi: f32) -> Vec3 {
            Vec3::new(self.range(lo, hi), self.range(lo, hi), self.range(lo, hi))
        }
        fn trs(&mut self, max_scale: f32, spread: f32) -> Mat4 {
            Mat4::from_scale_rotation_translation(
                self.vec3(0.2, max_scale),
                Quat::from_euler(
                    EulerRot::XYZ,
                    self.range(-3.0, 3.0),
                    self.range(-3.0, 3.0),
                    self.range(-3.0, 3.0),
                ),
                self.vec3(-spread, spread),
            )
        }
    }

    fn joint(bind_origin: Vec3) -> SkinJoint {
        SkinJoint {
            name: String::new(),
            parent: None,
            rest_local: Mat4::IDENTITY,
            rest_translation: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
            rest_scale: Vec3::ONE,
            inverse_bind: Mat4::from_translation(-bind_origin),
            rest: Mat4::IDENTITY,
        }
    }

    /// A mesh with `positions`, per-vertex `weights`/`bones` (4 each) and one joint per origin.
    fn skin_mesh(
        positions: Vec<f32>,
        weights: Vec<f32>,
        bones: Vec<u32>,
        origins: &[Vec3],
    ) -> MeshData {
        let n = positions.len() / 3;
        MeshData {
            positions,
            normals: [0.0, 0.0, 1.0].repeat(n),
            colors: vec![1.0; n * 4],
            materials: [1.0, 0.0, 0.0].repeat(n),
            uvs: vec![0.0; n * 2],
            tangents: [1.0, 0.0, 0.0, 1.0].repeat(n),
            weights,
            bone_indices: bones,
            skin_joints: origins.iter().copied().map(joint).collect(),
            textures: None,
            authored_material: false,
            base_color_mean: [1.0; 4],
            rim: None,
            double_sided: false,
            indices: Vec::new(),
            hash: 0,
            lods: Vec::new(),
        }
    }

    fn skinned_instance(model: Mat4, offset: u32) -> MeshInstance {
        let mut m = instance(MeshShape::Asset(0), model);
        m.bone_offset = offset;
        m
    }

    #[test]
    fn skinned_world_box_contains_every_posed_vertex() {
        let origins = [
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.5, 2.0, -0.5),
        ];
        let mut rng = Lcg(7);
        let (mut positions, mut weights, mut bones) = (Vec::new(), Vec::new(), Vec::new());
        for v in 0..64 {
            let p = rng.vec3(-1.0, 3.0);
            positions.extend([p.x, p.y, p.z]);
            let influences = 1 + v % 4;
            let mut w = [0.0_f32; 4];
            let mut b = [0_u32; 4];
            let mut total = 0.0;
            for k in 0..influences {
                w[k] = rng.range(0.1, 1.0);
                b[k] = (rng.next() * 3.0) as u32 % 3;
                total += w[k];
            }
            for weight in w.iter_mut().take(influences) {
                *weight /= total;
            }
            weights.extend(w);
            bones.extend(b);
        }
        let mesh = skin_mesh(positions.clone(), weights.clone(), bones.clone(), &origins);
        let bounds = AssetBounds::of_mesh(&mesh);
        let skin = bounds.skin.expect("a valid skin has bounds");
        for _ in 0..200 {
            let palette: Vec<Mat4> = (0..3).map(|_| rng.trs(3.0, 5.0)).collect();
            let model = rng.trs(3.0, 20.0);
            let world = skin.world_box(&palette, 1, &model).expect("finite box");
            for v in 0..64 {
                let p = Vec3::new(positions[v * 3], positions[v * 3 + 1], positions[v * 3 + 2]);
                let blended = (0..4)
                    .map(|k| {
                        palette[bones[v * 4 + k] as usize].transform_point3(p) * weights[v * 4 + k]
                    })
                    .sum::<Vec3>();
                let at = model.transform_point3(blended);
                assert!(
                    at.cmpge(world.min).all() && at.cmple(world.max).all(),
                    "vertex {v} at {at:?} outside {world:?}"
                );
            }
        }
    }

    /// Two rigid joints; the first vertex follows joint 0, the second joint 1.
    fn ragdoll_mesh() -> MeshData {
        skin_mesh(
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            vec![0; 8]
                .into_iter()
                .enumerate()
                .map(|(i, _)| u32::from(i == 4))
                .collect(),
            &[Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)],
        )
    }

    #[test]
    fn a_scattered_ragdoll_box_covers_both_parts_and_can_be_culled() {
        let assets = [AssetBounds::of_mesh(&ragdoll_mesh())];
        let palette = [
            Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0)),
            Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ];
        let meshes = [skinned_instance(Mat4::IDENTITY, 1)];
        let bounds = instance_bounds(&meshes, &assets, &palette);
        let b = bounds[0].expect("posed bounds");
        assert!(b.min.z <= -10.0 && b.max.z >= 10.0, "{b:?}");
        // A camera at z = -50 looking away (-Z) sees neither part.
        let view = Mat4::look_at_rh(
            Vec3::new(0.0, 0.0, -50.0),
            Vec3::new(0.0, 0.0, -100.0),
            Vec3::Y,
        );
        let proj = Mat4::perspective_rh(90.0_f32.to_radians(), 1.0, 1.0, 20.0);
        let away = CullVolume::Frustum(Frustum::from_view_proj(proj * view));
        assert!(cull(&bounds, &away).is_empty());
    }

    #[test]
    fn a_skinned_instance_posed_behind_the_camera_is_culled() {
        let assets = [AssetBounds::of_mesh(&ragdoll_mesh())];
        let palette = [
            Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0)),
            Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ];
        let meshes = [skinned_instance(Mat4::IDENTITY, 1)];
        let bounds = instance_bounds(&meshes, &assets, &palette);
        assert!(cull(&bounds, &CullVolume::Frustum(frustum())).is_empty());
    }

    #[test]
    fn skin_bounds_fall_back_to_always_drawn() {
        let origins = [Vec3::ZERO];
        let one = |weights: Vec<f32>, bones: Vec<u32>| {
            AssetBounds::of_mesh(&skin_mesh(vec![0.0; 3], weights, bones, &origins))
        };
        assert!(one(vec![1.0, 0.0, 0.0, 0.0], vec![0; 4]).skin.is_some());
        // An out-of-range bone with weight, and the same bone with zero weight (ignored).
        assert!(
            one(vec![1.0, 0.0, 0.0, 0.0], vec![1, 0, 0, 0])
                .skin
                .is_none()
        );
        assert!(
            one(vec![1.0, 0.0, 0.0, 0.0], vec![0, 9, 0, 0])
                .skin
                .is_some()
        );
        // Unnormalized, negative and non-finite weights.
        assert!(one(vec![0.5, 0.0, 0.0, 0.0], vec![0; 4]).skin.is_none());
        assert!(one(vec![1.5, -0.5, 0.0, 0.0], vec![0; 4]).skin.is_none());
        assert!(
            one(vec![f32::NAN, 0.0, 0.0, 0.0], vec![0; 4])
                .skin
                .is_none()
        );
        // No joints: unskinned data.
        let bare = skin_mesh(vec![0.0; 3], vec![1.0, 0.0, 0.0, 0.0], vec![0; 4], &[]);
        assert!(AssetBounds::of_mesh(&bare).skin.is_none());
        // A palette that stops short of the asset's joint count, or has no slice at the offset.
        let assets = [AssetBounds::of_mesh(&ragdoll_mesh())];
        let far = Mat4::from_translation(Vec3::new(1000.0, 0.0, 0.0));
        let short = [Mat4::IDENTITY];
        let meshes = [skinned_instance(far, 1), skinned_instance(far, 5)];
        assert_eq!(instance_bounds(&meshes, &assets, &short), vec![None, None]);
        assert_eq!(instance_bounds(&meshes, &[], &[]), vec![None, None]);
    }

    #[test]
    fn bounds_table_rows_follow_the_slots() {
        let asset = Aabb {
            min: Vec3::new(-1.0, 0.0, -2.0),
            max: Vec3::new(1.0, 4.0, 2.0),
        };
        let assets = [
            AssetBounds {
                local: Some(asset),
                skin: None,
            },
            AssetBounds::default(),
        ];
        let table = bounds_table(&assets);
        assert_eq!(table.len(), 5);
        for shape in [MeshShape::Cube, MeshShape::Plane, MeshShape::Sphere] {
            let row = table[bounds_slot(shape) as usize];
            let want = local_bounds(shape, &assets).unwrap();
            assert_eq!(
                (row.known, Vec3::from(row.min), Vec3::from(row.max)),
                (1, want.min, want.max)
            );
        }
        let row = table[bounds_slot(MeshShape::Asset(0)) as usize];
        assert_eq!(
            (row.known, row.min, row.max),
            (1, [-1.0, 0.0, -2.0], [1.0, 4.0, 2.0])
        );
        assert_eq!(
            table[bounds_slot(MeshShape::Asset(1)) as usize],
            CullBoundsRaw::default()
        );
        assert_eq!(std::mem::size_of::<CullBoundsRaw>(), 32);
    }
}
