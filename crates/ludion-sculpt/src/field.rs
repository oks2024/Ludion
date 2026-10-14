//! Signed distances for the six sculpt primitives and the polynomial smooth min/max that fold them.
//!
//! Distances are negative inside, zero on the surface, positive outside. Every primitive is
//! evaluated in its local frame; [`PartTransform`] maps a model-space sample into that frame by the
//! inverse rotation. Arithmetic is plain `f32`; `sqrt`, `sin` and `cos` go through
//! [`ludion_core::math`] so the field is bit-identical on every platform.

use std::sync::Arc;

use ludion_core::math;

use crate::edt::SignedDistanceGrid;
use crate::sources::PAD;

/// A voxel model's occupancy as a distance field, in the part's local frame.
///
/// The grid keeps the voxel origin convention — x and z centred on the model, y standing on the
/// ground — scaled by `size` world units per voxel, so the part's local origin is the model's
/// bottom centre and its `at` places exactly what a `Mesh.shape: voxel` entity's translation would.
/// The transform itself is in voxel units, so the sample scales back out by `size`.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelField {
    /// Shared across the parts naming one document: the transform runs once per distinct path.
    grid: Arc<SignedDistanceGrid>,
    /// World units per voxel.
    size: f32,
    /// The local-frame position of the padded grid's coordinate `(0, 0, 0)`.
    origin: [f32; 3],
    /// The model's own local-frame box. The padding ring carries no occupancy, so no part of the
    /// surface leaves it.
    bounds: ([f32; 3], [f32; 3]),
}

impl VoxelField {
    /// The field of `grid` — the transform of a `dims` model padded by [`crate::sources::PAD`] —
    /// at `size` world units per voxel.
    pub(crate) fn new(grid: Arc<SignedDistanceGrid>, dims: [usize; 3], size: f32) -> Self {
        debug_assert_eq!(
            grid.dims(),
            dims.map(|n| n + 2 * PAD),
            "the transform must carry the padding ring this frame is measured against"
        );
        let span = dims.map(|n| n as f32 * size);
        let (half_x, half_z) = (span[0] * 0.5, span[2] * 0.5);
        let pad = PAD as f32 * size;
        VoxelField {
            grid,
            size,
            origin: [-half_x - pad, -pad, -half_z - pad],
            bounds: ([-half_x, 0.0, -half_z], [half_x, span[1], half_z]),
        }
    }

    /// Signed distance from the local-frame point `p`, in world units.
    fn distance(&self, p: [f32; 3]) -> f32 {
        let voxel = [0, 1, 2].map(|axis| (p[axis] - self.origin[axis]) / self.size);
        self.grid.sample(voxel) * self.size
    }
}

/// An imported mesh's triangle soup as a distance field, in the part's local frame.
///
/// The sibling of [`VoxelField`] over a mesh source (`crate::sources`): the grid is the soup's
/// signed distance sampled on a lattice of the mesh's own box, so the part's local frame is the mesh
/// document's own coordinates scaled by `size` world units per document unit, and `at` places
/// exactly what a `Mesh.shape: mesh` entity's `Transform` would.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshField {
    /// Shared across the parts naming one document: the sampling runs once per distinct path.
    grid: Arc<SignedDistanceGrid>,
    /// World units one grid cell spans, the source's own cell scaled by the part's `size`.
    cell: f32,
    /// The local-frame position of the lattice's coordinate `(0, 0, 0)`.
    origin: [f32; 3],
    /// The mesh's own box in the local frame. The padding ring carries no surface, so none of it
    /// leaves this box.
    bounds: ([f32; 3], [f32; 3]),
}

impl MeshField {
    /// The field of a mesh source — `grid` sampled at `cell` document units per lattice cell with
    /// its coordinate `(0, 0, 0)` at `origin`, over a mesh whose own box is `bounds` — read at
    /// `size` world units per document unit.
    pub(crate) fn new(
        grid: Arc<SignedDistanceGrid>,
        cell: f32,
        origin: [f32; 3],
        bounds: ([f32; 3], [f32; 3]),
        size: f32,
    ) -> Self {
        MeshField {
            grid,
            cell: cell * size,
            origin: scale(origin, size),
            bounds: (scale(bounds.0, size), scale(bounds.1, size)),
        }
    }

    /// Signed distance from the local-frame point `p`, in world units.
    fn distance(&self, p: [f32; 3]) -> f32 {
        let lattice = [0, 1, 2].map(|axis| (p[axis] - self.origin[axis]) / self.cell);
        self.grid.sample(lattice) * self.cell
    }
}

/// Smooth minimum of the running field `a` and an incoming part `b` over blend radius `k`.
///
/// Returns `(value, weight)`. With `h = clamp(0.5 + 0.5 (b - a) / k, 0, 1)` the value is the
/// polynomial smooth min `mix(b, a, h) - k h (1 - h)`. `h` is the share of `a`; the returned
/// `weight = 1 - h` is the share of the incoming part `b` — its material / skinning weight.
///
/// Outside the blend band (`|b - a| >= k`) the value is exactly `min(a, b)` and the weight exactly
/// `1` (b wins) or `0` (a wins), so `k <= 0` is the exact `min` with no division. A tie keeps `a`
/// (weight `0`), and an unordered difference (both operands the same infinity, or a NaN) keeps `a`.
/// The empty field `a = +inf` returns `(b, 1)`.
pub fn smin(a: f32, b: f32, k: f32) -> (f32, f32) {
    let k = k.max(0.0);
    let d = b - a;
    if d >= k || d.is_nan() {
        return (a, 0.0);
    }
    if d <= -k {
        return (b, 1.0);
    }
    let h = 0.5 + 0.5 * d / k;
    (b + (a - b) * h - k * h * (1.0 - h), 1.0 - h)
}

/// Smooth maximum of the running field `a` and an incoming part `b` over blend radius `k`:
/// exactly `-smin(-a, -b, k)`, value negated, weight unchanged — `weight` is the share of `b`.
///
/// `k <= 0` is the exact `max`, weight `1` when `b > a` and `0` otherwise.
pub fn smax(a: f32, b: f32, k: f32) -> (f32, f32) {
    let (value, weight) = smin(-a, -b, k);
    (-value, weight)
}

