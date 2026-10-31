// SMAA 1x, pass 2: blending weight calculation (HIGH preset: 16 orthogonal and 8 diagonal search
// steps, corner rounding 25, diagonal detection and corner detection on). A port of
// `SMAABlendingWeightCalculationPS` and its helpers from SMAA (<https://github.com/iryoku/smaa>),
// MIT-licensed; the notice is in `THIRD-PARTY-NOTICES.md` and `licenses/SMAA-MIT.txt`.
//
// Copyright (C) 2013 Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro, Diego
// Gutierrez.
//
// The reference coordinate math is unchanged (its tables are top-left-origin, like wgpu's). Three
// departures, none of them numeric:
//   - every fetch is `textureSampleLevel` / `textureLoad`: the searches loop and branch per pixel, and
//     `textureSample` is not allowed in non-uniform control flow;
//   - the reference's `SampleLevelZeroOffset` is a coordinate add of the offset in texels (WGSL needs
//     a constant offset, and these are not constants once they pass through a helper);
//   - the search table is point-sampled (`textureLoad`) where the reference uses its linear sampler
//     at texel centres, which the scale and bias below land on.
//
// `VsOut` + `vs_main` come from the shared `shaders/snippets/fullscreen_vs.wgsl`.

struct SmaaWeightsParams {
    // (1 / width, 1 / height, width, height) of the render extent.
    rt_metrics: vec4<f32>,
};

@group(0) @binding(0) var smaa_weights_edges: texture_2d<f32>;
@group(0) @binding(1) var smaa_area_tex: texture_2d<f32>;
@group(0) @binding(2) var smaa_search_tex: texture_2d<f32>;
@group(0) @binding(3) var smaa_weights_samp: sampler;
@group(0) @binding(4) var<uniform> smaa_weights_params: SmaaWeightsParams;

const SMAA_MAX_SEARCH_STEPS: u32 = 16u;
const SMAA_MAX_SEARCH_STEPS_DIAG: u32 = 8u;
const SMAA_CORNER_ROUNDING_NORM: f32 = 0.25;

// The lookup tables' geometry; `smaa_lut.rs` generates the tables and a test ties these to its own.
const SMAA_AREATEX_MAX_DISTANCE: u32 = 16u;
const SMAA_AREATEX_MAX_DISTANCE_DIAG: u32 = 20u;
const SMAA_AREATEX_WIDTH: u32 = 160u;
const SMAA_AREATEX_HEIGHT: u32 = 560u;
// The area table stacks 7 subsample offsets; one block's height is a seventh of the table.
const SMAA_AREATEX_SUBTEX_COUNT: u32 = 7u;
const SMAA_SEARCHTEX_WIDTH: u32 = 64u;
const SMAA_SEARCHTEX_HEIGHT: u32 = 16u;

const AREATEX_SIZE: vec2<f32> = vec2<f32>(160.0, 560.0);
const AREATEX_SUBTEX_SIZE: f32 = 1.0 / 7.0;
const SEARCHTEX_SIZE: vec2<f32> = vec2<f32>(66.0, 33.0);
const SEARCHTEX_PACKED_SIZE: vec2<f32> = vec2<f32>(64.0, 16.0);

fn rt() -> vec4<f32> {
    return smaa_weights_params.rt_metrics;
}

fn edges_at(uv: vec2<f32>) -> vec2<f32> {
    return textureSampleLevel(smaa_weights_edges, smaa_weights_samp, uv, 0.0).rg;
}

// `SMAASampleLevelZeroOffset`: the edges at `uv` shifted by whole texels.
fn edges_off(uv: vec2<f32>, texels: vec2<i32>) -> vec2<f32> {
    return edges_at(uv + vec2<f32>(texels) * rt().xy);
}

// Decodes two binary values from a bilinear-filtered access (the 0.25 offset puts a quarter of the
// neighbour in each fetch).
fn decode_diag_bilinear2(e_in: vec2<f32>) -> vec2<f32> {
    var e = e_in;
    e.x = e.x * abs(5.0 * e.x - 5.0 * 0.75);
    return round(e);
}

fn decode_diag_bilinear4(e_in: vec4<f32>) -> vec4<f32> {
    var e = e_in;
    e.x = e.x * abs(5.0 * e.x - 5.0 * 0.75);
    e.z = e.z * abs(5.0 * e.z - 5.0 * 0.75);
    return round(e);
}

