// The occluded-silhouette mask (`Plan/occluded-silhouette.md` decision 1): only the meshes that
// resolve an `OccludedSilhouette`, drawn into an `R8Unorm` style-index target and their own depth
// attachment. `composite` reads both beside `scene_depth` to tint the pixels a nearer surface hides.
// Present only when a mesh resolves one, so a silhouette-free frame never pays for it.
//
// It is the prepass's vertex stage verbatim (skinning included) and its cutout test, so a cutout
// hides exactly the texels the scene pass draws. There is **no** fade discard: the mask holds the
// whole body, and the scene's own fade still shows the real one. The style index rides the
// per-instance flash lane (`.x`), which this pass owns, and is written as `index / 255`.
//
// The vertex inputs are mesh.wgsl's verbatim, as the prepass declares them, so the pipeline reuses
// `MESH_VERTEX_ATTRS`/`MESH_INSTANCE_ATTRS`. The `Camera` uniform and the bone palette (@group 0) and
// the per-draw material (@group 4) are snippets composed ahead of this file.

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) vertex_material: vec4<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) tangent: vec4<f32>,
    @location(6) m0: vec4<f32>,
    @location(7) m1: vec4<f32>,
    @location(8) m2: vec4<f32>,
    @location(9) m3: vec4<f32>,
    @location(10) color: vec4<f32>,
    @location(11) instance_material: vec4<f32>,
    @location(12) n0: vec4<f32>,
    @location(13) n1: vec4<f32>,
    @location(14) n2: vec4<f32>,
    @location(15) n3: vec4<f32>,
    @location(16) fade: vec4<f32>,
    // The silhouette style index in `.x` (1-based), set per instance by `build_silhouette_draws`.
    @location(17) flash: vec4<f32>,
    @location(18) weights: vec4<f32>,
    @location(19) bones: vec4<u32>,
    // Declared to match the shared `MeshInstanceRaw` layout (the mesh pass's rim lane); unread here.
    @location(20) rim: vec4<f32>,
    // Declared to match the shared `MeshInstanceRaw` layout (the mesh pass's medium lane); unread here.
    @location(21) medium: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) style: f32,
    @location(1) uv: vec2<f32>,
    // The scene pass's own `vertex_color * color`, so the cutout coverage interpolates identically.
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var position = in.position;
    let bone_offset = u32(in.fade.z);
    if (bone_offset != 0u) {
        let skin = skin_matrix(bone_offset, in.bones, in.weights);
        position = (skin * vec4<f32>(position, 1.0)).xyz;
    }
    let world = model * vec4<f32>(position, 1.0);
    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.style = in.flash.x;
    out.uv = in.uv;
    out.color = in.vertex_color * in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var coverage = in.color.a;
    if ((material.flags & (MATERIAL_ALPHA_MASK | MATERIAL_BASE_COLOR)) == (MATERIAL_ALPHA_MASK | MATERIAL_BASE_COLOR)) {
        coverage = coverage * textureSample(base_color_tex, material_samp, in.uv).a;
    }
    if (material_masked(coverage)) {
        discard;
    }
    return vec4<f32>(in.style / 255.0, 0.0, 0.0, 1.0);
}
