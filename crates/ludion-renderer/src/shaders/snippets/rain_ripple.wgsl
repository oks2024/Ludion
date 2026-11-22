// The ripple field rain leaves on standing water: a height gradient, a pure function of world xz, game time
// and the seed, so a held clock holds it. CPU twin: `rain.rs::ripple_slope` (same arithmetic; its constants
// and hash are pinned there). Binding-free; the scene pass (`mesh.wgsl`) and the lit decal pass
// (`decal_lit.wgsl`) perturb their shading normal with it, and `rain.wgsl` shares the hash rounds.
//
// World xz is tiled in cells of RIPPLE_CELL metres. Each cell sends one ring per RIPPLE_PERIOD seconds from
// a hashed origin inside it. A ring is `sin(k (d - r)) / k` (`k = 2 pi / RIPPLE_WAVELENGTH`, so the crest
// slope equals its amplitude) under a smooth window of half-width RIPPLE_WINDOW, faded by `(1 - age)^2`.
// A ring reaches under a cell's width, so the 3x3 cells around a point hold every ring that touches it.

const RIPPLE_CELL: f32 = 0.35;
const RIPPLE_PERIOD: f32 = 0.7;
const RIPPLE_RADIUS: f32 = 0.16;
const RIPPLE_WAVELENGTH: f32 = 0.05;
const RIPPLE_WINDOW: f32 = 0.03;

// One round of PCG: a cheap integer hash with good avalanche.
fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

// The top 24 bits as a float in [0, 1).
fn unit_hash(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

// The height gradient at world `xz`.
fn ripple_slope(xz: vec2<f32>, time: f32, seed: u32) -> vec2<f32> {
    let cell = vec2<i32>(floor(xz / RIPPLE_CELL));
    let k = 6.283185307179586 / RIPPLE_WAVELENGTH;
    var sum = vec2<f32>(0.0);
    for (var dj = -1; dj <= 1; dj++) {
        for (var di = -1; di <= 1; di++) {
            let i = cell.x + di;
            let j = cell.y + dj;
            var h = pcg(seed ^ 0x9e3779b9u);
            h = pcg(h ^ bitcast<u32>(i));
            h = pcg(h ^ bitcast<u32>(j));
            let phase = unit_hash(pcg(h + 1u));
            let origin = vec2<f32>(
                (f32(i) + unit_hash(pcg(h + 2u))) * RIPPLE_CELL,
                (f32(j) + unit_hash(pcg(h + 3u))) * RIPPLE_CELL,
            );
            let age = fract(time / RIPPLE_PERIOD + phase);
            let delta = xz - origin;
            let d = length(delta);
            let x = d - age * RIPPLE_RADIUS;
            let u = x / RIPPLE_WINDOW;
            if (abs(u) < 1.0 && d > 1.0e-6) {
                let window = (1.0 - u * u) * (1.0 - u * u);
                let fade = (1.0 - age) * (1.0 - age);
                sum += delta / d * (cos(k * x) * window * fade);
            }
        }
    }
    return sum;
}

// `n` leaned by the ripple slope at `world`, at `weight`; only up-facing surfaces ripple (`geometric` is
// the unperturbed normal).
fn ripple_normal(n: vec3<f32>, geometric: vec3<f32>, world: vec3<f32>, time: f32, seed: u32, weight: f32) -> vec3<f32> {
    let s = ripple_slope(world.xz, time, seed);
    let up = smoothstep(0.7, 0.95, geometric.y);
    return normalize(n + weight * up * vec3<f32>(-s.x, 0.0, -s.y));
}
