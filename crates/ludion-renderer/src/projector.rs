//! The projector pass's CPU half: instance packing, the drop-shadow occlusion atlas and the decal
//! batches.
//!
//! The pass itself (`shaders/projector.wgsl`) is the engine's generic way to paint onto the opaque
//! scene; a drop shadow is its first client and a decal its second. This module is pure — no GPU,
//! no scene beyond [`DropShadowInstance`] and [`DecalInstance`] — so the layout arithmetic, the
//! matrices, the exclusion ranges and the batching are unit-tested without an adapter.

use std::ops::Range;

use ludion_core::math::glam::{Mat3, Mat4, Vec3};

use crate::scene::{
    DROP_SHADOW_ANCHOR_TOLERANCE, DecalBlend, DecalInstance, DecalReceivers, DropShadowInstance,
    RenderScene, TextureWrap,
};
use crate::shader_gen;

/// What a projector instance paints. Written into every instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ProjectorKind {
    /// A drop shadow: an analytic footprint, first-surface tested against an occlusion tile.
    DropShadow = 0,
    /// A decal: a box volume sampling a map, tested against the receivers it lists.
    Decal = 1,
}

/// How a projector instance combines with the scene colour. Written into every instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum ProjectorBlend {
    /// `scene *= output`: overlapping projectors darken multiplicatively.
    Multiply = 0,
    /// `scene += output`: the decal's glow adds to the lit colour.
    Add = 1,
    /// A lit decal's surface, `lerp(scene, shaded, c)`, drawn by the lit pass.
    Alpha = 2,
    /// A lit decal's tint and specular lobe, drawn by the lit pass.
    Coat = 3,
}

/// Which pass a decal draws in: the projector (`multiply`, `add`) or the lit pass (`alpha`,
/// `coat`). Each packs only its own blends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DecalPass {
    Projector,
    Lit,
}

impl DecalPass {
    /// The blend `decal` draws with in this pass, or `None` when the other pass draws it.
    fn blend(self, decal: &DecalInstance) -> Option<ProjectorBlend> {
        match (self, decal.blend) {
            (DecalPass::Projector, DecalBlend::Multiply) => Some(ProjectorBlend::Multiply),
            (DecalPass::Projector, DecalBlend::Add) => Some(ProjectorBlend::Add),
            (DecalPass::Lit, DecalBlend::Alpha) => Some(ProjectorBlend::Alpha),
            (DecalPass::Lit, DecalBlend::Coat) => Some(ProjectorBlend::Coat),
            _ => None,
        }
    }
}

/// The occlusion tile edge a small caster count gets, in texels.
pub const OCCLUSION_TILE: u32 = 128;

/// The occlusion atlas's edge ceiling, in texels. Past 256 casters the tiles shrink to stay under
/// it, down to one texel each: up to 2048² casters.
pub const OCCLUSION_ATLAS_MAX: u32 = 2048;

/// The headroom added to a footprint's half-extent so the soft edge and anti-aliasing fringe fit
/// inside the volume and the occlusion tile.
const FOOTPRINT_PAD: f32 = 1.05;

/// The occlusion atlas: a square grid of square tiles, one per caster, row-major.
///
/// The caster count is capped only by the ceiling (2048² one-texel tiles). Past 256 casters the tiles shrink (the lowest `EntityId`s
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

/// Every mesh index the receiver-id pass must redraw, ascending and deduplicated: those some decal's
/// receiver list names. An [`DecalReceivers::Unskinned`] decal needs none: the scene pass flags a
/// skinned surface in `scene_normal`'s alpha sign. Empty means no decal needs the pass.
pub(crate) fn receiver_union(scene: &RenderScene) -> Vec<u32> {
    let mut union: Vec<u32> = scene
        .decals
        .iter()
        .flat_map(|decal| match &decal.receivers {
            DecalReceivers::Exclude(list) | DecalReceivers::Only(list) => list.iter().copied(),
            DecalReceivers::Unskinned => [].iter().copied(),
        })
        .collect();
    union.sort_unstable();
    union.dedup();
    union
}

