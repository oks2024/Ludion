// The unit quad every billboard-like pass draws, binding-free (ground_marker, particle, sprite).

// The six corners of a quad (two triangles) in [-0.5, 0.5], the sprite pass's convention.
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5),
    vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, 0.5), vec2<f32>(-0.5, 0.5),
);

// The quad is drawn slightly larger than its shape so the `fwidth` fringe has room to fall off
// instead of being clipped at the shape's own edge. A ratio rather than an absolute margin: the
// extra band is then the same fraction of any shape, and its fragments are fully transparent.
// The sprite pass draws the exact quad and does not read it.
const QUAD_PAD: f32 = 1.05;