// The diagonal searches return (steps, last coverage, last edge r, last edge g).
fn search_diag1(uv: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    var coord = vec4<f32>(uv, -1.0, 1.0);
    let t = vec3<f32>(rt().xy, 1.0);
    var e = vec2<f32>(0.0, 0.0);
    while (coord.z < f32(SMAA_MAX_SEARCH_STEPS_DIAG - 1u) && coord.w > 0.9) {
        let next = t * vec3<f32>(dir, 1.0) + coord.xyz;
        coord = vec4<f32>(next, coord.w);
        e = edges_at(coord.xy);
        coord.w = dot(e, vec2<f32>(0.5, 0.5));
    }
    return vec4<f32>(coord.z, coord.w, e);
}

fn search_diag2(uv: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    var coord = vec4<f32>(uv, -1.0, 1.0);
    coord.x = coord.x + 0.25 * rt().x;
    let t = vec3<f32>(rt().xy, 1.0);
    var e = vec2<f32>(0.0, 0.0);
    while (coord.z < f32(SMAA_MAX_SEARCH_STEPS_DIAG - 1u) && coord.w > 0.9) {
        let next = t * vec3<f32>(dir, 1.0) + coord.xyz;
        coord = vec4<f32>(next, coord.w);
        // Both edges in one bilinear fetch, then unpacked.
        e = decode_diag_bilinear2(edges_at(coord.xy));
        coord.w = dot(e, vec2<f32>(0.5, 0.5));
    }
    return vec4<f32>(coord.z, coord.w, e);
}

// The area for a diagonal distance and crossing edges `e`, on the second half of the table.
fn area_diag(dist: vec2<f32>, e: vec2<f32>, offset: f32) -> vec2<f32> {
    var uv = f32(SMAA_AREATEX_MAX_DISTANCE_DIAG) * e + dist;
    uv = uv / AREATEX_SIZE + 0.5 / AREATEX_SIZE;
    uv.x = uv.x + 0.5;
    uv.y = uv.y + AREATEX_SUBTEX_SIZE * offset;
    return textureSampleLevel(smaa_area_tex, smaa_weights_samp, uv, 0.0).rg;
}

// Searches for diagonal patterns and returns their weights.
fn calculate_diag_weights(uv: vec2<f32>, e: vec2<f32>) -> vec2<f32> {
    var weights = vec2<f32>(0.0, 0.0);
    var d = vec4<f32>(0.0);

    if (e.r > 0.0) {
        let s = search_diag1(uv, vec2<f32>(-1.0, 1.0));
        d.x = s.x + f32(s.w > 0.9);
        d.z = s.y;
    } else {
        d.x = 0.0;
        d.z = 0.0;
    }
    let s1 = search_diag1(uv, vec2<f32>(1.0, -1.0));
    d.y = s1.x;
    d.w = s1.y;

    if (d.x + d.y > 2.0) {
        let coords = vec4<f32>(-d.x + 0.25, d.x, d.y, -d.y - 0.25) * rt().xyxy + uv.xyxy;
        let c0 = edges_off(coords.xy, vec2<i32>(-1, 0));
        let c1 = edges_off(coords.zw, vec2<i32>(1, 0));
        let decoded = decode_diag_bilinear4(vec4<f32>(c0, c1));
        // The reference swizzles the decoded value back as c.yxwz.
        let c = vec4<f32>(decoded.y, decoded.x, decoded.w, decoded.z);
        var cc = vec2<f32>(2.0, 2.0) * c.xz + c.yw;
        cc = select(cc, vec2<f32>(0.0, 0.0), d.zw >= vec2<f32>(0.9, 0.9));
        weights = weights + area_diag(d.xy, cc, 0.0);
    }

    let s2 = search_diag2(uv, vec2<f32>(-1.0, -1.0));
    d.x = s2.x;
    d.z = s2.y;
    if (edges_off(uv, vec2<i32>(1, 0)).r > 0.0) {
        let s3 = search_diag2(uv, vec2<f32>(1.0, 1.0));
        d.y = s3.x + f32(s3.w > 0.9);
        d.w = s3.y;
    } else {
        d.y = 0.0;
        d.w = 0.0;
    }

    if (d.x + d.y > 2.0) {
        let coords = vec4<f32>(-d.x, -d.x, d.y, d.y) * rt().xyxy + uv.xyxy;
        var c: vec4<f32>;
        c.x = edges_off(coords.xy, vec2<i32>(-1, 0)).g;
        c.y = edges_off(coords.xy, vec2<i32>(0, -1)).r;
        let czw = edges_off(coords.zw, vec2<i32>(1, 0));
        c.z = czw.g;
        c.w = czw.r;
        var cc = vec2<f32>(2.0, 2.0) * c.xz + c.yw;
        cc = select(cc, vec2<f32>(0.0, 0.0), d.zw >= vec2<f32>(0.9, 0.9));
        weights = weights + area_diag(d.xy, cc, 0.0).yx;
    }
    return weights;
}

