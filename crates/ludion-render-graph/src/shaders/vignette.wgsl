// The "vignette" pass: sample a full-screen input texture and darken it toward the edges.
//
// This is the graph's single sampling-pass shape. It reads one texture (bound at group 0) and
// writes one target; chaining it after the gradient — and after itself — is what gives the graph
// real dependency edges and a sequence of transient lifetimes for the aliaser to reuse. The effect
// is a pure function of UV and the sampled color, so the whole pipeline stays deterministic.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let color = textureSample(src_tex, src_sampler, in.uv);
    // Distance from center, 0 at the middle and ~0.707 at the corners; fade the color down with it.
    let d = distance(in.uv, vec2<f32>(0.5, 0.5));
    let falloff = clamp(1.0 - d * 0.9, 0.0, 1.0);
    return vec4<f32>(color.rgb * falloff, 1.0);
}
