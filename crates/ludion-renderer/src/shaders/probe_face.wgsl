// One captured probe cube face: the lit geometry as linear HDR, with sky visibility in alpha.
//
// The face is rendered with a right-handed camera and stored mirrored in x, because the cube-map
// convention (`ibl.wgsl` `face_dir`) is left-handed against it. Alpha is 1 where the depth buffer
// is untouched (sky) and 0 on geometry: the scene pass writes alpha 1 for both, so it carries no
// mask. The rgb is 0 on sky: a probe shades its sky fraction from the live sky cube, so the capture
// holds geometry only. The second target is the emissive part of the same face (the scene pass's
// fourth target), mirrored and sky-masked identically with alpha 0.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`; `hdr_safe` from `snippets/hdr.wgsl`.

struct ProbeFace {
    // Unused: every fullscreen effect binds a uniform.
    pad: vec4<f32>,
}

@group(0) @binding(0) var face_scene: texture_2d<f32>;
@group(0) @binding(1) var face_depth: texture_depth_2d;
// Declared but never sampled: exact `textureLoad`s only.
@group(0) @binding(2) var face_emissive: texture_2d<f32>;
@group(0) @binding(3) var face_samp: sampler;
@group(0) @binding(4) var<uniform> face_params: ProbeFace;

struct FaceOut {
    @location(0) color: vec4<f32>,
    @location(1) emissive: vec4<f32>,
}

@fragment
fn fs_main(in: VsOut) -> FaceOut {
    let dims = vec2<i32>(textureDimensions(face_scene));
    let out_texel = vec2<i32>(floor(in.pos.xy));
    let texel = vec2<i32>(dims.x - 1 - out_texel.x, out_texel.y);
    let sky = textureLoad(face_depth, texel, 0) >= 1.0;
    let color = hdr_safe(textureLoad(face_scene, texel, 0).rgb);
    let emissive = hdr_safe(textureLoad(face_emissive, texel, 0).rgb);
    return FaceOut(
        vec4<f32>(select(color, vec3<f32>(0.0), sky), select(0.0, 1.0, sky)),
        vec4<f32>(select(emissive, vec3<f32>(0.0), sky), 0.0),
    );
}