// How much length to add in the last search step, from the bilinearly interpolated edges (0, 1 or
// 2, by which edges and crossing edges are active).
fn search_length(e: vec2<f32>, offset: f32) -> f32 {
    var scale = SEARCHTEX_SIZE * vec2<f32>(0.5, -1.0);
    var bias = SEARCHTEX_SIZE * vec2<f32>(offset, 1.0);
    scale = scale + vec2<f32>(-1.0, 1.0);
    bias = bias + vec2<f32>(0.5, -0.5);
    scale = scale * (1.0 / SEARCHTEX_PACKED_SIZE);
    bias = bias * (1.0 / SEARCHTEX_PACKED_SIZE);
    let uv = scale * e + bias;
    let texel = clamp(
        vec2<i32>(floor(uv * SEARCHTEX_PACKED_SIZE)),
        vec2<i32>(0, 0),
        vec2<i32>(i32(SMAA_SEARCHTEX_WIDTH) - 1, i32(SMAA_SEARCHTEX_HEIGHT) - 1),
    );
    return textureLoad(smaa_search_tex, texel, 0).r;
}

// The orthogonal searches. Their start coordinate is offset by (-0.25, -0.125) in `fs_main`, so one
// bilinear fetch reads four edges in a row and the different offsets per direction tell which of
// them are active.
fn search_x_left(uv_in: vec2<f32>, end: f32) -> f32 {
    var uv = uv_in;
    var e = vec2<f32>(0.0, 1.0);
    while (uv.x > end && e.g > 0.8281 && e.r == 0.0) {
        e = edges_at(uv);
        uv = uv - vec2<f32>(2.0, 0.0) * rt().xy;
    }
    let offset = -(255.0 / 127.0) * search_length(e, 0.0) + 3.25;
    return rt().x * offset + uv.x;
}

fn search_x_right(uv_in: vec2<f32>, end: f32) -> f32 {
    var uv = uv_in;
    var e = vec2<f32>(0.0, 1.0);
    while (uv.x < end && e.g > 0.8281 && e.r == 0.0) {
        e = edges_at(uv);
        uv = uv + vec2<f32>(2.0, 0.0) * rt().xy;
    }
    let offset = -(255.0 / 127.0) * search_length(e, 0.5) + 3.25;
    return -rt().x * offset + uv.x;
}

fn search_y_up(uv_in: vec2<f32>, end: f32) -> f32 {
    var uv = uv_in;
    var e = vec2<f32>(1.0, 0.0);
    while (uv.y > end && e.r > 0.8281 && e.g == 0.0) {
        e = edges_at(uv);
        uv = uv - vec2<f32>(0.0, 2.0) * rt().xy;
    }
    let offset = -(255.0 / 127.0) * search_length(e.gr, 0.0) + 3.25;
    return rt().y * offset + uv.y;
}

fn search_y_down(uv_in: vec2<f32>, end: f32) -> f32 {
    var uv = uv_in;
    var e = vec2<f32>(1.0, 0.0);
    while (uv.y < end && e.r > 0.8281 && e.g == 0.0) {
        e = edges_at(uv);
        uv = uv + vec2<f32>(0.0, 2.0) * rt().xy;
    }
    let offset = -(255.0 / 127.0) * search_length(e.gr, 0.5) + 3.25;
    return -rt().y * offset + uv.y;
}

// The areas on each side of the current edge, from the distances and both crossing edges.
fn area(dist: vec2<f32>, e1: f32, e2: f32, offset: f32) -> vec2<f32> {
    // Rounding prevents precision errors of bilinear filtering.
    var uv = f32(SMAA_AREATEX_MAX_DISTANCE) * round(4.0 * vec2<f32>(e1, e2)) + dist;
    uv = uv / AREATEX_SIZE + 0.5 / AREATEX_SIZE;
    uv.y = AREATEX_SUBTEX_SIZE * offset + uv.y;
    return textureSampleLevel(smaa_area_tex, smaa_weights_samp, uv, 0.0).rg;
}

