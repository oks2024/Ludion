// The projector pass: the engine's generic way to paint an analytic footprint onto whatever the
// opaque scene drew. One box volume per instance is rasterized (back faces only, so every covered
// pixel is shaded once and a camera inside the box still works); each covered pixel rebuilds its
// world position from `scene_depth`, evaluates the instance, and multiplies the result into `scene`.
// An instance carries its `kind`, its `blend` and its own occlusion key, so a new kind is a new
// branch over the same stages.
//
// Stages of the fragment, each a small function so a new kind reuses them:
//   1. the world position of the pixel (`world_from_depth`), and the receiver's geometric normal
//      rebuilt from the depth buffer (`geometric_normal`) for the plane test;
//   2. the footprint's coverage in the instance's local XZ plane (`footprint_coverage`);
//   3. the facing term from the opaque normal (`facing`);
//   4. the height fade below the anchor (`drop_fade`);
//   5. the first-surface test against the instance's occlusion tile (`first_surface`).
//
// A drop shadow is `kind` 0, `blend` 0 (multiply): the output is `1 - darkening`, which the
// pipeline multiplies into the scene colour. The CPU bakes every constant the shader needs into the
// instance, so no trigonometry or tolerance is duplicated here.
//
// `sd_rounded_box` comes from the shared `shaders/snippets/rounded_box.wgsl`.

// The instance. `ids` = (kind, blend, occlusion tile index or 0xFFFFFFFF for none, 0).
struct Projector {
    ids: vec4<u32>,
    // xyz = the anchor with the receiver tolerance added to y (the volume's top), w = the volume's
    // height (the max distance plus the tolerance).
    top: vec4<f32>,
    // xy = the rounded box's half-extent in the local XZ plane, z = its corner radius, w = softness.
    shape: vec4<f32>,
    // x = opacity, y = max distance, z = fade distance, w = the footprint bound `b` (the volume's
    // half-width in X and Z, and the occlusion tile's half-width).
    look: vec4<f32>,
    // x = cos(yaw), y = sin(yaw), z = the occlusion depth bias, w = the raw anchor height.
    yaw: vec4<f32>,
    // xy = the occlusion tile's uv offset in the atlas, z = its uv scale.
    tile: vec4<f32>,
    // World to the occlusion tile's orthographic clip space.
    occ_view: mat4x4<f32>,
}

struct ProjectorParams {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
}

// The opaque pass's depth, at the render extent (loaded, never filtered).
@group(0) @binding(0) var proj_depth: texture_depth_2d;
// World normal in `xyz`; zero where no opaque surface was drawn.
@group(0) @binding(1) var proj_normal: texture_2d<f32>;
// The drop-shadow occlusion atlas: the nearest surface straight below each caster's anchor.
@group(0) @binding(2) var proj_occlusion: texture_depth_2d;
@group(0) @binding(3) var<uniform> proj_params: ProjectorParams;
@group(0) @binding(4) var<storage, read> projectors: array<Projector>;

const NO_OCCLUSION: u32 = 0xFFFFFFFFu;
// The particle pass's soft-edge gaussian: `exp(-SOFT_FALLOFF)` is the value at the rim.
const SOFT_FALLOFF: f32 = 4.0;
// How far a side's extrapolated depth may miss the centre, as a share of the one-tap depth step,
// before that axis counts as crossing a discontinuity on both sides.
const EXTRAPOLATION_SLACK: f32 = 0.03;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) instance: u32,
}

// Corner `vi` of a cube in [-1, 1]^3 drawn as 36 vertices: six faces of two triangles, wound
// counter-clockwise seen from outside so the front-face cull keeps the back faces.
fn cube_corner(vi: u32) -> vec3<f32> {
    let face = vi / 6u;
    let axis = face / 2u;
    let sign = select(-1.0, 1.0, face % 2u == 0u);
    var quad = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var uv = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = uv[quad[vi % 6u]];
    // A face's tangent axes are the next two cyclically, so t1 x t2 = +axis; the negative face
    // mirrors `u` to reverse the winding.
    let u = c.x * sign;
    let v = c.y;
    if (axis == 0u) {
        return vec3<f32>(sign, u, v);
    } else if (axis == 1u) {
        return vec3<f32>(v, sign, u);
    }
    return vec3<f32>(u, v, sign);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let p = projectors[ii];
    let corner = cube_corner(vi);
    let b = p.look.w;
    let world = vec3<f32>(
        p.top.x + corner.x * b,
        p.top.y - p.top.w * (1.0 - corner.y) * 0.5,
        p.top.z + corner.z * b,
    );
    var out: VsOut;
    out.clip = proj_params.view_proj * vec4<f32>(world, 1.0);
    out.instance = ii;
    return out;
}

