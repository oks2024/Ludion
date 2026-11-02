// The grain and the ordered output dither, applied in the **encoded** domain. Binding-free: the
// caller passes its uniform values and the height of the target the pattern is sized from. Shared by
// the last output pass of the chain — `composite`, `smaa_blend`, or `resolve`/`upscale` under a
// resample — which owns both. Needs `bayer.wgsl` (`BAYER4`) and `srgb.wgsl` ahead of it.

// The grain cell's floor, in pixels of the target the grain is applied at. A cell finer than one pixel
// is not representable, and `grain_size` is validated with an *inclusive* zero floor by choice — so
// this is what makes `grain_size: 0.0` mean "the finest grain there is" instead of a division by zero
// and an undefined float→integer conversion. The `MIN_GAMMA` precedent: defence in depth against a
// *validating* value, not the primary answer.
const MIN_GRAIN_CELL: f32 = 1.0;

// Wellons' `triple32` integer hash — three xorshift-multiply rounds, searched and then exhaustively
// verified bias-free over the whole 32-bit domain.
//
// **Integer arithmetic, deliberately, and never `fract(sin(dot(...)))`.** A transcendental's last
// bits are not guaranteed across backends or driver revisions, and this arc rests on byte-identity
// and on the grain reproducing exactly from one process to the next. Wrapping `u32` multiplies and
// shifts are specified to the bit everywhere.
fn hash32(seed: u32) -> u32 {
    var x = seed;
    x = x ^ (x >> 17u);
    x = x * 0xed5ad4bbu;
    x = x ^ (x >> 11u);
    x = x * 0xac4c1b51u;
    x = x ^ (x >> 15u);
    x = x * 0x31848babu;
    x = x ^ (x >> 14u);
    return x;
}

// Monochrome additive film grain, as a signed offset in the **encoded** domain (see
// `encoded_output`).
//
// `grain_size` is a fraction of the frame's **height**, converted to pixels of the target the grain is
// applied at (`height`) — never authored as a pixel count. That is the screen-relative rule: grain
// authored per *render* pixel would be box-filtered straight back out by a resample, so raising the
// quality setting would change the look. Under a resample the grain is applied after it, at the output
// extent, so the cell is the same fraction of the frame at every scale.
//
// **Uniform amplitude, monochrome, and not luminance-weighted.** Real emulsion grain peaks in the
// midtones and all but vanishes in the toe — but grain is in the roster because it breaks up 8-bit
// night gradients, which is a *deep shadow* problem, so weighting it toward the midtones would remove
// the benefit exactly where it is wanted.
//
// **The pattern is static — no frame index — and that is a decision, not an omission.** `RenderScene`
// carries no frame counter and `extract()` has no access to one, and `ludion render` is single-frame
// regardless. Animating it wants a `frame` on `RenderScene` fed by the live loop (0 for a single-frame
// render), which is deterministic world state and therefore is *not* what decision 5's anti-TAA
// argument rules out. Trigger: the first live build that authors grain and reads as fixed-pattern
// dirt in motion.
//
// The hash's top 23 bits become a value in (0, 1) by an exact power-of-two divide, offset by half a
// step the way the Bayer tile is, so the mapping to [-grain, grain] has a mean of exactly zero and
// grain adds no net brightness.
fn grain_offset(frag: vec2<f32>, grain: f32, grain_size: f32, height: f32) -> f32 {
    let cell_px = max(grain_size * height, MIN_GRAIN_CELL);
    let cell = vec2<u32>(floor(frag / cell_px));
    let noise = f32(hash32(cell.x ^ hash32(cell.y)) >> 9u) + 0.5;
    return (noise * (1.0 / 4194304.0) - 1.0) * grain;
}

// The grain and the ordered output dither, applied in the **encoded** domain, through one shared
// round trip.
//
// The target is `Rgba8UnormSrgb`, so one step of the stored integer is 1/255 of the *sRGB-encoded*
// value, not of the linear one. Perturbing the linear output instead would be worth wildly different
// numbers of output steps at different luminances — most wrong in the deep shadows, which is exactly
// where an 8-bit night-sky gradient bands and is the defect both of these exist to break up. So the
// offsets are applied between an explicit encode and decode, and the hardware re-encodes what we hand
// back. The round trip costs well under an LSB in f32 and needs no format change, which is what keeps
// the offset-free path byte-identical.
//
// One trip carries both because the sum of two offsets is one offset: a second encode/decode would
// quantize nothing extra and would cost a second `pow` pair. The order they are summed in is not
// observable — addition commutes — but the chain order they stand for is (see `composite.wgsl`'s
// header): grain is the emulsion recording what the lens delivered, so it sits behind the vignette,
// and the dither is strictly last because it hides the *output* quantization and anything after it
// would re-quantize what it just spread.
//
// With grain off this is exactly the dither's own arithmetic: `offset` starts at an exact zero and
// `0.0 + x` is exactly `x` in IEEE.
fn encoded_output(
    color: vec3<f32>,
    frag: vec2<f32>,
    grain: f32,
    grain_size: f32,
    dither: u32,
    height: f32,
) -> vec3<f32> {
    var offset = 0.0;
    if (grain != 0.0) {
        offset = offset + grain_offset(frag, grain, grain_size, height);
    }
    if (dither != 0u) {
        let bx = u32(frag.x) % 4u;
        let by = u32(frag.y) % 4u;
        // `BAYER4` holds {0, …, 15}/16; adding half a step (1/32) centres the tile on 0.5, so the
        // offsets are symmetric about zero and the dither adds no net brightness.
        offset = offset + (BAYER4[by * 4u + bx] + 1.0 / 32.0 - 0.5) / 255.0;
    }
    return srgb_to_linear(linear_to_srgb(color) + offset);
}
