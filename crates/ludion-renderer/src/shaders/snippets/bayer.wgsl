// 4×4 ordered (Bayer) dither matrix, normalized to thresholds in {0, 1, …, 15}/16, so the maximum
// entry is 15/16 = 0.9375. A 4×4 tile is chosen over 8×8: 16 gradation levels are ample for both of
// its consumers, and the compact tile reads as fine stipple rather than a large visible grid.
//
// Shared by the mesh pass's screen-door occlusion fade (a per-fragment discard), the output dither
// (`output_encode.wgsl`, a sub-LSB offset in the encoded domain, applied by the last output pass), the
// SSAO estimator's hemisphere rotation and the light-shaft march's step offset. Those are different mechanisms over the same tile, and the
// engine has exactly one ordered pattern rather than one per consumer — a second table would drift and
// would read as two different stipples on the same frame.
//
// The two estimators pick it over an aperiodic hash for the property the others do not need: a 4×4
// period means a 4×4 box blur (`box_blur.wgsl`) averages exactly one full period, so the pattern
// cancels *exactly* rather than leaving residue.
//
// Both index it by the fragment's pixel position (`fragcoord % 4`), which makes each deterministic,
// texture-free and non-temporal: no frame index, so a single-frame `ludion render` is reproducible
// across processes.
const BAYER4: array<f32, 16> = array<f32, 16>(
     0.0 / 16.0,  8.0 / 16.0,  2.0 / 16.0, 10.0 / 16.0,
    12.0 / 16.0,  4.0 / 16.0, 14.0 / 16.0,  6.0 / 16.0,
     3.0 / 16.0, 11.0 / 16.0,  1.0 / 16.0,  9.0 / 16.0,
    15.0 / 16.0,  7.0 / 16.0, 13.0 / 16.0,  5.0 / 16.0,
);
