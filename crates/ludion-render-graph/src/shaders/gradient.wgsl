// The "gradient" pass: a deterministic full-screen procedural gradient with no inputs.
//
// A single oversized triangle covers the framebuffer (the classic three-vertex fullscreen trick);
// the fragment color is a pure function of the normalized screen position, so the output depends
// only on the target resolution — nothing random, nothing time-based. This is the graph's source
// pass: it writes the first transient with no reads, forcing at least one dependency edge to the
// passes that sample it.

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // Clip-space positions of the covering triangle; the third vertex extends past the screen so
    // the visible region is exactly the [0,1] UV square.
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    // Map clip space to [0,1], flipping Y so uv.y = 0 is the top of the image.
    out.uv = vec2<f32>(xy.x * 0.5 + 0.5, 1.0 - (xy.y * 0.5 + 0.5));
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Horizontal red ramp, vertical green ramp, constant blue — a recognizable, fully deterministic
    // test pattern.
    return vec4<f32>(in.uv.x, in.uv.y, 0.25, 1.0);
}
