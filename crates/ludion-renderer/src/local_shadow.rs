//! Local-light shadow planning: which shadow-casting point and spot lights get shadow views this
//! frame, the view matrices they render with, and where each lands in the shared atlas.
//!
//! Pure CPU and deterministic. The plan is a function of the scene alone: candidates are the lights
//! with shadow tunables whose footprint (the cluster binner's own sphere) touches the camera
//! frustum, ranked nearest-first, then granted greedily into a fixed 8×8-cell atlas grid.
//!
//! # Ranking and ties
//!
//! A candidate's rank key is `(max(0, |camera − footprint centre| − footprint radius), light-list
//! index)`, compared with `f32::total_cmp`. The light-list index is the order of
//! [`RenderScene::lights`] followed by [`RenderScene::spot_lights`] (the order `cluster.rs` packs
//! them in); the scene carries no entity id, so an exact tie resolves to the lower index — extract
//! order within a kind, points first across kinds.
//!
//! # Atlas layout
//!
//! The atlas is `ATLAS_CELLS × ATLAS_CELLS` square cells of `atlas_edge / ATLAS_CELLS` texels. A
//! spot view takes an aligned 2×2 block of cells, a cube face one cell, so 16 spot tiles fit at any
//! quality. A light is granted only if all its views fit; otherwise it is skipped and the next
//! candidate is tried. An `atlas_edge` of 0, or one not divisible by 16, grants nothing.
//!
//! # Cube face contract
//!
//! A point light, and a spot wider than [`WIDE_SPOT_DEG`], renders six 90° views in this fixed order;
//! the sampling shader picks the face by the major axis of the light-to-fragment vector and must
//! mirror the table:
//!
//! | face | direction | up |
//! |------|-----------|----|
//! | 0 | +X | −Y |
//! | 1 | −X | −Y |
//! | 2 | +Y | +Z |
//! | 3 | −Y | −Z |
//! | 4 | +Z | −Y |
//! | 5 | −Z | −Y |
//!
//! Views are right-handed `look_to_rh` from the light position, clip depth is wgpu's `[0, 1]`, and
//! `light_space = projection * view` like the sun's.

use ludion_core::math::glam::{Mat4, Vec3};

use crate::cluster::light_sphere;
use crate::cull::{CullVolume, Frustum};
use crate::scene::{DebugSegment, LocalShadowData, RenderScene};

/// Cells per atlas side.
pub(crate) const ATLAS_CELLS: u32 = 8;
/// A spot whose outer half-angle exceeds this many degrees takes the six-face path.
pub(crate) const WIDE_SPOT_DEG: f32 = 60.0;
/// Extra half-angle (per side) a spot's shadow frustum covers beyond its outer cone.
const SPOT_FOV_MARGIN_DEG: f32 = 1.0;

/// Light-first-view value of a light with no shadow views this frame.
pub(crate) const UNSHADOWED: u32 = u32::MAX;

/// A square region of the atlas, in texels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AtlasRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) size: u32,
}

/// One shadow view to render.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct LocalShadowView {
    /// Index into the light list (points, then spots).
    pub(crate) light: u32,
    /// 0 for a spot; 0..6 for a cube face (table in the module docs).
    pub(crate) face: u8,
    pub(crate) light_space: Mat4,
    pub(crate) rect: AtlasRect,
    /// Depth bias in world units, from the light's tunables.
    pub(crate) bias: f32,
    /// Near and far planes of the projection, for linearising the depth tile.
    pub(crate) near: f32,
    pub(crate) far: f32,
    pub(crate) cull: CullVolume,
}

/// What a planning pass decided, for the profile report.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LocalShadowStats {
    /// Casting lights whose footprint touches the camera frustum.
    pub requested: u32,
    /// Casting lights granted a set of atlas tiles.
    pub granted: u32,
    /// `requested − granted`: candidates that did not fit the atlas.
    pub skipped: u32,
    /// Shadow views drawn: a spot takes one, a point light (or a wide spot) six.
    pub views: u32,
    /// The atlas edge in texels, 0 when no view was granted.
    pub atlas_edge: u32,
}