// The world-space position of the fragment at `uv` whose stored device depth is `depth`
// (`composite.wgsl`'s convention: `uv.y` runs top-down).
fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = proj_params.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

// The geometric world normal of the surface at `pixel`, rebuilt from the depth buffer: per screen
// axis the neighbour on the pixel's own surface gives a world delta, and the two deltas cross. On a
// plane device depth is affine in screen space, so the side whose two taps extrapolate to the
// centre's depth is that surface (a silhouette pixel borrows from its own side). The shaded normal in
// `proj_normal` carries normal maps and smooth vertex normals, which tilt the receiver's plane
// test; this one follows the geometry. Oriented to `n.y >= 0`; `fallback` (the shaded normal) where
// the cross degenerates or an axis has no side on the pixel's own surface (a sliver, a concave corner).
fn geometric_normal(
    pixel: vec2<i32>,
    depth: f32,
    world: vec3<f32>,
    fallback: vec3<f32>,
) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(proj_depth));
    let dims = vec2<f32>(size);
    var delta: array<vec3<f32>, 2>;
    var trusted = true;
    for (var axis = 0; axis < 2; axis++) {
        let step = select(vec2<i32>(0, 1), vec2<i32>(1, 0), axis == 0);
        let top = size - vec2<i32>(1);
        let lo = clamp(pixel - step, vec2<i32>(0), top);
        let hi = clamp(pixel + step, vec2<i32>(0), top);
        let lo2 = clamp(pixel - step * 2, vec2<i32>(0), top);
        let hi2 = clamp(pixel + step * 2, vec2<i32>(0), top);
        let d_lo = textureLoad(proj_depth, lo, 0);
        let d_hi = textureLoad(proj_depth, hi, 0);
        // A tap clamped at the border repeats another sample: that side cannot be judged and loses.
        let e_lo = select(
            abs(2.0 * d_lo - textureLoad(proj_depth, lo2, 0) - depth),
            1e30,
            all(lo == pixel) || all(lo2 == lo),
        );
        let e_hi = select(
            abs(2.0 * d_hi - textureLoad(proj_depth, hi2, 0) - depth),
            1e30,
            all(hi == pixel) || all(hi2 == hi),
        );
        let forward = e_hi <= e_lo;
        let other = select(lo, hi, forward);
        let d_other = select(d_lo, d_hi, forward);
        trusted = trusted && min(e_lo, e_hi) <= EXTRAPOLATION_SLACK * abs(d_other - depth) + 1e-6;
        let w_other = world_from_depth((vec2<f32>(other) + 0.5) / dims, d_other);
        delta[axis] = select(world - w_other, w_other - world, forward);
    }
    // Screen x then y (y runs down), so the cross points away from the surface for a front face.
    var n = cross(delta[1], delta[0]);
    n = select(n, -n, n.y < 0.0);
    let len = length(n);
    return select(fallback, n / max(len, 1e-12), trusted && len > 1e-12);
}

