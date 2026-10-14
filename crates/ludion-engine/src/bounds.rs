//! `render.bounds` — pure-CPU world AABB + screen rect per renderable ("what did I draw, and where").
//!
//! This is extract + projection math, never a render: it works headless with no GPU, exactly like
//! `render.dump_graph`. It reuses [`extract`](crate::extract)'s [`place`](crate::extract::place) walk
//! for world geometry — the same code path a real render sizes things with — so a reported bound can
//! never disagree with what would be drawn (the sizing rule). On top of that geometry it computes,
//! per renderable entity: the world-space AABB, and the pixel rect that AABB projects to under the
//! current camera.
//!
//! "Headless" means no GPU, not no disk: placement reads an atlas document when a `Sprite` names a
//! region and authors no `size`, because that quad's size *is* its art's. It goes through a
//! fresh asset cache, never the host's ([`place_uncached`](crate::extract::place_uncached)) — this is a query, not
//! a per-frame path — and an atlas that will not load costs the quad its derived size, never the
//! answer: every other entity still reports.
//!
//! A document-backed mesh is sized from its real extent, loaded through the engine's
//! [`AssetCache`] (a miss loads and inserts, a hit is a map lookup, so a repeat poll is cheap and
//! the answer never depends on whether a render ran first): a voxel model, assembly or assembly
//! part from its grid, an asset, sculpt or character from its vertex extent at rest. A document
//! that will not load costs that mesh its real size and nothing else — it keeps the unit box.
//!
//! Camera rule matches a render's: exactly one [`Camera`](crate::components::Camera) is used, more
//! than one is `L2101`. The one deliberate difference — a scene with *no* camera is `L2102` here,
//! where a render falls back to a default framing — is because a screen rect projected against an
//! invented viewpoint would silently mislead; see the `L2102` explanation.

use std::path::Path;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3, Vec4};
use ludion_core::time::{SimTime, TickRate};
use ludion_ecs::World;
use ludion_protocol::command::ViewPose;
use ludion_protocol::response::{EntityBounds, ScreenRect, WorldAabb};
use ludion_renderer::{MeshShape, TextRun};

use crate::asset_cache::AssetCache;
use crate::extract::{
    PlacedMesh, PlacedMeshSource, PlacedSprite, build_camera, place_uncached, single_camera,
};
use crate::voxel_mesh::grid_box;

/// A clipped point whose homogeneous `w` is at or below this threshold is dropped from the screen
/// projection: a perspective divide by a non-positive `w` is meaningless. Near-plane clipping leaves
/// `w` at the near distance, so only a degenerate projection reaches it.
const BEHIND_CAMERA_W: f32 = 1e-6;

/// Computes the render bounds of every renderable entity in `world`, at `render_size` pixels.
///
/// Returns the render size (echoed so the rects are self-describing) and one [`EntityBounds`] per
/// renderable entity, in ascending entity-slot order (the order [`extract`](crate::extract) walks).
/// An entity carrying several renderables (say a `Mesh` and a `Sprite`) yields one entry whose AABB
/// is their union.
///
/// `project_root` roots the assets a bound reads — an atlas named by a `Sprite` that authors no
/// `size`, and each document-backed mesh, loaded through `cache` — and is the engine's own content
/// root, so a bound is sized against the files a render would sample.
///
/// `view` is the host's [`render.view`](crate::render_view) override, passed only when the request
/// asked for `camera: "view"` and one is in force. It supplies the viewpoint itself, so a world with
/// no camera answers rather than raising `L2102` — the reason that code exists (a rect projected
/// against an invented viewpoint) does not apply to a viewpoint a client named.
///
/// # Errors
/// - `L2101` if the world has more than one camera (identical to a render).
/// - `L2102` if the world has no camera and no override (a screen rect needs a real viewpoint).
pub(crate) fn compute(
    world: &World,
    render_size: (u32, u32),
    project_root: &Path,
    cache: &mut AssetCache,
    view: Option<&ViewPose>,
) -> Result<([u32; 2], Vec<EntityBounds>), Diagnostic> {
    let ids: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let camera = match (view, single_camera(world, &ids)?) {
        (Some(pose), scene) => crate::render_view::camera(world, scene, pose, render_size),
        (None, Some(id)) => build_camera(world, id, render_size),
        (None, None) => return Err(no_camera()),
    };

    // Placement at the clip's first instant, the tick-0 rule `extract` (the three-argument wrapper)
    // follows: a bound is a query with no sim time of its own, and reporting one against an invented
    // instant would be a different answer every time the same world was asked.
    let placed = place_uncached(
        world,
        &ids,
        &camera,
        project_root,
        SimTime::from_ticks(0, TickRate::DEFAULT),
    );
    let view_proj = camera.projection * camera.view;
    let (width, height) = render_size;
    let mut extents = Extents {
        root: project_root,
        cache,
        vertex_boxes: StableMap::new(),
    };

    let mut entities = Vec::new();
    for &id in &ids {
        // Union the world AABB of every renderable this entity carries (usually exactly one).
        let mut aabb: Option<Aabb> = None;
        for sprite in placed.sprites.iter().filter(|s| s.id == id) {
            aabb = Some(union(aabb, sprite_aabb(sprite)));
        }
        for mesh in placed.meshes.iter().filter(|m| m.id == id) {
            aabb = Some(union(aabb, mesh_aabb(mesh, &mut extents)));
        }
        for (_, text, _, _) in placed.texts.iter().filter(|(tid, ..)| *tid == id) {
            aabb = Some(union(aabb, text_aabb(text)));
        }

        let Some(aabb) = aabb else { continue };
        let (screen_rect, on_screen) = project(&aabb, view_proj, width, height);
        entities.push(EntityBounds {
            entity: id,
            world_aabb: WorldAabb {
                min: aabb.min.to_array(),
                max: aabb.max.to_array(),
            },
            screen_rect,
            on_screen,
        });
    }

    Ok(([width, height], entities))
}