/// The frame's local-shadow views and the per-light lookup.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct LocalShadowPlan {
    /// In grant order; a cube light's six faces are consecutive.
    pub(crate) views: Vec<LocalShadowView>,
    /// Per light-list entry, the index of its first view in `views`, or [`UNSHADOWED`].
    pub(crate) light_first_view: Vec<u32>,
    /// Light-list indices of the candidates that were not granted, in rank order.
    pub(crate) skipped: Vec<u32>,
    pub(crate) stats: LocalShadowStats,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Spot,
    Cube,
}

struct Candidate {
    index: u32,
    kind: Kind,
    position: Vec3,
    direction: Vec3,
    range: f32,
    outer: f32,
    shadow: LocalShadowData,
    rank: f32,
}

/// The cube-face table: direction and up for faces 0..6.
const CUBE_FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::NEG_Y),
    (Vec3::NEG_X, Vec3::NEG_Y),
    (Vec3::Y, Vec3::Z),
    (Vec3::NEG_Y, Vec3::NEG_Z),
    (Vec3::Z, Vec3::NEG_Y),
    (Vec3::NEG_Z, Vec3::NEG_Y),
];

/// Plans the frame's local shadows and returns only what was decided — the profile panel, with the
/// views themselves dropped.
///
/// This is the profile driver's entry point (`ludion_engine::profile`): it does exactly the planning
/// the renderer's per-frame prologue does, and needs no GPU adapter — it is pure CPU arithmetic over
/// the extracted scene. Deliberately a separate call rather than a hook inside
/// [`Renderer::render`](crate::Renderer::render): a profiler must never make the path it measures pay
/// for being measured.
pub fn local_shadow_stats(scene: &RenderScene, atlas_edge: u32) -> LocalShadowStats {
    plan_local_shadows(scene, atlas_edge).stats
}

const GRANTED: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
const DENIED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
const MARK_HALF: f32 = 0.25;
const MARK_AIM_MAX: f32 = 1.5;

/// Marks every requested shadowed light in world space: a cross at its position, green when the
/// frame granted it tiles and red when it was skipped, plus a spot's aim line.
///
/// Plans exactly as [`local_shadow_stats`] does, at `atlas_edge`, so the marks agree with the profile
/// panel. It is the source of the `local_shadows` debug-draw category: grants depend on the camera,
/// so the engine appends these to the extracted scene's lines rather than drawing them from the world.
pub fn local_shadow_marks(scene: &RenderScene, atlas_edge: u32) -> Vec<DebugSegment> {
    let plan = plan_local_shadows(scene, atlas_edge);
    let point_count = scene.lights.len();
    let granted = (0..plan.light_first_view.len() as u32)
        .filter(|&i| plan.light_first_view[i as usize] != UNSHADOWED)
        .map(|i| (i, GRANTED));
    let skipped = plan.skipped.iter().map(|&i| (i, DENIED));
    let mut out = Vec::new();
    for (index, color) in granted.chain(skipped) {
        let index = index as usize;
        let (position, aim) = match index.checked_sub(point_count) {
            None => (scene.lights[index].position, None),
            Some(s) => {
                let l = &scene.spot_lights[s];
                (l.position, Some((l.direction, l.range)))
            }
        };
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            out.push(DebugSegment {
                a: (position - axis * MARK_HALF).to_array(),
                b: (position + axis * MARK_HALF).to_array(),
                color,
            });
        }
        if let Some((direction, range)) = aim {
            out.push(DebugSegment {
                a: position.to_array(),
                b: (position + direction * range.min(MARK_AIM_MAX)).to_array(),
                color,
            });
        }
    }
    out
}