/// What the decal passes plan for a frame's decals, for the profiler: the decals by blend mode,
/// the draws and the meshes the receiver-id pass redraws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecalStats {
    /// Decals blended as alpha.
    pub alpha: u32,
    /// Decals blended as multiply.
    pub multiply: u32,
    /// Decals blended as add.
    pub add: u32,
    /// Decals blended as coat.
    pub coat: u32,
    /// Decal draws: the projector's batches, and two per lit `alpha` or `coat` decal.
    pub draws: u32,
    /// Meshes the receiver-id pass redraws; 0 means the pass is not scheduled.
    pub receiver_meshes: u32,
}

/// The [`DecalStats`] of `scene`, from the same packing and receiver union the frame draws with: a
/// pure CPU entry point, so the profiler needs no adapter.
pub fn decal_stats(scene: &RenderScene) -> DecalStats {
    let mut stats = DecalStats::default();
    for decal in &scene.decals {
        match decal.blend {
            DecalBlend::Alpha => stats.alpha += 1,
            DecalBlend::Multiply => stats.multiply += 1,
            DecalBlend::Add => stats.add += 1,
            DecalBlend::Coat => stats.coat += 1,
        }
    }
    stats.draws = (pack_decals(&scene.decals, DecalPass::Projector).2.len() as u32)
        + 2 * (stats.alpha + stats.coat);
    stats.receiver_meshes = receiver_union(scene).len() as u32;
    stats
}

