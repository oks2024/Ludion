// Volumetric light shafts: a ray-march of the sun shadow map along the eye ray, at a fraction of the
// output extent, producing the **scalar** in-scattered fraction the composite tints with the sun's
// own colour (Plan/post-process-pipeline.md, decision 13).
//
// The pass exists only when the scene both asks for shafts and carries a sun: there is nothing to
// march without a shadow map, so a sunless scene asking for shafts simply gets none.
//
// # The estimator
//
// For each pixel, walk `shafts.steps` points along the ray from the camera through that pixel and ask
// the shadow map whether the sun reaches each one. A lit point scatters some of that light toward the
// camera; a shadowed one scatters nothing. The single-scattering integral this approximates is
//
//     L = ∫ σ_s(t) · p(cos θ) · V(t) · T(t) dt
//
// over the marched segment, where `V` is the shadow map's answer, `T(t) = exp(-∫₀ᵗ σ_t ds)` the
// medium's extinction of that scattered light on its way back to the lens, and `p` the phase function
// below.
// The result is a fraction in roughly `[0, p_max]`, scaled by `PostProcess.shafts` — the authored
// intensity is folded in **here**, so the blurred target *is* the term the composite adds, exactly as
// `ssao.wgsl` folds its own intensity in.
//
// `p` is the **Henyey–Greenstein** phase function (Henyey & Greenstein 1941), the standard
// single-parameter fit for forward-scattering atmospheric media:
//
//     p(cos θ) = (1 - g²) / (4π · (1 + g² - 2g·cos θ)^{3/2})
//
// with `θ` measured between the view ray and the direction *toward* the light.
//
// # This is one term of the composite's medium, not an effect of its own
//
// The medium is single-scattering with **one** extinction σ_t = `PostProcess.fog_density` and one
// scattering albedo α = `PostProcess.fog_albedo`, so σ_s = α·σ_t. The composite's fog is that
// medium's *ambient* in-scatter (isotropic, closed-form); this march is its *directional* one, the
// share of the sun's light the medium throws back along the eye ray. Both read the same σ_t and the
// same α, which is why thickening the fog both hazes the frame and brightens the beams, and why the
// fog dims its own beams. `g` is `PostProcess.fog_anisotropy`: at 0 the sun's contribution spreads
// as uniform fill, and it is the medium's forward bias that turns it into a beam.
//
// # The same medium, not just the same density
//
// The two terms also share the medium's **geometry** — where it begins and how it thins with height —
// through `shaders/snippets/medium.wgsl`, which both compose. `T(t)` here is
// `exp(-medium_optical_depth(…, t))`, the very function `composite.wgsl`'s `fog_factor` evaluates at
// the surface, and each sample's in-scatter coefficient is σ_s scaled by `medium_density_ratio` — the
// medium's *local* density at that point rather than its nominal one. So `fog_height` /
// `fog_height_falloff` thicken the beams toward the ground exactly as they thicken the haze, and a
// `fog_start` that clears the geometry removes the beams along with the fog.
//
// σ_s stays **outside** the loop and the ratio multiplies inside, which is what makes the height-free
// medium free rather than merely cheap: the ratio is then exactly `1.0` and the optical depth exactly
// `σ_t·t`, and multiplying by an exact 1.0 preserves every bit. The collapse is the formula's own
// limit, so there is no branch to skip and no second code path to keep in step.
//
// # The march's end
//
// A ray stops at whichever comes first: the surface the scene pass recorded at that pixel (so
// foreground geometry occludes the beams behind it), or the distance past which the medium's own
// extinction has nothing left to contribute — `SHAFT_OPTICAL_CUTOFF / σ_t`. That second term is why
// there is no reach field: the reach of an exponential medium is a function of its extinction, so a
// range knob would be a second spelling of `fog_density`.
//
// The march still *starts* at the camera under a nonzero `fog_start`, and the steps inside it
// contribute exactly zero through the density ratio rather than being skipped. Re-distributing the
// steps over `[fog_start, end]` would sample the medium better, but it would move every pixel of the
// height-free case for a handful of wasted samples — the neutral bytes are worth more than the
// resolution those steps would buy.
//
// # Determinism
//
// Nothing here reads a clock, a frame index or a random number generator. The step offset is
// `BAYER4` indexed by the fragment's pixel position, so two processes rendering the same scene on the
// same adapter produce identical bytes. Bayer rather than an aperiodic hash for the reason SSAO
// records: its period is 4×4, so `box_blur.wgsl` averages exactly one full period and the pattern
// cancels *exactly* instead of leaving residue. Sixteen offsets over `shafts.steps` steps is what lets
// a march this short resolve without banding once blurred — the amortisation decision 5 gave up with
// temporal accumulation, bought back spatially.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl` and `BAYER4` from
// `shaders/snippets/bayer.wgsl`, both composed ahead of this body (Plan/shader-bindgen.md dec. 16).