/// Selects, ranks and lays out the frame's local shadow views.
pub(crate) fn plan_local_shadows(scene: &RenderScene, atlas_edge: u32) -> LocalShadowPlan {
    let light_count = scene.lights.len() + scene.spot_lights.len();
    let mut plan = LocalShadowPlan {
        views: Vec::new(),
        light_first_view: vec![UNSHADOWED; light_count],
        skipped: Vec::new(),
        stats: LocalShadowStats::default(),
    };
    let frustum = Frustum::from_view_proj(scene.camera.projection * scene.camera.view);
    let camera = scene.camera.position;
    let point_count = scene.lights.len();

    let mut candidates = Vec::new();
    let mut consider = |index: usize,
                        kind: Kind,
                        position,
                        direction: Vec3,
                        range: f32,
                        outer,
                        cos_outer,
                        shadow| {
        let Some(shadow) = shadow else { return };
        let (center, radius) = light_sphere(position, direction, range, cos_outer);
        let usable = Vec3::is_finite(position)
            && center.is_finite()
            && radius.is_finite()
            && range.is_finite()
            && shadow_near_fits(&shadow, range)
            && (kind == Kind::Cube || direction.is_finite());
        if !usable || !frustum.intersects_sphere(center, radius) {
            return;
        }
        candidates.push(Candidate {
            index: index as u32,
            kind,
            position,
            direction,
            range,
            outer,
            shadow,
            rank: (camera.distance(center) - radius).max(0.0),
        });
    };
    for (i, l) in scene.lights.iter().enumerate() {
        consider(
            i,
            Kind::Cube,
            l.position,
            Vec3::ZERO,
            l.range,
            0.0,
            -2.0,
            l.shadow,
        );
    }
    for (i, l) in scene.spot_lights.iter().enumerate() {
        let outer = l.cos_outer.clamp(-1.0, 1.0).acos();
        let kind = if outer > WIDE_SPOT_DEG.to_radians() {
            Kind::Cube
        } else {
            Kind::Spot
        };
        consider(
            point_count + i,
            kind,
            l.position,
            l.direction,
            l.range,
            outer,
            l.cos_outer,
            l.shadow,
        );
    }
    candidates.sort_by(|a, b| a.rank.total_cmp(&b.rank).then(a.index.cmp(&b.index)));
    plan.stats.requested = candidates.len() as u32;

    let budget = atlas_edge > 0 && atlas_edge.is_multiple_of(2 * ATLAS_CELLS);
    let cell = atlas_edge / ATLAS_CELLS;
    let mut atlas = Atlas::default();
    for c in &candidates {
        if !budget {
            plan.skipped.push(c.index);
            continue;
        }
        let tentative = atlas;
        let first = plan.views.len() as u32;
        let granted = match c.kind {
            Kind::Spot => atlas.spot_block().map(|(cx, cy)| {
                plan.views.push(spot_view(c, rect(cx, cy, cell, 2)));
            }),
            Kind::Cube => (0..CUBE_FACES.len() as u8).try_for_each(|face| {
                let (cx, cy) = atlas.cube_cell()?;
                plan.views.push(cube_view(c, face, rect(cx, cy, cell, 1)));
                Some(())
            }),
        };
        if granted.is_some() {
            plan.light_first_view[c.index as usize] = first;
            plan.stats.granted += 1;
        } else {
            atlas = tentative;
            plan.views.truncate(first as usize);
            plan.skipped.push(c.index);
        }
    }
    plan.stats.skipped = plan.stats.requested - plan.stats.granted;
    plan.stats.views = plan.views.len() as u32;
    plan.stats.atlas_edge = if plan.views.is_empty() { 0 } else { atlas_edge };
    plan
}

/// The shadow view needs `0 < near < range` to form a projection.
fn shadow_near_fits(shadow: &LocalShadowData, range: f32) -> bool {
    shadow.near.is_finite() && shadow.near > 0.0 && shadow.near < range
}