/// Sphere of `radius` centered on the origin.
pub fn sphere(p: [f32; 3], radius: f32) -> f32 {
    length(p) - radius
}

/// Capsule of `radius` around the segment `from`–`to`. A zero-length segment is a sphere at `from`.
pub fn capsule(p: [f32; 3], radius: f32, from: [f32; 3], to: [f32; 3]) -> f32 {
    let pa = sub(p, from);
    let ba = sub(to, from);
    let bb = dot(ba, ba);
    let h = if bb > 0.0 {
        (dot(pa, ba) / bb).clamp(0.0, 1.0)
    } else {
        0.0
    };
    length(sub(pa, scale(ba, h))) - radius
}

/// Axis-aligned ellipsoid with semi-axes `radii` (each `> 0`) centered on the origin.
///
/// The bound-and-refine approximation `k0 (k0 - 1) / k1`, `k0 = |p / r|`, `k1 = |p / r²|`: its sign
/// is exact (that of `k0 - 1`) and it is exact on the three axes. The center returns
/// `-min(radii)`, where the formula divides by zero.
pub fn ellipsoid(p: [f32; 3], radii: [f32; 3]) -> f32 {
    let k0 = length([p[0] / radii[0], p[1] / radii[1], p[2] / radii[2]]);
    let k1 = length([
        p[0] / (radii[0] * radii[0]),
        p[1] / (radii[1] * radii[1]),
        p[2] / (radii[2] * radii[2]),
    ]);
    if k1 == 0.0 {
        return -radii[0].min(radii[1]).min(radii[2]);
    }
    k0 * (k0 - 1.0) / k1
}

/// Box of full extents `size` centered on the origin, its edges rounded by `round`.
///
/// The outer extents stay `size` whatever `round` is; `round` is clamped to `[0, min(size) / 2]`.
pub fn rounded_box(p: [f32; 3], size: [f32; 3], round: f32) -> f32 {
    let half = scale(size, 0.5);
    let r = round.max(0.0).min(half[0].min(half[1]).min(half[2]));
    let q = [
        p[0].abs() - half[0] + r,
        p[1].abs() - half[1] + r,
        p[2].abs() - half[2] + r,
    ];
    let outside = length([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
    let inside = q[0].max(q[1]).max(q[2]).min(0.0);
    outside + inside - r
}

/// Torus around the Y axis: its ring of major `radius` lies in the XZ plane, `tube` is the minor
/// radius.
pub fn torus(p: [f32; 3], radius: f32, tube: f32) -> f32 {
    let ring = math::sqrt(p[0] * p[0] + p[2] * p[2]) - radius;
    math::sqrt(ring * ring + p[1] * p[1]) - tube
}

/// Solid capped cone, apex up: the base disc of `radius` is centered on the origin in the XZ plane
/// and the apex sits at `(0, height, 0)`.
///
/// Exact: the distance to the nearer of the base disc and the slanted side, measured in the
/// cross-section `(|p.xz|, p.y)`.
pub fn cone(p: [f32; 3], radius: f32, height: f32) -> f32 {
    let x = math::sqrt(p[0] * p[0] + p[2] * p[2]);
    let y = p[1];

    let base_dx = (x - radius).max(0.0);
    let base = base_dx * base_dx + y * y;

    // The side runs from the rim (radius, 0) along e = (-radius, height) to the apex.
    let (ex, ey) = (-radius, height);
    let ee = ex * ex + ey * ey;
    let t = if ee > 0.0 {
        (((x - radius) * ex + y * ey) / ee).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let sx = x - radius - ex * t;
    let sy = y - ey * t;
    let side = sx * sx + sy * sy;

    let distance = math::sqrt(base.min(side));
    let inside = y > 0.0 && x * height < radius * (height - y);
    if inside { -distance } else { distance }
}

/// One sculpt primitive with its shape parameters, in its local frame.
///
/// Every analytic shape is `Copy`-sized; [`Primitive::Voxel`] carries a shared distance grid
/// instead, which is why the enum clones rather than copies.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    /// See [`sphere`].
    Sphere {
        /// Radius.
        radius: f32,
    },
    /// See [`capsule`]. The endpoints are local-frame coordinates.
    Capsule {
        /// Radius around the segment.
        radius: f32,
        /// First segment endpoint.
        from: [f32; 3],
        /// Second segment endpoint.
        to: [f32; 3],
    },
    /// See [`ellipsoid`].
    Ellipsoid {
        /// Semi-axes along X, Y, Z.
        radii: [f32; 3],
    },
    /// See [`rounded_box`].
    Box {
        /// Full extents along X, Y, Z.
        size: [f32; 3],
        /// Edge rounding radius; `0` is the sharp box.
        round: f32,
    },
    /// See [`torus`].
    Torus {
        /// Major radius of the ring in the XZ plane.
        radius: f32,
        /// Minor radius of the tube.
        tube: f32,
    },
    /// See [`cone`]: base disc centered on the local origin, apex at `(0, height, 0)`.
    Cone {
        /// Base disc radius.
        radius: f32,
        /// Base-to-apex height.
        height: f32,
    },
    /// See [`VoxelField`]: a voxel model's occupancy, bottom-centred in its local frame.
    Voxel(VoxelField),
    /// See [`MeshField`]: an imported mesh's triangles, in the mesh's own coordinates.
    Mesh(MeshField),
}

impl Primitive {
    /// Signed distance from the local-frame point `p`.
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        match self {
            Primitive::Voxel(voxel) => voxel.distance(p),
            Primitive::Mesh(mesh) => mesh.distance(p),
            &Primitive::Sphere { radius } => sphere(p, radius),
            &Primitive::Capsule { radius, from, to } => capsule(p, radius, from, to),
            &Primitive::Ellipsoid { radii } => ellipsoid(p, radii),
            &Primitive::Box { size, round } => rounded_box(p, size, round),
            &Primitive::Torus { radius, tube } => torus(p, radius, tube),
            &Primitive::Cone { radius, height } => cone(p, radius, height),
        }
    }

    /// The local-frame axis-aligned box `(min, max)` enclosing the solid.
    pub fn local_bounds(&self) -> ([f32; 3], [f32; 3]) {
        match self {
            Primitive::Voxel(voxel) => voxel.bounds,
            Primitive::Mesh(mesh) => mesh.bounds,
            &Primitive::Sphere { radius } => ([-radius; 3], [radius; 3]),
            &Primitive::Capsule { radius, from, to } => (
                [
                    from[0].min(to[0]) - radius,
                    from[1].min(to[1]) - radius,
                    from[2].min(to[2]) - radius,
                ],
                [
                    from[0].max(to[0]) + radius,
                    from[1].max(to[1]) + radius,
                    from[2].max(to[2]) + radius,
                ],
            ),
            &Primitive::Ellipsoid { radii } => (scale(radii, -1.0), radii),
            &Primitive::Box { size, .. } => (scale(size, -0.5), scale(size, 0.5)),
            &Primitive::Torus { radius, tube } => {
                let reach = radius + tube;
                ([-reach, -tube, -reach], [reach, tube, reach])
            }
            &Primitive::Cone { radius, height } => {
                ([-radius, 0.0, -radius], [radius, height, radius])
            }
        }
    }
}

/// Where a part sits in model space: its local origin `at` and the inverse of its rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartTransform {
    /// Model-space position of the local origin.
    pub at: [f32; 3],
    /// Model-to-local rotation, rows first (`local[i] = dot(inverse_rotation[i], p - at)`).
    pub inverse_rotation: [[f32; 3]; 3],
}

