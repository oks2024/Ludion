// The SSR trace's shared pure functions, binding-free. `ssr_confidence` is the twin of
// `ssr_trace.rs::confidence`; `ssr_trace.rs`'s `needs_gpu_the_ssr_confidence_snippet_matches_its_rust_twin`
// fails on any edit made to one side.

const SSR_EDGE_FADE: f32 = 0.1;
const SSR_LENGTH_FADE: f32 = 0.2;
const SSR_BACKFACE_FADE: f32 = 0.1;

// A trace texel's source pixel at the render extent: the render pixel its centre lands in. The trace
// takes its origin depth, normal and roughness from exactly this pixel, and the upsample reads the
// same mapping.
fn ssr_source_pixel(j: vec2<i32>, render: vec2<u32>, trace: vec2<u32>) -> vec2<i32> {
    let p = floor((vec2<f32>(j) + vec2<f32>(0.5)) * vec2<f32>(render) / vec2<f32>(trace));
    return vec2<i32>(min(p, vec2<f32>(render) - vec2<f32>(1.0)));
}

// How far to trust a hit, 0..=1: a product of linear ramps, so off-screen, too long, too thick,
// back-facing and too rough all fall to 0. `fade` is (full-below, zero-above) roughness.
fn ssr_confidence(
    uv: vec2<f32>,
    hit_distance: f32,
    thickness_excess: f32,
    facing: f32,
    roughness: f32,
    fade: vec2<f32>,
    max_distance: f32,
) -> f32 {
    let edge = saturate(min(uv.x, 1.0 - uv.x) / SSR_EDGE_FADE) * saturate(min(uv.y, 1.0 - uv.y) / SSR_EDGE_FADE);
    let length_fade = saturate((1.0 - hit_distance / max_distance) / SSR_LENGTH_FADE);
    let thickness = saturate(2.0 * (1.0 - thickness_excess));
    let back_face = saturate(-facing / SSR_BACKFACE_FADE);
    let rough = saturate((fade.y - roughness) / (fade.y - fade.x));
    return edge * length_fade * thickness * back_face * rough;
}
