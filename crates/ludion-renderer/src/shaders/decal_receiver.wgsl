// The decal receiver-id pass: the meshes some decal lists, redrawn once more into an `R32Float` target
// that holds `mesh index + 1` where the redraw is the visible surface and 0 everywhere else. The
// projector reads it per pixel to honour a decal's `Exclude`/`Only` list, so a decal can skip the
// player's skinned mesh or paint only the floor without a scene-wide id attachment.
//
// There is no depth attachment: a fragment writes only where its depth matches the opaque pass's
// `scene_depth` (within `DEPTH_SLACK`), so a listed mesh hidden behind another surface never claims
// the pixel. The screen-door discard is the prepass's, so a mesh the camera-occlusion policy has
// dithered away claims only the pixels the scene pass drew. The cutout test is the scene pass's too
// (`material_masked` over the same coverage), so a mask mesh's holes claim nothing.
//
// The vertex inputs are mesh.wgsl's verbatim, as the prepass's are (all twenty locations), so the
// pipeline reuses `MESH_VERTEX_ATTRS`/`MESH_INSTANCE_ATTRS`. The `Camera` uniform and the bone palette
// (@group 0), the shared `BAYER4` tile and the per-draw material (@group 4) are snippets composed
// ahead of this file.

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
    // Declared to match the shared `MeshInstanceRaw` layout (the mesh pass's medium lane); unread here.
    @location(21) medium: f32,
};

// Where this draw's instances start in `recv_ids`.
struct RecvDraw {
    base: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

// The opaque pass's depth, at the render extent (loaded, never filtered).
@group(1) @binding(0) var recv_depth: texture_depth_2d;
// `scene.meshes` index of every instance of every mesh draw, the draws' `sources` concatenated.
@group(1) @binding(1) var<storage, read> recv_ids: array<u32>;
@group(1) @binding(2) var<uniform> recv_draw: RecvDraw;

// How far the redraw's depth may sit from `scene_depth`, as a share of the distance to the far
// plane's depth (1 - depth), plus an absolute floor: two rasterizations of one surface through
// different vertex paths differ in the last bits.
const DEPTH_SLACK: f32 = 1e-3;
const DEPTH_FLOOR: f32 = 1e-7;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_y: f32,
    @location(1) @interpolate(flat) fade: vec2<f32>,
    @location(2) @interpolate(flat) instance: u32,
    @location(3) uv: vec2<f32>,
    // The scene pass's own `vertex_color * color`, so the cutout coverage interpolates identically.
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn, @builtin(instance_index) instance: u32) -> VsOut {
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
    out.world_y = world.y;
    out.fade = in.fade.xy;
    out.instance = instance;
    out.uv = in.uv;
    out.color = in.vertex_color * in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The prepass's screen-door discard, over the same tile at the same extent.
    var eff_fade = 1.0;
    if (in.world_y > in.fade.y) {
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
    let scene = textureLoad(recv_depth, vec2<i32>(in.clip.xy), 0);
    if (abs(in.clip.z - scene) > DEPTH_SLACK * (1.0 - scene) + DEPTH_FLOOR) {
        discard;
    }
    return vec4<f32>(f32(recv_ids[recv_draw.base + in.instance] + 1u), 0.0, 0.0, 0.0);
}
