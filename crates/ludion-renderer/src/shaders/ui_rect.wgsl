// Screen-space UI rect pass: instanced pixel-space quads, textured × tint, premultiplied, unlit.
//
// Each instance is a pixel-space rect `[x, y, w, h]` (top-left origin, +Y down) plus a linear RGBA
// tint. The vertex stage expands 6 vertices from the vertex index and converts pixel coordinates to
// NDC using the viewport extent (top-left pixel origin, +Y down → NDC +Y up). Target is the LDR
// sRGB output — colors are linear, the hardware encodes on store.
//
// Every sampled texture is PREMULTIPLIED — a `UiRect` texture rides the same convention as a sprite
// because both resolve through the one texture cache — and the blend is `One, OneMinusSrcAlpha`, so
// the tint weights the texel by its own alpha rather than the blend doing it. An untextured rect
// binds the 1×1 white texture, on which premultiplication is the identity, so the tint is still the
// solid fill (the sprite convention).

struct Ui {
    // xy = viewport pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> ui: Ui;
@group(1) @binding(0) var rect_tex: texture_2d<f32>;
@group(1) @binding(1) var rect_sampler: sampler;

struct VsIn {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    // The sampled sub-rectangle (u0, v0, u1, v1), top-left origin: the whole texture (a loose PNG or
    // an untextured fill) is (0, 0, 1, 1); one packed atlas region is a sub-rect.
    @location(2) uv: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// The six corners of a quad (two triangles) as [0,1] fractions of the rect.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> VsOut {
    let corner = CORNERS[vi];
    let px = in.rect.xy + corner * in.rect.zw;
    let ndc = vec2<f32>(
        px.x / ui.viewport.x * 2.0 - 1.0,
        1.0 - px.y / ui.viewport.y * 2.0,
    );
    var out: VsOut;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    // `corner` is already the [0,1] top-left-origin unit UV, so the sub-rect map is the identity for
    // the default (0, 0, 1, 1) — a whole-texture rect samples exactly as before.
    out.uv = in.uv.xy + corner * (in.uv.zw - in.uv.xy);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(rect_tex, rect_sampler, in.uv);
    // The texel is premultiplied, so the tint's own alpha scales its colour as well as its coverage.
    return vec4<f32>(texel.rgb * in.color.rgb * in.color.a, texel.a * in.color.a);
}
