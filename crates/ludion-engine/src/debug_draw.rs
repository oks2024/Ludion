//! World-space debug draw: named categories of lines a system draws, the host chooses to see.
//!
//! A system calls [`debug_draw`]`(world, "motor")` and chains [`DebugPen`] primitives; the lines land
//! in the [`DebugDraw`] resource the engine installs into every world it builds. At each frame
//! boundary the engine calls [`DebugDraw::end_tick`], so the resource shows exactly the **last
//! completed tick** — a paused sim keeps its final drawing, and nothing accumulates.
//!
//! Which categories reach the renderer is the **host's** choice ([`DebugCategories`], set through
//! [`Engine::set_debug_draw`](crate::Engine::set_debug_draw)) and is applied only when a
//! [`RenderScene`](ludion_renderer::RenderScene) is extracted. The pen always records, and the
//! filter lives on the `Engine`, never in the world: no system can branch on it, and the drawing is
//! never hashed, snapshotted or recorded. The resource registers no component.

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::math::glam::Vec3;
use ludion_core::math::{quat_rotate, sin_cos};
use ludion_ecs::World;
use ludion_renderer::DebugSegment;

/// Segments in one full circle (a ring, a great circle of a sphere).
const CIRCLE_SEGMENTS: usize = 16;
/// Segments in one capsule end-cap half circle.
const ARC_SEGMENTS: usize = CIRCLE_SEGMENTS / 2;

/// The category [`segments`] draws from the world's `Parent` links.
pub(crate) const HIERARCHY: &str = "hierarchy";

/// The category [`segments`] draws from the world's reflection probes.
pub(crate) const PROBES: &str = "probes";

/// The category [`segments`] draws from the live `PhysicsWorld`'s colliders.
pub(crate) const COLLIDERS: &str = "colliders";

/// The category name that stands for every category in [`DebugCategories`]; no system may draw under it.
const ALL: &str = "all";

/// An ECS resource: the lines systems drew this tick (`pending`) and the last completed tick's
/// (`last`), by category. Installed by the engine into every world it builds; excluded from the
/// `WorldHash` and from snapshots.
#[derive(Default, Debug)]
pub struct DebugDraw {
    pending: BTreeMap<&'static str, Vec<DebugSegment>>,
    last: BTreeMap<&'static str, Vec<DebugSegment>>,
}

impl DebugDraw {
    /// The segments drawn so far this tick under `category`.
    #[cfg(test)]
    pub(crate) fn pending(&self, category: &str) -> &[DebugSegment] {
        self.pending.get(category).map_or(&[], Vec::as_slice)
    }

    /// Ends the tick: what was drawn becomes what is shown, and the next tick starts empty.
    pub fn end_tick(&mut self) {
        self.last = std::mem::take(&mut self.pending);
    }

    /// The categories the last completed tick drew under, sorted.
    pub fn drawn(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.last.keys().copied()
    }
}

/// Draws into `category` of the world's [`DebugDraw`]. A world without the resource (a bare host, a
/// plain [`World`]) gets a pen that draws nothing.
///
/// `"all"` is reserved for the host's filter ([`DebugCategories`]) and must not be a category name.
///
/// # Example
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::debug_draw;
///
/// // No resource installed: the pen is a no-op rather than a panic.
/// let mut world = World::new();
/// debug_draw(&mut world, "motor")
///     .line([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0, 1.0])
///     .cross([0.0, 1.0, 0.0], 0.1, [1.0, 1.0, 0.0, 1.0]);
/// ```
pub fn debug_draw<'w>(world: &'w mut World, category: &'static str) -> DebugPen<'w> {
    debug_assert_ne!(category, ALL, "`all` is reserved for the host's filter");
    DebugPen {
        draw: world.resource_mut::<DebugDraw>(),
        category,
    }
}

/// A chainable pen over one category. Points are world space, colours linear RGBA.
pub struct DebugPen<'w> {
    draw: Option<&'w mut DebugDraw>,
    category: &'static str,
}

