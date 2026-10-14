// Shared fullscreen-triangle vertex stage for the post/passthrough passes (bloom threshold, blur,
// composite, overlay blit). One oversized triangle covers the framebuffer (the classic three-vertex
// trick); the third vertex extends past the screen so the visible region is exactly the [0,1] UV
// square, V flipped so uv.y = 0 is the top. Composed ahead of each pass's own bindings + `fs_main`
// (Plan/shader-bindgen.md decision 16), so the stage lives in one place instead of four copies.
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));
    return out;
}
