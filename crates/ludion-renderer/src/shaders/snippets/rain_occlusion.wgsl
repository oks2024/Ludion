// The overhead rain-occlusion map's one read: the height of the nearest shadow caster over a world
// point's column (`rain_occlusion` pass, `rain.rs::occlusion_view`). Binding-free — the caller passes its
// own view and map — so the streaks (`rain.wgsl`) and the mesh wetness (`mesh_shading.wgsl`) share the
// arithmetic.
//
// The view is straight down and orthographic, so its depth is affine in height: column 1's z and the
// translation's z invert it.

// `top` of a column that holds no caster.
const NO_OCCLUDER: f32 = -1.0e9;

// `(top, inside)`: `inside` is 1 when `world`'s column lies within the map and 0 outside it; `top` is
// the caster's height there, or `NO_OCCLUDER` over a texel nothing was drawn into.
fn occluder_top(view: mat4x4<f32>, map: texture_depth_2d, world: vec3<f32>) -> vec2<f32> {
    let clip = view * vec4<f32>(world, 1.0);
    let uv = vec2<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0))) {
        return vec2<f32>(NO_OCCLUDER, 0.0);
    }
    let size = vec2<i32>(textureDimensions(map));
    let texel = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - vec2<i32>(1));
    let depth = textureLoad(map, texel, 0);
    if (depth >= 1.0) {
        return vec2<f32>(NO_OCCLUDER, 1.0);
    }
    return vec2<f32>((depth - view[3].z) / view[1].z, 1.0);
}