// Corner detection: the factor that softens a weight where the line turns a corner.
fn horizontal_corner_factor(uv: vec4<f32>, d: vec2<f32>) -> vec2<f32> {
    let left_right = step(d.xy, d.yx);
    var rounding = (1.0 - SMAA_CORNER_ROUNDING_NORM) * left_right;
    rounding = rounding / (left_right.x + left_right.y);
    var factor = vec2<f32>(1.0, 1.0);
    factor.x = factor.x - rounding.x * edges_off(uv.xy, vec2<i32>(0, 1)).r;
    factor.x = factor.x - rounding.y * edges_off(uv.zw, vec2<i32>(1, 1)).r;
    factor.y = factor.y - rounding.x * edges_off(uv.xy, vec2<i32>(0, -2)).r;
    factor.y = factor.y - rounding.y * edges_off(uv.zw, vec2<i32>(1, -2)).r;
    return clamp(factor, vec2<f32>(0.0), vec2<f32>(1.0));
}

fn vertical_corner_factor(uv: vec4<f32>, d: vec2<f32>) -> vec2<f32> {
    let left_right = step(d.xy, d.yx);
    var rounding = (1.0 - SMAA_CORNER_ROUNDING_NORM) * left_right;
    rounding = rounding / (left_right.x + left_right.y);
    var factor = vec2<f32>(1.0, 1.0);
    factor.x = factor.x - rounding.x * edges_off(uv.xy, vec2<i32>(1, 0)).g;
    factor.x = factor.x - rounding.y * edges_off(uv.zw, vec2<i32>(1, 1)).g;
    factor.y = factor.y - rounding.x * edges_off(uv.xy, vec2<i32>(-2, 0)).g;
    factor.y = factor.y - rounding.y * edges_off(uv.zw, vec2<i32>(-2, 1)).g;
    return clamp(factor, vec2<f32>(0.0), vec2<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Pixel centres are exact here; the reference interpolates them from the vertex stage.
    let pixcoord = in.pos.xy;
    let uv = pixcoord * rt().xy;

    // The search start offsets and ends (the reference's vertex stage).
    let offset0 = rt().xyxy * vec4<f32>(-0.25, -0.125, 1.25, -0.125) + uv.xyxy;
    let offset1 = rt().xyxy * vec4<f32>(-0.125, -0.25, -0.125, 1.25) + uv.xyxy;
    let steps = f32(SMAA_MAX_SEARCH_STEPS);
    let offset2 = rt().xxyy * (vec4<f32>(-2.0, 2.0, -2.0, 2.0) * steps)
        + vec4<f32>(offset0.x, offset0.z, offset1.y, offset1.w);

    var weights = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var e = edges_at(uv);

    if (e.g > 0.0) { // Edge at north
        // Diagonals have both north and west edges, so searching one boundary is enough. They take
        // priority: finding one skips the horizontal and vertical processing.
        let diag = calculate_diag_weights(uv, e);
        weights.r = diag.x;
        weights.g = diag.y;
        if (weights.r == -weights.g) {
            var d: vec2<f32>;
            var coords: vec3<f32>;
            coords.x = search_x_left(offset0.xy, offset2.x);
            coords.y = offset1.y; // texcoord.y - 0.25 * rt.y
            d.x = coords.x;
            let e1 = edges_at(coords.xy).r;

            coords.z = search_x_right(offset0.zw, offset2.y);
            d.y = coords.z;

            d = abs(round(rt().zz * d - pixcoord.xx));
            // The area table is compressed quadratically.
            let sqrt_d = sqrt(d);
            let e2 = edges_off(coords.zy, vec2<i32>(1, 0)).r;

            let a = area(sqrt_d, e1, e2, 0.0);
            coords.y = uv.y;
            let factor = horizontal_corner_factor(coords.xyzy, d);
            weights.r = a.x * factor.x;
            weights.g = a.y * factor.y;
        } else {
            e.r = 0.0; // Skip vertical processing.
        }
    }

    if (e.r > 0.0) { // Edge at west
        var d: vec2<f32>;
        var coords: vec3<f32>;
        coords.y = search_y_up(offset1.xy, offset2.z);
        coords.x = offset0.x; // texcoord.x - 0.25 * rt.x
        d.x = coords.y;
        let e1 = edges_at(coords.xy).g;

        coords.z = search_y_down(offset1.zw, offset2.w);
        d.y = coords.z;

        d = abs(round(rt().ww * d - pixcoord.yy));
        let sqrt_d = sqrt(d);
        let e2 = edges_off(coords.xz, vec2<i32>(0, 1)).g;

        let a = area(sqrt_d, e1, e2, 0.0);
        coords.x = uv.x;
        let factor = vertical_corner_factor(coords.xyxz, d);
        weights.b = a.x * factor.x;
        weights.a = a.y * factor.y;
    }
    return weights;
}
