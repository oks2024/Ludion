// The window blit both presenters share (`blit.rs`): a fullscreen triangle whose UVs sample the
// frame texture upright (UV origin top-left maps to the frame's top row, the orientation readback also
// packs). Its binding names match `vignette.wgsl`'s, so bindgen unifies the group; the pipeline's
// layout is hand-written in `Blit::new`, below the renderer.
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, in.uv);
}