impl PartTransform {
    /// A transform from `at` and Euler XYZ angles in degrees — the engine's `Transform.rotation`
    /// convention, intrinsic `R = Rx · Ry · Rz` (glam's `EulerRot::XYZ`), built with
    /// [`ludion_core::math`] trig. `[0, 0, 0]` is exactly the identity.
    pub fn new(at: [f32; 3], rotation_degrees: [f32; 3]) -> Self {
        let [rx, ry, rz] = rotation_degrees.map(|deg| deg * DEG_TO_RAD);
        let (sx, cx) = (math::sin(rx), math::cos(rx));
        let (sy, cy) = (math::sin(ry), math::cos(ry));
        let (sz, cz) = (math::sin(rz), math::cos(rz));
        let x = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
        let y = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let z = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
        let rotation = mat_mul(mat_mul(x, y), z);
        PartTransform {
            at,
            inverse_rotation: transpose(rotation),
        }
    }

    /// The model-space point `p` in the part's local frame: `R⁻¹ (p - at)`.
    pub fn to_local(&self, p: [f32; 3]) -> [f32; 3] {
        mat_vec(self.inverse_rotation, sub(p, self.at))
    }

    /// The local-frame point `p` in model space: `at + R p`.
    pub fn to_model(&self, p: [f32; 3]) -> [f32; 3] {
        add(self.at, mat_vec(transpose(self.inverse_rotation), p))
    }
}

/// A primitive placed in model space.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedPrimitive {
    /// The shape in its local frame.
    pub primitive: Primitive,
    /// Where the local frame sits in model space.
    pub transform: PartTransform,
}

impl PlacedPrimitive {
    /// Signed distance from the model-space point `p`.
    pub fn distance(&self, p: [f32; 3]) -> f32 {
        self.primitive.distance(self.transform.to_local(p))
    }

    /// A conservative model-space axis-aligned box `(min, max)` enclosing the solid: the eight
    /// corners of the local bounds carried into model space.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        self.bounds_of(self.primitive.local_bounds())
    }

    /// [`PlacedPrimitive::bounds`] over a local box the caller supplies — a
    /// [`Modifiers::local_bounds`] when the part deforms.
    pub fn bounds_of(&self, (lo, hi): ([f32; 3], [f32; 3])) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in 0..8 {
            let local = [
                if corner & 1 == 0 { lo[0] } else { hi[0] },
                if corner & 2 == 0 { lo[1] } else { hi[1] },
                if corner & 4 == 0 { lo[2] } else { hi[2] },
            ];
            let model = self.transform.to_model(local);
            for axis in 0..3 {
                min[axis] = min[axis].min(model[axis]);
                max[axis] = max[axis].max(model[axis]);
            }
        }
        (min, max)
    }
}

/// A `bend` or a `twist`: `axis` is `0`, `1` or `2`, and the rotation grows by `degrees` per world
/// unit travelled along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    /// The axis the angle grows along.
    pub axis: usize,
    /// Degrees per world unit, either sign.
    pub degrees: f32,
}

/// A `taper`: along `axis`, the two perpendicular coordinates are scaled from `from` to `to` across
/// the part's own undeformed extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Taper {
    /// The axis the scale runs along.
    pub axis: usize,
    /// The scale at the low end of the extent, `> 0`.
    pub from: f32,
    /// The scale at the high end, `> 0`.
    pub to: f32,
}

/// A seeded value-noise displacement of a part's distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Noise {
    /// Peak displacement in world units.
    pub amplitude: f32,
    /// The noise lattice's spacing in world units, `> 0`.
    pub scale: f32,
    /// The seed the lattice values are hashed from.
    pub seed: u64,
}

impl Noise {
    /// The displacement at the local-frame point `p`, in `±amplitude`.
    fn displacement(&self, p: [f32; 3]) -> f32 {
        let lattice = [p[0] / self.scale, p[1] / self.scale, p[2] / self.scale];
        self.amplitude * (value_noise(lattice, self.seed) * 2.0 - 1.0)
    }
}

