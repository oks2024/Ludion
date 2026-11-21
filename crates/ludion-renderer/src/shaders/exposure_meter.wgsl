// The exposure meter (`Plan/post-film-and-screen-effects.md` decisions 16-17): one workgroup reads a
// fixed 128x72 grid of the finished HDR `scene` (resolution-independent: 9216 samples, 36 per
// thread), bins log2 luminance into 64 bins over [LOG2_MIN, LOG2_MAX), and one thread then trims the
// ends, takes the mean and writes the 65x1 table `composite` reads: texel b < 64 = (weight, summed
// log2, 0, 0); texel 64 = (correction EV, mean log2, total weight, gain).
//
// Determinism: the histogram is built with integer atomics (order-independent) and the only float
// arithmetic runs on one thread in fixed bin order, so one adapter gives the same bits every run.
// Overflow bound: 9216 samples * weight 16 * |round(log2 * Q)| 4096 < 2^31.

struct MeterParams {
    // x = low trim, y = high trim: the shares of the total weight dropped from each end.
    clip: vec2<f32>,
    // x = min, y = max of the correction, in EV.
    range: vec2<f32>,
    // 1 = centre weighting, 0 = average.
    center: u32,
    // The frame's viewmodel depth slice (0 without a viewmodel): a pixel below it weighs nothing.
    viewmodel_slice: f32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var meter_scene: texture_2d<f32>;
@group(0) @binding(1) var meter_depth: texture_depth_2d;
@group(0) @binding(2) var meter_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(3) var<uniform> meter: MeterParams;

const GRID: vec2<u32> = vec2<u32>(128u, 72u);
const SAMPLES: u32 = 9216u;
const THREADS: u32 = 256u;
const BINS: u32 = 64u;
const LOG2_MIN: f32 = -16.0;
const LOG2_MAX: f32 = 8.0;
// Fixed-point scale of the per-bin log2 sums: the trimmed mean is exact to 1/(2 * Q) EV.
const Q: f32 = 256.0;
const MID_GREY: f32 = 0.18;

var<workgroup> bin_weight: array<atomic<u32>, 64>;
var<workgroup> bin_sum_q: array<atomic<i32>, 64>;

@compute @workgroup_size(256)
fn meter_main(@builtin(local_invocation_index) tid: u32) {
    let dims = textureDimensions(meter_scene);
    for (var k = 0u; k < SAMPLES / THREADS; k = k + 1u) {
        let s = tid + THREADS * k;
        let cell = vec2<u32>(s % GRID.x, s / GRID.x);
        let pixel = min(
            vec2<u32>(floor((vec2<f32>(cell) + 0.5) * vec2<f32>(dims) / vec2<f32>(GRID))),
            dims - vec2<u32>(1u),
        );
        let depth = textureLoad(meter_depth, vec2<i32>(pixel), 0);
        if (meter.viewmodel_slice > 0.0 && depth < meter.viewmodel_slice) {
            continue;
        }
        var w = select(4u, 1u, depth >= 1.0);
        if (meter.center != 0u) {
            let cell_uv = (vec2<f32>(cell) + 0.5) / vec2<f32>(GRID);
            let r = length(cell_uv * 2.0 - 1.0) / sqrt(2.0);
            w = w * (4u - min(3u, u32(r * 4.0)));
        }
        let rgb = textureLoad(meter_scene, vec2<i32>(pixel), 0).rgb;
        var lum = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        if (!(lum >= 0.0)) {
            lum = 0.0;
        }
        let l2 = clamp(log2(max(lum, 1e-9)), LOG2_MIN, LOG2_MAX - 1e-4);
        let bin = min(u32((l2 - LOG2_MIN) / (LOG2_MAX - LOG2_MIN) * f32(BINS)), BINS - 1u);
        atomicAdd(&bin_weight[bin], w);
        atomicAdd(&bin_sum_q[bin], i32(w) * i32(round(l2 * Q)));
    }
    workgroupBarrier();
    if (tid != 0u) {
        return;
    }

    var total = 0u;
    for (var b = 0u; b < BINS; b = b + 1u) {
        total = total + atomicLoad(&bin_weight[b]);
    }
    let lo = f32(total) * meter.clip.x;
    let hi = f32(total) * (1.0 - meter.clip.y);
    var cumulative = 0.0;
    var acc = 0.0;
    var n = 0.0;
    for (var b = 0u; b < BINS; b = b + 1u) {
        let w = f32(atomicLoad(&bin_weight[b]));
        let sum_q = f32(atomicLoad(&bin_sum_q[b]));
        if (w > 0.0) {
            let included = clamp(min(cumulative + w, hi) - max(cumulative, lo), 0.0, w);
            acc = acc + included * (sum_q / w) / Q;
            n = n + included;
        }
        cumulative = cumulative + w;
        textureStore(meter_out, vec2<i32>(i32(b), 0), vec4<f32>(w, sum_q / Q, 0.0, 0.0));
    }
    var mean = 0.0;
    var correction = 0.0;
    if (n > 0.0) {
        mean = acc / n;
        correction = clamp(log2(MID_GREY) - mean, meter.range.x, meter.range.y);
    }
    textureStore(
        meter_out,
        vec2<i32>(64, 0),
        vec4<f32>(correction, mean, f32(total), exp2(correction)),
    );
}
