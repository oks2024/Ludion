// Debug-line pass: world-space 1-px lines, drawn inside the overlay pass after the tonemapped frame
// has been blitted into the LDR output and before the UI, so bloom, depth of field and tonemapping
// never touch them.
//
// The pipeline has no depth attachment. Occlusion is a manual test against the scene pass's depth
// buffer, bound as a texture: a fragment behind scene geometry is drawn dimmed (x-ray) rather than
// dropped, so a hidden collider outline stays readable. The depth buffer is at the *render* extent
// and the overlay target at the *output* extent — they differ under supersampling and sub-native
// scale — so the fragment maps to a depth texel by ratio.
//
// The `Camera` uniform (@group 0) is a shared snippet composed ahead of this file
// (`shaders/snippets/camera.wgsl`). Colors are linear; the sRGB target encodes on store.

struct DebugView {
    // xy = the output (overlay target) pixel extent; zw = padding to a 16-byte uniform.
    viewport: vec2<f32>,
    pad: vec2<f32>,
};

@group(1) @binding(0) var debug_depth: texture_depth_2d;
@group(1) @binding(1) var<uniform> debug_view: DebugView;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// Alpha multiplier for a line the scene's geometry hides.
const OCCLUDED_ALPHA: f32 = 0.3;
// Depth slack so a line lying on a surface (a collider outline on its own mesh) does not read as hidden.
const DEPTH_BIAS: f32 = 1e-4;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let extent = vec2<f32>(textureDimensions(debug_depth));
    let texel = clamp(
        vec2<i32>(in.clip.xy * extent / debug_view.viewport),
        vec2<i32>(0, 0),
        vec2<i32>(extent) - vec2<i32>(1, 1),
    );
    let scene = textureLoad(debug_depth, texel, 0);
    let hidden = in.clip.z > scene + DEPTH_BIAS;
    let alpha = select(in.color.a, in.color.a * OCCLUDED_ALPHA, hidden);
    return vec4<f32>(in.color.rgb, alpha);
}
