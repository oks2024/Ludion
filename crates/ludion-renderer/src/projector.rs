//! The projector pass's CPU half: instance packing and the drop-shadow occlusion atlas.
//!
//! The pass itself (`shaders/projector.wgsl`) is the engine's generic way to paint an analytic
//! footprint onto the opaque scene; a drop shadow is its first client. This module is pure — no
//! GPU, no scene beyond [`DropShadowInstance`] — so the layout arithmetic, the matrices and the
//! exclusion ranges are unit-tested without an adapter.

use std::ops::Range;

use ludion_core::math::glam::{Mat4, Vec3};

use crate::scene::{DROP_SHADOW_ANCHOR_TOLERANCE, DropShadowInstance};
use crate::shader_gen;

/// What a projector instance paints. Written into every instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ProjectorKind {
    /// A drop shadow: an analytic footprint, first-surface tested against an occlusion tile.
    DropShadow = 0,
}

/// How a projector instance combines with the scene colour. Written into every instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ProjectorBlend {
    /// `scene *= output`: overlapping projectors darken multiplicatively.
    Multiply = 0,
}

/// The occlusion tile edge a small caster count gets, in texels.
pub const OCCLUSION_TILE: u32 = 128;

/// The occlusion atlas's edge ceiling, in texels. Past `OCCLUSION_TILE²`-worth of casters the
/// tiles shrink to stay under it.
pub const OCCLUSION_ATLAS_MAX: u32 = 2048;

/// The headroom added to a footprint's half-extent so the soft edge and anti-aliasing fringe fit
/// inside the volume and the occlusion tile.
const FOOTPRINT_PAD: f32 = 1.05;

/// The occlusion atlas: a square grid of square tiles, one per caster, row-major.
///
/// There is no cap on the caster count. Past 256 casters the tiles shrink (the lowest `EntityId`s
/// come first, the extract order) so the atlas never grows beyond [`OCCLUSION_ATLAS_MAX`]; nothing
/// is dropped and no diagnostic is raised.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OcclusionAtlas {
    /// Tiles per row (and per column), a power of two.
    pub per_row: u32,
    /// A tile's edge in texels.
    pub tile: u32,
    /// The atlas's edge in texels, `per_row * tile`.
    pub edge: u32,
}

/// The atlas layout for `count` casters, or `None` for none.
pub fn occlusion_atlas(count: usize) -> Option<OcclusionAtlas> {
    if count == 0 {
        return None;
    }
    let per_row = (count as f64).sqrt().ceil() as u32;
    let per_row = per_row.next_power_of_two();
    let tile = (OCCLUSION_ATLAS_MAX / per_row).clamp(1, OCCLUSION_TILE);
    Some(OcclusionAtlas {
        per_row,
        tile,
        edge: per_row * tile,
    })
}

impl OcclusionAtlas {
    /// The top-left texel of tile `index`.
    pub fn tile_origin(&self, index: usize) -> (u32, u32) {
        let index = index as u32;
        (
            (index % self.per_row) * self.tile,
            (index / self.per_row) * self.tile,
        )
    }
}

/// Half the width of the square volume (and occlusion tile) around a drop shadow's anchor: the
/// footprint's farthest reach from its centre at any yaw, with the fringe padding.
pub fn footprint_bound(shape: crate::GroundMarkerShape) -> f32 {
    let (half, radius) = shape.rounded_box();
    (half[0] + radius) * FOOTPRINT_PAD
}

/// The straight-down orthographic view of one caster's occlusion tile: world to clip, with `z = 0`
/// at the anchor's top (the anchor plus the receiver tolerance) and `z = 1` at `max_distance`
/// below the anchor.
pub fn occlusion_view(ds: &DropShadowInstance) -> Mat4 {
    let b = footprint_bound(ds.shape);
    let eye = ds.anchor + Vec3::Y * DROP_SHADOW_ANCHOR_TOLERANCE;
    let view = Mat4::look_at_rh(eye, eye - Vec3::Y, -Vec3::Z);
    let proj = Mat4::orthographic_rh(
        -b,
        b,
        -b,
        b,
        0.0,
        DROP_SHADOW_ANCHOR_TOLERANCE + ds.max_distance,
    );
    proj * view
}

