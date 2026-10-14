// Procedural sky background: a fullscreen triangle drawn at far depth inside the scene pass, filling
// only the pixels no mesh wrote (depth compare LessEqual, no depth write). Emitted only when the
// scene carries an `Environment`, so an env-less scene keeps the exact clear-color path.
//
// The gradient is authored (zenith / horizon / ground, linear HDR) — no image asset, no physical
// daylight model. The optional sun/moon disc is drawn here in the background only (its direction
// comes from the scene's sun); it is excluded from the IBL prefilter, so it is not double-counted.
//
// `sky_gradient` comes from `shaders/snippets/sky_gradient.wgsl`, composed ahead of this body: the
// composite's haze in-scatters the same function, so the medium and the background it meets at the
// horizon evaluate one sky, not two spellings of one.

struct Sky {
    // Inverse view-projection, to turn an NDC point back into a world-space view ray.
    inv_view_proj: mat4x4<f32>,
    // World-space camera position (xyz).
    cam_pos: vec4<f32>,
    // Gradient colors (rgb), linear HDR.
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
    // xyz = normalized direction toward the sun/moon (−sun travel direction); w = cos(disc half-angle).
    sun_dir: vec4<f32>,
    // x = disc intensity, y = has-disc flag (1 = draw), zw = pad.
    disc: vec4<f32>,
};
@group(0) @binding(0) var<uniform> sky: Sky;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // The standard oversized fullscreen triangle: uv (0,0),(2,0),(0,2) → ndc (-1,-1),(3,-1),(-1,3).
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VsOut;
    // z = w = 1 → depth 1.0 (far plane), so LessEqual fills only cleared (background) pixels.
    out.clip = vec4<f32>(ndc.x, ndc.y, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Reconstruct the world-space view ray for this pixel from the far-plane NDC point.
    let world_h = sky.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(world_h.xyz / world_h.w - sky.cam_pos.xyz);

    var color = sky_gradient(dir, sky.zenith.rgb, sky.horizon.rgb, sky.ground.rgb);

    // Optional sun/moon disc: a soft-edged bright cap around the sun direction.
    if (sky.disc.y > 0.5) {
        let cos_ang = dot(dir, sky.sun_dir.xyz);
        let edge = sky.sun_dir.w;
        let soft = edge + (1.0 - edge) * 0.15 + 1e-5;
        let d = smoothstep(edge, soft, cos_ang);
        color = color + vec3<f32>(sky.disc.x) * d;
    }

    return vec4<f32>(color, 1.0);
}