/// The `L2102` no-camera diagnostic.
fn no_camera() -> Diagnostic {
    Diagnostic::new(
        code::RENDER_NO_CAMERA,
        "render.bounds needs a camera, but the scene has none",
    )
    .with_help("add one entity with a `Camera` component (a render falls back to a default framing, but a projected screen rect needs a real viewpoint)")
}

/// A world-space axis-aligned bounding box under construction. Shared with
/// [`voxelize`](crate::voxelizer::voxelize), which sizes the GI grid from the same union of the same
/// transformed object boxes this query reports — one sizing rule, so the grid and the reported
/// bounds can never describe different worlds.
#[derive(Clone, Copy)]
pub(crate) struct Aabb {
    pub(crate) min: Vec3,
    pub(crate) max: Vec3,
}

impl Aabb {
    /// The tight AABB of a set of world points (never called empty).
    fn of_points(points: &[Vec3]) -> Aabb {
        let mut min = points[0];
        let mut max = points[0];
        for &p in &points[1..] {
            min = min.min(p);
            max = max.max(p);
        }
        Aabb { min, max }
    }

    /// The eight corners of the box.
    fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }
}

/// Merges an optional running AABB with a new one.
pub(crate) fn union(acc: Option<Aabb>, next: Aabb) -> Aabb {
    match acc {
        None => next,
        Some(a) => Aabb {
            min: a.min.min(next.min),
            max: a.max.max(next.max),
        },
    }
}

/// A sprite/billboard quad's world AABB: the box bounding its four corners
/// (`center ± right/2 ± up/2`, with `right`/`up` the full-size vectors from [`place`]).
fn sprite_aabb(s: &PlacedSprite) -> Aabb {
    let hr = s.right * 0.5;
    let hu = s.up * 0.5;
    Aabb::of_points(&[
        s.center - hr - hu,
        s.center + hr - hu,
        s.center + hr + hu,
        s.center - hr + hu,
    ])
}

/// A mesh's world AABB: its local box, transformed by the instance's model matrix.
fn mesh_aabb(mesh: &PlacedMesh, extents: &mut Extents) -> Aabb {
    let (lo, hi) = extents.local_box(&mesh.source);
    transformed_box(mesh.model, lo, hi)
}

/// The assets a query sizes meshes from: the content root, the engine's cache, and the vertex-extent
/// boxes already worked out this query (a district repeats one kit mesh hundreds of times).
struct Extents<'a> {
    root: &'a Path,
    cache: &'a mut AssetCache,
    /// Keyed by source kind and path; a document that would not load is memoized as its fallback box.
    vertex_boxes: StableMap<String, (Vec3, Vec3)>,
}

