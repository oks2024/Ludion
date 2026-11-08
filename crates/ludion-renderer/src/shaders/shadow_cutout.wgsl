// The cutout shadow caster: `shadow.wgsl`'s depth-only pass plus a fragment stage that discards a mask
// material's holes, so an `alpha_mode: mask` mesh (a fence, foliage, a grate) casts the shadow its
// visible texels would, not its whole silhouette. It is a separate file because a material group in
// `shadow.wgsl`'s layout would force every opaque caster to bind one; those keep the depth-only pass.
//
// The coverage is the scene pass's and the prepass's own expression (`Mesh.color` alpha times the base
// map's alpha), over `snippets/mesh_material.wgsl`'s `material_masked`. Fade stays out of shadows, as
// in `shadow.wgsl`.
//
// The vertex inputs are mesh.wgsl's verbatim, as in prepass.wgsl, so the pipeline reuses
// `MESH_VERTEX_ATTRS`/`MESH_INSTANCE_ATTRS` rather than a third hand-offset attribute array.

struct ShadowView {
    light_space: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vertex_color: vec4<f32>,
    @location(3) vertex_material: vec3<f32>,
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
    @location(17) flash: vec4<f32>,
    @location(18) weights: vec4<f32>,
    @location(19) bones: vec4<u32>,
    // Declared to match the shared `MeshInstanceRaw` layout (the mesh pass's rim lane); unread here.
    @location(20) rim: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
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
    var out: VsOut;
    out.clip = shadow_view.light_space * (model * vec4<f32>(position, 1.0));
    out.uv = in.uv;
    out.color = in.vertex_color * in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) {
    var coverage = in.color.a;
    if ((material.flags & MATERIAL_BASE_COLOR) != 0u) {
        coverage = coverage * textureSample(base_color_tex, material_samp, in.uv).a;
    }
    if (material_masked(coverage)) {
        discard;
    }
}
