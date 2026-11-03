// The world-space position of a pixel from its stored device depth, binding-free: the caller passes
// its uniform's `inv_view_proj` (ssao, projector, dof_coc).

// The world-space position of the pixel at `uv` whose stored device depth is `depth`. wgpu's clip
// space is xy in [-1, 1] with +y up and z in [0, 1] near→far; `uv.y` runs top-down (the fullscreen
// triangle flips it), which is the one sign to get right.
fn world_from_depth(uv: vec2<f32>, depth: f32, inv_view_proj: mat4x4<f32>) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}
