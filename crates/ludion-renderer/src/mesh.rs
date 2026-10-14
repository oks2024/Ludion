//! Procedural low-poly primitives: cube, plane, sphere.
//!
//! glTF import is deferred (`engine-architecture.md` — deferred list), so meshes are generated here.
//! Normals are **faceted** (one geometric normal per triangle, not smoothed), which is the flat,
//! low-poly HD-2D look. Each primitive is a non-indexed triangle list of [`MeshVertex`]; the renderer
//! builds one vertex buffer per shape at startup and draws instanced.

use bytemuck::{Pod, Zeroable};
use ludion_core::math::glam::Vec3;

use crate::scene::MeshShape;

/// One mesh vertex, six lanes: object-space position, faceted normal, per-vertex RGBA color,
/// material, texture coordinates, and tangent frame. `#[repr(C)]` + `Pod` so a slice casts straight
/// to GPU bytes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct MeshVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Faceted (per-triangle) object-space normal.
    pub normal: [f32; 3],
    /// Per-vertex RGBA color (linear); the shader multiplies it by the per-instance color. The
    /// procedural primitives are opaque white, so the product is exactly the instance color.
    pub color: [f32; 4],
    /// Per-vertex material `(roughness, metallic, emissive)`. The procedural primitives carry the
    /// default material `(1, 0, 0)`, so the per-instance material governs them.
    pub material: [f32; 3],
    /// Texture coordinates. The procedural primitives are unmapped and carry `(0, 0)`.
    pub uv: [f32; 2],
    /// Tangent frame `(x, y, z, handedness)`: the tangent along +U, `w` the bitangent's sign. The
    /// procedural primitives carry the default frame.
    pub tangent: [f32; 4],
    /// Skin weights for the four bones in [`bones`](MeshVertex::bones), summing to 1. The procedural
    /// primitives are unskinned and carry the default `(1, 0, 0, 0)`.
    pub weights: [f32; 4],
    /// Indices into the instance's slice of the renderer's bone palette. The procedural primitives
    /// are unskinned and carry the default `(0, 0, 0, 0)` (the root bone, never read — see
    /// [`MeshInstance::bone_offset`](crate::scene::MeshInstance::bone_offset)).
    pub bones: [u32; 4],
}

/// The default material `(roughness 1, metallic 0, emissive 0)` — matte non-metal, worn by every
/// vertex that authors none.
pub(crate) const DEFAULT_MATERIAL: [f32; 3] = [1.0, 0.0, 0.0];

/// The default tangent frame `(+X, handedness +1)` — worn by every vertex whose source authors
/// none. A placeholder, not a derived frame: geometry with no UVs has no tangent to derive.
pub(crate) const DEFAULT_TANGENT: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// The default skin weights `(1, 0, 0, 0)` — worn by every unskinned vertex, paired with
/// [`DEFAULT_BONES`].
pub(crate) const DEFAULT_WEIGHTS: [f32; 4] = [1.0, 0.0, 0.0, 0.0];

/// The default bone indices `(0, 0, 0, 0)` — worn by every unskinned vertex. Never read: an
/// unskinned instance's `bone_offset` is `0`, and the shader's unskinned branch never indexes the
/// palette.
pub(crate) const DEFAULT_BONES: [u32; 4] = [0, 0, 0, 0];

/// Generates the non-indexed, faceted triangle list for a primitive `shape`.
///
/// Only the three procedural primitives have generated geometry; [`MeshShape::Asset`] geometry comes
/// from a mesh document and is uploaded through the indexed asset-mesh path, never here.
pub(crate) fn vertices(shape: MeshShape) -> Vec<MeshVertex> {
    match shape {
        MeshShape::Cube => cube(),
        MeshShape::Plane => plane(),
        MeshShape::Sphere => sphere(24, 16),
        MeshShape::Asset(_) => unreachable!("asset meshes are uploaded via the indexed path"),
    }
}

/// Emits two triangles (a quad `a,b,c,d` wound CCW) with a shared faceted normal.
fn quad(out: &mut Vec<MeshVertex>, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    tri(out, a, b, c);
    tri(out, a, c, d);
}