impl DebugPen<'_> {
    fn push(&mut self, a: Vec3, b: Vec3, color: [f32; 4]) {
        if let Some(draw) = self.draw.as_deref_mut() {
            draw.pending
                .entry(self.category)
                .or_default()
                .push(DebugSegment {
                    a: a.to_array(),
                    b: b.to_array(),
                    color,
                });
        }
    }

    /// A line from `a` to `b`.
    pub fn line(&mut self, a: [f32; 3], b: [f32; 3], color: [f32; 4]) -> &mut Self {
        self.push(Vec3::from(a), Vec3::from(b), color);
        self
    }

    /// A line from `origin` along `dir` (its length is the ray's length).
    pub fn ray(&mut self, origin: [f32; 3], dir: [f32; 3], color: [f32; 4]) -> &mut Self {
        let o = Vec3::from(origin);
        self.push(o, o + Vec3::from(dir), color);
        self
    }

    /// A polyline through `points`; fewer than two draws nothing.
    pub fn path(&mut self, points: &[[f32; 3]], color: [f32; 4]) -> &mut Self {
        for pair in points.windows(2) {
            self.push(Vec3::from(pair[0]), Vec3::from(pair[1]), color);
        }
        self
    }

    /// Three axis-aligned lines through `p`, each `2 * size` long.
    pub fn cross(&mut self, p: [f32; 3], size: f32, color: [f32; 4]) -> &mut Self {
        let p = Vec3::from(p);
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            self.push(p - axis * size, p + axis * size, color);
        }
        self
    }

    /// Three lines from `origin` along the rotated +X (red), +Y (green) and +Z (blue) axes, each
    /// `size` long. `rotation` is a unit quaternion `[x, y, z, w]`; the colours are fixed.
    pub fn axes(&mut self, origin: [f32; 3], rotation: [f32; 4], size: f32) -> &mut Self {
        let o = Vec3::from(origin);
        for (axis, color) in [
            ([size, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]),
            ([0.0, size, 0.0], [0.0, 1.0, 0.0, 1.0]),
            ([0.0, 0.0, size], [0.0, 0.0, 1.0, 1.0]),
        ] {
            self.push(o, o + Vec3::from(quat_rotate(rotation, axis)), color);
        }
        self
    }

    /// The twelve edges of the axis-aligned box `min`..`max`.
    pub fn aabb(&mut self, min: [f32; 3], max: [f32; 3], color: [f32; 4]) -> &mut Self {
        let (lo, hi) = (Vec3::from(min), Vec3::from(max));
        let corner = |i: usize| {
            Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            )
        };
        for i in 0..8usize {
            for bit in [1usize, 2, 4] {
                if i & bit == 0 {
                    self.push(corner(i), corner(i | bit), color);
                }
            }
        }
        self
    }

    /// Three great circles of `radius` around `center`.
    pub fn sphere(&mut self, center: [f32; 3], radius: f32, color: [f32; 4]) -> &mut Self {
        let c = Vec3::from(center);
        self.arc(
            c,
            Vec3::X,
            Vec3::Y,
            radius,
            CIRCLE_SEGMENTS,
            0.0,
            TAU,
            color,
        );
        self.arc(
            c,
            Vec3::Y,
            Vec3::Z,
            radius,
            CIRCLE_SEGMENTS,
            0.0,
            TAU,
            color,
        );
        self.arc(
            c,
            Vec3::Z,
            Vec3::X,
            radius,
            CIRCLE_SEGMENTS,
            0.0,
            TAU,
            color,
        );
        self
    }

    /// A capsule of `radius` around the segment `a`..`b`: a ring and four side lines along the shaft,
    /// and two half-circle arcs (in perpendicular planes) capping each end. `a == b` is a sphere.
    pub fn capsule(&mut self, a: [f32; 3], b: [f32; 3], radius: f32, color: [f32; 4]) -> &mut Self {
        let (a, b) = (Vec3::from(a), Vec3::from(b));
        let Some(axis) = (b - a).try_normalize() else {
            return self.sphere(a.to_array(), radius, color);
        };
        let helper = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let u = axis.cross(helper).normalize();
        let v = axis.cross(u);
        for end in [a, b] {
            self.arc(end, u, v, radius, CIRCLE_SEGMENTS, 0.0, TAU, color);
        }
        for side in [u, v, -u, -v] {
            self.push(a + side * radius, b + side * radius, color);
        }
        for (end, cap) in [(a, -axis), (b, axis)] {
            self.arc(end, u, cap, radius, ARC_SEGMENTS, 0.0, PI, color);
            self.arc(end, v, cap, radius, ARC_SEGMENTS, 0.0, PI, color);
        }
        self
    }

    /// `segments` chords along `center + radius * (cos t * e1 + sin t * e2)` for `t` in `start..start + sweep`.
    #[allow(clippy::too_many_arguments)]
    fn arc(
        &mut self,
        center: Vec3,
        e1: Vec3,
        e2: Vec3,
        radius: f32,
        segments: usize,
        start: f32,
        sweep: f32,
        color: [f32; 4],
    ) {
        let point = |i: usize| {
            #[allow(clippy::cast_precision_loss)]
            let t = start + sweep * (i as f32) / (segments as f32);
            let (s, c) = sin_cos(t);
            center + radius * (c * e1 + s * e2)
        };
        for i in 0..segments {
            self.push(point(i), point(i + 1), color);
        }
    }
}

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// Which debug categories the host wants drawn. Host render state on the
/// [`Engine`](crate::Engine), never in a world.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum DebugCategories {
    /// Nothing: the default, and byte-identical to an engine without debug draw.
    #[default]
    None,
    /// Every category.
    All,
    /// Exactly these categories.
    Only(BTreeSet<String>),
}

