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

/// The category the engine appends to the extracted scene's lines: each requested shadowed local
/// light, green when granted and red when skipped. Grants depend on the camera, so it is drawn from
/// the extracted scene, not from the world.
pub(crate) const LOCAL_SHADOWS: &str = "local_shadows";

/// The category the engine appends to the extracted scene's lines: the camera cull view's verdict on
/// every world mesh box, green kept and red culled. It depends on the camera, so it is drawn from the
/// extracted scene, not from the world.
pub(crate) const CULL: &str = "cull";

/// The category [`segments`] draws from the live `PhysicsWorld`'s colliders.
pub(crate) const COLLIDERS: &str = "colliders";

/// The host-side `nav` category and its sub-filters; `nav` alone enables all three parts.
pub(crate) const NAV: &str = "nav";
pub(crate) const NAV_GRID: &str = "nav:grid";
pub(crate) const NAV_ROUTES: &str = "nav:routes";
pub(crate) const NAV_FLOW: &str = "nav:flow";

/// The host-side `perception` category and its sub-filters; `perception` alone enables all four parts.
pub(crate) const PERCEPTION: &str = "perception";
pub(crate) const PERCEPTION_ZONES: &str = "perception:zones";
pub(crate) const PERCEPTION_RAYS: &str = "perception:rays";
pub(crate) const PERCEPTION_NOISES: &str = "perception:noises";
pub(crate) const PERCEPTION_MEMORY: &str = "perception:memory";

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
        self.arc_chords(
            c,
            Vec3::X,
            Vec3::Y,
            radius,
            CIRCLE_SEGMENTS,
            0.0,
            TAU,
            color,
        );
        self.arc_chords(
            c,
            Vec3::Y,
            Vec3::Z,
            radius,
            CIRCLE_SEGMENTS,
            0.0,
            TAU,
            color,
        );
        self.arc_chords(
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
            self.arc_chords(end, u, v, radius, CIRCLE_SEGMENTS, 0.0, TAU, color);
        }
        for side in [u, v, -u, -v] {
            self.push(a + side * radius, b + side * radius, color);
        }
        for (end, cap) in [(a, -axis), (b, axis)] {
            self.arc_chords(end, u, cap, radius, ARC_SEGMENTS, 0.0, PI, color);
            self.arc_chords(end, v, cap, radius, ARC_SEGMENTS, 0.0, PI, color);
        }
        self
    }

    /// `segments` chords along `center + radius * (cos t * e1 + sin t * e2)` for `t` in `start..start + sweep`.
    #[allow(clippy::too_many_arguments)]
    fn arc_chords(
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
        let point = |i: usize| arc_point(center, e1, e2, radius, arc_t(start, sweep, i, segments));
        for i in 0..segments {
            self.push(point(i), point(i + 1), color);
        }
    }

    /// A full ring of `radius` around `center` in the plane ⊥ `normal`, with a chord count that
    /// grows with the radius. Degenerate input (non-finite, `radius <= 0`, zero `normal`) draws nothing.
    pub fn circle(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        color: [f32; 4],
    ) -> &mut Self {
        if !valid(&[center, normal], &[radius], color) || radius <= 0.0 {
            return self;
        }
        let normal = Vec3::from(normal);
        let Some(n) = normal.try_normalize() else {
            return self;
        };
        let helper = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let Some((e1, e2)) = plane_basis(normal, helper) else {
            return self;
        };
        let segments = segments_for(radius, TAU);
        self.arc_chords(center.into(), e1, e2, radius, segments, 0.0, TAU, color);
        self
    }

    /// An open arc of `radius` around `center` in the plane ⊥ `normal`. It starts at the
    /// direction `from_dir` (projected into the plane) and sweeps `sweep_deg` degrees, positive
    /// counter-clockwise looking down `-normal`. `sweep_deg` is in `[-360, 360]`; `0` draws nothing.
    pub fn arc(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        from_dir: [f32; 3],
        radius: f32,
        sweep_deg: f32,
        color: [f32; 4],
    ) -> &mut Self {
        if !valid(&[center, normal, from_dir], &[radius, sweep_deg], color)
            || radius <= 0.0
            || sweep_deg == 0.0
            || sweep_deg.abs() > 360.0
        {
            return self;
        }
        let Some((e1, e2)) = plane_basis(normal.into(), from_dir.into()) else {
            return self;
        };
        let sweep = sweep_deg.to_radians();
        let segments = segments_for(radius, sweep);
        self.arc_chords(center.into(), e1, e2, radius, segments, 0.0, sweep, color);
        self
    }

    /// A fan outline: two edges from `apex` and the outer arc between them, `half_angle_deg` either
    /// side of `forward` in the plane ⊥ `normal`. `half_angle_deg` is in `(0, 180]`; `180` is a full disc.
    pub fn wedge(
        &mut self,
        apex: [f32; 3],
        normal: [f32; 3],
        forward: [f32; 3],
        half_angle_deg: f32,
        range: f32,
        color: [f32; 4],
    ) -> &mut Self {
        self.ring_sector(apex, normal, forward, half_angle_deg, 0.0, range, color)
    }

    /// A wedge with its inner corner cut away: two radial edges, the outer arc and, when
    /// `r_inner > 0`, the inner arc. Needs `0 <= r_inner < r_outer`; angles as [`wedge`](Self::wedge).
    #[allow(clippy::too_many_arguments)]
    pub fn ring_sector(
        &mut self,
        apex: [f32; 3],
        normal: [f32; 3],
        forward: [f32; 3],
        half_angle_deg: f32,
        r_inner: f32,
        r_outer: f32,
        color: [f32; 4],
    ) -> &mut Self {
        if !valid(
            &[apex, normal, forward],
            &[half_angle_deg, r_inner, r_outer],
            color,
        ) || half_angle_deg <= 0.0
            || half_angle_deg > 180.0
            || r_inner < 0.0
            || r_outer <= r_inner
        {
            return self;
        }
        let Some((e1, e2)) = plane_basis(normal.into(), forward.into()) else {
            return self;
        };
        self.sector(
            apex.into(),
            e1,
            e2,
            half_angle_deg.to_radians(),
            r_inner,
            r_outer,
            color,
        );
        self
    }

    /// A cone from `apex` along `axis`: a base circle `length` along the axis and four generators
    /// to it. `half_angle_deg` is in `(0, 90)`.
    pub fn cone(
        &mut self,
        apex: [f32; 3],
        axis: [f32; 3],
        half_angle_deg: f32,
        length: f32,
        color: [f32; 4],
    ) -> &mut Self {
        if !valid(&[apex, axis], &[half_angle_deg, length], color)
            || half_angle_deg <= 0.0
            || half_angle_deg >= 90.0
            || length <= 0.0
        {
            return self;
        }
        let Some(a) = Vec3::from(axis).try_normalize() else {
            return self;
        };
        let helper = if a.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let Some((e1, e2)) = plane_basis(a, helper) else {
            return self;
        };
        let (s, c) = sin_cos(half_angle_deg.to_radians());
        let radius = length * s / c;
        let apex = Vec3::from(apex);
        let base = apex + a * length;
        let segments = segments_for(radius, TAU);
        self.arc_chords(base, e1, e2, radius, segments, 0.0, TAU, color);
        for quarter in 0..4u8 {
            let t = f32::from(quarter) * (PI / 2.0);
            self.push(apex, arc_point(base, e1, e2, radius, t), color);
        }
        self
    }

    /// A sensor frustum from `apex`: a horizontal fan (about `up`) and a vertical fan (in the
    /// forward/up plane) at `range`, joined by four edges to the apex. Both half angles are in `(0, 90]`;
    /// `forward` must not be parallel to `up`.
    #[allow(clippy::too_many_arguments)]
    pub fn frustum_wedge(
        &mut self,
        apex: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
        half_h_deg: f32,
        half_v_deg: f32,
        range: f32,
        color: [f32; 4],
    ) -> &mut Self {
        if !valid(
            &[apex, forward, up],
            &[half_h_deg, half_v_deg, range],
            color,
        ) || half_h_deg <= 0.0
            || half_h_deg > 90.0
            || half_v_deg <= 0.0
            || half_v_deg > 90.0
            || range <= 0.0
        {
            return self;
        }
        let (forward, up) = (Vec3::from(forward), Vec3::from(up));
        let (Some((h1, h2)), Some((v1, v2))) = (
            plane_basis(up, forward),
            plane_basis(forward.cross(up), forward),
        ) else {
            return self;
        };
        let apex = Vec3::from(apex);
        self.sector(apex, h1, h2, half_h_deg.to_radians(), 0.0, range, color);
        self.sector(apex, v1, v2, half_v_deg.to_radians(), 0.0, range, color);
        self
    }

    /// The outline of the sector `-half..half` between `r_inner` and `r_outer` in the basis `e1`, `e2`:
    /// edge at `-half`, outer arc, edge at `+half`, then the inner arc back when `r_inner > 0`. Edge
    /// ends reuse the arcs' own angles, so they meet the arcs bit-exactly.
    #[allow(clippy::too_many_arguments)]
    fn sector(
        &mut self,
        apex: Vec3,
        e1: Vec3,
        e2: Vec3,
        half: f32,
        r_inner: f32,
        r_outer: f32,
        color: [f32; 4],
    ) {
        let sweep = 2.0 * half;
        let n_outer = segments_for(r_outer, sweep);
        let n_inner = segments_for(r_inner, sweep);
        let at = |radius: f32, t: f32| arc_point(apex, e1, e2, radius, t);
        let outer_lo = arc_t(-half, sweep, 0, n_outer);
        let outer_hi = arc_t(-half, sweep, n_outer, n_outer);
        let inner_hi = arc_t(half, -sweep, 0, n_inner);
        let inner_lo = arc_t(half, -sweep, n_inner, n_inner);
        self.push(at(r_inner, inner_lo), at(r_outer, outer_lo), color);
        self.arc_chords(apex, e1, e2, r_outer, n_outer, -half, sweep, color);
        self.push(at(r_outer, outer_hi), at(r_inner, inner_hi), color);
        if r_inner > 0.0 {
            self.arc_chords(apex, e1, e2, r_inner, n_inner, half, -sweep, color);
        }
    }
}

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// Chord sagitta the adaptive circle count aims for, metres.
const SAGITTA: f32 = 0.02;

