// The depth+normal prepass: every opaque mesh rasterized once more, writing world-space normals into
// an HDR colour target and depth into its own depth attachment. Present only when the scene enables
// ambient occlusion (Plan/post-process-pipeline.md decision 8), so an AO-free frame never pays for it.
//
// It owns its depth rather than sharing the scene pass's: the graph derives pass order from *reads*
// alone, so two passes writing one resource are unordered in principle, and the scene pass therefore
// keeps its own `Less` depth test and its own buffer. The cost is that geometry is rasterized twice on
// an AO frame — which is exactly the geometry SSAO reads anyway.
//
// **Opaque meshes only.** Sprites, billboards and text are alpha-blended, so they have no single depth
// or normal to record; the sky is background and is never an occluder. AO is therefore computed from
// the mesh silhouette alone, which is the same restriction the shadow pass already lives under.
//
// The vertex inputs are mesh.wgsl's verbatim — all twenty locations, including the ones this pass
// ignores — so the pipeline reuses `MESH_VERTEX_ATTRS`/`MESH_INSTANCE_ATTRS` instead of hand-writing a
// third attribute array with hand-computed byte offsets into `MeshInstanceRaw`.
//
// The `Camera` uniform and the bone palette (@group 0), the shared `BAYER4` dither tile and the
// per-draw material (@group 4, `snippets/mesh_material.wgsl`) are snippets composed ahead of this file
// (Plan/shader-bindgen.md decision 16). The material is read only for the cutout test and the
// double-sided back-face flip, so a cutout occludes exactly the texels the scene pass draws.

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
    // Declared to match the shared `MeshInstanceRaw` layout (the mesh pass's hit-flash lane); the
    // prepass writes only depth + normals and never reads it.
    @location(17) flash: vec4<f32>,
    @location(18) weights: vec4<f32>,
    @location(19) bones: vec4<u32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) @interpolate(flat) fade: vec2<f32>,
    @location(3) uv: vec2<f32>,
    // The scene pass's own `vertex_color * color`, so the cutout coverage interpolates identically.
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var position = in.position;
    var normal = in.normal;
    let bone_offset = u32(in.fade.z);
    if (bone_offset != 0u) {
        let skin = skin_matrix(bone_offset, in.bones, in.weights);
        position = (skin * vec4<f32>(position, 1.0)).xyz;
        normal = skin_normal(skin, normal);
    }
    let world = model * vec4<f32>(position, 1.0);
    let normal_mat = mat4x4<f32>(in.n0, in.n1, in.n2, in.n3);
    var out: VsOut;
    out.clip = camera.view_proj * world;
    out.world_pos = world.xyz;
    out.world_normal = (normal_mat * vec4<f32>(normal, 0.0)).xyz;
    out.fade = in.fade.xy;
    out.uv = in.uv;
    out.color = in.vertex_color * in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    // The same screen-door discard the scene pass applies, over the same tile at the same extent, so
    // the two passes rasterize the same silhouette. Without it a building the camera-occlusion policy
    // has dithered away would still occlude — AO cast by geometry the viewer cannot see.
    var eff_fade = 1.0;
    if (in.world_pos.y > in.fade.y) {
        eff_fade = in.fade.x;
    }
    let bx = u32(in.clip.x) % 4u;
    let by = u32(in.clip.y) % 4u;
    if (eff_fade <= BAYER4[by * 4u + bx]) {
        discard;
    }
    var coverage = in.color.a;
    if ((material.flags & (MATERIAL_ALPHA_MASK | MATERIAL_BASE_COLOR)) == (MATERIAL_ALPHA_MASK | MATERIAL_BASE_COLOR)) {
        coverage = coverage * textureSample(base_color_tex, material_samp, in.uv).a;
    }
    if (material_masked(coverage)) {
        discard;
    }
    var normal = normalize(in.world_normal);
    if (material_back_face(front_facing)) {
        normal = -normal;
    }
    // World-space, stored signed: the target is `Rgba16Float`, so a normal needs no encode/decode pair
    // (an 8-bit unorm target would, and an sRGB one would be outright wrong).
    return vec4<f32>(normal, 1.0);
}