struct Shafts {
    // Clip→world, for reconstructing the eye ray and the recorded surface's world position.
    inv_view_proj: mat4x4<f32>,
    // World→light-clip: the same orthographic view-projection the shadow map was rendered with.
    light_space: mat4x4<f32>,
    // The camera's world position — where every ray starts.
    cam_pos: vec3<f32>,
    // The medium's extinction per world unit, σ_t (`PostProcess.fog_density`) — the same number the
    // composite's fog integrates.
    density: f32,
    // The normalized direction the sunlight travels, so the phase angle is measured against `-sun_dir`.
    sun_dir: vec3<f32>,
    // How much of the estimate to apply (`PostProcess.shafts`, the effect's gate and strength).
    intensity: f32,
    // The medium's single-scattering albedo α (`PostProcess.fog_albedo`): σ_s = α·σ_t is what the
    // estimate scales by, so an absorbing medium extinguishes without glowing.
    albedo: f32,
    // The Henyey–Greenstein asymmetry g (`PostProcess.fog_anisotropy`), exclusive `(-1, 1)` — the
    // phase denominator is singular at ±1.
    anisotropy: f32,
    // The medium's geometry, the same three numbers `composite.wgsl` shapes its haze with
    // (`PostProcess.fog_start` / `fog_height` / `fog_height_falloff`): where the medium begins along
    // the ray, the world height its density is σ_t at, and how fast it thins going up.
    fog_start: f32,
    fog_height: f32,
    fog_height_falloff: f32,
    // Steps per ray (`QualityParams.shaft_steps`). How finely the integral is sampled is a
    // **quality** setting, and the arc's standing split puts quality on the client's side of the line
    // and look on the scene's — the same call `ssao.wgsl`'s `samples` makes. At full quality it is
    // thirty-two, where the Bayer offsets plus the 4×4 blur (512 effective samples per blurred texel)
    // stop showing steps on a hard beam edge; a machine that cannot afford that marches coarser
    // rather than losing the beams.
    steps: u32,
    // The matrices give this struct 16-byte alignment, so its size rounds up to a multiple of 16 and
    // the two lanes past `steps` are part of the layout. They are spelled because `shader_gen`'s
    // generated POD carries no implicit padding — a hole has to have a name.
    pad0: f32,
    pad1: f32,
};

// The scene pass's depth, at the render extent. `Depth32Float` is unfilterable and this pass binds
// the shared *filtering* sampler, so every depth fetch is a `textureLoad` — the same pairing the
// composite's fog and the SSAO estimator rely on.
@group(0) @binding(0) var shafts_depth: texture_depth_2d;
// The sun shadow map, loaded rather than comparison-sampled: a marched point is in free space, not on
// a surface, so there is no acne to fight and nothing to filter — one exact texel is the answer.
@group(0) @binding(1) var shafts_shadow: texture_depth_2d;
@group(0) @binding(2) var shafts_samp: sampler;
@group(0) @binding(3) var<uniform> shafts: Shafts;

// How many e-foldings of the medium's extinction are worth marching. Past `3/σ_t` the transmittance
// is under 5% and the remaining integral cannot reach a display step.
const SHAFT_OPTICAL_CUTOFF: f32 = 3.0;

// The extinction floor used only to bound the march's length. A `fog_density` of exactly zero is a
// legal authored value — an empty medium scatters nothing — and this keeps the division finite while
// the `σ_s` factor in the integral still takes the result to zero.
const SHAFT_MIN_DENSITY: f32 = 1e-4;

const FOUR_PI: f32 = 12.5663706143592;

