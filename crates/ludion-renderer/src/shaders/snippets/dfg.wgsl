// Shared visibility term and multi-scatter DFG compensation — composed into mesh, reflections, ssr_blend and the ibl compute.

// Smith height-correlated visibility (the 1 / (4 ndl ndv) denominator is folded in).
fn visibility_smith(ndl: f32, ndv: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let v = ndl * sqrt(ndv * ndv * (1.0 - a2) + a2);
    let l = ndv * sqrt(ndl * ndl * (1.0 - a2) + a2);
    return 0.5 / max(v + l, 1e-5);
}

// Multi-scatter compensation (Fdez-Agüera 2019): a+b is the single-scatter albedo at F0 = 1.
fn dfg_energy(ab: vec2<f32>, f0: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(1.0) + f0 * (1.0 / max(ab.x + ab.y, 1e-4) - 1.0);
}
fn dfg_specular(ab: vec2<f32>, f0: vec3<f32>) -> vec3<f32> {
    return (f0 * ab.x + vec3<f32>(ab.y)) * dfg_energy(ab, f0);
}
