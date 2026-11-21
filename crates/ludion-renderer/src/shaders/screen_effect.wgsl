// One `ScreenEffect`: a retro-display filter drawn over a rectangle of the frame, reading the whole-frame
// layer the overlay (or the previous effect) wrote and replacing the pixels inside the rect. Pixels
// outside the rect are never drawn, so they keep the blit's copy of the layer exactly.
//
// The vertex stage emits the rect's quad (six vertices, no vertex buffer) from the uniform; the
// fragment evaluates the stages below in order, each behind a uniform branch on its own zero, so a
// stage the author left at zero costs nothing and cannot move a pixel. Work happens in the **encoded**
// domain (the presets are display-referred), and every stage is integer-hash or polynomial: no
// `sin`/`cos`, so the animation reproduces across backends and processes. Time and the tick arrive in
// the uniform — the renderer keeps no clock of its own. Needs `bayer.wgsl`, `srgb.wgsl` and
// `output_encode.wgsl` (`hash32`) ahead of it.

struct ScreenEffectParams {
    // [x, y, w, h] of the region in output pixels, top-left origin.
    rect: vec4<f32>,
    viewport: vec2<f32>,
    time_s: f32,
    frame: u32,
    seed: u32,
    scanline_count: u32,
    posterize: u32,
    // 0 none, 1 aperture, 2 slot, 3 shadow.
    mask: u32,
    // xyz = the mono tint; w unused.
    phosphor: vec4<f32>,
    scanlines: f32,
    mask_strength: f32,
    curvature: f32,
    glow: f32,
    chroma_bleed: f32,
    noise: f32,
    tracking: f32,
    roll: f32,
    jitter: f32,
    flicker: f32,
    sweep: f32,
    sweep_period: f32,
    interlace: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var screen_fx_src: texture_2d<f32>;
@group(0) @binding(1) var screen_fx_samp: sampler;
@group(0) @binding(2) var<uniform> screen_fx_params: ScreenEffectParams;

const FX_CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let px = screen_fx_params.rect.xy + FX_CORNERS[vi] * screen_fx_params.rect.zw;
    let ndc = px / screen_fx_params.viewport * 2.0 - vec2<f32>(1.0);
    return vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
}

// Two integer inputs to a value in [0, 1): the top 24 bits of the hash over the xor-folded pair.
fn fx_hash(a: u32, b: u32) -> f32 {
    return f32(hash32(a ^ hash32(b ^ 0x9e3779b9u)) >> 8u) / 16777216.0;
}

// The layer at a pixel position, clamped inside the rect, in the encoded domain.
fn fx_tap(p: vec2<f32>) -> vec3<f32> {
    let r = screen_fx_params.rect;
    let q = clamp(p, r.xy + vec2<f32>(0.5), r.xy + r.zw - vec2<f32>(0.5));
    let linear = textureSampleLevel(
        screen_fx_src, screen_fx_samp, q / screen_fx_params.viewport, 0.0).rgb;
    return linear_to_srgb(linear);
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let e = screen_fx_params;
    let p = floor(frag.xy);
    let pu = vec2<u32>(p);
    var uv = (frag.xy - e.rect.xy) / e.rect.zw;
    let line = u32(floor(clamp(uv.y, 0.0, 0.9999) * f32(e.scanline_count)));

    if e.roll != 0.0 {
        uv.y = fract(uv.y + e.roll * e.time_s);
    }
    var inside = true;
    if e.curvature != 0.0 {
        var c = uv * 2.0 - vec2<f32>(1.0);
        c = c * (1.0 + e.curvature * dot(c, c));
        uv = c * 0.5 + vec2<f32>(0.5);
        inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    }
    if e.tracking != 0.0 {
        let band = u32(floor(clamp(uv.y, 0.0, 0.9999) * 6.0));
        uv.x = uv.x + e.tracking * 0.03 * (fx_hash(band ^ e.seed, e.frame / 4u) - 0.5);
    }
    if e.jitter != 0.0 {
        uv.x = uv.x + e.jitter * 0.004 * (fx_hash(line ^ e.seed, e.frame) - 0.5);
    }

    let sp = e.rect.xy + uv * e.rect.zw;
    var c: vec3<f32>;
    if e.chroma_bleed != 0.0 {
        let d = vec2<f32>(e.chroma_bleed * 0.01 * e.rect.w, 0.0);
        c = vec3<f32>(fx_tap(sp - d).r, fx_tap(sp).g, fx_tap(sp + d).b);
    } else {
        c = fx_tap(sp);
    }

    if e.glow != 0.0 {
        var acc = vec3<f32>(0.0);
        acc = acc + fx_tap(sp + vec2<f32>(-2.0, -2.0)) + fx_tap(sp + vec2<f32>(0.0, -2.0));
        acc = acc + fx_tap(sp + vec2<f32>(2.0, -2.0)) + fx_tap(sp + vec2<f32>(-2.0, 0.0));
        acc = acc + fx_tap(sp + vec2<f32>(2.0, 0.0)) + fx_tap(sp + vec2<f32>(-2.0, 2.0));
        acc = acc + fx_tap(sp + vec2<f32>(0.0, 2.0)) + fx_tap(sp + vec2<f32>(2.0, 2.0));
        c = c + e.glow * 0.5 * (acc * 0.125);
    }
    if any(e.phosphor.xyz > vec3<f32>(0.0)) {
        c = vec3<f32>(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722))) * e.phosphor.xyz;
    }
    if e.posterize >= 2u {
        let n = f32(e.posterize) - 1.0;
        c = floor(c * n + vec3<f32>(0.5)) / n;
    }
    if e.scanlines != 0.0 {
        let f = fract(uv.y * f32(e.scanline_count));
        c = c * (1.0 - e.scanlines * (1.0 - 4.0 * f * (1.0 - f)));
    }
    if e.interlace != 0u && ((line + e.frame) & 1u) == 1u {
        c = c * (1.0 - 0.5 * max(e.scanlines, 0.3));
    }
    if e.mask_strength > 0.0 && e.mask != 0u {
        var col = pu.x;
        if e.mask == 2u {
            col = pu.x + ((pu.y / 2u) & 1u);
        } else if e.mask == 3u {
            col = pu.x + (pu.y & 1u) * 2u;
        }
        let stripe = col % 3u;
        for (var i = 0u; i < 3u; i = i + 1u) {
            let gain = select(0.6, 1.0, stripe == i);
            c[i] = c[i] * mix(1.0, gain, e.mask_strength);
        }
    }
    if e.sweep != 0.0 && e.sweep_period > 0.0 {
        let s = fract(e.time_s / e.sweep_period);
        if uv.y <= s {
            let t = clamp(1.0 - (s - uv.y) / 0.08, 0.0, 1.0);
            c = c + vec3<f32>(e.sweep * t * t);
        }
    }
    if e.noise != 0.0 {
        let n = fx_hash(pu.x ^ hash32(pu.y ^ e.seed), e.frame);
        c = c + vec3<f32>(e.noise * (n - 0.5));
    }
    if e.flicker != 0.0 {
        c = c * (1.0 + e.flicker * (2.0 * fx_hash(e.frame, e.seed) - 1.0));
    }

    let out = select(vec3<f32>(0.0), srgb_to_linear(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), inside);
    return vec4<f32>(out, 1.0);
}