impl DebugCategories {
    /// Whether `name` is drawn.
    pub fn enables(&self, name: &str) -> bool {
        match self {
            DebugCategories::None => false,
            DebugCategories::All => true,
            DebugCategories::Only(set) => set.contains(name),
        }
    }

    /// Parses a comma-separated list (`--debug-draw motor,path`): `all` gives [`All`](Self::All),
    /// an empty list [`None`](Self::None).
    pub fn parse(list: &str) -> Self {
        Self::from_names(
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    /// From category names; any `all` entry gives [`All`](Self::All), no entries [`None`](Self::None).
    pub fn from_names(names: Vec<String>) -> Self {
        if names.iter().any(|name| name == ALL) {
            DebugCategories::All
        } else if names.is_empty() {
            DebugCategories::None
        } else {
            DebugCategories::Only(names.into_iter().collect())
        }
    }

    /// The names [`from_names`](Self::from_names) round-trips; [`All`](Self::All) is `["all"]`.
    pub fn names(&self) -> Vec<String> {
        match self {
            DebugCategories::None => Vec::new(),
            DebugCategories::All => vec![ALL.to_string()],
            DebugCategories::Only(set) => set.iter().cloned().collect(),
        }
    }

    /// The offline renders' summary clause — `debug draw: path`, and where a named category drew
    /// nothing `debug draw: path; nothing drawn for: motr`, so a typo shows. `drawn` is what
    /// [`DebugDraw::drawn`] reported; `None` (nothing asked for) gives no clause.
    pub fn summary(&self, drawn: &[&str]) -> Option<String> {
        let silent: Vec<&str> = match self {
            DebugCategories::None => return None,
            DebugCategories::All => {
                return Some(if drawn.is_empty() {
                    "debug draw: all; nothing drawn".to_string()
                } else {
                    "debug draw: all".to_string()
                });
            }
            DebugCategories::Only(set) => set
                .iter()
                .map(String::as_str)
                .filter(|name| !drawn.contains(name))
                .collect(),
        };
        let asked = self.names().join(",");
        Some(if silent.is_empty() {
            format!("debug draw: {asked}")
        } else {
            format!(
                "debug draw: {asked}; nothing drawn for: {}",
                silent.join(",")
            )
        })
    }
}

/// The last completed tick's segments in the categories `filter` enables, in category order.
///
/// The `hierarchy` category is drawn here from the live world rather than by a system, so it is
/// always current, even paused, and costs nothing unless enabled. It sorts among the categories;
/// `probes` and `colliders` (every physics body's wireframe) are drawn the same way.
pub(crate) fn segments(world: &World, filter: &DebugCategories) -> Vec<DebugSegment> {
    if *filter == DebugCategories::None {
        return Vec::new();
    }
    let Some(draw) = world.resource::<DebugDraw>() else {
        return Vec::new();
    };
    let mut categories: BTreeMap<&str, Vec<DebugSegment>> = draw
        .last
        .iter()
        .filter(|(category, _)| filter.enables(category))
        .map(|(category, drawn)| (*category, drawn.clone()))
        .collect();
    if filter.enables(HIERARCHY) {
        categories
            .entry(HIERARCHY)
            .or_default()
            .extend(crate::hierarchy::debug_lines(world));
    }
    if filter.enables(PROBES) {
        categories
            .entry(PROBES)
            .or_default()
            .extend(crate::extract::probe_debug_lines(world));
    }
    if filter.enables(COLLIDERS) {
        categories
            .entry(COLLIDERS)
            .or_default()
            .extend(crate::scene_physics::collider_debug_lines(world));
    }
    categories.into_values().flatten().collect()
}

/// The categories the world has drawn: what [`DebugDraw::drawn`] lists plus `hierarchy` when any
/// entity has a `Parent`, `probes` when any has a `ReflectionProbe` and `colliders` when the
/// physics world holds a body, sorted.
pub(crate) fn drawn(world: &World) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = world
        .resource::<DebugDraw>()
        .map_or_else(Vec::new, |draw| draw.drawn().collect());
    if crate::hierarchy::any_parent(world) {
        names.push(HIERARCHY);
    }
    if crate::extract::any_probe(world) {
        names.push(PROBES);
    }
    if crate::scene_physics::any_body(world) {
        names.push(COLLIDERS);
    }
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    fn world() -> World {
        let mut world = World::new();
        world.insert_resource(DebugDraw::default());
        world
    }

    fn count(world: &mut World, draw: impl FnOnce(&mut DebugPen<'_>)) -> usize {
        draw(&mut debug_draw(world, "t"));
        world.resource::<DebugDraw>().unwrap().pending["t"].len()
    }

    #[test]
    fn primitives_draw_their_segment_counts() {
        assert_eq!(
            count(&mut world(), |p| {
                p.line([0.0; 3], [1.0; 3], RED);
            }),
            1
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.ray([0.0; 3], [1.0; 3], RED);
            }),
            1
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.path(&[[0.0; 3], [1.0; 3], [2.0; 3]], RED);
            }),
            2
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.cross([0.0; 3], 1.0, RED);
            }),
            3
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.axes([0.0; 3], [0.0, 0.0, 0.0, 1.0], 1.0);
            }),
            3
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.aabb([0.0; 3], [1.0; 3], RED);
            }),
            12
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.sphere([0.0; 3], 1.0, RED);
            }),
            3 * CIRCLE_SEGMENTS
        );
        let capsule = 2 * CIRCLE_SEGMENTS + 4 + 4 * ARC_SEGMENTS;
        assert_eq!(
            count(&mut world(), |p| {
                p.capsule([0.0; 3], [0.0, 2.0, 0.0], 0.5, RED);
            }),
            capsule
        );
        assert_eq!(
            count(&mut world(), |p| {
                p.capsule([1.0; 3], [1.0; 3], 0.5, RED);
            }),
            3 * CIRCLE_SEGMENTS
        );
    }

    #[test]
    fn a_path_of_fewer_than_two_points_draws_nothing() {
        let mut world = world();
        debug_draw(&mut world, "t")
            .path(&[], RED)
            .path(&[[0.0; 3]], RED);
        assert!(world.resource::<DebugDraw>().unwrap().pending.is_empty());
    }

    #[test]
    fn capsule_points_lie_on_the_capsule() {
        let mut world = world();
        debug_draw(&mut world, "t").capsule([0.0; 3], [0.0, 2.0, 0.0], 0.5, RED);
        for s in &world.resource::<DebugDraw>().unwrap().pending["t"] {
            for p in [s.a, s.b] {
                let along = p[1].clamp(0.0, 2.0);
                let dy = p[1] - along;
                let dist = (p[0] * p[0] + dy * dy + p[2] * p[2]).sqrt();
                assert!((dist - 0.5).abs() < 1e-4, "{p:?} is {dist} from the axis");
            }
        }
    }

    #[test]
    fn end_tick_moves_pending_to_last_and_empties_it() {
        let mut world = world();
        debug_draw(&mut world, "t").cross([0.0; 3], 1.0, RED);
        let draw = world.resource_mut::<DebugDraw>().unwrap();
        draw.end_tick();
        assert_eq!(draw.last["t"].len(), 3);
        assert!(draw.pending.is_empty());
        assert_eq!(draw.drawn().collect::<Vec<_>>(), ["t"]);
        draw.end_tick();
        assert!(draw.last.is_empty());
    }

    #[test]
    fn segments_follow_the_filter_in_category_order() {
        let mut world = world();
        debug_draw(&mut world, "b").line([0.0; 3], [1.0; 3], RED);
        debug_draw(&mut world, "a").cross([0.0; 3], 1.0, RED);
        world.resource_mut::<DebugDraw>().unwrap().end_tick();
        assert!(segments(&world, &DebugCategories::None).is_empty());
        assert_eq!(segments(&world, &DebugCategories::All).len(), 4);
        let only_b = DebugCategories::parse("b");
        assert_eq!(segments(&world, &only_b).len(), 1);
    }

    /// A parent at `(1, 0, 0)` and a child one metre above it, in local space.
    fn family() -> World {
        use crate::components::Transform;
        use ludion_ecs::Parent;
        let mut world = world();
        let parent = world.spawn();
        let child = world.spawn();
        world
            .insert(
                parent,
                Transform {
                    translation: [1.0, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                child,
                Transform {
                    translation: [0.0, 1.0, 0.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world.insert(child, Parent(parent)).unwrap();
        world
    }

    #[test]
    fn the_hierarchy_category_draws_parent_to_child_world_lines() {
        let world = family();
        let lines = segments(&world, &DebugCategories::parse("hierarchy"));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].a, [1.0, 0.0, 0.0]);
        assert_eq!(lines[0].b, [1.0, 1.0, 0.0]);
        assert_eq!(segments(&world, &DebugCategories::All).len(), 1);
        assert!(segments(&world, &DebugCategories::None).is_empty());
        assert!(segments(&world, &DebugCategories::parse("path")).is_empty());
    }

    #[test]
    fn drawn_lists_hierarchy_only_when_a_parent_exists() {
        assert!(drawn(&world()).is_empty());
        assert_eq!(drawn(&family()), ["hierarchy"]);
        let mut world = family();
        debug_draw(&mut world, "a").cross([0.0; 3], 1.0, RED);
        debug_draw(&mut world, "z").cross([0.0; 3], 1.0, RED);
        world.resource_mut::<DebugDraw>().unwrap().end_tick();
        assert_eq!(drawn(&world), ["a", "hierarchy", "z"]);
    }

    fn with_probe(shape: crate::components::ProbeShape) -> World {
        use crate::components::{ReflectionProbe, Transform};
        let mut world = world();
        let id = world.spawn();
        world
            .insert(
                id,
                Transform {
                    translation: [1.0, 2.0, 3.0],
                    ..Transform::default()
                },
            )
            .unwrap();
        world
            .insert(
                id,
                ReflectionProbe {
                    shape,
                    blend: 0.5,
                    priority: 0,
                },
            )
            .unwrap();
        world
    }

    #[test]
    fn the_probes_category_draws_each_volume_and_its_capture_point() {
        use crate::components::ProbeShape;
        let probes = DebugCategories::parse("probes");
        let boxed = with_probe(ProbeShape::Box {
            extent: [1.0, 2.0, 3.0],
        });
        let lines = segments(&boxed, &probes);
        assert_eq!(lines.len(), 12 + 3);
        let red = ludion_renderer::PROBE_VIEW_PALETTE[0].map(ludion_sprite::srgb_decode);
        assert!(
            lines
                .iter()
                .all(|l| l.color == [red[0], red[1], red[2], 1.0])
        );
        assert!(lines.iter().any(|l| l.a == [0.0, 0.0, 0.0]));
        let sphere = with_probe(ProbeShape::Sphere { radius: 2.0 });
        assert_eq!(segments(&sphere, &probes).len(), 96 + 3);
        assert!(segments(&sphere, &DebugCategories::None).is_empty());
        assert!(segments(&sphere, &DebugCategories::parse("path")).is_empty());
    }

    #[test]
    fn probes_past_the_renderer_cap_draw_gray_whatever_their_priority() {
        use crate::components::{ProbeShape, ReflectionProbe, Transform};
        let mut world = world();
        let count = ludion_renderer::MAX_PROBES + 1;
        for i in 0..count {
            let id = world.spawn();
            world.insert(id, Transform::default()).unwrap();
            let probe = ReflectionProbe {
                shape: ProbeShape::Box {
                    extent: [1.0, 1.0, 1.0],
                },
                blend: 0.5,
                priority: if i == count - 1 { 9 } else { 0 },
            };
            world.insert(id, probe).unwrap();
        }
        let lines = segments(&world, &DebugCategories::parse("probes"));
        assert_eq!(lines.len(), count * 15);
        let red = ludion_renderer::PROBE_VIEW_PALETTE[0].map(ludion_sprite::srgb_decode);
        assert_eq!(lines[0].color, [red[0], red[1], red[2], 1.0]);
        assert_eq!(lines[count * 15 - 1].color, [0.25, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn drawn_lists_probes_only_when_a_probe_exists() {
        use crate::components::ProbeShape;
        assert!(drawn(&world()).is_empty());
        let world = with_probe(ProbeShape::Sphere { radius: 1.0 });
        assert_eq!(drawn(&world), ["probes"]);
    }

    #[test]
    fn categories_parse_and_round_trip() {
        assert_eq!(DebugCategories::parse(""), DebugCategories::None);
        assert_eq!(DebugCategories::parse("all"), DebugCategories::All);
        assert_eq!(DebugCategories::parse("motor, all"), DebugCategories::All);
        let only = DebugCategories::parse("path,motor");
        assert!(only.enables("motor") && only.enables("path") && !only.enables("rig"));
        assert_eq!(only.names(), ["motor", "path"]);
        assert_eq!(DebugCategories::from_names(only.names()), only);
        assert_eq!(DebugCategories::All.names(), ["all"]);
        assert!(!DebugCategories::None.enables("motor"));
    }

    #[test]
    fn the_summary_names_the_categories_that_drew_nothing() {
        assert_eq!(DebugCategories::None.summary(&[]), None);
        assert_eq!(
            DebugCategories::parse("path").summary(&["path"]).as_deref(),
            Some("debug draw: path")
        );
        assert_eq!(
            DebugCategories::parse("path,motr")
                .summary(&["path"])
                .as_deref(),
            Some("debug draw: motr,path; nothing drawn for: motr")
        );
        assert_eq!(
            DebugCategories::All.summary(&[]).as_deref(),
            Some("debug draw: all; nothing drawn")
        );
    }

    #[test]
    fn axes_point_along_the_rotated_axes_in_rgb() {
        let mut world = world();
        let half = std::f32::consts::FRAC_1_SQRT_2;
        // 90 degrees about +Y: X maps to -Z, Z maps to +X.
        debug_draw(&mut world, "t").axes([1.0, 2.0, 3.0], [0.0, half, 0.0, half], 2.0);
        let segments = &world.resource::<DebugDraw>().unwrap().pending["t"];
        let colors: Vec<_> = segments.iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            [
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0]
            ]
        );
        let ends = [[1.0, 2.0, 1.0], [1.0, 4.0, 3.0], [3.0, 2.0, 3.0]];
        for (segment, end) in segments.iter().zip(ends) {
            assert_eq!(segment.a, [1.0, 2.0, 3.0]);
            for (got, want) in segment.b.iter().zip(end) {
                assert!((got - want).abs() < 1e-5, "{:?} vs {end:?}", segment.b);
            }
        }
    }

    #[test]
    fn a_pen_over_a_world_without_the_resource_is_a_no_op() {
        let mut world = World::new();
        debug_draw(&mut world, "t").sphere([0.0; 3], 1.0, RED);
        assert!(world.resource::<DebugDraw>().is_none());
    }

    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Clone, Copy, Debug)]
    struct Probe {
        until: u64,
    }

    fn probe(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
        let until = world.query::<(&Probe,)>().next().map_or(0, |(p,)| p.until);
        if ctx.frame.as_u64() < until {
            debug_draw(world, "probe").line([0.0; 3], [0.0, 1.0, 0.0], RED);
        }
    }

    /// A game whose `probe` system draws until frame `until`, beside a follower on a closed path.
    fn engine(until: u64) -> crate::Engine {
        let root =
            std::env::temp_dir().join(format!("ludion-debug-draw-unit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("ring.path.yaml"),
            "format: 1\nkind: path\nname: ring\ninterpolation: linear\nclosed: true\npoints:\n\
             \x20 - - 0.0\n    - 0.0\n    - 0.0\n\
             \x20 - - 4.0\n    - 0.0\n    - 0.0\n\
             \x20 - - 4.0\n    - 0.0\n    - 4.0\n",
        )
        .unwrap();
        let mut engine = crate::Engine::builder()
            .project_root(&root)
            .components(|registry| registry.register::<Probe>("Probe"))
            .system("probe", probe)
            .build()
            .unwrap();
        engine
            .load_inline_scene(&format!(
                "format: 1\nentities:\n  - components:\n      Probe:\n        until: {until}\n  \
                 - components:\n      Transform: {{}}\n      PathFollower:\n        path: ring.path.yaml\n"
            ))
            .unwrap();
        engine
    }

    /// A ring follower with `orient` set (`None`: the key is omitted), no probe.
    fn oriented_engine(orient: Option<&str>) -> crate::Engine {
        let mut engine = engine(0);
        let orient = orient.map_or(String::new(), |o| format!("        orient: {o}\n"));
        engine
            .load_inline_scene(&format!(
                "format: 1\nentities:\n  - components:\n      Transform: {{}}\n      PathFollower:\n        \
                 path: ring.path.yaml\n{orient}"
            ))
            .unwrap();
        engine
    }

    fn lines(engine: &mut crate::Engine) -> Vec<DebugSegment> {
        engine.extract_render_scene(64, 64).unwrap().debug_lines
    }

    #[test]
    fn extraction_carries_only_the_enabled_categories() {
        let mut engine = engine(1000);
        engine.advance_live();
        assert!(
            lines(&mut engine).is_empty(),
            "nothing is enabled by default"
        );

        engine.set_debug_draw(DebugCategories::All);
        let all = lines(&mut engine);
        engine.set_debug_draw(DebugCategories::parse("path"));
        let path = lines(&mut engine);
        engine.set_debug_draw(DebugCategories::parse("probe"));
        let probe = lines(&mut engine);
        assert_eq!(probe.len(), 1);
        assert!(!path.is_empty());
        assert_eq!(
            all.len(),
            path.len() + probe.len(),
            "a toggle reaches the next extract"
        );
        assert_eq!(engine.debug_draw_drawn(), ["path", "probe"]);
    }

    #[test]
    fn an_oriented_follower_draws_its_axes_under_path() {
        let path_lines = |orient| {
            let mut engine = oriented_engine(orient);
            engine.set_debug_draw(DebugCategories::parse("path"));
            engine.advance_live();
            lines(&mut engine)
        };
        let plain = path_lines(None);
        let oriented = path_lines(Some("yaw"));
        assert_eq!(oriented.len(), plain.len() + 3);
        for color in [
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0],
        ] {
            assert_eq!(oriented.iter().filter(|s| s.color == color).count(), 1);
        }
    }

    #[test]
    fn only_the_last_tick_shows_and_a_pause_keeps_it() {
        let mut engine = engine(2);
        engine.set_debug_draw(DebugCategories::parse("probe"));
        engine.advance_live();
        engine.advance_live();
        assert_eq!(lines(&mut engine).len(), 1, "the last tick's line, once");
        assert_eq!(
            lines(&mut engine).len(),
            1,
            "extraction does not consume it"
        );
        engine.advance_live();
        assert!(
            lines(&mut engine).is_empty(),
            "a tick that drew nothing clears it"
        );
    }

    /// A static unit box and a rigid sphere above it, in a free-space physics world.
    fn physics_engine() -> crate::Engine {
        let root = std::env::temp_dir().join(format!(
            "ludion-debug-draw-colliders-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut engine = crate::Engine::builder()
            .project_root(&root)
            .build()
            .unwrap();
        engine
            .load_inline_scene(
                "format: 1\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      \
                 Transform: {}\n      Collider:\n        shape:\n          box:\n            \
                 half_extents:\n              - 0.5\n              - 0.5\n              - 0.5\n        \
                 body: static\n  - name: ball\n    components:\n      Transform:\n        translation:\n          \
                 - 0.0\n          - 5.0\n          - 0.0\n      Collider:\n        shape:\n          \
                 sphere:\n            radius: 0.5\n        body:\n          rigid:\n            mass: 1.0\n      \
                 Velocity: {}\n",
            )
            .unwrap();
        engine
    }

    /// Per-tick world hashes and the final snapshot of a mesh floor with a falling rigid sphere,
    /// extracting a render scene after every tick when `categories` enables something.
    fn run_mesh_scene(categories: DebugCategories) -> (Vec<u64>, Vec<u8>, usize) {
        let root = std::env::temp_dir().join(format!(
            "ludion-debug-draw-colliders-mesh-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let floor = ludion_scene::MeshDocument {
            positions: vec![
                -5.0, 0.0, -5.0, 5.0, 0.0, -5.0, -5.0, 0.0, 5.0, 5.0, 0.0, 5.0,
            ],
            indices: vec![0, 2, 1, 1, 2, 3],
            ..ludion_scene::MeshDocument::default()
        };
        std::fs::write(
            root.join("floor.mesh.yaml"),
            ludion_scene::emit_mesh(&floor),
        )
        .unwrap();
        let mut engine = crate::Engine::builder()
            .project_root(&root)
            .build()
            .unwrap();
        engine.set_debug_draw(categories);
        engine
            .load_inline_scene(
                "format: 1\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      \
                 Transform: {}\n      Collider:\n        shape:\n          mesh:\n            path: floor.mesh.yaml\n        \
                 body: static\n  - name: ball\n    components:\n      Transform:\n        translation:\n          \
                 - 0.0\n          - 2.0\n          - 0.0\n      Collider:\n        shape:\n          \
                 sphere:\n            radius: 0.5\n        body:\n          rigid:\n            mass: 1.0\n      \
                 Velocity: {}\n",
            )
            .unwrap();
        let mut hashes = Vec::new();
        let mut drawn = 0;
        for _ in 0..30 {
            engine.advance_live();
            drawn += lines(&mut engine).len();
            hashes.push(engine.world_hash().unwrap().as_u64());
        }
        let snapshot = match engine
            .execute(ludion_protocol::Command::EngineSnapshot)
            .unwrap()
            .body
        {
            ludion_protocol::ResponseBody::Snapshot { snapshot } => {
                serde_json::to_vec(&snapshot).unwrap()
            }
            other => panic!("expected a snapshot, got {other:?}"),
        };
        (hashes, snapshot, drawn)
    }

    #[test]
    fn extracting_the_colliders_category_never_perturbs_the_hash_or_snapshot() {
        let (hashes, snapshot, drawn) = run_mesh_scene(DebugCategories::parse("colliders"));
        let (plain_hashes, plain_snapshot, plain_drawn) = run_mesh_scene(DebugCategories::None);
        assert!(drawn > 0, "the colliders category drew nothing");
        assert_eq!(plain_drawn, 0);
        assert_eq!(hashes, plain_hashes, "per-tick hashes");
        assert_eq!(snapshot, plain_snapshot, "snapshot bytes");
    }

    #[test]
    fn the_colliders_category_draws_every_body_in_its_kind_colour() {
        let mut engine = physics_engine();
        engine.set_debug_draw(DebugCategories::parse("colliders"));
        assert!(
            engine.debug_draw_drawn().is_empty(),
            "no PhysicsWorld before the first tick"
        );
        engine.advance_live();
        let drawn = lines(&mut engine);
        let grey = [0.55, 0.55, 0.55, 1.0];
        assert_eq!(drawn.iter().filter(|s| s.color == grey).count(), 12);
        assert!(
            drawn.iter().any(|s| s.color == [1.0, 0.45, 0.05, 1.0]),
            "the rigid sphere draws orange"
        );
        assert_eq!(engine.debug_draw_drawn(), ["colliders"]);

        engine.set_debug_draw(DebugCategories::parse("path"));
        assert!(lines(&mut engine).is_empty());
    }
}
