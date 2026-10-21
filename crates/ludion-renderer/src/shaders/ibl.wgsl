// IBL prefilter compute — three entry points that bake the analytic sky gradient into the mesh
// pass's IBL inputs, run at environment load/change (keyed by a content hash of the sky colors).
//
// Deterministic: fixed sample counts and a pure Hammersley sequence, no RNG and no atomics — every
// output texel is an independent function of its direction, so the same environment produces
// byte-identical textures on a pinned GPU. Each pipeline uses an auto-derived bind group layout;
// shader-bindgen reflects the uniform PODs, binding indices and workgroup sizes into `shader_gen.rs`.
//
// The sun/moon disc is deliberately absent here — the IBL integrates the smooth gradient alone, so
// the analytic `DirectionalLight` (and the sky background's disc) are never double-counted.

const PI: f32 = 3.14159265358979;

struct SkyColors {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
};
// x = roughness for this specular mip, y = face size (unused, informational), zw = pad.
struct PrefilterParams {
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> colors: SkyColors;
@group(0) @binding(1) var<uniform> prefilter_params: PrefilterParams;
@group(0) @binding(2) var irradiance_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(3) var specular_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var brdf_out: texture_storage_2d<rgba16float, write>;

// The authored gradient, byte-for-byte the same easing as `sky.wgsl`'s `sky_gradient` (they cannot
// share a module — one is composed for a render pipeline, this drives compute — so both encode it).
fn sky_gradient(dir: vec3<f32>) -> vec3<f32> {
    let t = dir.y;
    if (t >= 0.0) {
        return mix(colors.horizon.rgb, colors.zenith.rgb, pow(clamp(t, 0.0, 1.0), 0.5));
    }
    return mix(colors.horizon.rgb, colors.ground.rgb, pow(clamp(-t, 0.0, 1.0), 0.5));
}

// Cube-face direction for face `face` (0..5 = +X,-X,+Y,-Y,+Z,-Z) at face-local `uv` in [-1, 1] — the
// standard wgpu cube convention (matches `TextureViewDimension::Cube` sampling).
fn face_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    var d: vec3<f32>;
    switch (face) {
        case 0u: { d = vec3<f32>( 1.0, -uv.y, -uv.x); }
        case 1u: { d = vec3<f32>(-1.0, -uv.y,  uv.x); }
        case 2u: { d = vec3<f32>( uv.x,  1.0,  uv.y); }
        case 3u: { d = vec3<f32>( uv.x, -1.0, -uv.y); }
        case 4u: { d = vec3<f32>( uv.x, -uv.y,  1.0); }
        default: { d = vec3<f32>(-uv.x, -uv.y, -1.0); }
    }
    return normalize(d);
}

fn radical_inverse_vdc(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}
fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse_vdc(i));
}

fn tangent_basis(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) < 0.999);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(1.0 - cos_t * cos_t);
    let h = vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
    return tangent_basis(n) * h;
}

const IRR_SAMPLES: u32 = 64u;
const SPEC_SAMPLES: u32 = 64u;
const BRDF_SAMPLES: u32 = 256u;

// Diffuse irradiance: the cosine-weighted mean incoming radiance at each cube texel's normal. Stored
// as the mean (no 1/PI), so the mesh shader's `irradiance * albedo` matches the point-light
// convention (no 1/PI on the diffuse lobe there either).
@compute @workgroup_size(8, 8, 1)
fn irradiance(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(irradiance_out);
    if (gid.x >= size.x || gid.y >= size.y) { return; }
    let uv = (vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5) / vec2<f32>(size)) * 2.0 - 1.0;
    let n = face_dir(gid.z, uv);
    let tb = tangent_basis(n);
    var acc = vec3<f32>(0.0);
    for (var i = 0u; i < IRR_SAMPLES; i = i + 1u) {
        let xi = hammersley(i, IRR_SAMPLES);
        let phi = 2.0 * PI * xi.x;
        let cos_t = sqrt(1.0 - xi.y);
        let sin_t = sqrt(xi.y);
        let local = vec3<f32>(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
        acc = acc + sky_gradient(tb * local);
    }
    acc = acc / f32(IRR_SAMPLES);
    textureStore(irradiance_out, vec2<i32>(gid.xy), i32(gid.z), vec4<f32>(acc, 1.0));
}

// Specular prefilter: GGX-importance-sampled prefiltered radiance for this mip's roughness, under the
// N = V = R assumption. Mip 0 (roughness 0) is the mirror sample.
@compute @workgroup_size(8, 8, 1)
fn prefilter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(specular_out);
    if (gid.x >= size.x || gid.y >= size.y) { return; }
    let uv = (vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5) / vec2<f32>(size)) * 2.0 - 1.0;
    let n = face_dir(gid.z, uv);
    let v = n;
    let roughness = prefilter_params.params.x;
    var color = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = 0u; i < SPEC_SAMPLES; i = i + 1u) {
        let xi = hammersley(i, SPEC_SAMPLES);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let ndl = dot(n, l);
        if (ndl > 0.0) {
            color = color + sky_gradient(l) * ndl;
            total = total + ndl;
        }
    }
    color = color / max(total, 1e-4);
    textureStore(specular_out, vec2<i32>(gid.xy), i32(gid.z), vec4<f32>(color, 1.0));
}

fn geometry_smith_ibl(ndv: f32, ndl: f32, roughness: f32) -> f32 {
    let k = (roughness * roughness) / 2.0;
    let gv = ndv / (ndv * (1.0 - k) + k);
    let gl = ndl / (ndl * (1.0 - k) + k);
    return gv * gl;
}

// The split-sum BRDF LUT (environment-independent): the scale/bias for f0, integrated once. R = scale,
// G = bias; the mesh shader forms `f0 * scale + bias`.
@compute @workgroup_size(8, 8, 1)
fn brdf(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = textureDimensions(brdf_out);
    if (gid.x >= size.x || gid.y >= size.y) { return; }
    let ndv = max((f32(gid.x) + 0.5) / f32(size.x), 1e-4);
    let roughness = (f32(gid.y) + 0.5) / f32(size.y);
    let v = vec3<f32>(sqrt(1.0 - ndv * ndv), 0.0, ndv);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    var a = 0.0;
    var b = 0.0;
    for (var i = 0u; i < BRDF_SAMPLES; i = i + 1u) {
        let xi = hammersley(i, BRDF_SAMPLES);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let ndl = max(l.z, 0.0);
        let ndh = max(h.z, 0.0);
        let vdh = max(dot(v, h), 0.0);
        if (ndl > 0.0) {
            let g = geometry_smith_ibl(ndv, ndl, roughness);
            let g_vis = (g * vdh) / max(ndh * ndv, 1e-6);
            let fc = pow(1.0 - vdh, 5.0);
            a = a + (1.0 - fc) * g_vis;
            b = b + fc * g_vis;
        }
    }
    textureStore(brdf_out, vec2<i32>(gid.xy), vec4<f32>(a / f32(BRDF_SAMPLES), b / f32(BRDF_SAMPLES), 0.0, 1.0));
}