/// What a part's `deform` and `noise` do to its field, compiled once against the primitive's own
/// local bounds — the deformation frame, the taper's normalisation and the grown box all need them.
///
/// **Frame.** Every deformer acts about the *part's own centre*: the midpoint of the primitive's
/// undeformed local box. A [`Primitive::Capsule`]'s endpoints are document coordinates, so its local
/// frame is the model frame and a bend measured from the local origin would swing the part across
/// the model; measuring from the box centre makes `bend`, `twist` and `taper` mean the same thing
/// for every shape — the part turns about itself, wherever it sits. A primitive whose box is already
/// centred on its local origin (a sphere, a box, an ellipsoid, a torus) skips the shift entirely,
/// so its arithmetic is untouched.
///
/// **Order.** The sample point passes through `bend`, then `twist`, then `taper`, whatever order the
/// document writes them in, and the noise is added to the distance afterwards. So the domain map is
/// `D = taper ∘ twist ∘ bend` and the solid is its preimage: the box travels back through the
/// inverses in the opposite order, taper first.
///
/// **Bounds, not distance.** `primitive(D(p))` is not a true distance — a deformer stretches the
/// domain, so the magnitude is off by up to `|∂D/∂p|` — but it is sign-exact and monotone across the
/// surface, which is all surface nets reads: the sign per corner, then one linear interpolation
/// along a crossed edge. Nothing rescales it, because a per-part divisor also shrinks the part's
/// value inside the smooth-min fold, where it would win the blend share far outside its own
/// geometry and steal a neighbour's surface and material. The conservative bound lives in the box
/// instead: the primitive's box, padded by `noise.amplitude` in the primitive's own frame, travels
/// back through the deformers' inverses, and that box is where the surface can be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modifiers {
    bend: Option<Turn>,
    twist: Option<Turn>,
    taper: Option<Taper>,
    noise: Option<Noise>,
    /// The deformation frame's origin in the primitive's local frame, the undeformed box's centre;
    /// `None` when that centre is exactly the local origin and the shift is a no-op.
    centre: Option<[f32; 3]>,
    /// The taper's `(low end, extent)` along its axis, from the primitive's undeformed box measured
    /// in the deformation frame.
    taper_extent: (f32, f32),
    bounds: ([f32; 3], [f32; 3]),
}

impl Modifiers {
    /// Compiles the part's modifiers against `local_bounds`, the primitive's own undeformed box.
    pub fn new(
        local_bounds: ([f32; 3], [f32; 3]),
        bend: Option<Turn>,
        twist: Option<Turn>,
        taper: Option<Taper>,
        noise: Option<Noise>,
    ) -> Self {
        let midpoint = [0, 1, 2].map(|axis| (local_bounds.0[axis] + local_bounds.1[axis]) * 0.5);
        let centre = (midpoint != [0.0; 3]).then_some(midpoint);
        let relative = match centre {
            Some(c) => (sub(local_bounds.0, c), sub(local_bounds.1, c)),
            None => local_bounds,
        };

        let taper_extent = taper.map_or((0.0, 0.0), |taper| {
            let (lo, hi) = (relative.0[taper.axis], relative.1[taper.axis]);
            (lo, hi - lo)
        });

        // The box is grown in the deformation frame, back through the domain map's inverses in the
        // opposite order to the one `distance` applies them. `noise` displaces the primitive's own
        // distance, so it pads the primitive's box first and the deformers carry the padding.
        let mut bounds = relative;
        if let Some(noise) = noise {
            let reach = noise.amplitude.abs();
            bounds.0 = bounds.0.map(|v| v - reach);
            bounds.1 = bounds.1.map(|v| v + reach);
        }
        if let Some(taper) = taper {
            bounds = grow_taper(bounds, taper);
        }
        if let Some(twist) = twist {
            bounds = grow_plane(bounds, perpendicular(twist.axis));
        }
        if let Some(bend) = bend {
            bounds = grow_plane(bounds, bend_plane(bend.axis));
        }
        if let Some(c) = centre {
            bounds = (add(bounds.0, c), add(bounds.1, c));
        }

        Modifiers {
            bend,
            twist,
            taper,
            noise,
            centre,
            taper_extent,
            bounds,
        }
    }

    /// The local-frame box the deformed, displaced solid stays inside, for [`PlacedPrimitive::
    /// bounds_of`].
    pub fn local_bounds(&self) -> ([f32; 3], [f32; 3]) {
        self.bounds
    }

    /// `primitive`'s signed distance at the local-frame point `p`, deformed and displaced.
    ///
    /// Sign-exact, not a true distance: see the type's **Bounds, not distance**.
    pub fn distance(&self, primitive: &Primitive, p: [f32; 3]) -> f32 {
        let mut q = match self.centre {
            Some(c) => sub(p, c),
            None => p,
        };
        if let Some(bend) = self.bend {
            q = turn(
                q,
                bend_plane(bend.axis),
                bend.degrees * DEG_TO_RAD * q[bend.axis],
            );
        }
        if let Some(twist) = self.twist {
            q = turn(
                q,
                perpendicular(twist.axis),
                twist.degrees * DEG_TO_RAD * q[twist.axis],
            );
        }
        if let Some(taper) = self.taper {
            let scale = self.taper_scale(taper, q[taper.axis]);
            for axis in perpendicular(taper.axis) {
                q[axis] /= scale;
            }
        }
        if let Some(c) = self.centre {
            q = add(q, c);
        }
        let mut distance = primitive.distance(q);
        if let Some(noise) = self.noise {
            distance += noise.displacement(q);
        }
        distance
    }

    /// The cross-section scale at `coordinate` along the taper's axis: `from` at the low end of the
    /// part's undeformed extent, `to` at the high end, clamped outside it.
    fn taper_scale(&self, taper: Taper, coordinate: f32) -> f32 {
        let (low, extent) = self.taper_extent;
        let t = if extent > 0.0 {
            ((coordinate - low) / extent).clamp(0.0, 1.0)
        } else {
            0.0
        };
        taper.from + (taper.to - taper.from) * t
    }
}

/// A rotation of `angle` radians in the plane of `[a, b]`, the third coordinate untouched.
fn turn(p: [f32; 3], [a, b]: [usize; 2], angle: f32) -> [f32; 3] {
    let (s, c) = (math::sin(angle), math::cos(angle));
    let mut q = p;
    q[a] = c * p[a] - s * p[b];
    q[b] = s * p[a] + c * p[b];
    q
}

