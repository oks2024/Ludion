// The soft-edge gaussian the particle pass and the projector share, binding-free (particle,
// projector).

// The gaussian's exponent at the rim: `exp(-SOFT_FALLOFF)` is the value there. Steep enough that the
// truncation the renormalization at each use performs is a small correction rather than a visible
// step, shallow enough to stay a soft puff.
const SOFT_FALLOFF: f32 = 4.0;
