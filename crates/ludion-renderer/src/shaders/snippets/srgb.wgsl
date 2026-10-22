// The sRGB transfer function pair the hardware applies on store, written out so the dither can work
// in the encoded domain. Values outside [0, 1] ride the same branches (the `none` operator can hand
// this an HDR value); the negative side takes the linear segment, which is odd-symmetric.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c < vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c < vec3<f32>(0.04045));
}