impl Extents<'_> {
    /// The object-space AABB used for bounds. Each procedural primitive has its box (matching
    /// `ludion_renderer::mesh` — cube and sphere span ±0.5 on every axis; the plane is flat in Y,
    /// ±0.5 in X and Z). A voxel model, an assembly and a single assembly part are their grid's
    /// [`grid_box`] (bottom-centered: a model sits on the ground); an asset mesh, a sculpt and a
    /// character are the min/max of their vertices at rest. When the document does not load the
    /// answer is [`fallback_box`], because `render.bounds` must answer for a broken scene.
    fn local_box(&mut self, source: &PlacedMeshSource) -> (Vec3, Vec3) {
        let fallback = fallback_box(source);
        let loaded = match source {
            PlacedMeshSource::Primitive(MeshShape::Plane) => {
                return (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 0.0, 0.5));
            }
            PlacedMeshSource::Primitive(_) => return fallback,
            PlacedMeshSource::Voxel(rel) => self
                .cache
                .load_voxel_grid(self.root, rel)
                .ok()
                .map(|grid| grid_box(grid.extent)),
            PlacedMeshSource::Assembly(rel) => self
                .cache
                .load_assembly(self.root, rel)
                .ok()
                .map(|data| grid_box(data.size)),
            PlacedMeshSource::AssemblyPart(rel, part) => self
                .cache
                .load_assembly(self.root, rel)
                .ok()
                .and_then(|data| {
                    data.parts
                        .iter()
                        .find(|candidate| candidate.name == *part)
                        .map(|found| grid_box(found.size))
                }),
            PlacedMeshSource::Asset(rel) => return self.vertex_box('a', rel, fallback),
            PlacedMeshSource::Sculpt(rel) => return self.vertex_box('s', rel, fallback),
            PlacedMeshSource::Character(rel) => return self.vertex_box('c', rel, fallback),
        };
        loaded.unwrap_or(fallback)
    }

    /// The vertex-extent box of the mesh-valued source `rel` (`kind` tags asset / sculpt /
    /// character), memoized for the query; `fallback` when it will not load or has no vertices.
    fn vertex_box(&mut self, kind: char, rel: &str, fallback: (Vec3, Vec3)) -> (Vec3, Vec3) {
        let key = format!("{kind}{rel}");
        if let Some(found) = self.vertex_boxes.get(&key) {
            return *found;
        }
        let found = self.load_vertex_box(kind, rel).unwrap_or(fallback);
        self.vertex_boxes.insert(key, found);
        found
    }

    fn load_vertex_box(&mut self, kind: char, rel: &str) -> Option<(Vec3, Vec3)> {
        let mesh = match kind {
            'a' => Arc::clone(self.cache.load_mesh(self.root, rel).ok()?),
            's' => Arc::clone(self.cache.load_sculpt(self.root, rel).ok()?),
            _ => {
                let path = self.cache.load_character(self.root, rel).ok()?.mesh.clone();
                Arc::clone(self.cache.load_mesh(self.root, &path).ok()?)
            }
        };
        let mut points = mesh
            .positions
            .chunks_exact(3)
            .map(|p| Vec3::new(p[0], p[1], p[2]));
        let first = points.next()?;
        Some(points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
}

/// The box a source gets when its document cannot be loaded: ±0.5 for a primitive, asset mesh or
/// character, bottom-centered (x,z ∈ ±0.5, y ∈ [0, 1] — a model sits on the ground) for a voxel
/// model, assembly, assembly part or sculpt.
fn fallback_box(source: &PlacedMeshSource) -> (Vec3, Vec3) {
    match source {
        PlacedMeshSource::Voxel(_)
        | PlacedMeshSource::Assembly(_)
        | PlacedMeshSource::AssemblyPart(_, _)
        | PlacedMeshSource::Sculpt(_) => (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)),
        _ => (Vec3::splat(-0.5), Vec3::splat(0.5)),
    }
}

/// The world AABB of the object-space box `[lo, hi]` under `model`: the eight transformed corners
/// bounded, which is correct (if not tight) for a rotated box.
pub(crate) fn transformed_box(model: Mat4, lo: Vec3, hi: Vec3) -> Aabb {
    let local = Aabb { min: lo, max: hi };
    let world: Vec<Vec3> = local
        .corners()
        .iter()
        .map(|&c| model.transform_point3(c))
        .collect();
    Aabb::of_points(&world)
}

/// A text run's world AABB, matching the renderer's layout: the run spans its laid-out extent
/// (`TextRun::extent`, from the same layout the quads came from), down-and-right from `position` (the top-left of the first glyph cell;
/// lines drop along −Y).
fn text_aabb(run: &TextRun) -> Aabb {
    let [width, height] = run.extent;
    let p = run.position;
    Aabb {
        min: Vec3::new(p.x, p.y - height, p.z),
        max: Vec3::new(p.x + width, p.y, p.z),
    }
}