/// A bend turns in the plane of its axis and the next one (`x`→`y`, `y`→`z`, `z`→`x`).
fn bend_plane(axis: usize) -> [usize; 2] {
    [axis, (axis + 1) % 3]
}

/// The two axes a twist (or a taper) leaves its own axis out of, in ascending index order.
fn perpendicular(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

/// The box's largest radius in the plane of `[a, b]`.
fn radius(bounds: ([f32; 3], [f32; 3]), [a, b]: [usize; 2]) -> f32 {
    let reach = |axis: usize| bounds.0[axis].abs().max(bounds.1[axis].abs());
    let (ra, rb) = (reach(a), reach(b));
    math::sqrt(ra * ra + rb * rb)
}

/// A rotation in the plane of `[a, b]` preserves both the radius in that plane and the third
/// coordinate, so the preimage of the box sits inside the cylinder of that radius — whatever the
/// angle.
fn grow_plane(bounds: ([f32; 3], [f32; 3]), plane: [usize; 2]) -> ([f32; 3], [f32; 3]) {
    let r = radius(bounds, plane);
    let (mut lo, mut hi) = bounds;
    for axis in plane {
        lo[axis] = -r;
        hi[axis] = r;
    }
    (lo, hi)
}

/// A taper divides the perpendicular coordinates by a scale in `[min(from, to), max(from, to)]`, so
/// the preimage of the box is the box's perpendicular extents multiplied by that range.
fn grow_taper(bounds: ([f32; 3], [f32; 3]), taper: Taper) -> ([f32; 3], [f32; 3]) {
    let (smallest, largest) = (taper.from.min(taper.to), taper.from.max(taper.to));
    let (mut lo, mut hi) = bounds;
    for axis in perpendicular(taper.axis) {
        let corners = [
            bounds.0[axis] * smallest,
            bounds.0[axis] * largest,
            bounds.1[axis] * smallest,
            bounds.1[axis] * largest,
        ];
        lo[axis] = corners.iter().copied().fold(f32::INFINITY, f32::min);
        hi[axis] = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    }
    (lo, hi)
}

/// Value noise in `[0, 1)`: the eight lattice values around `p` interpolated trilinearly, each axis'
/// weight run through the standard `3t² - 2t³` fade so the field is smooth across a lattice plane
/// rather than creased along it.
pub fn value_noise(p: [f32; 3], seed: u64) -> f32 {
    let base = [floor(p[0]), floor(p[1]), floor(p[2])];
    let fade = [0, 1, 2].map(|axis| {
        let t = p[axis] - base[axis] as f32;
        t * t * (3.0 - 2.0 * t)
    });
    let mut value = 0.0;
    for corner in 0..8 {
        let step = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let weight = [0, 1, 2]
            .map(|axis| {
                if step[axis] == 0 {
                    1.0 - fade[axis]
                } else {
                    fade[axis]
                }
            })
            .iter()
            .product::<f32>();
        let at = [0, 1, 2].map(|axis| base[axis] + step[axis] as i64);
        value += weight * lattice_value(at, seed);
    }
    value
}

/// The `[0, 1)` value of one lattice point, an explicit integer hash of its coordinates and the
/// seed — stateless, so there is no stream to advance and no order to get out of step. The sprite
/// bake's per-texel draw, addressed by lattice point instead.
fn lattice_value(at: [i64; 3], seed: u64) -> f32 {
    let mut state = splitmix64(seed);
    for coordinate in at {
        state = splitmix64(state ^ splitmix64(coordinate as u64));
    }
    // The top 24 bits divide exactly by 2^24, so every draw is representable in `f32`.
    ((state >> 40) as f32) / 16_777_216.0
}

/// SplitMix64 — a fixed, portable integer mixer, the sprite bake's copy. Not `DeterministicRng`,
/// because the noise needs a *stateless* draw addressed by lattice point, not a sequence.
fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `floor` as an `i64`: truncation rounds toward zero, so a negative non-integer needs one more.
fn floor(value: f32) -> i64 {
    let truncated = value as i64;
    if value < 0.0 && truncated as f32 != value {
        truncated - 1
    } else {
        truncated
    }
}

const DEG_TO_RAD: f32 = core::f32::consts::PI / 180.0;

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length(a: [f32; 3]) -> f32 {
    math::sqrt(dot(a, a))
}

fn mat_vec(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let columns = transpose(b);
    [
        [
            dot(a[0], columns[0]),
            dot(a[0], columns[1]),
            dot(a[0], columns[2]),
        ],
        [
            dot(a[1], columns[0]),
            dot(a[1], columns[1]),
            dot(a[1], columns[2]),
        ],
        [
            dot(a[2], columns[0]),
            dot(a[2], columns[1]),
            dot(a[2], columns[2]),
        ],
    ]
}

fn transpose(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::math::glam::{EulerRot, Mat3, Vec3};

    const EPS: f32 = 1e-5;

    fn near(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < EPS,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn sphere_distances() {
        near(sphere([0.0; 3], 0.5), -0.5);
        near(sphere([0.5, 0.0, 0.0], 0.5), 0.0);
        near(sphere([0.0, 0.0, -1.5], 0.5), 1.0);
    }

    #[test]
    fn capsule_distances() {
        let (from, to) = ([0.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        near(capsule([0.0, 1.0, 0.0], 0.5, from, to), -0.5);
        near(capsule([0.5, 1.0, 0.0], 0.5, from, to), 0.0);
        near(capsule([0.0, 2.5, 0.0], 0.5, from, to), 0.0);
        near(capsule([0.0, -1.0, 0.0], 0.5, from, to), 0.5);
        near(capsule([2.0, 0.0, 0.0], 0.5, from, from), 1.5);
    }

    #[test]
    fn ellipsoid_is_exact_on_axes() {
        let radii = [0.22, 0.18, 0.2];
        near(ellipsoid([0.3, 0.0, 0.0], radii), 0.3 - 0.22);
        near(ellipsoid([0.0, 0.1, 0.0], radii), 0.1 - 0.18);
        near(ellipsoid([0.0, 0.0, -0.2], radii), 0.0);
        near(ellipsoid([0.0; 3], radii), -0.18);
    }

    #[test]
    fn ellipsoid_sign_is_exact_near_the_surface() {
        let radii = [1.0, 0.5, 0.25];
        let direction = [0.6, 0.5, 0.62];
        let at_surface = 1.0 / length([0.6, 1.0, 2.48]);
        for (factor, inside) in [(0.999, true), (1.001, false), (0.5, true), (2.0, false)] {
            let d = ellipsoid(scale(direction, at_surface * factor), radii);
            assert_eq!(d < 0.0, inside, "factor {factor} gave {d}");
        }
    }

    #[test]
    fn box_distances() {
        let size = [2.0, 2.0, 2.0];
        near(rounded_box([0.0; 3], size, 0.0), -1.0);
        near(rounded_box([1.0, 0.0, 0.0], size, 0.0), 0.0);
        near(rounded_box([2.0, 2.0, 2.0], size, 0.0), math::sqrt(3.0));
        near(rounded_box([1.0, 1.0, 1.0], size, 0.0), 0.0);
        near(rounded_box([0.0, 0.0, 0.3], [4.0, 4.0, 1.0], 0.0), -0.2);
    }

    #[test]
    fn rounded_box_keeps_extents_and_cuts_corners() {
        let size = [2.0, 2.0, 2.0];
        near(rounded_box([0.0; 3], size, 0.25), -1.0);
        near(rounded_box([1.0, 0.0, 0.0], size, 0.25), 0.0);
        near(
            rounded_box([1.0, 1.0, 1.0], size, 0.25),
            math::sqrt(3.0 * 0.0625) - 0.25,
        );
        near(
            rounded_box([1.0, 1.0, 1.0], size, 5.0),
            math::sqrt(3.0) - 1.0,
        );
    }

    #[test]
    fn torus_distances() {
        near(torus([1.0, 0.0, 0.0], 1.0, 0.25), -0.25);
        near(torus([0.0, 0.0, -1.0], 1.0, 0.25), -0.25);
        near(torus([1.25, 0.0, 0.0], 1.0, 0.25), 0.0);
        near(torus([0.0; 3], 1.0, 0.25), 0.75);
        near(torus([0.0, 1.0, 1.0], 1.0, 0.25), 0.75);
    }

    #[test]
    fn cone_distances() {
        near(cone([0.0, 2.0, 0.0], 1.0, 2.0), 0.0);
        near(cone([1.0, 0.0, 0.0], 1.0, 2.0), 0.0);
        near(cone([0.0, 0.01, 0.0], 1.0, 2.0), -0.01);
        near(cone([0.0, -1.0, 0.0], 1.0, 2.0), 1.0);
        near(cone([0.0, 3.0, 0.0], 1.0, 2.0), 1.0);
        near(
            cone([1.0, 1.0, 0.0], 1.0, 1.0),
            core::f32::consts::FRAC_1_SQRT_2,
        );
        near(
            cone([0.0, 0.5, 0.0], 1.0, 1.0),
            -0.5 * core::f32::consts::FRAC_1_SQRT_2,
        );
        near(cone([2.0, -1.0, 0.0], 1.0, 1.0), core::f32::consts::SQRT_2);
    }

    #[test]
    fn rotation_matches_the_engine_euler_convention() {
        let degrees = [30.0, -45.0, 110.0];
        let transform = PartTransform::new([0.0; 3], degrees);
        let [rx, ry, rz] = degrees.map(|d: f32| d.to_radians());
        let glam = Mat3::from_euler(EulerRot::XYZ, rx, ry, rz);
        for local in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.3, -0.7, 0.2]] {
            let ours = transform.to_model(local);
            let theirs = glam * Vec3::from(local);
            for axis in 0..3 {
                near(ours[axis], theirs[axis]);
            }
        }
        assert_eq!(
            PartTransform::new([0.0; 3], [0.0; 3]).inverse_rotation,
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
    }

    #[test]
    fn rotated_box_distance_equals_local_distance_at_the_rotated_point() {
        let primitive = Primitive::Box {
            size: [1.2, 0.3, 0.8],
            round: 0.02,
        };
        let placed = PlacedPrimitive {
            primitive,
            transform: PartTransform::new([0.5, 1.0, -0.25], [20.0, 65.0, -35.0]),
        };
        for local in [
            [0.0, 0.0, 0.0],
            [0.6, 0.0, 0.0],
            [0.7, 0.2, -0.5],
            [-0.1, 0.1, 0.3],
            [1.5, -1.0, 2.0],
        ] {
            let model = placed.transform.to_model(local);
            near(
                placed.distance(model),
                rounded_box(local, [1.2, 0.3, 0.8], 0.02),
            );
        }
    }

    #[test]
    fn bounds_rotate_the_local_box() {
        let placed = PlacedPrimitive {
            primitive: Primitive::Box {
                size: [2.0, 2.0, 2.0],
                round: 0.0,
            },
            transform: PartTransform::new([1.0, 0.0, 0.0], [0.0, 45.0, 0.0]),
        };
        let (min, max) = placed.bounds();
        near(min[0], 1.0 - core::f32::consts::SQRT_2);
        near(max[0], 1.0 + core::f32::consts::SQRT_2);
        near(min[1], -1.0);
        near(max[2], core::f32::consts::SQRT_2);
    }

    #[test]
    fn bounds_enclose_every_inside_sample() {
        let transform = PartTransform::new([0.2, -0.1, 0.3], [35.0, -60.0, 15.0]);
        let primitives = [
            Primitive::Sphere { radius: 0.4 },
            Primitive::Capsule {
                radius: 0.2,
                from: [-0.3, 0.1, 0.0],
                to: [0.4, 0.6, -0.2],
            },
            Primitive::Ellipsoid {
                radii: [0.5, 0.2, 0.3],
            },
            Primitive::Box {
                size: [0.8, 0.3, 0.5],
                round: 0.05,
            },
            Primitive::Torus {
                radius: 0.5,
                tube: 0.15,
            },
            Primitive::Cone {
                radius: 0.3,
                height: 0.7,
            },
        ];
        for primitive in primitives {
            let placed = PlacedPrimitive {
                primitive: primitive.clone(),
                transform,
            };
            let (min, max) = placed.bounds();
            let steps = 24;
            for z in 0..=steps {
                for y in 0..=steps {
                    for x in 0..=steps {
                        let p = [x, y, z].map(|i| -1.0 + 2.0 * i as f32 / steps as f32);
                        if placed.distance(p) < 0.0 {
                            for axis in 0..3 {
                                assert!(
                                    min[axis] <= p[axis] && p[axis] <= max[axis],
                                    "{primitive:?}: inside sample {p:?} escapes {min:?}..{max:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn value_noise_is_a_unit_interval_lattice_and_follows_its_seed() {
        for p in [
            [0.0; 3],
            [0.5, 0.25, -0.75],
            [-3.2, 17.9, 0.0],
            [-0.0, -1.0, 4.5],
        ] {
            let value = value_noise(p, 9);
            assert!((0.0..1.0).contains(&value), "{p:?} drew {value}");
            assert_eq!(value.to_bits(), value_noise(p, 9).to_bits());
        }
        assert_ne!(
            value_noise([0.3, 0.2, 0.1], 0),
            value_noise([0.3, 0.2, 0.1], 1)
        );
        // A lattice point is its own hashed value, and the fade makes the crossing continuous.
        let at = value_noise([2.0, -1.0, 3.0], 4);
        near(value_noise([1.999_99, -1.0, 3.0], 4), at);
        near(value_noise([2.000_01, -1.0, 3.0], 4), at);
    }

    #[test]
    fn a_deformed_box_grows_to_hold_the_deformed_solid() {
        let primitive = Primitive::Box {
            size: [0.4, 1.2, 0.4],
            round: 0.0,
        };
        let plain = primitive.local_bounds();
        let cases = [
            (
                "bend",
                Modifiers::new(
                    plain,
                    Some(Turn {
                        axis: 1,
                        degrees: 40.0,
                    }),
                    None,
                    None,
                    None,
                ),
            ),
            (
                "twist",
                Modifiers::new(
                    plain,
                    None,
                    Some(Turn {
                        axis: 1,
                        degrees: 90.0,
                    }),
                    None,
                    None,
                ),
            ),
            (
                "taper",
                Modifiers::new(
                    plain,
                    None,
                    None,
                    Some(Taper {
                        axis: 1,
                        from: 1.0,
                        to: 2.5,
                    }),
                    None,
                ),
            ),
            (
                "noise",
                Modifiers::new(
                    plain,
                    None,
                    None,
                    None,
                    Some(Noise {
                        amplitude: 0.05,
                        scale: 0.2,
                        seed: 1,
                    }),
                ),
            ),
        ];
        let steps = 20;
        for (name, modifiers) in cases {
            let (lo, hi) = modifiers.local_bounds();
            for z in 0..=steps {
                for y in 0..=steps {
                    for x in 0..=steps {
                        let p = [x, y, z].map(|i| -2.0 + 4.0 * i as f32 / steps as f32);
                        if modifiers.distance(&primitive, p) < 0.0 {
                            for axis in 0..3 {
                                assert!(
                                    lo[axis] <= p[axis] && p[axis] <= hi[axis],
                                    "{name}: inside sample {p:?} escapes {lo:?}..{hi:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A primitive whose box is already centred on its local origin takes exactly the arithmetic it
    /// took before the deformation frame existed — bit for bit, so no centred model's hash moves.
    #[test]
    fn a_centred_primitive_deforms_about_the_local_origin_bit_for_bit() {
        let primitive = Primitive::Box {
            size: [0.4, 1.2, 0.4],
            round: 0.05,
        };
        let bend = Turn {
            axis: 1,
            degrees: 55.0,
        };
        let taper = Taper {
            axis: 1,
            from: 1.0,
            to: 0.4,
        };
        let modifiers = Modifiers::new(
            primitive.local_bounds(),
            Some(bend),
            None,
            Some(taper),
            None,
        );
        for p in [
            [0.1, 0.3, -0.2],
            [0.0; 3],
            [-0.35, -0.55, 0.4],
            [0.8, 0.9, 0.7],
        ] {
            let mut q = turn(
                p,
                bend_plane(bend.axis),
                bend.degrees * DEG_TO_RAD * p[bend.axis],
            );
            let scale = modifiers.taper_scale(taper, q[taper.axis]);
            for axis in perpendicular(taper.axis) {
                q[axis] /= scale;
            }
            assert_eq!(
                modifiers.distance(&primitive, p).to_bits(),
                primitive.distance(q).to_bits(),
                "{p:?}"
            );
        }
    }

    /// A capsule's endpoints are model coordinates, so its local origin is the model's. The
    /// deformers still turn it about its own centre: the centre point keeps its undeformed distance
    /// and the grown box stays around the part instead of sweeping to the origin.
    #[test]
    fn a_capsule_bends_about_its_own_centre_not_the_model_origin() {
        let primitive = Primitive::Capsule {
            radius: 0.06,
            from: [0.0, 1.2, 0.0],
            to: [0.0, 1.5, 0.0],
        };
        let modifiers = Modifiers::new(
            primitive.local_bounds(),
            Some(Turn {
                axis: 1,
                degrees: 140.0,
            }),
            None,
            None,
            None,
        );
        // The centre of the segment is the pivot, so it is untouched — deep inside, as undeformed.
        near(modifiers.distance(&primitive, [0.0, 1.35, 0.0]), -0.06);
        // The whole swept box stays around the horn; measured from the model origin it reached the
        // floor and took the lattice with it.
        let (lo, hi) = modifiers.local_bounds();
        assert!(lo[1] > 1.0, "the grown box fell to {lo:?}");
        assert!(hi[1] < 1.7, "the grown box rose to {hi:?}");
    }

    /// The grown box holds the deformed solid for a primitive whose box is *not* centred on its
    /// local origin — the capsule and the cone, each deformer and their stack.
    #[test]
    fn an_off_centre_part_stays_inside_its_grown_box() {
        let bend = Some(Turn {
            axis: 2,
            degrees: 70.0,
        });
        let twist = Some(Turn {
            axis: 1,
            degrees: 120.0,
        });
        let taper = Some(Taper {
            axis: 1,
            from: 1.0,
            to: 0.45,
        });
        let noise = Some(Noise {
            amplitude: 0.04,
            scale: 0.15,
            seed: 7,
        });
        let primitives = [
            Primitive::Capsule {
                radius: 0.18,
                from: [0.3, 0.6, -0.1],
                to: [0.5, 1.1, 0.2],
            },
            Primitive::Cone {
                radius: 0.25,
                height: 0.9,
            },
        ];
        let cases = [
            ("bend", bend, None, None, None),
            ("twist", None, twist, None, None),
            ("taper", None, None, taper, None),
            ("noise", None, None, None, noise),
            ("all", bend, twist, taper, noise),
        ];
        let steps = 40;
        for primitive in &primitives {
            for (name, bend, twist, taper, noise) in cases {
                let modifiers = Modifiers::new(primitive.local_bounds(), bend, twist, taper, noise);
                let (lo, hi) = modifiers.local_bounds();
                // Sample a margin past the box on every side: an escapee has to be findable.
                let (from, to) = (lo.map(|v| v - 0.3), hi.map(|v| v + 0.3));
                for z in 0..=steps {
                    for y in 0..=steps {
                        for x in 0..=steps {
                            let p = [x, y, z].map(|i| i as f32 / steps as f32);
                            let p = [0, 1, 2]
                                .map(|axis| from[axis] + p[axis] * (to[axis] - from[axis]));
                            if modifiers.distance(primitive, p) < 0.0 {
                                for axis in 0..3 {
                                    assert!(
                                        lo[axis] <= p[axis] && p[axis] <= hi[axis],
                                        "{primitive:?} {name}: inside sample {p:?} escapes \
                                         {lo:?}..{hi:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// A taper scales the cross-section across the part's own extent and leaves the ends it names.
    #[test]
    fn taper_scales_the_cross_section_along_its_axis() {
        let primitive = Primitive::Box {
            size: [0.4, 2.0, 0.4],
            round: 0.0,
        };
        let modifiers = Modifiers::new(
            primitive.local_bounds(),
            None,
            None,
            Some(Taper {
                axis: 1,
                from: 1.0,
                to: 0.5,
            }),
            None,
        );
        // Near the bottom the scale is still ~1, so the surface sits on the box's own half-width.
        assert!(modifiers.distance(&primitive, [0.19, -0.9, 0.0]) < 0.0);
        assert!(modifiers.distance(&primitive, [0.21, -0.9, 0.0]) > 0.0);
        // Near the top it is ~0.5, so the half-width is about 0.1.
        assert!(modifiers.distance(&primitive, [0.09, 0.9, 0.0]) < 0.0);
        assert!(modifiers.distance(&primitive, [0.12, 0.9, 0.0]) > 0.0);
    }

    #[test]
    fn smin_is_min_at_zero_blend() {
        for (a, b) in [(0.3, -0.2), (-1.0, 2.0), (0.5, 0.5), (0.0, -0.0)] {
            assert_eq!(smin(a, b, 0.0).0, a.min(b));
            let expected_weight = if b < a { 1.0 } else { 0.0 };
            assert_eq!(smin(a, b, 0.0).1, expected_weight);
        }
        assert_eq!(smin(0.3, -0.2, 0.0), (-0.2, 1.0));
        assert_eq!(smin(-0.3, 0.2, 0.0), (-0.3, 0.0));
    }

    #[test]
    fn smin_is_symmetric_in_value_and_complementary_in_weight() {
        for (a, b, k) in [
            (0.1, 0.15, 0.2),
            (-0.3, 0.2, 0.1),
            (0.0, 0.0, 0.08),
            (1.0, 0.95, 0.5),
            (0.4, -0.4, 1.0),
        ] {
            let (ab, w_ab) = smin(a, b, k);
            let (ba, w_ba) = smin(b, a, k);
            near(ab, ba);
            near(w_ab + w_ba, 1.0);
            assert!(ab <= a.min(b) + EPS);
        }
    }

    #[test]
    fn smin_blends_inside_the_band_and_is_exact_outside() {
        assert_eq!(smin(0.0, 0.0, 0.2), (-0.05, 0.5));
        assert_eq!(smin(0.0, 0.5, 0.2), (0.0, 0.0));
        assert_eq!(smin(0.5, 0.0, 0.2), (0.0, 1.0));
        let (value, weight) = smin(0.1, 0.0, 0.2);
        assert!(value < 0.0 && weight > 0.5 && weight < 1.0);
    }

    #[test]
    fn smin_on_the_empty_field_takes_the_part() {
        assert_eq!(smin(f32::INFINITY, 0.25, 0.1), (0.25, 1.0));
        assert_eq!(smin(f32::INFINITY, 0.25, 0.0), (0.25, 1.0));
        assert_eq!(smax(f32::INFINITY, -0.25, 0.1), (f32::INFINITY, 0.0));
    }

    #[test]
    fn smax_is_negated_smin_of_negations() {
        for (a, b, k) in [
            (0.1, 0.15, 0.2),
            (-0.3, 0.2, 0.1),
            (0.0, 0.0, 0.08),
            (0.7, -0.1, 0.0),
            (-0.2, 0.4, 0.0),
        ] {
            let (value, weight) = smax(a, b, k);
            let (negated, negated_weight) = smin(-a, -b, k);
            assert_eq!(value.to_bits(), (-negated).to_bits());
            assert_eq!(weight.to_bits(), negated_weight.to_bits());
        }
        assert_eq!(smax(0.7, -0.1, 0.0), (0.7, 0.0));
        assert_eq!(smax(-0.2, 0.4, 0.0), (0.4, 1.0));
    }
}
