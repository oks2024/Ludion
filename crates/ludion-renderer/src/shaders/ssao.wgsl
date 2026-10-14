// Screen-space ambient occlusion, computed from the depth+normal prepass into a half-resolution
// factor the mesh pass multiplies into its ambient + IBL term (Plan/post-process-pipeline.md
// decision 8). The output is the finished factor — `1.0` unoccluded, `0.0` fully occluded — with the
// authored intensity already folded in, so the mesh shader does one texture read and one multiply.
//
// # The estimator
//
// For each pixel: reconstruct the world position `p` and read the world normal `n`, then throw a fixed
// cosine-weighted hemisphere of `ssao.samples` points around `n` inside a **world-space** radius.
// Each point is projected back into the frame and its depth compared against what the prepass
// actually recorded there; a recorded surface nearer the camera than the sample point means something
// is in the way. The occluded fraction, range-checked so a distant foreground object cannot cast AO
// across a gap, is the estimate.
//
// The radius is in world units, which is what makes the effect resolution- and render-scale-invariant
// by construction: a crease is a fixed size in the world, so its shading is a fixed thing rather than
// a number of pixels.
//
// # Determinism
//
// Nothing here reads a clock, a frame index or a random number generator. The kernel is a closed-form
// low-discrepancy sequence and the per-pixel rotation is a pure function of the fragment's position,
// so two processes rendering the same scene on the same adapter produce identical bytes — the engine's
// standing rule, and the reason `Plan/post-process-pipeline.md` decision 5's rejection of temporal
// accumulation costs this pass nothing but a blur.
//
// **The rotation is `BAYER4`, not interleaved gradient noise.** Both decorrelate neighbouring pixels;
// the Bayer tile is chosen because it has a *period* — 4×4 — and `ssao_blur` averages exactly one full
// period, so the pattern cancels completely instead of leaving the residue an aperiodic hash would.
// It is also the engine's one ordered tile already (`shaders/snippets/bayer.wgsl`), shared with the
// screen-door fade, the output dither and the light-shaft march — which is why `ssao_blur` and
// `shafts_blur` are two passes over one shader (`box_blur.wgsl`).
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl` and `BAYER4` from
// `shaders/snippets/bayer.wgsl`, both composed ahead of this body (Plan/shader-bindgen.md dec. 16).

struct Ssao {
    // World→clip, for projecting a hemisphere sample back into the frame.
    view_proj: mat4x4<f32>,
    // Clip→world, for reconstructing a world position from a stored depth.
    inv_view_proj: mat4x4<f32>,
    // The camera's world position — distances are measured from it, which is monotone along a ray and
    // so is a valid occlusion ordering.
    cam_pos: vec3<f32>,
    // The hemisphere's world-space radius (`PostProcess.ssao_radius`).
    radius: f32,
    // World-space depth bias: a sample must be this much further away than the recorded surface before
    // it counts as occluded (`PostProcess.ssao_bias`). Fights the self-occlusion a flat lit surface
    // would otherwise show as a uniform grey wash.
    bias: f32,
    // How much of the estimate to apply (`PostProcess.ssao`, the effect's gate and strength). Folded in
    // here rather than in the mesh pass so the AO texture *is* the factor.
    intensity: f32,
    // The hemisphere's sample count (`QualityParams.ssao_samples`). Sixteen is the conventional
    // quality/cost point and what a scene's own authoring implies; a machine that cannot afford it
    // lowers the count rather than losing the effect. A uniform rather than a `const` because it is a
    // **quality** setting — the client's side of the arc's standing split between what the machine
    // pays for and what the scene authors.
    samples: u32,
    pad0: f32,
};

// The prepass's depth. `Depth32Float` is unfilterable and this pass binds the shared *filtering*
// sampler, so every depth fetch is a `textureLoad` — the composite pass's fog reads scene depth the
// same way and for the same reason.
@group(0) @binding(0) var ssao_depth: texture_depth_2d;
// The prepass's world-space normals.
@group(0) @binding(1) var ssao_normal: texture_2d<f32>;
@group(0) @binding(2) var ssao_samp: sampler;
@group(0) @binding(3) var<uniform> ssao: Ssao;

const TAU: f32 = 6.28318530717959;

// Roberts' generalized golden-ratio (R2) 2D low-discrepancy sequence: `frac(i * a)` with `a` the
// reciprocals of the plastic number's powers. Chosen over Hammersley because it needs no bit
// reversal and is well distributed at *any* prefix length, so lowering `ssao.samples` keeps the same
// set of directions instead of quietly unbalancing them. The *radii* are deliberately not a prefix:
// the ramp below divides by the count, so a shorter kernel re-spreads its samples over the same
// radius range rather than crowding them all into the near end.
const R2_ALPHA: vec2<f32> = vec2<f32>(0.7548776662466927, 0.5698402909980532);

fn r2(i: u32) -> vec2<f32> {
    return fract(f32(i + 1u) * R2_ALPHA);
}

// A cosine-weighted direction in the +Z hemisphere from a unit square sample. Cosine weighting is what
// the ambient term itself is weighted by, so the estimator and the light it modulates agree.
fn cosine_hemisphere(u: vec2<f32>) -> vec3<f32> {
    let phi = TAU * u.x;
    let cos_theta = sqrt(1.0 - u.y);
    let sin_theta = sqrt(u.y);
    return vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
}

// The world-space position of the pixel at `uv` whose stored device depth is `depth`. wgpu's clip
// space is xy in [-1, 1] with +y up and z in [0, 1] near→far; `uv.y` runs top-down (the fullscreen
// triangle flips it), which is the one sign to get right. Same reconstruction the composite's fog
// uses.
fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = ssao.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // This pass runs at a fraction of the *output* extent while the prepass ran at the *render*
    // extent, so a target pixel maps to a source texel rather than to one 1:1. Snapping to that
    // texel's centre keeps the depth (loaded) and the normal (sampled through the shared filtering
    // sampler, which the binding convention requires this pass to bind) describing the same surface.
    let dims = vec2<f32>(textureDimensions(ssao_depth));
    let texel = floor(in.uv * dims);
    let depth = textureLoad(ssao_depth, vec2<i32>(texel), 0);
    // Nothing was drawn here: the background is not a surface and cannot be occluded.
    if (depth >= 1.0) {
        return vec4<f32>(1.0);
    }
    let source_uv = (texel + vec2<f32>(0.5)) / dims;
    let n = normalize(textureSampleLevel(ssao_normal, ssao_samp, source_uv, 0.0).xyz);
    let p = world_from_depth(source_uv, depth);
    let p_dist = length(p - ssao.cam_pos);

    // A per-pixel rotation of the tangent frame about the normal, so neighbouring pixels sample
    // different directions and `ssao_blur` can trade the resulting noise for a wider effective kernel.
    let angle = BAYER4[(u32(in.pos.y) % 4u) * 4u + (u32(in.pos.x) % 4u)] * TAU;
    var axis = vec3<f32>(0.0, 0.0, 1.0);
    if (abs(n.z) > 0.9) {
        axis = vec3<f32>(1.0, 0.0, 0.0);
    }
    let t0 = normalize(cross(axis, n));
    let b0 = cross(n, t0);
    let tangent = t0 * cos(angle) + b0 * sin(angle);
    let bitangent = cross(n, tangent);

    var occlusion = 0.0;
    for (var i: u32 = 0u; i < ssao.samples; i = i + 1u) {
        // Sample points cluster toward the origin (the squared ramp), which is where contact shadows
        // live and where the estimate is worth the most.
        let f = f32(i) / f32(ssao.samples);
        let d = cosine_hemisphere(r2(i)) * mix(0.1, 1.0, f * f);
        let s = p + (tangent * d.x + bitangent * d.y + n * d.z) * ssao.radius;

        let clip = ssao.view_proj * vec4<f32>(s, 1.0);
        if (clip.w <= 0.0) {
            continue;
        }
        let sndc = clip.xyz / clip.w;
        let suv = vec2<f32>(sndc.x * 0.5 + 0.5, 0.5 - sndc.y * 0.5);
        if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
            continue;
        }
        let stexel = floor(suv * dims);
        let sdepth = textureLoad(ssao_depth, vec2<i32>(stexel), 0);
        // The sample projected onto background: no surface, so no occluder.
        if (sdepth >= 1.0) {
            continue;
        }
        let q = world_from_depth((stexel + vec2<f32>(0.5)) / dims, sdepth);
        let q_dist = length(q - ssao.cam_pos);
        if (q_dist < length(s - ssao.cam_pos) - ssao.bias) {
            // Range check: an occluder far outside the hemisphere is a different object seen past an
            // edge, not a crease, and must not darken this pixel.
            occlusion = occlusion
                + smoothstep(0.0, 1.0, ssao.radius / max(abs(p_dist - q_dist), 1e-4));
        }
    }

    let ao = 1.0 - ssao.intensity * occlusion / f32(ssao.samples);
    return vec4<f32>(clamp(ao, 0.0, 1.0));
}
