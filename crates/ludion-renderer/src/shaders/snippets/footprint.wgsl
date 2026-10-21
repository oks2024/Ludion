// The footprint rule shared by the SSR pyramid passes (and the shape of `resolve.wgsl`'s loop).
//
// Target texel `i` on an axis owns the source interval `[i*r, (i+1)*r)` for `r = src / dst`. The
// source texels it touches are `floor(lo) ..= ceil(hi) - 1`; callers clamp them to the source. At an
// odd parent size a floor-halving would drop the last row or column — this rule keeps it, so a
// reduce never loses a texel. Binding-free.

fn footprint_ratio(src: vec2<u32>, dst: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(src) / dst;
}

// The interval's lower edge in source-texel coordinates.
fn footprint_lo(target_texel: vec2<f32>, r: vec2<f32>) -> vec2<f32> {
    return target_texel * r;
}

fn footprint_first(lo: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(floor(lo));
}

fn footprint_last(lo: vec2<f32>, r: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(ceil(lo + r)) - vec2<i32>(1, 1);
}

// How much of source texel `t` the interval `[lo, lo + r)` covers.
fn footprint_weight(lo: vec2<f32>, r: vec2<f32>, t: vec2<i32>) -> f32 {
    let hi = lo + r;
    let wx = min(hi.x, f32(t.x + 1)) - max(lo.x, f32(t.x));
    let wy = min(hi.y, f32(t.y + 1)) - max(lo.y, f32(t.y));
    return wx * wy;
}