/// The depth slack of the first-surface test, in the tile's depth units: a centimetre plus two
/// world-space texels, so a receiver is not shadowed away by its own rasterized depth.
pub fn occlusion_bias(ds: &DropShadowInstance, atlas: &OcclusionAtlas) -> f32 {
    let texel_world = 2.0 * footprint_bound(ds.shape) / atlas.tile as f32;
    (0.01 + 2.0 * texel_world) / (DROP_SHADOW_ANCHOR_TOLERANCE + ds.max_distance)
}

/// The instance ranges of a mesh draw to render for a caster: those of `sources` (ascending
/// `scene.meshes` indices, one per instance) not in `exclude` (ascending).
pub(crate) fn kept_ranges(sources: &[u32], exclude: &[u32]) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    let mut skip = exclude.iter().copied().peekable();
    for (position, &source) in sources.iter().enumerate() {
        while skip.next_if(|&e| e < source).is_some() {}
        if skip.peek() == Some(&source) {
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

/// The GPU instance of drop shadow `ds`, whose occlusion tile is `index` of `atlas`.
pub(crate) fn projector_instance(
    ds: &DropShadowInstance,
    atlas: &OcclusionAtlas,
    index: usize,
) -> shader_gen::Projector {
    let (half, radius) = ds.shape.rounded_box();
    let (ox, oy) = atlas.tile_origin(index);
    let edge = atlas.edge as f32;
    let (sin, cos) = ds.yaw.sin_cos();
    shader_gen::Projector {
        ids: [
            ProjectorKind::DropShadow as u32,
            ProjectorBlend::Multiply as u32,
            index as u32,
            0,
        ],
        top: [
            ds.anchor.x,
            ds.anchor.y + DROP_SHADOW_ANCHOR_TOLERANCE,
            ds.anchor.z,
            ds.max_distance + DROP_SHADOW_ANCHOR_TOLERANCE,
        ],
        shape: [half[0], half[1], radius, ds.softness],
        look: [
            ds.opacity,
            ds.max_distance,
            ds.fade_distance,
            footprint_bound(ds.shape),
        ],
        yaw: [cos, sin, occlusion_bias(ds, atlas), ds.anchor.y],
        tile: [
            ox as f32 / edge,
            oy as f32 / edge,
            atlas.tile as f32 / edge,
            0.0,
        ],
        occ_view: occlusion_view(ds).to_cols_array_2d(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GroundMarkerShape;

    fn shadow(shape: GroundMarkerShape) -> DropShadowInstance {
        DropShadowInstance {
            shape,
            anchor: Vec3::new(3.0, 2.0, -1.0),
            yaw: 0.7,
            opacity: 0.6,
            softness: 0.3,
            max_distance: 4.0,
            fade_distance: 1.0,
            exclude: Vec::new(),
        }
    }

    #[test]
    fn the_atlas_tiles_shrink_only_past_256_casters() {
        let table = [
            (0, None),
            (1, Some((128, 128))),
            (4, Some((128, 256))),
            (5, Some((128, 512))),
            (64, Some((128, 1024))),
            (256, Some((128, 2048))),
            (257, Some((64, 2048))),
            (4096, Some((32, 2048))),
        ];
        for (count, expected) in table {
            assert_eq!(
                occlusion_atlas(count).map(|a| (a.tile, a.edge)),
                expected,
                "{count} casters"
            );
        }
    }

    #[test]
    fn tiles_run_row_major_and_stay_inside_the_atlas() {
        let atlas = occlusion_atlas(5).unwrap();
        assert_eq!(atlas.tile_origin(0), (0, 0));
        assert_eq!(atlas.tile_origin(1), (128, 0));
        assert_eq!(atlas.tile_origin(4), (0, 128));
        for index in 0..5 {
            let (x, y) = atlas.tile_origin(index);
            assert!(x + atlas.tile <= atlas.edge && y + atlas.tile <= atlas.edge);
        }
    }

    #[test]
    fn the_bound_covers_a_disc_and_a_segments_half_length() {
        let disc = footprint_bound(GroundMarkerShape::Disc { radius: 2.0 });
        assert!((disc - 2.1).abs() < 1e-6);
        let segment = footprint_bound(GroundMarkerShape::Segment {
            length: 8.0,
            width: 2.0,
        });
        assert!((segment - 4.2).abs() < 1e-6);
    }

    #[test]
    fn the_occlusion_view_maps_the_anchor_top_to_zero_and_the_far_plane_to_one() {
        for shape in [
            GroundMarkerShape::Disc { radius: 1.5 },
            GroundMarkerShape::Segment {
                length: 6.0,
                width: 1.0,
            },
        ] {
            let ds = shadow(shape);
            let m = occlusion_view(&ds);
            let b = footprint_bound(shape);
            let top = ds.anchor + Vec3::Y * DROP_SHADOW_ANCHOR_TOLERANCE;
            let near = m.project_point3(top);
            assert!(near.z.abs() < 1e-5, "{near:?}");
            assert!(near.x.abs() < 1e-5 && near.y.abs() < 1e-5);
            let far = m.project_point3(ds.anchor - Vec3::Y * ds.max_distance);
            assert!((far.z - 1.0).abs() < 1e-5, "{far:?}");
            let east = m.project_point3(top + Vec3::X * b);
            let west = m.project_point3(top - Vec3::X * b);
            assert!((east.x - 1.0).abs() < 1e-5 && (west.x + 1.0).abs() < 1e-5);
            let south = m.project_point3(top + Vec3::Z * b);
            assert!((south.y.abs() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_bias_grows_as_the_tile_shrinks() {
        let ds = shadow(GroundMarkerShape::Disc { radius: 1.0 });
        let coarse = occlusion_bias(&ds, &occlusion_atlas(257).unwrap());
        let fine = occlusion_bias(&ds, &occlusion_atlas(1).unwrap());
        assert!(coarse > fine && fine > 0.01 / 4.05);
    }

    #[test]
    fn kept_ranges_skip_exactly_the_excluded_sources() {
        let ranges = |sources: &[u32], exclude: &[u32]| kept_ranges(sources, exclude);
        assert_eq!(ranges(&[2, 5, 9], &[]), vec![0..3]);
        assert_eq!(ranges(&[2, 5, 9], &[2, 5, 9]), Vec::<Range<u32>>::new());
        assert_eq!(ranges(&[2, 5, 9, 11, 12], &[5, 11]), [0..1, 2..3, 4..5]);
        assert_eq!(ranges(&[2, 5, 9], &[0, 1, 3, 4, 10]), vec![0..3]);
        assert_eq!(ranges(&[], &[1, 2]), Vec::<Range<u32>>::new());
        assert_eq!(ranges(&[4, 5, 6, 7], &[5, 6]), [0..1, 3..4]);
    }

    #[test]
    fn an_instance_carries_its_kind_blend_and_tile() {
        let ds = shadow(GroundMarkerShape::Disc { radius: 1.0 });
        let atlas = occlusion_atlas(5).unwrap();
        let instance = projector_instance(&ds, &atlas, 4);
        assert_eq!(instance.ids, [0, 0, 4, 0]);
        assert_eq!(instance.tile[..3], [0.0, 0.25, 0.25]);
        assert_eq!(instance.look[3], footprint_bound(ds.shape));
        assert_eq!(instance.yaw[3], 2.0);
    }
}