fn rect(cx: u32, cy: u32, cell: u32, cells: u32) -> AtlasRect {
    AtlasRect {
        x: cx * cell,
        y: cy * cell,
        size: cells * cell,
    }
}

fn spot_view(c: &Candidate, rect: AtlasRect) -> LocalShadowView {
    let direction = c.direction.normalize_or_zero();
    let up = if direction.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = Mat4::look_to_rh(c.position, direction, up);
    let fov = 2.0 * (c.outer + SPOT_FOV_MARGIN_DEG.to_radians());
    let projection = Mat4::perspective_rh(fov, 1.0, c.shadow.near, c.range);
    let light_space = projection * view;
    LocalShadowView {
        light: c.index,
        face: 0,
        light_space,
        rect,
        bias: c.shadow.bias,
        near: c.shadow.near,
        far: c.range,
        cull: CullVolume::Frustum(Frustum::from_view_proj(light_space)),
    }
}

fn cube_view(c: &Candidate, face: u8, rect: AtlasRect) -> LocalShadowView {
    let (direction, up) = CUBE_FACES[face as usize];
    let view = Mat4::look_to_rh(c.position, direction, up);
    let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, c.shadow.near, c.range);
    LocalShadowView {
        light: c.index,
        face,
        light_space: projection * view,
        rect,
        bias: c.shadow.bias,
        near: c.shadow.near,
        far: c.range,
        cull: CullVolume::Sphere {
            center: c.position,
            radius: c.range,
        },
    }
}

/// Cell occupancy of the atlas grid. Spots take whole aligned 2×2 blocks; cube faces fill a
/// partially used block before opening a fresh one, so at most one block is ever partial and the
/// grid never strands a spot that the free-cell count admits.
#[derive(Clone, Copy)]
struct Atlas {
    used: [bool; (ATLAS_CELLS * ATLAS_CELLS) as usize],
}

impl Default for Atlas {
    fn default() -> Self {
        Atlas {
            used: [false; (ATLAS_CELLS * ATLAS_CELLS) as usize],
        }
    }
}

impl Atlas {
    fn at(&mut self, x: u32, y: u32) -> &mut bool {
        &mut self.used[(y * ATLAS_CELLS + x) as usize]
    }

