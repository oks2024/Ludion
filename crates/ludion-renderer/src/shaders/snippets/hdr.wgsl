// Scene radiance ceiling: below half-float's 65504 so a store never rounds to inf.
const HDR_CEILING: f32 = 60000.0;

// Clamps into [0, HDR_CEILING] and reads NaN as 0. Bit test: WGSL has no isnan, and a fast-math
// backend may fold `x != x` away. Exact for any finite input in range, so normal content renders
// byte-identically.
fn hdr_safe(c: vec3<f32>) -> vec3<f32> {
    let bits = bitcast<vec3<u32>>(c);
    let nan = ((bits & vec3<u32>(0x7f800000u)) == vec3<u32>(0x7f800000u))
        & ((bits & vec3<u32>(0x007fffffu)) != vec3<u32>(0u));
    return clamp(select(c, vec3<f32>(0.0), nan), vec3<f32>(0.0), vec3<f32>(HDR_CEILING));
}
