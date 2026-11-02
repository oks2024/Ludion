// The transparent layer's fragment value: a layer fragment at `world` of coverage `weight` writes
// (distance * weight, weight, 0, weight), so under `(One, OneMinusSrcAlpha)` blending the target holds
// the coverage-weighted view distance in R and the accumulated coverage in G. R / G is the layer's
// mean distance. Needs the `Camera` snippet ahead of it. Distance is the length of the view ray, or
// under an orthographic camera the depth along the single view direction (`view_ray.wgsl`).
const TRANSPARENT_LAYER_MAX_DISTANCE: f32 = 60000.0;

fn transparent_layer(world: vec3<f32>, weight: f32) -> vec4<f32> {
    let to_point = world - camera.cam_pos.xyz;
    let along = select(length(to_point), dot(to_point, camera.view_forward.xyz),
                       camera.view_forward.w > 0.5);
    let t = clamp(along, 0.0, TRANSPARENT_LAYER_MAX_DISTANCE);
    return vec4<f32>(t * weight, weight, 0.0, weight);
}