/// The instance ranges of a mesh draw whose source is in `keep` (ascending): the twin of
/// [`kept_ranges`] for the receiver-id pass, which redraws only the listed meshes.
pub(crate) fn only_ranges(sources: &[u32], keep: &[u32]) -> Vec<Range<u32>> {
    let mut ranges: Vec<Range<u32>> = Vec::new();
    let mut listed = keep.iter().copied().peekable();
    for (position, &source) in sources.iter().enumerate() {
        while listed.next_if(|&k| k < source).is_some() {}
        if listed.peek() != Some(&source) {
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

/// A map's identity and wrap, `None` for the default.
pub(crate) type MapKey = Option<(u64, TextureWrap)>;

/// One draw of consecutive decals that share a blend and their maps.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct DecalBatch {
    /// How the batch's pipeline blends.
    pub blend: ProjectorBlend,
    /// The base-colour map's identity and wrap, `None` for the white default.
    pub map: MapKey,
    /// The normal map's, `None` for none; always `None` in the projector pass, which has no use
    /// for it.
    pub normal: MapKey,
    /// The metallic-roughness map's, `None` for the white default; `None` in the projector pass.
    pub metallic_roughness: MapKey,
    /// The batch's instances in the packed decal list.
    pub range: Range<u32>,
    /// The input index of the batch's first decal, whose maps the batch binds.
    pub first: usize,
}

/// The decals `pass` draws, packed in scene order: their GPU instances, the receiver lists
/// concatenated (each ascending, so the shader can binary-search its slice), and the batches.
///
/// A batch is a maximal run of *consecutive* drawables with one blend and one set of maps: merging
/// non-adjacent runs would reorder a multiply against an add. A lit pass issues its draws per
/// instance, so its batches only decide the bind groups.
pub(crate) fn pack_decals(
    decals: &[DecalInstance],
    pass: DecalPass,
) -> (Vec<shader_gen::ProjectorDecal>, Vec<u32>, Vec<DecalBatch>) {
    let mut instances = Vec::new();
    let mut receiver_ids = Vec::new();
    let mut batches: Vec<DecalBatch> = Vec::new();
    for (index, decal) in decals.iter().enumerate() {
        let Some(blend) = pass.blend(decal) else {
            continue;
        };
        let key = |map: &Option<crate::scene::MeshTexture>| {
            map.as_ref().map(|map| (map.image.identity(), map.wrap))
        };
        let lit = pass == DecalPass::Lit;
        let map = key(&decal.maps.base_color);
        let normal = if lit { key(&decal.maps.normal) } else { None };
        let metallic_roughness = if lit {
            key(&decal.maps.metallic_roughness)
        } else {
            None
        };
        let position = instances.len() as u32;
        match batches.last_mut() {
            Some(last)
                if last.blend == blend
                    && last.map == map
                    && last.normal == normal
                    && last.metallic_roughness == metallic_roughness =>
            {
                last.range.end += 1;
            }
            _ => batches.push(DecalBatch {
                blend,
                map,
                normal,
                metallic_roughness,
                range: position..position + 1,
                first: index,
            }),
        }
        instances.push(decal_instance(decal, blend, receiver_ids.len() as u32));
        match &decal.receivers {
            DecalReceivers::Exclude(list) | DecalReceivers::Only(list) => {
                receiver_ids.extend_from_slice(list);
            }
            DecalReceivers::Unskinned => {}
        }
    }
    (instances, receiver_ids, batches)
}

/// The cosines of a decal's angle fade `[start, end]` in degrees, `(cos end, cos start)`: the
/// shader's smoothstep runs from the first to the second.
fn angle_cosines(angle_fade: [f32; 2]) -> (f32, f32) {
    let [start, end] = angle_fade;
    (
        ludion_core::math::cos(end.to_radians()),
        ludion_core::math::cos(start.to_radians()),
    )
}

/// The GPU instance of `decal`, whose receiver list starts at `receiver_offset` of the packed ids.
fn decal_instance(
    decal: &DecalInstance,
    blend: ProjectorBlend,
    receiver_offset: u32,
) -> shader_gen::ProjectorDecal {
    let (cos_end, cos_start) = angle_cosines(decal.angle_fade);
    let mirrored = Mat3::from_mat4(decal.box_to_world).determinant() < 0.0;
    let axis = decal.box_to_world.y_axis.truncate().normalize_or_zero();
    let (mode, list): (u32, &[u32]) = match &decal.receivers {
        DecalReceivers::Exclude(list) => (0, list),
        DecalReceivers::Only(list) => (shader_gen::RECEIVERS_ONLY, list),
        DecalReceivers::Unskinned => (shader_gen::RECEIVERS_UNSKINNED, &[]),
    };
    shader_gen::ProjectorDecal {
        ids: [
            ProjectorKind::Decal as u32,
            blend as u32,
            u32::from(decal.maps.normal.is_some()) * shader_gen::LIT_NORMAL_MAP,
            0,
        ],
        box_to_world: decal.box_to_world.to_cols_array_2d(),
        world_to_box: decal.world_to_box.to_cols_array_2d(),
        color: decal.color,
        uv_rect: decal.uv_rect,
        look: [
            decal.opacity,
            cos_end,
            cos_start,
            if mirrored { -1.0 } else { 1.0 },
        ],
        axis: [axis.x, axis.y, axis.z, 0.0],
        receivers: [mode, receiver_offset, list.len() as u32, 0],
        surface: [
            decal.roughness.unwrap_or(-1.0),
            decal.metallic,
            decal.maps.normal_scale,
            0.0,
        ],
    }
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

    fn decal(blend: DecalBlend) -> DecalInstance {
        DecalInstance {
            box_to_world: Mat4::IDENTITY,
            world_to_box: Mat4::IDENTITY,
            blend,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: crate::scene::DecalMaps::default(),
            receivers: DecalReceivers::Exclude(Vec::new()),
        }
    }

    fn mapped(blend: DecalBlend, hash: u64, wrap: TextureWrap) -> DecalInstance {
        let image = crate::TextureImage {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
            hash,
            filter_linear: false,
            premultiplied_on_load: false,
            mean: [1.0; 4],
        };
        let mut d = decal(blend);
        d.maps.base_color = Some(crate::scene::MeshTexture {
            image: crate::scene::MapImage::Source(std::sync::Arc::new(image)),
            wrap,
        });
        d
    }

    #[test]
    fn runs_batch_in_order_and_never_merge_across_a_different_blend() {
        let decals = [
            decal(DecalBlend::Multiply),
            decal(DecalBlend::Multiply),
            decal(DecalBlend::Add),
            decal(DecalBlend::Multiply),
        ];
        let (instances, _, batches) = pack_decals(&decals, DecalPass::Projector);
        assert_eq!(instances.len(), 4);
        let shape: Vec<_> = batches.iter().map(|b| (b.blend, b.range.clone())).collect();
        assert_eq!(
            shape,
            [
                (ProjectorBlend::Multiply, 0..2),
                (ProjectorBlend::Add, 2..3),
                (ProjectorBlend::Multiply, 3..4),
            ]
        );
    }

    #[test]
    fn a_different_map_or_wrap_starts_a_new_batch() {
        let decals = [
            mapped(DecalBlend::Multiply, 7, TextureWrap::Clamp),
            mapped(DecalBlend::Multiply, 7, TextureWrap::Clamp),
            mapped(DecalBlend::Multiply, 7, TextureWrap::Repeat),
            mapped(DecalBlend::Multiply, 8, TextureWrap::Repeat),
            decal(DecalBlend::Multiply),
        ];
        let (_, _, batches) = pack_decals(&decals, DecalPass::Projector);
        let shape: Vec<_> = batches
            .iter()
            .map(|b| (b.map, b.range.clone(), b.first))
            .collect();
        assert_eq!(
            shape,
            [
                (Some((7, TextureWrap::Clamp)), 0..2, 0),
                (Some((7, TextureWrap::Repeat)), 2..3, 2),
                (Some((8, TextureWrap::Repeat)), 3..4, 3),
                (None, 4..5, 4),
            ]
        );
    }

    #[test]
    fn the_projector_packs_only_multiply_and_add() {
        let decals = [
            decal(DecalBlend::Alpha),
            decal(DecalBlend::Multiply),
            decal(DecalBlend::Coat),
            decal(DecalBlend::Add),
        ];
        let (instances, _, batches) = pack_decals(&decals, DecalPass::Projector);
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].ids, [1, 0, 0, 0]);
        assert_eq!(instances[1].ids, [1, 1, 0, 0]);
        // The multiply and the add are not adjacent in the input but are in the packed list.
        assert_eq!(batches.len(), 2);
        assert!(
            pack_decals(&[decal(DecalBlend::Alpha)], DecalPass::Projector)
                .2
                .is_empty()
        );
    }

    #[test]
    fn the_lit_pass_packs_only_alpha_and_coat_in_order() {
        let decals = [
            decal(DecalBlend::Alpha),
            decal(DecalBlend::Multiply),
            decal(DecalBlend::Coat),
            decal(DecalBlend::Add),
            decal(DecalBlend::Alpha),
        ];
        let (instances, _, batches) = pack_decals(&decals, DecalPass::Lit);
        let blends: Vec<u32> = instances.iter().map(|i| i.ids[1]).collect();
        assert_eq!(blends, [2, 3, 2]);
        let shape: Vec<_> = batches.iter().map(|b| (b.blend, b.range.clone())).collect();
        assert_eq!(
            shape,
            [
                (ProjectorBlend::Alpha, 0..1),
                (ProjectorBlend::Coat, 1..2),
                (ProjectorBlend::Alpha, 2..3),
            ]
        );
        assert_eq!(batches[2].first, 4);
        assert!(
            pack_decals(&[decal(DecalBlend::Multiply)], DecalPass::Lit)
                .2
                .is_empty()
        );
    }

    fn with_maps(
        normal: Option<(u64, TextureWrap)>,
        mr: Option<(u64, TextureWrap)>,
    ) -> DecalInstance {
        let map = |(hash, wrap)| mapped(DecalBlend::Alpha, hash, wrap).maps.base_color;
        let mut d = decal(DecalBlend::Alpha);
        d.maps.normal = normal.and_then(map);
        d.maps.metallic_roughness = mr.and_then(map);
        d
    }

    #[test]
    fn a_lit_batch_splits_on_the_normal_and_metallic_roughness_identity() {
        let clamp = TextureWrap::Clamp;
        let decals = [
            with_maps(Some((1, clamp)), None),
            with_maps(Some((1, clamp)), None),
            with_maps(Some((2, clamp)), None),
            with_maps(Some((2, clamp)), Some((3, clamp))),
            with_maps(Some((2, clamp)), Some((3, TextureWrap::Repeat))),
        ];
        let (_, _, batches) = pack_decals(&decals, DecalPass::Lit);
        let ranges: Vec<_> = batches.iter().map(|b| b.range.clone()).collect();
        assert_eq!(ranges, [0..2, 2..3, 3..4, 4..5]);
        // The projector pass ignores both maps: it would not split here.
        let (_, _, unlit) = pack_decals(
            &decals.clone().map(|mut d| {
                d.blend = DecalBlend::Multiply;
                d
            }),
            DecalPass::Projector,
        );
        assert_eq!(unlit.len(), 1);
    }

    #[test]
    fn the_surface_lane_carries_the_roughness_metallic_and_normal_scale() {
        let mut painted = decal(DecalBlend::Alpha);
        painted.roughness = Some(0.3);
        painted.metallic = 0.8;
        painted.maps.normal_scale = 2.0;
        let own = decal(DecalBlend::Alpha);
        let (instances, _, _) = pack_decals(&[painted, own], DecalPass::Lit);
        assert_eq!(instances[0].surface, [0.3, 0.8, 2.0, 0.0]);
        assert_eq!(instances[1].surface[0], -1.0);
    }

    #[test]
    fn the_normal_map_flag_is_bit_zero_of_the_map_word() {
        let clamp = TextureWrap::Clamp;
        let (instances, _, _) = pack_decals(
            &[
                with_maps(Some((1, clamp)), None),
                with_maps(None, Some((3, clamp))),
            ],
            DecalPass::Lit,
        );
        assert_eq!(instances[0].ids[2], 1);
        assert_eq!(instances[1].ids[2], 0);
    }

    #[test]
    fn receiver_slices_carry_their_mode_offset_and_length() {
        let mut a = decal(DecalBlend::Multiply);
        a.receivers = DecalReceivers::Exclude(vec![2, 5]);
        let b = decal(DecalBlend::Multiply);
        let mut c = decal(DecalBlend::Multiply);
        c.receivers = DecalReceivers::Only(vec![1, 3, 9]);
        let (instances, ids, _) = pack_decals(&[a, b, c], DecalPass::Projector);
        assert_eq!(ids, [2, 5, 1, 3, 9]);
        assert_eq!(instances[0].receivers, [0, 0, 2, 0]);
        assert_eq!(instances[1].receivers, [0, 2, 0, 0]);
        assert_eq!(instances[2].receivers, [1, 2, 3, 0]);
    }

    #[test]
    fn decal_stats_count_each_mode_the_draws_and_the_receiver_union() {
        let mut multiply = decal(DecalBlend::Multiply);
        multiply.receivers = DecalReceivers::Only(vec![1, 4]);
        let mut add = decal(DecalBlend::Add);
        add.receivers = DecalReceivers::Exclude(vec![4, 6]);
        let mut scene = RenderScene::empty((64, 64));
        scene.decals = vec![
            decal(DecalBlend::Alpha),
            multiply.clone(),
            multiply,
            add,
            decal(DecalBlend::Coat),
        ];
        assert_eq!(
            decal_stats(&scene),
            DecalStats {
                alpha: 1,
                multiply: 2,
                add: 1,
                coat: 1,
                // Two projector batches, and a base and a finish for each of the two lit decals.
                draws: 6,
                receiver_meshes: 3,
            }
        );
        assert_eq!(
            decal_stats(&RenderScene::empty((64, 64))),
            DecalStats::default()
        );
    }

    #[test]
    fn the_union_is_ascending_and_deduplicated() {
        let mut a = decal(DecalBlend::Multiply);
        a.receivers = DecalReceivers::Exclude(vec![2, 5]);
        let mut b = decal(DecalBlend::Add);
        b.receivers = DecalReceivers::Only(vec![1, 5, 9]);
        let around = |decals: Vec<DecalInstance>| {
            let mut scene = RenderScene::empty((64, 64));
            scene.decals = decals;
            scene
        };
        assert_eq!(receiver_union(&around(vec![a, b])), [1, 2, 5, 9]);
        assert!(receiver_union(&around(vec![decal(DecalBlend::Multiply)])).is_empty());
        assert!(receiver_union(&around(Vec::new())).is_empty());
    }

    fn mesh(bone_offset: u32) -> crate::scene::MeshInstance {
        crate::scene::MeshInstance {
            porosity: crate::DEFAULT_POROSITY,
            medium: 1.0,
            shape: crate::scene::MeshShape::Cube,
            model: Mat4::IDENTITY,
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
            bone_offset,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
        }
    }

    #[test]
    fn an_all_decal_packs_no_ids_and_redraws_no_skinned_mesh() {
        for skinned in [1usize, 50] {
            let mut scene = RenderScene::empty((64, 64));
            scene.meshes = (0..skinned + 3)
                .map(|i| mesh(if i >= 3 { 1 } else { 0 }))
                .collect();
            let mut only = decal(DecalBlend::Multiply);
            only.receivers = DecalReceivers::Only(vec![0]);
            let mut all = decal(DecalBlend::Multiply);
            all.receivers = DecalReceivers::Unskinned;
            scene.decals = vec![all.clone(), all.clone(), only, all];
            let (instances, ids, _) = pack_decals(&scene.decals, DecalPass::Projector);
            assert_eq!(ids, [0], "{skinned} skinned: only the Only list packs ids");
            for index in [0, 1, 3] {
                assert_eq!(
                    instances[index].receivers,
                    [2, if index == 3 { 1 } else { 0 }, 0, 0]
                );
            }
            // Only the Only list's mesh is redrawn: the scene pass flags the skinned ones.
            assert_eq!(receiver_union(&scene), [0]);
            assert_eq!(decal_stats(&scene).receiver_meshes, 1);
        }
    }

    #[test]
    fn only_ranges_keep_exactly_the_listed_sources() {
        assert_eq!(only_ranges(&[2, 5, 9], &[]), Vec::<Range<u32>>::new());
        assert_eq!(only_ranges(&[2, 5, 9], &[2, 5, 9]), vec![0..3]);
        assert_eq!(only_ranges(&[2, 5, 9, 11, 12], &[5, 11]), [1..2, 3..4]);
        assert_eq!(only_ranges(&[4, 5, 6, 7], &[0, 5, 6, 20]), vec![1..3]);
        assert_eq!(only_ranges(&[], &[1]), Vec::<Range<u32>>::new());
        for sources in [&[2u32, 5, 9, 11][..], &[1, 2, 3]] {
            let list = [2, 9];
            let kept = kept_ranges(sources, &list);
            let only = only_ranges(sources, &list);
            let count = |r: &[Range<u32>]| r.iter().map(|r| r.len()).sum::<usize>();
            assert_eq!(count(&kept) + count(&only), sources.len());
        }
    }

    #[test]
    fn a_mirrored_box_stores_the_sign_and_the_axis_is_normalized() {
        let mut d = decal(DecalBlend::Multiply);
        d.box_to_world = Mat4::from_scale(Vec3::new(-2.0, 3.0, 2.0));
        let instance = pack_decals(&[d], DecalPass::Projector).0.remove(0);
        assert_eq!(instance.look[3], -1.0);
        assert_eq!(instance.axis, [0.0, 1.0, 0.0, 0.0]);
        let plain = pack_decals(&[decal(DecalBlend::Multiply)], DecalPass::Projector)
            .0
            .remove(0);
        assert_eq!(plain.look[3], 1.0);
    }

    #[test]
    fn the_angle_cosines_are_baked_end_first() {
        let mut d = decal(DecalBlend::Multiply);
        d.angle_fade = [60.0, 90.0];
        let look = pack_decals(&[d], DecalPass::Projector).0[0].look;
        assert!((look[1] - 0.0).abs() < 1e-6, "cos 90 = {}", look[1]);
        assert!((look[2] - 0.5).abs() < 1e-6, "cos 60 = {}", look[2]);
        // A step: both cosines are bit-equal, which is what the shader tests for.
        let mut step = decal(DecalBlend::Multiply);
        step.angle_fade = [45.0, 45.0];
        let look = pack_decals(&[step], DecalPass::Projector).0[0].look;
        assert_eq!(look[1], look[2]);
    }
}
