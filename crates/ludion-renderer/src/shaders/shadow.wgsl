// Depth-only sun shadow caster pass: every mesh rendered into the sun's orthographic shadow map.
//
// Consumes only the vertex position and the per-instance model matrix — the same two vertex buffers
// the mesh pass binds (MeshVertex + MeshInstanceRaw), so the shadow pipeline reuses them verbatim and
// only the positions + model rows are read (the other locations are simply not declared here). There
// is no color target and no fragment stage: the pass writes depth alone.
//
// Occlusion fade (Environment/IBL) is deliberately NOT applied here: a faded/dithered instance
// still casts its full, solid shadow. Fade is a camera-occlusion readability aid on the beauty pass
// only — dithering the shadow too would make a cutaway building's shadow flicker or vanish, which
// reads worse, and this depth-only pass has no fragment stage to `discard` from. v0 keeps shadows
// fade-independent (revisit only if a real look demands see-through shadows).
//
// The light-space view-projection is this shader's own uniform (`ShadowView.light_space`), distinct
// in name and slot from mesh.wgsl's group-2 `sun` (Plan/shadow-mapping.md constraint 3) — the two
// carry the same matrix value (uploaded from one `SunData.light_space`) but never share a WGSL name.

struct ShadowView {
    light_space: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;

struct VsIn {
    @location(0) position: vec3<f32>,
    // Per-instance model matrix rows (locations 6-9, matching mesh.wgsl's instance buffer).
    @location(6) m0: vec4<f32>,
    @location(7) m1: vec4<f32>,
    @location(8) m2: vec4<f32>,
    @location(9) m3: vec4<f32>,
    // The bone-palette offset lane (mesh.wgsl's `fade.z`) — a skinned mesh casts its deformed shadow,
    // not its rest pose. `0` = unskinned, never read.
    @location(16) fade: vec4<f32>,
    @location(18) weights: vec4<f32>,
    @location(19) bones: vec4<u32>,
};

@vertex
fn vs_main(in: VsIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var position = in.position;
    let bone_offset = u32(in.fade.z);
    if (bone_offset != 0u) {
        let skin = skin_matrix(bone_offset, in.bones, in.weights);
        position = (skin * vec4<f32>(position, 1.0)).xyz;
    }
    let world = model * vec4<f32>(position, 1.0);
    return shadow_view.light_space * world;
}