// The footprint's coverage at `world`: the rounded-box distance in the instance's yaw-rotated local
// XZ plane, hard-edged (analytic anti-aliasing over one screen texel) or soft (a gaussian that
// reaches zero at the outline), mixed by softness like the particle pass. `fwidth` runs here, so
// the caller must reach this in uniform control flow.
fn footprint_coverage(p: Projector, world: vec3<f32>) -> f32 {
    let dx = world.x - p.top.x;
    let dz = world.z - p.top.z;
    let local = vec2<f32>(p.yaw.x * dx - p.yaw.y * dz, p.yaw.y * dx + p.yaw.x * dz);
    let sd = sd_rounded_box(local, p.shape.xy, p.shape.z);
    let aa = max(fwidth(sd), 1e-6);
    let hard = 1.0 - smoothstep(-aa, aa, sd);
    let rho = clamp(1.0 + sd / max(p.shape.z, 1e-6), 0.0, 1.0);
    let rim = exp(-SOFT_FALLOFF);
    let soft = clamp((exp(-SOFT_FALLOFF * rho * rho) - rim) / (1.0 - rim), 0.0, 1.0);
    return mix(hard, soft, clamp(p.shape.w, 0.0, 1.0));
}

// How squarely the surface faces up: a wall gets 0, a 45 degree slope 0.71.
fn facing(normal: vec3<f32>) -> f32 {
    return clamp(normal.y, 0.0, 1.0);
}

// The height fade, the twin of `drop_shadow_fade` in `scene.rs`: 1 until `fade_distance` before the
// max distance, a linear ramp to 0 at it, 0 beyond. `below` is the anchor's height minus the pixel's.
fn drop_fade(below: f32, max_distance: f32, fade_distance: f32) -> f32 {
    let ramp = (max_distance - below) / max(fade_distance, 1e-6);
    let inside = select(ramp, 1.0, below <= max_distance - fade_distance);
    return select(inside, 0.0, below > max_distance);
}

// Whether `world` is the first surface seen straight down from the anchor: its depth in the
// instance's occlusion tile agrees with the tile's nearest surface to within the bias. Outside the
// tile counts as not first.
fn first_surface(p: Projector, world: vec3<f32>) -> f32 {
    let o = p.occ_view * vec4<f32>(world, 1.0);
    let inside = abs(o.x) <= 1.0 && abs(o.y) <= 1.0;
    let edge = vec2<f32>(textureDimensions(proj_occlusion));
    let origin = p.tile.xy * edge;
    let size = p.tile.z * edge.x;
    let local = (vec2<f32>(o.x, -o.y) * 0.5 + vec2<f32>(0.5)) * size;
    let texel = origin + clamp(floor(local), vec2<f32>(0.0), vec2<f32>(size - 1.0));
    let nearest = textureLoad(proj_occlusion, vec2<i32>(texel), 0);
    return select(0.0, 1.0, inside && o.z <= nearest + p.yaw.z);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The interpolated `clip` is the fragment position here: pixel coordinates, centre at +0.5.
    let frag = in.clip;
    let p = projectors[in.instance];
    let pixel = vec2<i32>(frag.xy);
    let depth = textureLoad(proj_depth, pixel, 0);
    let world = world_from_depth(frag.xy / vec2<f32>(textureDimensions(proj_depth)), depth);
    let normal = textureLoad(proj_normal, pixel, 0).xyz;

    // Every stage runs for every pixel and the rejections only zero the result, so the footprint's
    // `fwidth` stays in uniform control flow.
    let coverage = footprint_coverage(p, world);
    let below = p.yaw.w - world.y;
    let fade = drop_fade(below, p.look.y, p.look.z);
    let first = select(first_surface(p, world), 1.0, p.ids.z == NO_OCCLUSION);

    // The receiver's plane extended to under the anchor: a ramp the caster stands on reaches about
    // the anchor's height and is kept whole; a flat top above the anchor stays above it and is
    // never shadowed, which also keeps a caster standing on the anchor from darkening itself.
    // Geometry of the caster within the tolerance band under the anchor top can still darken:
    // there is no per-pixel entity id.
    // The plane comes from the depth buffer's geometry, not the shaded normal, so a normal map or
    // smooth normals cannot cut the rim.
    let plane = geometric_normal(pixel, depth, world, normal);
    let y_under = world.y + dot(plane.xz, world.xz - p.top.xz) / max(plane.y, 1e-3);
    let kept = depth < 1.0 && y_under <= p.top.y && below <= p.look.y;
    let darkening = p.look.x * coverage * facing(normal) * fade * first;
    let shade = select(0.0, darkening, kept);
    return vec4<f32>(vec3<f32>(1.0 - shade), 1.0);
}
