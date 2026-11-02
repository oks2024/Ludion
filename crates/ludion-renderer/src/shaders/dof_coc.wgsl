// The depth-of-field prefilter: the scene at a fraction of the output extent, carrying each texel's
// **signed circle of confusion** in its alpha channel (Plan/post-process-pipeline.md).
//
// # Why the gather does not do this itself
//
// A gather with N taps would otherwise fetch a colour *and* a depth per tap and reconstruct a world
// position from each — three fetches and a matrix multiply where this pass leaves one filterable
// RGBA fetch. It also lets the colour be **area-averaged** into the half-extent target once instead
// of point-sampled N times, and it lets the circle of confusion be taken from the **nearest** of the
// source texels a target texel covers, which is what stops a foreground silhouette from thinning as
// it is downsampled. So the pass earns its place three times over; it is the same bright-pass-then-
// blur shape bloom already has.
//
// # The circle of confusion
//
// A thin lens focused at distance `s` images a point at distance `d` as a disc whose diameter is
//
//     CoC = A · f² / (s - f) · |1/s - 1/d|  =  K · |1 - s/d|
//
// so up to the constant `K` — aperture and focal length, which no scene authors separately — the
// circle of confusion is exactly `1 - s/d`. `PostProcess.dof` **is** that constant, expressed as a
// fraction of the frame's height, and the expression below is the thin-lens formula verbatim:
//
//     coc = dof · (1 - focus_distance / distance)
//
// Three properties come free from writing it that way rather than as a band plus a ramp:
//
//   - it is exactly `0` at the focal plane, so the neutral case is the formula's own zero and not an
//     epsilon (the same posture `fog_height_falloff`'s analytic limit takes);
//   - it **saturates at `dof`** as the distance runs to infinity, so `dof` is literally the blur
//     radius the background reaches, and the background needs no special case at all — a pixel still
//     at the far plane reconstructs to the far plane's distance and lands on that saturation value;
//   - the sign separates near from far, which is the whole of what the gather needs to tell a
//     foreground that spreads over its neighbours from a background that must not.
//
// There is deliberately **no in-focus range field**. Under the thin-lens relation the sharp band is
// not independent of the aperture — it is `|1 - s/d| · dof < half a pixel`, so stopping down widens
// it — and spelling it separately would let a scene author a lens that cannot exist while making
// `dof` mean two things. It is the same call `fog_density` makes by setting both the medium's haze and
// how far its beams reach, and `vignette_radius` makes by needing no smoothness twin.
//
// The near side is clamped to the same `dof` magnitude as the far side. Physically it grows without
// bound as the distance runs to zero; the gather's disc has a fixed radius, so the clamp is what the
// kernel can actually represent, and it puts full near blur at half the focus distance.
//
// `VsOut` + `vs_main` come from `shaders/snippets/fullscreen_vs.wgsl`, composed ahead of this body
// (Plan/shader-bindgen.md decision 16).

struct DofCoc {
    // Clip→world, for reconstructing the recorded surface's world position from its depth.
    inv_view_proj: mat4x4<f32>,
    // The camera's world position — distances are measured from it, exactly as the SSAO estimator
    // and the light-shaft march measure theirs.
    cam_pos: vec3<f32>,
    // The focal plane's distance from the camera, in world units (`PostProcess.focus_distance`).
    focus_distance: f32,
    // The far field's circle of confusion, as a fraction of the frame's **height**
    // (`PostProcess.dof`, the effect's gate and strength). Screen-relative, never texels, so the
    // blur covers the same fraction of the image at any resolution and any render scale.
    max_coc: f32,
    // Nonzero when `coc_layer` is real: it mirrors the graph's own gate, as the composite's flag does.
    transparent_depth: u32,
    pad1: f32,
    pad2: f32,
    // `.xyz` is the unit view direction and `.w` is 1 under an orthographic camera (0 otherwise), where
    // distance is measured along the view and not out from the eye.
    view_forward: vec4<f32>,
};

// The scene pass's HDR colour, at the render extent.
@group(0) @binding(0) var coc_scene: texture_2d<f32>;
// The scene pass's depth. `Depth32Float` is unfilterable and this pass binds the shared *filtering*
// sampler, so every depth fetch is a `textureLoad` — the same pairing the composite's fog, the SSAO
// estimator and the shaft march all rely on.
@group(0) @binding(1) var coc_depth: texture_depth_2d;
// The transparent layer: weighted view distance in R, coverage in G, at the render extent — the
// **optional** input, behind the required reads. It exists when the scene draws a sprite, particle or
// world text under depth of field or fog; otherwise the recorder binds a 1x1 stand-in and
// `coc_params.transparent_depth` is zero, so the block below never reads it. Fetched by `textureLoad`
// like the depth.
@group(0) @binding(2) var coc_layer: texture_2d<f32>;
@group(0) @binding(3) var coc_samp: sampler;
@group(0) @binding(4) var<uniform> coc_params: DofCoc;