    /// The cells of block `(bx, by)` in row-major order.
    fn block_cells(bx: u32, by: u32) -> [(u32, u32); 4] {
        let (x, y) = (bx * 2, by * 2);
        [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
    }

    fn blocks() -> impl Iterator<Item = (u32, u32)> {
        (0..ATLAS_CELLS / 2).flat_map(|by| (0..ATLAS_CELLS / 2).map(move |bx| (bx, by)))
    }

    /// Claims the first free aligned 2×2 block, returning its first cell.
    fn spot_block(&mut self) -> Option<(u32, u32)> {
        let (bx, by) = Self::blocks().find(|&(bx, by)| {
            Self::block_cells(bx, by)
                .iter()
                .all(|&(x, y)| !*self.at(x, y))
        })?;
        for (x, y) in Self::block_cells(bx, by) {
            *self.at(x, y) = true;
        }
        Some((bx * 2, by * 2))
    }

    /// Claims one cell: the first free one in a partially used block, else the first cell of the
    /// first fully free block.
    fn cube_cell(&mut self) -> Option<(u32, u32)> {
        let mut fresh = None;
        for (bx, by) in Self::blocks() {
            let cells = Self::block_cells(bx, by);
            let taken = cells.iter().filter(|&&(x, y)| *self.at(x, y)).count();
            if taken == 0 {
                fresh = fresh.or(Some(cells[0]));
            } else if taken < 4 {
                let (x, y) = cells.into_iter().find(|&(x, y)| !*self.at(x, y))?;
                *self.at(x, y) = true;
                return Some((x, y));
            }
        }
        let (x, y) = fresh?;
        *self.at(x, y) = true;
        Some((x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{PointLightData, RenderCamera, SpotLightData, aspect_ratio};

    const EXTENT: (u32, u32) = (1920, 1080);

    fn scene_at_origin() -> RenderScene {
        let mut scene = RenderScene::empty(EXTENT);
        scene.camera = RenderCamera {
            view: Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y),
            projection: Mat4::perspective_rh(
                60.0_f32.to_radians(),
                aspect_ratio(EXTENT),
                0.1,
                1000.0,
            ),
            position: Vec3::ZERO,
        };
        scene
    }

    fn shadow() -> Option<LocalShadowData> {
        Some(LocalShadowData {
            bias: 0.05,
            near: 0.1,
        })
    }

    fn point(position: Vec3, range: f32, shadow: Option<LocalShadowData>) -> PointLightData {
        PointLightData {
            position,
            color: Vec3::ONE,
            intensity: 1.0,
            range,
            dynamic: false,
            shadow,
        }
    }

    fn spot(position: Vec3, range: f32, outer_deg: f32) -> SpotLightData {
        SpotLightData {
            position,
            direction: -Vec3::Z,
            color: Vec3::ONE,
            intensity: 1.0,
            range,
            cos_inner: (outer_deg * 0.5).to_radians().cos(),
            cos_outer: outer_deg.to_radians().cos(),
            shadow: shadow(),
        }
    }

    fn point_row(n: usize) -> Vec<PointLightData> {
        (0..n)
            .map(|i| point(Vec3::new(0.0, 0.0, -5.0 - i as f32), 3.0, shadow()))
            .collect()
    }

    fn spot_row(n: usize) -> Vec<SpotLightData> {
        (0..n)
            .map(|i| spot(Vec3::new(0.0, 0.0, -5.0 - i as f32), 3.0, 30.0))
            .collect()
    }

    #[test]
    fn the_same_scene_plans_identically() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(4);
        scene.spot_lights = spot_row(5);
        assert_eq!(
            plan_local_shadows(&scene, 2048),
            plan_local_shadows(&scene, 2048)
        );
    }

    #[test]
    fn equal_distance_ties_go_to_the_lower_index() {
        let mut scene = scene_at_origin();
        scene.lights = vec![
            point(Vec3::new(-3.0, 0.0, -10.0), 2.0, shadow()),
            point(Vec3::new(3.0, 0.0, -10.0), 2.0, shadow()),
        ];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.light_first_view, vec![0, 6]);
    }

    #[test]
    fn the_stats_entry_matches_the_plan_and_a_zero_edge_grants_nothing() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(2);
        scene.spot_lights = spot_row(2);
        assert_eq!(
            local_shadow_stats(&scene, 4096),
            plan_local_shadows(&scene, 4096).stats
        );
        let none = local_shadow_stats(&scene, 0);
        assert_eq!(none.requested, 4);
        assert_eq!(none.granted, 0);
        assert_eq!(none.skipped, none.requested);
    }

