// Sprite/billboard pass: instanced textured quads, premultiplied, into the HDR target.
//
// Each instance carries a world center plus FULL width/height `right`/`up` vectors — the unit quad's
// corners span [-0.5, 0.5], so the drawn quad is exactly |right| x |up| world units. Billboarding is
// already resolved on the CPU (for a billboard those vectors lie along the camera axes), so this
// shader just expands the quad. The 6 corners are generated from the vertex index — no vertex buffer.
//
// Every sampled texture is PREMULTIPLIED (the engine converts a loose PNG at load, an atlas is baked
// that way), and the blend is `One, OneMinusSrcAlpha`, so the tint weights the texel by its own alpha
// rather than the blend doing it. `flags` bit 0 selects lighting per instance — one pipeline, so a
// lit and an unlit quad of the same texture stay in one batch.

// The `Camera` uniform (@group 0) and the clustered-forward light set (@group 1: the `LightHeader`
// uniform, `light_list`, `cluster_table`, `cluster_lights`) and `shade_two_sided` are shared snippets
// composed ahead of this file (`shaders/snippets/camera.wgsl`, `snippets/lights.wgsl`,
// `snippets/billboard_light.wgsl`) — see Plan/shader-bindgen.md decision 16. This body declares only
// the sprite-specific texture group.

@group(2) @binding(0) var sprite_tex: texture_2d<f32>;
@group(2) @binding(1) var sprite_sampler: sampler;

// `flags` bit 0: the quad takes light. Bit 1: additive compositing (withhold the output alpha).
// Bit 2: the quad tiles/scrolls its sub-rectangle, so the fragment stage wraps instead of clamping.
const FLAG_LIT: u32 = 1u;
const FLAG_ADDITIVE: u32 = 2u;
const FLAG_WRAP: u32 = 4u;

struct VsIn {
    @location(0) center: vec3<f32>,
    @location(1) right: vec3<f32>,
    @location(2) up: vec3<f32>,
    @location(3) color: vec4<f32>,
    // The sampled sub-rectangle (u0, v0, u1, v1), top-left origin: the whole texture is (0, 0, 1, 1).
    @location(4) uv: vec4<f32>,
    @location(5) flags: u32,
    // Hit-flash (flash_color.rgb, amount): the final colour lerps toward flash_color by amount.
    @location(6) flash: vec4<f32>,
    // Tiling/scroll (repeat.x, repeat.y, offset.x, offset.y), read only under FLAG_WRAP.
    @location(7) uv_tile: vec4<f32>,
    // Sprite.medium: the scale on the medium's optical depth to this quad, read by `fs_depth` only.
    @location(8) medium: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) flags: u32,
    @location(5) @interpolate(flat) flash: vec4<f32>,
    // The clamping path's `uv` is already final; these three carry the pieces the wrapping path
    // needs to redo that mapping per fragment, after `fract` has folded the tile back into [0, 1).
    @location(6) unit_uv: vec2<f32>,
    @location(7) @interpolate(flat) rect: vec4<f32>,
    @location(8) @interpolate(flat) uv_tile: vec4<f32>,
    @location(9) @interpolate(flat) medium: f32,
};

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vi: u32) -> VsOut {
    let corner = CORNERS[vi];
    let world = in.center + corner.x * in.right + corner.y * in.up;
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(world, 1.0);
    out.world_pos = world;
    out.normal = normalize(cross(in.right, in.up));
    // UV: corner.x in [-0.5,0.5] -> [0,1]; flip V so the top of the texture is up. The unit UV then
    // maps into the instance's own sub-rectangle, which is the identity for (0, 0, 1, 1).
    let unit_uv = vec2<f32>(corner.x + 0.5, 0.5 - corner.y);
    out.uv = in.uv.xy + unit_uv * (in.uv.zw - in.uv.xy);
    out.color = in.color;
    out.flags = in.flags;
    out.flash = in.flash;
    out.unit_uv = unit_uv;
    out.rect = in.uv;
    out.uv_tile = in.uv_tile;
    out.medium = in.medium;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Wrapping is `fract` in the fragment stage, not a repeating sampler: the frame is one region of
    // a shared atlas, so only the sub-rectangle may repeat. The clamping path leaves `in.uv` exactly
    // as the vertex stage computed it. The sample itself stays outside the branch.
    var uv = in.uv;
    if ((in.flags & FLAG_WRAP) != 0u) {
        let local = fract(in.unit_uv * in.uv_tile.xy + in.uv_tile.zw);
        uv = in.rect.xy + local * (in.rect.zw - in.rect.xy);
    }
    let texel = textureSample(sprite_tex, sprite_sampler, uv);
    // The texel is premultiplied, so the tint's own alpha scales its colour as well as its coverage.
    var rgb = texel.rgb * in.color.rgb * in.color.a;
    let alpha = texel.a * in.color.a;
    if ((in.flags & FLAG_LIT) != 0u) {
        rgb = rgb * shade_two_sided(in.world_pos, in.normal, in.clip.xy);
    }
    // Hit-flash: lerp toward flash_color in premultiplied space (so the flash weights by the same
    // alpha the colour does). amount 0 mixes nothing — render-identical by construction.
    rgb = mix(rgb, in.flash.rgb * alpha, clamp(in.flash.a, 0.0, 1.0));
    // Additive: withhold the alpha so `(One, OneMinusSrcAlpha)` turns the `over` blend into a plain
    // add, exactly as the particle path does.
    let out_alpha = select(alpha, 0.0, (in.flags & FLAG_ADDITIVE) != 0u);
    return vec4<f32>(rgb, out_alpha);
}

// The transparent layer's twin of `fs_main` (`snippets/transparent_layer.wgsl`): the same UV and
// texel, reduced to the weight the colour pass blends with. An additive texel outputs no alpha, so
// its claim is the brightest channel of the colour it adds, lit and flashed, clamped to 1: a black
// texel leaves the background alone.
@fragment
fn fs_depth(in: VsOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    if ((in.flags & FLAG_WRAP) != 0u) {
        let local = fract(in.unit_uv * in.uv_tile.xy + in.uv_tile.zw);
        uv = in.rect.xy + local * (in.rect.zw - in.rect.xy);
    }
    let texel = textureSample(sprite_tex, sprite_sampler, uv);
    let alpha = texel.a * in.color.a;
    var weight = alpha;
    if ((in.flags & FLAG_ADDITIVE) != 0u) {
        // The colour `fs_main` adds: lit, then flashed.
        var rgb = texel.rgb * in.color.rgb * in.color.a;
        if ((in.flags & FLAG_LIT) != 0u) {
            rgb = rgb * shade_two_sided(in.world_pos, in.normal, in.clip.xy);
        }
        rgb = mix(rgb, in.flash.rgb * alpha, clamp(in.flash.a, 0.0, 1.0));
        weight = alpha * min(1.0, max(rgb.r, max(rgb.g, rgb.b)));
    }
    return transparent_layer(in.world_pos, weight, in.medium);
}