/// Chords in an arc of `radius` sweeping `sweep` radians: the full-circle count that keeps a chord's
/// sagitta near [`SAGITTA`] (clamped to 8..=64) scaled by the swept fraction, at least one.
fn segments_for(radius: f32, sweep: f32) -> usize {
    let full = (TAU * ludion_core::math::sqrt(radius / (8.0 * SAGITTA)))
        .ceil()
        .clamp(8.0, 64.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (full * sweep.abs() / TAU).ceil().max(1.0) as usize;
    n
}

/// An orthonormal pair `(e1, e2)` spanning the plane ⊥ `normal`, `e1` along `forward` projected into
/// it; `e2 = normal x e1`, so a positive sweep from `e1` toward `e2` is right-handed about `normal`.
fn plane_basis(normal: Vec3, forward: Vec3) -> Option<(Vec3, Vec3)> {
    let n = normal.try_normalize()?;
    let e1 = (forward - n * forward.dot(n)).try_normalize()?;
    Some((e1, n.cross(e1)))
}

/// The angle of chord point `i` of `segments` along `start..start + sweep`.
#[allow(clippy::cast_precision_loss)]
fn arc_t(start: f32, sweep: f32, i: usize, segments: usize) -> f32 {
    start + sweep * (i as f32) / (segments as f32)
}

/// `center + radius * (cos t * e1 + sin t * e2)`.
fn arc_point(center: Vec3, e1: Vec3, e2: Vec3, radius: f32, t: f32) -> Vec3 {
    let (s, c) = sin_cos(t);
    center + radius * (c * e1 + s * e2)
}

/// Whether every float a primitive was given is finite.
fn valid(points: &[[f32; 3]], scalars: &[f32], color: [f32; 4]) -> bool {
    points.iter().flatten().all(|v| v.is_finite())
        && scalars.iter().all(|v| v.is_finite())
        && color.iter().all(|v| v.is_finite())
}

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

/// Draws with a [`DebugPen`] into a scratch [`DebugDraw`] and returns the lines, so a host-side view
/// shares the primitives' chord counts and degenerate-input rules.
pub(crate) fn pen_lines(
    category: &'static str,
    draw: impl FnOnce(&mut DebugPen<'_>),
) -> Vec<DebugSegment> {
    let mut scratch = DebugDraw::default();
    draw(&mut DebugPen {
        draw: Some(&mut scratch),
        category,
    });
    scratch.pending.remove(category).unwrap_or_default()
}

/// The last completed tick's segments in the categories `filter` enables, in category order.
///
/// The `hierarchy` category is drawn here from the live world rather than by a system, so it is
/// always current, even paused, and costs nothing unless enabled. It sorts among the categories;
/// `probes`, `colliders` (every physics body's wireframe) and `nav` are drawn the same way. `nav`
/// shows the grid, the `NavRoute` polylines and the cached flow arrows; `nav:grid`, `nav:routes` and
/// `nav:flow` pick one part. Grid nodes and arrows within 30 m of the game camera are drawn (all when
/// there is no camera); routes are never culled. `perception` is drawn the same way from the last
/// tick's `PerceptionResults`: `perception:zones`, `perception:rays`, `perception:noises` and
/// `perception:memory` pick one part. `local_shadows`
/// is the exception: the engine appends it after extract, because it needs the camera's grants.
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
    let parts = crate::nav::NavDebugParts {
        grid: filter.enables(NAV) || filter.enables(NAV_GRID),
        routes: filter.enables(NAV) || filter.enables(NAV_ROUTES),
        flow: filter.enables(NAV) || filter.enables(NAV_FLOW),
    };
    if parts.grid || parts.routes || parts.flow {
        let eye = crate::extract::stream_camera(world, (1, 1), None)
            .ok()
            .map(|camera| camera.position.to_array());
        categories
            .entry(NAV)
            .or_default()
            .extend(crate::nav::debug_lines(world, parts, eye));
    }
    let parts = crate::perception_debug::PerceptionDebugParts {
        zones: filter.enables(PERCEPTION) || filter.enables(PERCEPTION_ZONES),
        rays: filter.enables(PERCEPTION) || filter.enables(PERCEPTION_RAYS),
        noises: filter.enables(PERCEPTION) || filter.enables(PERCEPTION_NOISES),
        memory: filter.enables(PERCEPTION) || filter.enables(PERCEPTION_MEMORY),
    };
    if parts.zones || parts.rays || parts.noises || parts.memory {
        categories
            .entry(PERCEPTION)
            .or_default()
            .extend(crate::perception_debug::debug_lines(world, parts));
    }
    categories.into_values().flatten().collect()
}

/// The categories the world has drawn: what [`DebugDraw::drawn`] lists plus `hierarchy` when any
/// entity has a `Parent`, `probes` when any has a `ReflectionProbe` and `colliders` when the
/// physics world holds a body, `nav`, `nav:grid`, `nav:routes` and `nav:flow` when there is something
/// to draw for them, `perception` and its four sub-filters likewise, `local_shadows` when any point or spot light casts and `cull` when
/// any entity has a `Mesh`, sorted.
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
    names.extend(crate::nav::debug_present(world));
    names.extend(crate::perception_debug::debug_present(world));
    if crate::extract::any_local_shadow_caster(world) {
        names.push(LOCAL_SHADOWS);
    }
    if crate::extract::any_mesh(world) {
        names.push(CULL);
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

    /// FNV-1a over every segment endpoint's bits: `sphere` and `capsule` geometry is frozen.
    #[test]
    fn sphere_and_capsule_output_is_pinned() {
        let mut world = world();
        debug_draw(&mut world, "t")
            .sphere([0.3, 1.0, -2.0], 0.7, RED)
            .capsule([0.0, 0.5, 0.0], [0.2, 1.5, 0.1], 0.4, RED)
            .capsule([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], 0.4, RED);
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for s in &world.resource::<DebugDraw>().unwrap().pending["t"] {
            for v in s.a.into_iter().chain(s.b) {
                for byte in v.to_bits().to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
        assert_eq!(hash, 12_741_808_649_133_389_326);
    }

    #[test]
    fn the_new_primitives_draw_their_segment_counts() {
        let up = [0.0, 1.0, 0.0];
        let fwd = [0.0, 0.0, 1.0];
        let n = |draw: &dyn Fn(&mut DebugPen<'_>)| count(&mut world(), |p| draw(p));
        assert_eq!(
            n(&|p| {
                p.circle([0.0; 3], up, 1.0, RED);
            }),
            16
        );
        assert_eq!(
            n(&|p| {
                p.arc([0.0; 3], up, fwd, 1.0, 90.0, RED);
            }),
            4
        );
        assert_eq!(
            n(&|p| {
                p.arc([0.0; 3], up, fwd, 1.0, -360.0, RED);
            }),
            16
        );
        assert_eq!(
            n(&|p| {
                p.wedge([0.0; 3], up, fwd, 45.0, 1.0, RED);
            }),
            2 + 4
        );
        assert_eq!(
            n(&|p| {
                p.ring_sector([0.0; 3], up, fwd, 45.0, 0.5, 1.0, RED);
            }),
            2 + 4 + 3
        );
        assert_eq!(
            n(&|p| {
                p.cone([0.0; 3], fwd, 45.0, 1.0, RED);
            }),
            16 + 4
        );
        assert_eq!(
            n(&|p| {
                p.frustum_wedge([0.0; 3], fwd, up, 45.0, 30.0, 1.0, RED);
            }),
            (2 + 4) + (2 + 3)
        );
    }

    #[test]
    fn the_segment_rule_scales_with_radius_and_sweep() {
        assert_eq!(segments_for(1.0, TAU), 16);
        assert_eq!(segments_for(0.3, TAU), 9);
        assert_eq!(segments_for(40.0, TAU), 64);
        assert_eq!(segments_for(0.001, TAU), 8);
        assert_eq!(segments_for(1.0, PI), 8);
        assert_eq!(segments_for(1.0, 0.001), 1);
        assert_eq!(segments_for(1.0, -TAU), 16);
    }

    #[test]
    fn a_90_degree_wedge_closes_on_its_edges() {
        let mut world = world();
        debug_draw(&mut world, "t").wedge(
            [1.0, 2.0, 3.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            45.0,
            2.0,
            RED,
        );
        let s = &world.resource::<DebugDraw>().unwrap().pending["t"];
        let (first, arc, last) = (s[0], &s[1..s.len() - 1], s[s.len() - 1]);
        assert_eq!(first.a, [1.0, 2.0, 3.0]);
        assert_eq!(
            arc[0].a, first.b,
            "the arc starts where the first edge ends"
        );
        assert_eq!(
            arc[arc.len() - 1].b,
            last.a,
            "the arc ends where the last edge starts"
        );
        assert_eq!(last.b, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_ring_sector_with_no_inner_radius_is_a_wedge() {
        let draw = |ring: bool| {
            let mut world = world();
            let (a, n, f) = ([0.5, 0.0, 0.5], [0.0, 1.0, 0.0], [1.0, 0.0, 1.0]);
            let mut pen = debug_draw(&mut world, "t");
            if ring {
                pen.ring_sector(a, n, f, 60.0, 0.0, 3.0, RED);
            } else {
                pen.wedge(a, n, f, 60.0, 3.0, RED);
            }
            world.resource::<DebugDraw>().unwrap().pending["t"].clone()
        };
        let (ring, wedge) = (draw(true), draw(false));
        assert!(!wedge.is_empty());
        assert_eq!(ring.len(), wedge.len());
        for (r, w) in ring.iter().zip(&wedge) {
            assert_eq!((r.a, r.b), (w.a, w.b));
        }
    }

    #[test]
    fn degenerate_shapes_draw_nothing() {
        let (o, up, fwd) = ([0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        let nan = [f32::NAN, 0.0, 0.0];
        let mut world = world();
        debug_draw(&mut world, "t")
            .circle(nan, up, 1.0, RED)
            .circle(o, [0.0; 3], 1.0, RED)
            .circle(o, up, 0.0, RED)
            .circle(o, up, f32::INFINITY, RED)
            .arc(o, up, up, 1.0, 90.0, RED)
            .arc(o, up, fwd, 1.0, 0.0, RED)
            .arc(o, up, fwd, 1.0, 361.0, RED)
            .wedge(o, up, up, 45.0, 1.0, RED)
            .wedge(o, up, fwd, 0.0, 1.0, RED)
            .wedge(o, up, fwd, 181.0, 1.0, RED)
            .wedge(o, up, fwd, 45.0, 0.0, RED)
            .ring_sector(o, up, fwd, 45.0, 1.0, 1.0, RED)
            .ring_sector(o, up, fwd, 45.0, -1.0, 1.0, RED)
            .cone(o, fwd, 90.0, 1.0, RED)
            .cone(o, fwd, 0.0, 1.0, RED)
            .cone(o, [0.0; 3], 45.0, 1.0, RED)
            .frustum_wedge(o, up, up, 45.0, 30.0, 1.0, RED)
            .frustum_wedge(o, fwd, up, 91.0, 30.0, 1.0, RED)
            .circle(o, up, 1.0, [f32::NAN, 0.0, 0.0, 1.0]);
        assert!(world.resource::<DebugDraw>().unwrap().pending.is_empty());
    }

    #[test]
    fn circle_and_arc_points_lie_on_the_circle_in_its_plane() {
        let (c, n) = ([1.0, 2.0, 3.0], Vec3::new(1.0, 2.0, -0.5).normalize());
        let mut world = world();
        debug_draw(&mut world, "t")
            .circle(c, n.to_array(), 2.5, RED)
            .arc(c, n.to_array(), [0.3, 0.1, 0.9], 2.5, -200.0, RED);
        let pending = &world.resource::<DebugDraw>().unwrap().pending["t"];
        assert!(pending.len() > 16);
        for s in pending {
            for p in [s.a, s.b] {
                let d = Vec3::from(p) - Vec3::from(c);
                assert!(
                    (d.length() - 2.5).abs() < 1e-5,
                    "{p:?} is {} away",
                    d.length()
                );
                assert!(d.dot(n).abs() < 1e-5, "{p:?} leaves the plane");
            }
        }
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

    /// The `local_shadows` marks: the fixture's shadowed point light (3 segments) and spot (4), green
    /// when granted and red when the quality masks shadows, absent when the category is off.
    #[test]
    fn the_local_shadows_category_marks_granted_green_and_masked_red() {
        const GREEN: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
        const DENIED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
        let mut engine = engine(0);
        engine
            .load_inline_scene(include_str!("../tests/fixtures/local-shadows.yaml"))
            .unwrap();
        engine.set_quality(crate::QualitySetting::full());
        assert!(lines(&mut engine).is_empty());

        engine.set_debug_draw(DebugCategories::parse("local_shadows"));
        assert_eq!(engine.debug_draw_drawn(), ["cull", "local_shadows"]);
        let marks = lines(&mut engine);
        assert_eq!(marks.len(), 3 + 4);
        assert!(marks.iter().all(|m| m.color == GREEN));

        engine.set_quality(
            crate::resolve_startup_quality(Some("low"), None).expect("`low` is a preset name"),
        );
        let marks = lines(&mut engine);
        assert_eq!(marks.len(), 3 + 4);
        assert!(marks.iter().all(|m| m.color == DENIED));

        engine.set_debug_draw(DebugCategories::None);
        assert!(lines(&mut engine).is_empty());
    }

    /// The `cull` marks: a mesh in front of the camera is a green box, one behind it a red box
    /// (12 edges each), and the category is absent when off.
    #[test]
    fn the_cull_category_marks_kept_green_and_culled_red() {
        const KEPT: [f32; 4] = [0.2, 1.0, 0.2, 1.0];
        const CULLED: [f32; 4] = [1.0, 0.2, 0.2, 1.0];
        let mut engine = engine(0);
        engine
            .load_inline_scene(
                "format: 1\nentities:\n  - components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 5.0\n      Camera: {}\n  \
                 - components:\n      Mesh: {}\n  \
                 - components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 20.0\n      Mesh: {}\n",
            )
            .unwrap();
        assert!(lines(&mut engine).is_empty());

        engine.set_debug_draw(DebugCategories::parse("cull"));
        assert_eq!(engine.debug_draw_drawn(), ["cull"]);
        let marks = lines(&mut engine);
        assert_eq!(marks.iter().filter(|m| m.color == KEPT).count(), 12);
        assert_eq!(marks.iter().filter(|m| m.color == CULLED).count(), 12);
        assert_eq!(marks.len(), 24);

        engine.set_debug_draw(DebugCategories::None);
        assert!(lines(&mut engine).is_empty());
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

    /// A block-style YAML 3-sequence at `indent` spaces.
    fn seq3(v: [f32; 3], indent: usize) -> String {
        v.iter()
            .map(|c| format!("{:indent$}- {c:?}\n", ""))
            .collect()
    }

    /// A 24 m floor, a camera over it, a goal-routed walker and a flow-field walker, on a 16 x 16 grid.
    fn nav_engine() -> crate::Engine {
        let root =
            std::env::temp_dir().join(format!("ludion-debug-draw-nav-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut engine = crate::Engine::builder()
            .project_root(&root)
            .nav(crate::NavConfig {
                nx: 16,
                nz: 16,
                y_min: -2.0,
                y_max: 8.0,
                ..crate::NavConfig::default()
            })
            .system("nav_update", crate::nav_update)
            .system("nav_steer", crate::nav_steer)
            .build()
            .unwrap();
        let mut yaml = String::from(
            "format: 1\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      Transform:\n        translation:\n",
        );
        yaml += &seq3([8.0, -0.5, 8.0], 10);
        yaml += "      Collider:\n        shape:\n          box:\n            half_extents:\n";
        yaml += &seq3([8.0, 0.5, 8.0], 14);
        yaml += "        body: static\n  - name: cam\n    components:\n      Camera: {}\n      Transform:\n        translation:\n";
        yaml += &seq3([8.0, 10.0, 8.0], 10);
        for (name, at, goal, kind) in [
            (
                "routed",
                [2.0, 0.0, 2.0],
                [12.0, 0.0, 12.0],
                "NavGoal:\n        at:\n",
            ),
            (
                "flowing",
                [3.0, 0.0, 3.0],
                [12.0, 0.0, 4.0],
                "NavAgent:\n        target:\n",
            ),
        ] {
            yaml += &format!(
                "  - name: {name}\n    components:\n      Transform:\n        translation:\n"
            );
            yaml += &seq3(at, 10);
            if kind.starts_with("NavGoal") {
                yaml += "      NavAgent:\n        neighbor_layers: 0\n      ";
            } else {
                yaml += "      ";
            }
            yaml += kind;
            yaml += &seq3(goal, 10);
            if kind.starts_with("NavAgent") {
                yaml += "        neighbor_layers: 0\n";
            }
        }
        engine.load_inline_scene(&yaml).unwrap();
        for _ in 0..4 {
            engine.advance_live();
        }
        engine
    }

    #[test]
    fn the_nav_category_draws_the_grid_routes_and_flow_with_sub_filters() {
        let mut engine = nav_engine();
        assert!(
            lines(&mut engine).is_empty(),
            "nothing is enabled by default"
        );
        let drawn = engine.debug_draw_drawn();
        for name in [NAV, NAV_GRID, NAV_ROUTES, NAV_FLOW] {
            assert!(drawn.contains(&name), "{name} missing from {drawn:?}");
        }
        let mut count = |filter: &str| {
            engine.set_debug_draw(DebugCategories::parse(filter));
            lines(&mut engine).len()
        };
        let (all, grid, routes, flow) = (
            count("nav"),
            count("nav:grid"),
            count("nav:routes"),
            count("nav:flow"),
        );
        assert!(grid > 0 && routes > 0 && flow > 0);
        assert!(routes < all && flow < all && grid < all);
        assert_eq!(all, grid + routes + flow, "`nav` is the three parts");
        assert_eq!(count("nav:routes,nav:flow"), routes + flow);
        assert_eq!(count("motor"), 0);
        assert_eq!(count(""), 0);
    }

    #[test]
    fn pen_lines_matches_the_pens_own_chords() {
        let mut world = world();
        let own = count(&mut world, |pen| {
            pen.circle([0.0; 3], [0.0, 1.0, 0.0], 2.0, RED);
        });
        let drawn = pen_lines("x", |pen| {
            pen.circle([0.0; 3], [0.0, 1.0, 0.0], 2.0, RED);
        });
        assert_eq!(drawn.len(), own);
        assert!(
            pen_lines("x", |pen| {
                pen.circle([0.0; 3], [0.0, 1.0, 0.0], -1.0, RED);
            })
            .is_empty()
        );
    }

    const GREEN: [f32; 4] = crate::perception_debug::SEEN;

    /// A guard at the origin facing -Z with one zone, a runner 10 m away with a clear point and a
    /// point behind a wall, memory on, and a listener-side `bang` emitted before the last tick.
    fn perception_engine() -> crate::Engine {
        let root = std::env::temp_dir().join(format!(
            "ludion-debug-draw-perception-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut engine = crate::Engine::builder()
            .project_root(&root)
            .perception(crate::PerceptionConfig {
                classes: vec!["player".into()],
                noise_kinds: vec!["bang".into()],
                ..crate::PerceptionConfig::default()
            })
            .system("perception_update", crate::perception_update)
            .build()
            .unwrap();
        let yaml = "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: wall\nentities:\n  \
            - name: guard\n    components:\n      Transform: {}\n      Perceiver:\n        eye:\n          - 0.0\n          \
            - 0.2\n          - 0.0\n        memory: {}\n        hearing:\n          hears:\n            - bang\n        \
            sight:\n          - name: main\n            range: 20.0\n            half_angle_deg: 60.0\n  \
            - name: runner\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          \
            - -10.0\n      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              \
            - 0.2\n              - 0.0\n            blocked_by:\n              - wall\n          - offset:\n              - 0.0\n              \
            - 3.0\n              - 0.0\n            blocked_by:\n              - wall\n  \
            - name: wall1\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 1.5\n          \
            - -5.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 2.0\n              \
            - 0.5\n              - 0.1\n        body: static\n        layer: wall\n";
        engine.load_inline_scene(yaml).unwrap();
        engine.advance_live();
        engine.advance_live();
        let bang = crate::Noise {
            at: [1.0, 0.0, 0.0],
            radius: 6.0,
            loudness: 1.0,
            kind: "bang".into(),
            source: None,
            surface: None,
            occlusion: false,
        };
        let ctx = ludion_ecs::SystemCtx::new(
            ludion_core::time::FrameIndex::new(0),
            ludion_core::time::TickRate::DEFAULT,
        );
        assert!(crate::emit_noise(engine.world_mut(), &ctx, bang).is_some());
        engine.advance_live();
        engine
    }

    fn lines_under(engine: &mut crate::Engine, filter: &str) -> Vec<DebugSegment> {
        engine.set_debug_draw(DebugCategories::parse(filter));
        lines(engine)
    }

    #[test]
    fn the_perception_category_draws_zones_rays_noises_and_memory_with_sub_filters() {
        let mut engine = perception_engine();
        assert!(
            lines(&mut engine).is_empty(),
            "nothing is enabled by default"
        );
        let drawn = engine.debug_draw_drawn();
        for name in [
            PERCEPTION,
            PERCEPTION_ZONES,
            PERCEPTION_RAYS,
            PERCEPTION_NOISES,
            PERCEPTION_MEMORY,
        ] {
            assert!(drawn.contains(&name), "{name} missing from {drawn:?}");
        }
        let all = lines_under(&mut engine, "perception").len();
        let zones = lines_under(&mut engine, "perception:zones").len();
        let rays = lines_under(&mut engine, "perception:rays").len();
        let noises = lines_under(&mut engine, "perception:noises").len();
        let memory = lines_under(&mut engine, "perception:memory").len();
        assert!(zones > 0 && rays > 0 && noises > 0 && memory > 0);
        assert_eq!(all, zones + rays + noises + memory);
        assert_eq!(lines_under(&mut engine, "motor").len(), 0);
    }

    #[test]
    fn a_seen_point_draws_a_green_ray_and_a_blocked_point_a_red_one() {
        let mut engine = perception_engine();
        let rays = lines_under(&mut engine, "perception:rays");
        let seen = rays.iter().filter(|s| s.color == GREEN).count();
        let blocked = rays
            .iter()
            .filter(|s| s.color == crate::perception_debug::BLOCKED)
            .count();
        assert_eq!(seen, 1, "one clear point");
        assert!(blocked >= 1, "the walled point and its blocker cross");
    }

    #[test]
    fn a_visible_zone_is_green_and_a_disabled_perceiver_draws_no_zone() {
        let mut engine = perception_engine();
        let zones = lines_under(&mut engine, "perception:zones");
        assert!(zones.iter().all(|s| s.color == GREEN));
        let guard = engine
            .world_mut()
            .query_ids::<(&crate::Perceiver,)>()
            .next()
            .map(|(id, _)| id)
            .unwrap();
        engine
            .world_mut()
            .get_mut::<crate::Perceiver>(guard)
            .unwrap()
            .unwrap()
            .enabled = false;
        assert!(lines_under(&mut engine, "perception:zones").is_empty());
    }

    #[test]
    fn perception_debug_never_moves_the_world_hash_or_the_extract() {
        let hashes = |enabled: bool| {
            let mut engine = perception_engine();
            let mut trace = Vec::new();
            for _ in 0..10 {
                if enabled {
                    lines_under(&mut engine, "perception");
                }
                engine.advance_live();
                trace.push(engine.world_hash().unwrap().as_u64());
            }
            trace
        };
        assert_eq!(hashes(true), hashes(false));
    }

    #[test]
    fn a_disabled_category_leaves_the_extracted_scene_untouched() {
        let mut toggled = perception_engine();
        let mut never = perception_engine();
        toggled.set_debug_draw(DebugCategories::parse("perception"));
        assert!(!lines(&mut toggled).is_empty());
        toggled.set_debug_draw(DebugCategories::None);
        let after = toggled.extract_render_scene(64, 64).unwrap();
        let baseline = never.extract_render_scene(64, 64).unwrap();
        assert!(
            after.debug_lines.is_empty(),
            "empty under the default filter"
        );
        assert_eq!(after, baseline);

        toggled.set_debug_draw(DebugCategories::parse("colliders"));
        let collider_lines = lines(&mut toggled);
        assert_eq!(
            collider_lines,
            crate::scene_physics::collider_debug_lines(toggled.world())
        );
        never.set_debug_draw(DebugCategories::parse("colliders"));
        assert_eq!(
            toggled.extract_render_scene(64, 64).unwrap(),
            never.extract_render_scene(64, 64).unwrap()
        );
    }
}