    #[test]
    fn marks_are_green_for_grants_red_for_skips_and_agree_with_the_stats() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(11);
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.skipped.len() as u32, plan.stats.skipped);
        assert_eq!(plan.stats.requested, 11);
        assert_eq!(plan.stats.granted + plan.stats.skipped, 11);
        let marks = local_shadow_marks(&scene, 4096);
        assert_eq!(marks.len(), 33);
        let green = marks.iter().filter(|m| m.color == GRANTED).count();
        assert_eq!(green as u32, plan.stats.granted * 3);
        assert_eq!(marks.len() - green, plan.skipped.len() * 3);
    }

    #[test]
    fn a_spot_mark_adds_an_aim_line_and_edge_zero_marks_everything_red() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(1);
        scene.spot_lights = spot_row(1);
        let marks = local_shadow_marks(&scene, 0);
        assert_eq!(marks.len(), 3 + 4);
        assert!(marks.iter().all(|m| m.color == DENIED));
        let granted = local_shadow_marks(&scene, 4096);
        assert_eq!(granted.len(), 3 + 4);
        assert!(granted.iter().all(|m| m.color == GRANTED));
    }

    #[test]
    fn a_light_outside_the_frustum_is_not_requested() {
        let mut scene = scene_at_origin();
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, 50.0), 2.0, shadow()),
            point(Vec3::new(0.0, 0.0, -10.0), 2.0, shadow()),
        ];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.stats.requested, 1);
        assert_eq!(plan.light_first_view, vec![UNSHADOWED, 0]);
    }

    #[test]
    fn lights_without_shadow_tunables_stay_unshadowed() {
        let mut scene = scene_at_origin();
        scene.lights = vec![point(Vec3::new(0.0, 0.0, -10.0), 2.0, None)];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.light_first_view, vec![UNSHADOWED]);
        assert_eq!(plan.stats, LocalShadowStats::default());
    }

    #[test]
    fn a_camera_inside_a_footprint_ranks_first() {
        let mut scene = scene_at_origin();
        scene.lights = vec![
            point(Vec3::new(0.0, 0.0, -10.0), 2.0, shadow()),
            point(Vec3::new(0.0, 0.0, -1.0), 5.0, shadow()),
        ];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.light_first_view, vec![6, 0]);
    }

    #[test]
    fn sixteen_spots_fit_and_the_seventeenth_is_skipped() {
        let mut scene = scene_at_origin();
        scene.spot_lights = spot_row(17);
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.stats.requested, 17);
        assert_eq!(plan.stats.granted, 16);
        assert_eq!(plan.stats.skipped, 1);
        assert_eq!(plan.light_first_view[16], UNSHADOWED);
        assert!(plan.light_first_view[..16].iter().all(|&v| v != UNSHADOWED));
    }

    #[test]
    fn a_cube_that_does_not_fit_is_skipped_for_a_later_spot() {
        let mut scene = scene_at_origin();
        scene.spot_lights = spot_row(15);
        // Farther than every spot: the point ranks 16th and finds 4 free cells, then a spot behind it.
        scene.lights = vec![point(Vec3::new(0.0, 0.0, -30.0), 3.0, shadow())];
        scene
            .spot_lights
            .push(spot(Vec3::new(0.0, 0.0, -40.0), 3.0, 30.0));
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.light_first_view[0], UNSHADOWED);
        assert_ne!(plan.light_first_view[16], UNSHADOWED);
        assert_eq!(plan.stats.granted, 16);
        assert_eq!(plan.stats.skipped, 1);
        assert_eq!(plan.stats.views, 16);
    }

    #[test]
    fn ten_cubes_fit_and_the_eleventh_is_skipped() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(11);
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.stats.granted, 10);
        assert_eq!(plan.stats.views, 60);
        assert_eq!(plan.light_first_view[10], UNSHADOWED);
    }

    #[test]
    fn rects_are_disjoint_and_inside_the_atlas() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(5);
        scene.spot_lights = spot_row(8);
        for edge in [2048, 4096] {
            let plan = plan_local_shadows(&scene, edge);
            assert!(plan.views.len() > 20);
            for (i, a) in plan.views.iter().enumerate() {
                let r = a.rect;
                assert!(r.x + r.size <= edge && r.y + r.size <= edge);
                for b in &plan.views[i + 1..] {
                    let s = b.rect;
                    let apart = r.x + r.size <= s.x
                        || s.x + s.size <= r.x
                        || r.y + r.size <= s.y
                        || s.y + s.size <= r.y;
                    assert!(apart, "{r:?} overlaps {s:?}");
                }
            }
        }
    }

    #[test]
    fn the_atlas_edge_scales_rects_not_grants() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(3);
        scene.spot_lights = spot_row(6);
        let big = plan_local_shadows(&scene, 4096);
        let small = plan_local_shadows(&scene, 2048);
        assert_eq!(big.light_first_view, small.light_first_view);
        assert_eq!(big.stats.atlas_edge, 4096);
        assert_eq!(small.stats.atlas_edge, 2048);
        for (b, s) in big.views.iter().zip(&small.views) {
            assert_eq!(b.rect.x, s.rect.x * 2);
            assert_eq!(b.rect.y, s.rect.y * 2);
            assert_eq!(b.rect.size, s.rect.size * 2);
        }
    }

    #[test]
    fn no_budget_grants_nothing() {
        let mut scene = scene_at_origin();
        scene.lights = point_row(2);
        for edge in [0, 100] {
            let plan = plan_local_shadows(&scene, edge);
            assert_eq!(plan.stats.requested, 2);
            assert_eq!(plan.stats.granted, 0);
            assert_eq!(plan.stats.skipped, 2);
            assert_eq!(plan.stats.atlas_edge, 0);
            assert!(plan.views.is_empty());
        }
    }

    #[test]
    fn a_spot_view_projects_its_axis_to_the_centre() {
        let mut scene = scene_at_origin();
        let mut s = spot(Vec3::new(0.0, 0.0, -2.0), 10.0, 30.0);
        s.shadow = Some(LocalShadowData {
            bias: 0.0,
            near: 0.5,
        });
        scene.spot_lights = vec![s];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.views.len(), 1);
        let m = plan.views[0].light_space;
        let mid = m.project_point3(Vec3::new(0.0, 0.0, -7.0));
        assert!(mid.x.abs() < 1e-5 && mid.y.abs() < 1e-5);
        assert!(mid.z > 0.0 && mid.z < 1.0);
        let near = m.project_point3(Vec3::new(0.0, 0.0, -2.5));
        let far = m.project_point3(Vec3::new(0.0, 0.0, -12.0));
        assert!(near.z.abs() < 1e-4, "{}", near.z);
        assert!((far.z - 1.0).abs() < 1e-4, "{}", far.z);
    }

    #[test]
    fn a_vertical_spot_does_not_degenerate() {
        let mut scene = scene_at_origin();
        let mut s = spot(Vec3::new(0.0, 5.0, -5.0), 10.0, 30.0);
        s.direction = -Vec3::Y;
        scene.spot_lights = vec![s];
        let plan = plan_local_shadows(&scene, 4096);
        let p = plan.views[0]
            .light_space
            .project_point3(Vec3::new(0.0, 0.0, -5.0));
        assert!(p.is_finite() && p.x.abs() < 1e-5 && p.y.abs() < 1e-5);
    }

    #[test]
    fn each_cube_face_sees_its_axis_and_not_the_opposite() {
        let mut scene = scene_at_origin();
        let origin = Vec3::new(0.0, 0.0, -10.0);
        scene.lights = vec![point(origin, 20.0, shadow())];
        let plan = plan_local_shadows(&scene, 4096);
        assert_eq!(plan.views.len(), 6);
        for (face, (dir, _)) in CUBE_FACES.iter().enumerate() {
            let target = origin + *dir * 5.0;
            let v = &plan.views[face];
            assert_eq!(v.face as usize, face);
            let p = v.light_space.project_point3(target);
            assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4, "face {face}: {p:?}");
            assert!(p.z > 0.0 && p.z < 1.0);
            let opposite = &plan.views[face ^ 1];
            let clip = opposite.light_space * target.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(
                clip.w <= 0.0 || ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0,
                "face {face}"
            );
            assert!(matches!(v.cull, CullVolume::Sphere { radius, .. } if radius == 20.0));
        }
    }

    #[test]
    fn a_spot_over_sixty_degrees_takes_six_views() {
        let mut scene = scene_at_origin();
        scene.spot_lights = vec![spot(Vec3::new(0.0, 0.0, -5.0), 5.0, 61.0)];
        assert_eq!(plan_local_shadows(&scene, 4096).stats.views, 6);
        scene.spot_lights = vec![spot(Vec3::new(0.0, 0.0, -5.0), 5.0, 59.0)];
        assert_eq!(plan_local_shadows(&scene, 4096).stats.views, 1);
    }
}
