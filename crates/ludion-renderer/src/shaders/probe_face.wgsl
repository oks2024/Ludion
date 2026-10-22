// One captured probe cube face: the lit scene as linear HDR, with sky visibility in alpha.
//
// The face is rendered with a right-handed camera and stored mirrored in x, because the cube-map
// convention (`ibl.wgsl` `face_dir`) is left-handed against it. Alpha is 1 where the depth buffer
// is untouched (sky) and 0 on geometry: the scene pass writes alpha 1 for both, so it carries no
// mask. The rgb keeps the sky, because a probe shades from it without a relight.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`; `hdr_safe` from `snippets/hdr.wgsl`.

struct ProbeFace {
    // Unused: every fullscreen effect binds a uniform.
    pad: vec4<f32>,
}

@group(0) @binding(0) var face_scene: texture_2d<f32>;
@group(0) @binding(1) var face_depth: texture_depth_2d;
// Declared but never sampled: exact `textureLoad`s only.
@group(0) @binding(2) var face_samp: sampler;
@group(0) @binding(3) var<uniform> face_params: ProbeFace;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(face_scene));
    let out_texel = vec2<i32>(floor(in.pos.xy));
    let texel = vec2<i32>(dims.x - 1 - out_texel.x, out_texel.y);
    let sky = textureLoad(face_depth, texel, 0) >= 1.0;
    return vec4<f32>(hdr_safe(textureLoad(face_scene, texel, 0).rgb), select(0.0, 1.0, sky));
}