// The world-space position of the pixel at `uv` whose stored device depth is `depth`. wgpu's clip
// space is xy in [-1, 1] with +y up and z in [0, 1] near→far; `uv.y` runs top-down (the fullscreen
// triangle flips it), which is the one sign to get right. The same reconstruction the composite's fog
// and the SSAO estimator use.
fn world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = shafts.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

// Whether the sun reaches `world_pos`: 1 lit, 0 shadowed. A point outside the light frustum is lit,
// the same convention the mesh pass's PCF lookup uses — a shadow map covers what it covers, and
// treating everything beyond it as dark would put a hard black wall at the frustum's edge.
fn sun_visibility(world_pos: vec3<f32>) -> f32 {
    let clip = shafts.light_space * vec4<f32>(world_pos, 1.0);
    if (clip.w <= 0.0) {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, ndc.y * -0.5 + 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) {
        return 1.0;
    }
    let dims = vec2<i32>(textureDimensions(shafts_shadow));
    let texel = clamp(vec2<i32>(uv * vec2<f32>(dims)), vec2<i32>(0), dims - vec2<i32>(1));
    let occluder = textureLoad(shafts_shadow, texel, 0);
    return select(0.0, 1.0, ndc.z <= occluder);
}

fn henyey_greenstein(cos_theta: f32) -> f32 {
    let g = shafts.anisotropy;
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    // The floor is defence in depth against a *validating* `g` close enough to ±1 that the exact
    // backscatter ray takes the denominator below f32 resolution; the component rejects ±1 itself.
    return (1.0 - g2) / (FOUR_PI * pow(max(denom, 1e-4), 1.5));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // This pass runs at a fraction of the *output* extent while the scene pass ran at the *render*
    // extent, so a target pixel maps to a source texel rather than to one 1:1. Snapping to that
    // texel's centre keeps the depth and the ray describing the same pixel of the scene.
    let dims = vec2<f32>(textureDimensions(shafts_depth));
    let texel = floor(in.uv * dims);
    let source_uv = (texel + vec2<f32>(0.5)) / dims;
    let depth = textureLoad(shafts_depth, vec2<i32>(texel), 0);

    let ray = normalize(world_from_depth(source_uv, 1.0) - shafts.cam_pos);
    var end = SHAFT_OPTICAL_CUTOFF / max(shafts.density, SHAFT_MIN_DENSITY);
    if (depth < 1.0) {
        // A surface was drawn here: the medium in front of it is all there is to march.
        end = min(end, length(world_from_depth(source_uv, depth) - shafts.cam_pos));
    }

    // The step offset, one full `BAYER4` period across every 4×4 block of this target — which is
    // exactly what `shafts_blur` averages away.
    let jitter = BAYER4[(u32(in.pos.y) % 4u) * 4u + (u32(in.pos.x) % 4u)];
    let dt = end / f32(shafts.steps);
    var visible = 0.0;
    for (var i: u32 = 0u; i < shafts.steps; i = i + 1u) {
        let t = (f32(i) + jitter) * dt;
        // `ray` is normalized, so its y *is* the height gained per unit marched — the `up` both
        // medium functions measure the sample's world height with.
        let transmittance = exp(-medium_optical_depth(
            shafts.density,
            shafts.fog_start,
            shafts.fog_height,
            shafts.fog_height_falloff,
            shafts.cam_pos.y,
            ray.y,
            t,
        ));
        let local = medium_density_ratio(
            shafts.fog_start,
            shafts.fog_height,
            shafts.fog_height_falloff,
            shafts.cam_pos.y,
            ray.y,
            t,
        );
        visible = visible + sun_visibility(shafts.cam_pos + ray * t) * transmittance * local;
    }

    let phase = henyey_greenstein(dot(ray, -shafts.sun_dir));
    // σ_s = α·σ_t is the coefficient of the in-scatter, so an absorbing medium (α → 0) buries the
    // surfaces behind it without glowing. It is the medium's *nominal* coefficient and stays out here
    // because each sample already carries its own height ratio; the two multiply to the local σ_s.
    let sigma_s = shafts.albedo * shafts.density;
    return vec4<f32>(visible * dt * sigma_s * phase * shafts.intensity);
}
