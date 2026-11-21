// The exposure meter's debug view (`ludion render --debug-view exposure_meter`), drawn in place of
// `composite`: the metered frame dimmed over the top three quarters, and the meter's 64-bin
// histogram over the bottom quarter. Bars are white inside the trimmed window and grey where
// `metering_clip` cut them; a green line marks the trimmed mean, a yellow one the mid-grey target,
// and a translucent blue band the means the correction does not clamp.
//
// `VsOut` + `vs_main` come from `snippets/fullscreen_vs.wgsl`. Textures sit at 0..1, the sampler
// at 2 and the uniform at 3 (see `FrameRecorder::fullscreen_effect`).

struct ExposureMeterDebug {
    // The composite's CPU-side exposure (`exposure` and its compensation); the table's gain is
    // applied here, as the composite applies it.
    exposure: f32,
    // The table's bin range in log2 luminance, `[log2_min, log2_max)`.
    log2_min: f32,
    log2_max: f32,
    // log2 of the mid-grey the correction aims for.
    target_log2: f32,
    // x = min, y = max of the correction, in EV.
    range: vec2<f32>,
    // x = low trim, y = high trim: the shares of the total weight dropped from each end.
    clip: vec2<f32>,
}

@group(0) @binding(0) var emd_scene: texture_2d<f32>;
// The 65x1 table, read with `textureLoad`: texels 0..63 = (weight, summed log2, 0, 0) per bin,
// texel 64 = (correction, mean log2, total weight, gain).
@group(0) @binding(1) var emd_table: texture_2d<f32>;
@group(0) @binding(2) var emd_samp: sampler;
@group(0) @binding(3) var<uniform> emd: ExposureMeterDebug;

const BINS: u32 = 64u;
const BACKDROP: f32 = 0.05;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let verdict = textureLoad(emd_table, vec2<i32>(64, 0), 0);
    if (in.uv.y < 0.75) {
        let c = textureSampleLevel(emd_scene, emd_samp, in.uv, 0.0).rgb * emd.exposure * verdict.w;
        return vec4<f32>(c / (1.0 + c) * 0.5, 1.0);
    }

    // Everything below draws in a panel: x across the log2 range, bar height up from the bottom.
    let span = emd.log2_max - emd.log2_min;
    let px = in.pos.x;
    let width = f32(textureDimensions(emd_scene).x);
    let log2_here = emd.log2_min + in.uv.x * span;
    let height_here = 1.0 - (in.uv.y - 0.75) * 4.0;
    let line = 2.0 * span / width;

    if (abs(log2_here - verdict.y) < line) {
        return vec4<f32>(0.0, 1.0, 0.0, 1.0);
    }
    if (abs(log2_here - emd.target_log2) < line) {
        return vec4<f32>(1.0, 1.0, 0.0, 1.0);
    }

    // The tallest bin, the total weight below this bin, and the total: a debug view, so the 64
    // texels are read per pixel.
    let bin = min(i32(in.uv.x * f32(BINS)), i32(BINS) - 1);
    var tallest = 0.0;
    var below = 0.0;
    for (var b = 0; b < i32(BINS); b = b + 1) {
        let w = textureLoad(emd_table, vec2<i32>(b, 0), 0).x;
        tallest = max(tallest, w);
        if (b < bin) {
            below = below + w;
        }
    }
    let weight = textureLoad(emd_table, vec2<i32>(bin, 0), 0).x;
    var color = vec3<f32>(BACKDROP);
    if (log2_here >= emd.target_log2 - emd.range.y && log2_here <= emd.target_log2 - emd.range.x) {
        color = mix(color, vec3<f32>(0.1, 0.25, 0.9), 0.35);
    }
    if (tallest > 0.0 && height_here <= weight / tallest) {
        // The bin is trimmed when the weight below it is inside the low cut or its top passes the
        // high cut, as the meter's `included` span decides.
        let total = verdict.z;
        let kept = min(below + weight, (1.0 - emd.clip.y) * total) - max(below, emd.clip.x * total);
        color = select(vec3<f32>(0.45), vec3<f32>(1.0), kept > 0.0);
    }
    return vec4<f32>(color, 1.0);
}