// The distance floor in the reciprocal. A surface exactly at the lens is not a scene an author can
// build, but a degenerate matrix could reconstruct one, and the division must stay finite.
const DOF_MIN_DISTANCE: f32 = 1e-3;

// The world-space position of the pixel at `uv` whose stored device depth is `depth`. wgpu's clip
// space is xy in [-1, 1] with +y up and z in [0, 1] near→far; `uv.y` runs top-down (the fullscreen
// triangle flips it), which is the one sign to get right. The same reconstruction the composite's
// fog, the SSAO estimator and the shaft march use.
fn coc_world_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    let world = coc_params.inv_view_proj * vec4<f32>(ndc, 1.0);
    return world.xyz / world.w;
}

// The distance a world point is from the lens: out from the eye, or along the view under an
// orthographic camera (the same measure the transparent layer's `t` uses). An early return, not a
// `select`: the perspective arithmetic must stay the one it always was.
fn coc_view_distance(world: vec3<f32>) -> f32 {
    let to_point = world - coc_params.cam_pos;
    if (coc_params.view_forward.w > 0.5) {
        return dot(to_point, coc_params.view_forward.xyz);
    }
    return length(to_point);
}

fn dof_circle_of_confusion(distance: f32) -> f32 {
    let coc = coc_params.max_coc
        * (1.0 - coc_params.focus_distance / max(distance, DOF_MIN_DISTANCE));
    return clamp(coc, -coc_params.max_coc, coc_params.max_coc);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(coc_depth));
    // The 2x2 block of source texels this target texel sits between at a 2:1 ratio (the native case).
    // Above render scale 1 the ratio widens and this is a 2x2 sample of a larger block — today's
    // behaviour for the bright pass at a wider ratio, recorded in the graph's module docs, not a new
    // approximation.
    let corner = floor(in.uv * dims - vec2<f32>(0.5));
    let last = dims - vec2<f32>(1.0);
    if (coc_params.transparent_depth != 0u) {
        // A block holding a transparent takes its circle of confusion from the transparent's own
        // distance, picking the nearest texel by *distance* because a layer texel has no device depth.
        // A block with no coverage falls through to the depth-picked path below: picking by distance
        // there could choose a different texel off-axis than picking by depth does, and that path is
        // the frame without a layer, byte for byte.
        var chosen_distance = 3.0e38;
        var chosen_coc = 0.0;
        var covered = false;
        for (var i = 0u; i < 4u; i = i + 1u) {
            let texel = clamp(
                corner + vec2<f32>(f32(i & 1u), f32(i >> 1u)),
                vec2<f32>(0.0),
                last,
            );
            let layer = textureLoad(coc_layer, vec2<i32>(texel), 0);
            covered = covered || layer.g > 0.0;
            let uv = (texel + vec2<f32>(0.5)) / dims;
            let depth = textureLoad(coc_depth, vec2<i32>(texel), 0);
            let back = coc_view_distance(coc_world_from_depth(uv, depth));
            var distance = back;
            var coc = dof_circle_of_confusion(back);
            if (layer.g > 0.0) {
                let t = layer.r / max(layer.g, 1e-4);
                coc = mix(coc, dof_circle_of_confusion(t), layer.g);
                if (layer.g >= 0.5) {
                    distance = t;
                }
            }
            if (distance < chosen_distance) {
                chosen_distance = distance;
                chosen_coc = coc;
            }
        }
        if (covered) {
            let color = hdr_safe(textureSampleLevel(coc_scene, coc_samp, in.uv, 0.0).rgb);
            return vec4<f32>(color, chosen_coc);
        }
    }
    var nearest = 1.0;
    var nearest_uv = in.uv;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let texel = clamp(
            corner + vec2<f32>(f32(i & 1u), f32(i >> 1u)),
            vec2<f32>(0.0),
            last,
        );
        let depth = textureLoad(coc_depth, vec2<i32>(texel), 0);
        // The *nearest* surface, not the average: a half-extent texel straddling a silhouette
        // belongs to the object in front of it, and taking the mean depth there would put the
        // silhouette at a distance nothing in the scene occupies.
        if (depth < nearest) {
            nearest = depth;
            nearest_uv = (texel + vec2<f32>(0.5)) / dims;
        }
    }

    // One bilinear tap, which at the native 2:1 ratio lands exactly between four source texels and so
    // *is* their box average — the bright pass's trick, for the same reason.
    let color = hdr_safe(textureSampleLevel(coc_scene, coc_samp, in.uv, 0.0).rgb);
    let world = coc_world_from_depth(nearest_uv, nearest);
    return vec4<f32>(color, dof_circle_of_confusion(coc_view_distance(world)));
}
