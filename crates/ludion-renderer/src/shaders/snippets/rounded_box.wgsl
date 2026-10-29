// Signed distance to a rounded box centred on the origin: negative inside, zero on the outline.
// The one footprint function of a disc (half-extent 0, radius r) and a capsule (half-extent
// ((L - w)/2, 0), radius w/2), shared by the ground-marker and projector passes.
fn sd_rounded_box(p: vec2<f32>, half_extent: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_extent;
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}
