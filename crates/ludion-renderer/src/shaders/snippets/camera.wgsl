// Shared camera uniform: the per-frame view-projection and world-space camera position, at
// @group(0) @binding(0). Composed ahead of every shader that projects world geometry (mesh, sprite,
// text) so the struct + binding live in exactly one place (Plan/shader-bindgen.md decision 16);
// `ludion-shader-bindgen` reflects it into `shader_gen::Camera` and hard-errors on any drift.
struct Camera {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