/// Projects a world AABB to a screen-space pixel rect, returning `(rect, on_screen)`.
///
/// The box is clipped against the camera's near plane (`clip.z = 0` in wgpu's 0..1 depth) before the
/// perspective divide: every corner on the near plane's inner side is kept, and each of the twelve
/// edges that crosses the plane contributes the point where it does. The rect bounds those points
/// (origin top-left, +Y down), so a box straddling the camera reports the rect of the part that is
/// drawn. When no part lies in front of the near plane the rect is `None`. `on_screen` is true only
/// when a non-null rect actually overlaps the `[0, size]` viewport.
fn project(aabb: &Aabb, view_proj: Mat4, width: u32, height: u32) -> (Option<ScreenRect>, bool) {
    let (w, h) = (width as f32, height as f32);
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut any = false;

    let clip: [Vec4; 8] = aabb.corners().map(|corner| view_proj * corner.extend(1.0));
    // `corners()` indexes x, y, z in bits 0, 1, 2: an edge joins two corners differing in one bit.
    let edges = (0..8_usize)
        .flat_map(|a| (0..3).map(move |bit| (a, a ^ (1 << bit))))
        .filter(|&(a, b)| a < b)
        .filter(|&(a, b)| (clip[a].z >= 0.0) != (clip[b].z >= 0.0))
        .map(|(a, b)| clip[a] + (clip[b] - clip[a]) * (clip[a].z / (clip[a].z - clip[b].z)));
    let inside = clip.iter().copied().filter(|c| c.z >= 0.0);

    for clip in inside.chain(edges) {
        if clip.w <= BEHIND_CAMERA_W {
            continue;
        }
        let ndc = clip.truncate() / clip.w;
        let sx = (ndc.x * 0.5 + 0.5) * w;
        let sy = (1.0 - (ndc.y * 0.5 + 0.5)) * h;
        min_x = min_x.min(sx);
        min_y = min_y.min(sy);
        max_x = max_x.max(sx);
        max_y = max_y.max(sy);
        any = true;
    }

    if !any {
        return (None, false);
    }
    let rect = ScreenRect {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    };
    let on_screen = rect.x < w && rect.y < h && rect.x + rect.w > 0.0 && rect.y + rect.h > 0.0;
    (Some(rect), on_screen)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Camera at the origin looking down −Z: a 90° perspective with near 1 and far 100.
    fn view_proj() -> Mat4 {
        Mat4::perspective_rh(90.0_f32.to_radians(), 1.0, 1.0, 100.0)
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A box straddling the near plane reports the rect of its drawn part. Hand-computed: the part
    /// at z in [-4, -1] reaches ndc x in [0, 2] and y in [0, 1] at the near plane (a 90° view at
    /// distance 1 has a half-extent of 1), and the far face at z = -4 projects inside that.
    #[test]
    fn a_box_crossing_the_near_plane_is_clipped_not_dropped() {
        let aabb = Aabb {
            min: Vec3::new(0.0, 0.0, -4.0),
            max: Vec3::new(2.0, 1.0, 1.0),
        };
        let (rect, on_screen) = project(&aabb, view_proj(), 256, 256);
        let rect = rect.expect("the drawn part has a rect");
        assert!(near(rect.x, 128.0), "{rect:?}");
        assert!(near(rect.y, 0.0), "{rect:?}");
        assert!(near(rect.w, 256.0), "{rect:?}");
        assert!(near(rect.h, 128.0), "{rect:?}");
        assert!(on_screen);
    }

    /// A box wholly behind the camera has no rect.
    #[test]
    fn a_box_behind_the_camera_has_no_rect() {
        let aabb = Aabb {
            min: Vec3::new(-1.0, -1.0, 1.0),
            max: Vec3::new(1.0, 1.0, 3.0),
        };
        assert_eq!(project(&aabb, view_proj(), 256, 256), (None, false));
    }

    /// A box wholly in front projects to its corners' bound, as before.
    #[test]
    fn a_box_in_front_bounds_its_projected_corners() {
        let aabb = Aabb {
            min: Vec3::new(-1.0, -1.0, -3.0),
            max: Vec3::new(1.0, 1.0, -2.0),
        };
        let (rect, on_screen) = project(&aabb, view_proj(), 256, 256);
        let rect = rect.expect("in front");
        // The near face (z = -2) spans ndc ±0.5, the far face (z = -3) ±1/3: the near face bounds.
        assert!(near(rect.x, 64.0), "{rect:?}");
        assert!(near(rect.y, 64.0), "{rect:?}");
        assert!(near(rect.w, 128.0), "{rect:?}");
        assert!(near(rect.h, 128.0), "{rect:?}");
        assert!(on_screen);
    }
}