/// Emits one triangle, computing and attaching its geometric (faceted) normal. Primitive vertices
/// are opaque white with the default material, so the per-instance color and material govern them.
fn tri(out: &mut Vec<MeshVertex>, a: Vec3, b: Vec3, c: Vec3) {
    let normal = (b - a).cross(c - a).normalize_or_zero().to_array();
    for p in [a, b, c] {
        out.push(MeshVertex {
            position: p.to_array(),
            normal,
            color: [1.0, 1.0, 1.0, 1.0],
            material: DEFAULT_MATERIAL,
            uv: [0.0, 0.0],
            tangent: DEFAULT_TANGENT,
            weights: DEFAULT_WEIGHTS,
            bones: DEFAULT_BONES,
        });
    }
}

/// A unit cube centered at the origin (side 1, corners at ±0.5).
fn cube() -> Vec<MeshVertex> {
    let mut out = Vec::with_capacity(36);
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let (n, s) = (0.5_f32, -0.5_f32);
    // +X, -X, +Y, -Y, +Z, -Z, each wound CCW when viewed from outside.
    quad(&mut out, p(n, s, n), p(n, s, s), p(n, n, s), p(n, n, n)); // +X
    quad(&mut out, p(s, s, s), p(s, s, n), p(s, n, n), p(s, n, s)); // -X
    quad(&mut out, p(s, n, n), p(n, n, n), p(n, n, s), p(s, n, s)); // +Y
    quad(&mut out, p(s, s, s), p(n, s, s), p(n, s, n), p(s, s, n)); // -Y
    quad(&mut out, p(s, s, n), p(n, s, n), p(n, n, n), p(s, n, n)); // +Z
    quad(&mut out, p(n, s, s), p(s, s, s), p(s, n, s), p(n, n, s)); // -Z
    out
}

/// A unit plane in the XZ plane (normal +Y), spanning ±0.5, centered at the origin.
fn plane() -> Vec<MeshVertex> {
    let mut out = Vec::with_capacity(6);
    let p = |x: f32, z: f32| Vec3::new(x, 0.0, z);
    quad(
        &mut out,
        p(-0.5, 0.5),
        p(0.5, 0.5),
        p(0.5, -0.5),
        p(-0.5, -0.5),
    );
    out
}

/// A unit-radius UV sphere with `sectors` around and `rings` from pole to pole, **smooth**: the
/// per-vertex normal is the exact analytic surface normal (the normalized object-space position),
/// which is what a roughness gradient needs to read continuously. Cube and plane stay faceted —
/// flat faces are correct there.
fn sphere(sectors: usize, rings: usize) -> Vec<MeshVertex> {
    use core::f32::consts::PI;
    use ludion_core::math::{cos, sin};
    let mut out = Vec::new();
    let point = |ring: usize, sector: usize| -> Vec3 {
        let phi = PI * (ring as f32) / (rings as f32); // 0..PI, pole to pole
        let theta = 2.0 * PI * (sector as f32) / (sectors as f32); // 0..2PI around
        Vec3::new(sin(phi) * cos(theta), cos(phi), sin(phi) * sin(theta)) * 0.5
    };
    // Radius-0.5 sphere centered at the origin: the outward normal at `p` is `normalize(p)`. Emit
    // each triangle reversed (`a, c, b`) so the outward-facing hemisphere is the drawn one under
    // `Face::Back` culling — a proper convex mesh, not the inside-out faceted primitive it replaces.
    let smooth_tri = |out: &mut Vec<MeshVertex>, a: Vec3, b: Vec3, c: Vec3| {
        for p in [a, c, b] {
            out.push(MeshVertex {
                position: p.to_array(),
                normal: p.normalize_or_zero().to_array(),
                color: [1.0, 1.0, 1.0, 1.0],
                material: DEFAULT_MATERIAL,
                uv: [0.0, 0.0],
                tangent: DEFAULT_TANGENT,
                weights: DEFAULT_WEIGHTS,
                bones: DEFAULT_BONES,
            });
        }
    };
    for ring in 0..rings {
        for sector in 0..sectors {
            let a = point(ring, sector);
            let b = point(ring + 1, sector);
            let c = point(ring + 1, sector + 1);
            let d = point(ring, sector + 1);
            // The pole rings collapse one edge to a point (a degenerate, invisible triangle).
            if ring == 0 {
                smooth_tri(&mut out, a, b, c);
            } else if ring == rings - 1 {
                smooth_tri(&mut out, a, b, d);
            } else {
                smooth_tri(&mut out, a, b, c);
                smooth_tri(&mut out, a, c, d);
            }
        }
    }
    out
}
