// Text pass: world-space textured quads sampling a font atlas (bitmap, or a thresholded SDF), alpha-blended, unlit.
//
// Vertices are generated on the CPU (one quad per glyph, positioned in world space with atlas UVs);
// this shader just projects them and modulates the atlas coverage by the text color.

// The `Camera` uniform (@group 0) is a shared snippet composed ahead of this file
// (`shaders/snippets/camera.wgsl`) — see Plan/shader-bindgen.md decision 16. This body declares only
// the vertex input and stage (text is unlit, so it composes no lights snippet). The atlas group and
// `glyph_coverage` come from `shaders/snippets/glyph.wgsl`, shared with `ui_text.wgsl`.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: f32,
};

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) sdf: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    return out;
}

// The transparent layer's twin of the vertex stage (`snippets/transparent_layer.wgsl`), with the
// world position the distance is measured at.
struct DepthOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: f32,
    @location(3) world: vec3<f32>,
};

@vertex
fn vs_depth(in: VsIn) -> DepthOut {
    var out: DepthOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    out.world = in.position;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = glyph_coverage(in.uv, in.sdf);
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}

@fragment
fn fs_depth(in: DepthOut) -> @location(0) vec4<f32> {
    let coverage = glyph_coverage(in.uv, in.sdf);
    return transparent_layer(in.world, in.color.a * coverage, 1.0);
}
