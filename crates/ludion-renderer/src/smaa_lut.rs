//! The SMAA lookup tables, generated at startup instead of committed as byte arrays.
//!
//! This is a port of `Scripts/AreaTex.py` and `Scripts/SearchTex.py` from SMAA
//! (<https://github.com/iryoku/smaa>), MIT-licensed; the notice is in `THIRD-PARTY-NOTICES.md` and
//! `licenses/SMAA-MIT.txt`.
//!
//! Copyright (C) 2013 Jorge Jimenez, Jose I. Echevarria, Belen Masia, Fernando Navarro, Diego
//! Gutierrez.
//!
//! The arithmetic is f64 in the upstream's operation order and truncates to `u8` the way its
//! `int(255.0 * a)` does, so both tables are byte-identical to the upstream `AreaTex.h` and
//! `SearchTex.h`. The tests pin that by hash. Row-major from the top-left, which is D3D's and
//! wgpu's.

type V2 = [f64; 2];

/// The area table's extent: 2 × 5 patterns × 16 wide, 7 subsample offsets × 5 patterns × 16 tall.
pub(crate) const AREA_TEX_SIZE: (u32, u32) = (160, 560);
/// The search table's extent.
pub(crate) const SEARCH_TEX_SIZE: (u32, u32) = (64, 16);

const SUBSAMPLE_OFFSETS_ORTHO: [f64; 7] = [0.0, -0.25, 0.25, -0.125, 0.125, -0.375, 0.375];
const SUBSAMPLE_OFFSETS_DIAG: [(f64, f64); 5] = [
    (0.0, 0.0),
    (0.25, -0.25),
    (-0.25, 0.25),
    (0.125, -0.125),
    (-0.125, 0.125),
];

const SIZE_ORTHO: usize = 16;
const SIZE_DIAG: usize = 20;
/// Samples per axis for the diagonal areas, which are computed by brute force.
const SAMPLES_DIAG: usize = 30;
/// The distance past which small u-patterns are no longer smoothed.
const SMOOTH_MAX_DISTANCE: f64 = 32.0;

const EDGES_ORTHO: [(usize, usize); 16] = [
    (0, 0),
    (3, 0),
    (0, 3),
    (3, 3),
    (1, 0),
    (4, 0),
    (1, 3),
    (4, 3),
    (0, 1),
    (3, 1),
    (0, 4),
    (3, 4),
    (1, 1),
    (4, 1),
    (1, 4),
    (4, 4),
];

const EDGES_DIAG: [(usize, usize); 16] = [
    (0, 0),
    (1, 0),
    (0, 2),
    (1, 2),
    (2, 0),
    (3, 0),
    (2, 2),
    (3, 2),
    (0, 1),
    (1, 1),
    (0, 3),
    (1, 3),
    (2, 1),
    (3, 1),
    (2, 3),
    (3, 3),
];

fn add(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}

fn half(a: V2) -> V2 {
    [a[0] / 2.0, a[1] / 2.0]
}

fn lerp(a: V2, b: V2, p: f64) -> V2 {
    [a[0] + (b[0] - a[0]) * p, a[1] + (b[1] - a[1]) * p]
}

fn saturate(a: f64) -> f64 {
    a.clamp(0.0, 1.0)
}

/// Smoothing for small u-patterns.
fn smootharea(d: f64, a1: V2, a2: V2) -> (V2, V2) {
    let b1 = [(a1[0] * 2.0).sqrt() * 0.5, (a1[1] * 2.0).sqrt() * 0.5];
    let b2 = [(a2[0] * 2.0).sqrt() * 0.5, (a2[1] * 2.0).sqrt() * 0.5];
    let p = saturate(d / SMOOTH_MAX_DISTANCE);
    (lerp(b1, a1, p), lerp(b2, a2, p))
}

/// The area under the line `p1`→`p2` over the pixel `x..x+1`, as (below, above).
fn ortho_area(p1: V2, p2: V2, x: f64) -> V2 {
    let d = [p2[0] - p1[0], p2[1] - p1[1]];
    let x1 = x;
    let x2 = x + 1.0;
    let y1 = p1[1] + d[1] * (x1 - p1[0]) / d[0];
    let y2 = p1[1] + d[1] * (x2 - p1[0]) / d[0];

    let inside = (x1 >= p1[0] && x1 < p2[0]) || (x2 > p1[0] && x2 <= p2[0]);
    if !inside {
        return [0.0, 0.0];
    }
    let is_trapezoid =
        1.0f64.copysign(y1) == 1.0f64.copysign(y2) || y1.abs() < 1e-4 || y2.abs() < 1e-4;
    if is_trapezoid {
        let a = (y1 + y2) / 2.0;
        if a < 0.0 {
            [a.abs(), 0.0]
        } else {
            [0.0, a.abs()]
        }
    } else {
        // Two triangles.
        let x = -p1[1] * d[0] / d[1] + p1[0];
        let a1 = if x > p1[0] { y1 * x.fract() / 2.0 } else { 0.0 };
        let a2 = if x < p2[0] {
            y2 * (1.0 - x.fract()) / 2.0
        } else {
            0.0
        };
        let a = if a1.abs() > a2.abs() { a1 } else { -a2 };
        if a < 0.0 {
            [a1.abs(), a2.abs()]
        } else {
            [a2.abs(), a1.abs()]
        }
    }
}

/// The area for an orthogonal `pattern` with `left` and `right` pixels to either end of the line,
/// biased by `offset`.
fn area_ortho(pattern: usize, left: f64, right: f64, offset: f64) -> V2 {
    let d = left + right + 1.0;
    let o1 = 0.5 + offset;
    let o2 = 0.5 + offset - 1.0;
    let area = ortho_area;
    let zero = [0.0, 0.0];

    match pattern {
        0 | 5 | 10 | 15 => zero,
        // We only offset L patterns on the crossing edge's side, so they converge with the
        // unfiltered pattern 0.
        1 => {
            if left <= right {
                area([0.0, o2], [d / 2.0, 0.0], left)
            } else {
                zero
            }
        }
        2 => {
            if left >= right {
                area([d / 2.0, 0.0], [d, o2], left)
            } else {
                zero
            }
        }
        3 => {
            let a1 = area([0.0, o2], [d / 2.0, 0.0], left);
            let a2 = area([d / 2.0, 0.0], [d, o2], left);
            let (a1, a2) = smootharea(d, a1, a2);
            [a1[0] + a2[0], a1[1] + a2[1]]
        }
        4 => {
            if left <= right {
                area([0.0, o1], [d / 2.0, 0.0], left)
            } else {
                zero
            }
        }
        // Blend the full offset Z with partially offset L patterns, so a Z's centre and sides
        // agree.
        6 => {
            if offset.abs() > 0.0 {
                let a1 = area([0.0, o1], [d, o2], left);
                let a2 = add(
                    area([0.0, o1], [d / 2.0, 0.0], left),
                    area([d / 2.0, 0.0], [d, o2], left),
                );
                half(add(a1, a2))
            } else {
                area([0.0, o1], [d, o2], left)
            }
        }
        7 => area([0.0, o1], [d, o2], left),
        8 => {
            if left >= right {
                area([d / 2.0, 0.0], [d, o1], left)
            } else {
                zero
            }
        }
        9 => {
            if offset.abs() > 0.0 {
                let a1 = area([0.0, o2], [d, o1], left);
                let a2 = add(
                    area([0.0, o2], [d / 2.0, 0.0], left),
                    area([d / 2.0, 0.0], [d, o1], left),
                );
                half(add(a1, a2))
            } else {
                area([0.0, o2], [d, o1], left)
            }
        }
        11 | 13 => area([0.0, o2], [d, o1], left),
        12 => {
            let a1 = area([0.0, o1], [d / 2.0, 0.0], left);
            let a2 = area([d / 2.0, 0.0], [d, o1], left);
            let (a1, a2) = smootharea(d, a1, a2);
            [a1[0] + a2[0], a1[1] + a2[1]]
        }
        14 => area([0.0, o1], [d, o2], left),
        _ => unreachable!("an orthogonal pattern is a 4-bit edge combination"),
    }
}

/// The diagonal line `p1`→`p2` of each pattern: the line's start, and the base its end adds
/// `(d, d)` to. A pattern with two possibilities blends them.
const DIAG_LINES: [&[(V2, V2)]; 16] = [
    &[([1.0, 1.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 0.0], [0.0, 0.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([0.0, 0.0], [1.0, 0.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 1.0], [0.0, 0.0]), ([1.0, 1.0], [1.0, 0.0])],
    &[([1.0, 1.0], [0.0, 0.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 1.0], [1.0, 0.0])],
    &[([1.0, 1.0], [1.0, 0.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([0.0, 0.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 1.0])],
    &[([1.0, 0.0], [1.0, 1.0])],
    &[([0.0, 0.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 0.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 0.0])],
    &[([1.0, 1.0], [1.0, 1.0])],
    &[([1.0, 1.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 1.0])],
    &[([1.0, 1.0], [1.0, 1.0]), ([1.0, 1.0], [1.0, 0.0])],
    &[([1.0, 1.0], [1.0, 1.0]), ([1.0, 0.0], [1.0, 0.0])],
];

/// The fraction of `SAMPLES_DIAG`² samples over the unit square at `p` that fall on the positive
/// side of the line `p1`→`p2`.
fn diag_area_1(p1: V2, p2: V2, p: V2, samples: &[f64; SAMPLES_DIAG]) -> f64 {
    if p1 == p2 {
        return 1.0;
    }
    let xm = (p1[0] + p2[0]) / 2.0;
    let ym = (p1[1] + p2[1]) / 2.0;
    let a = p2[1] - p1[1];
    let b = p1[0] - p2[0];
    let mut inside = 0.0;
    for ox in samples {
        let x = p[0] + ox;
        for oy in samples {
            let y = p[1] + oy;
            if a * (x - xm) + b * (y - ym) > 0.0 {
                inside += 1.0;
            }
        }
    }
    inside / (SAMPLES_DIAG * SAMPLES_DIAG) as f64
}

/// The area under a diagonal line, for the pixel and its opposite.
fn diag_area(
    pattern: usize,
    line: (V2, V2),
    d: f64,
    left: f64,
    offset: (f64, f64),
    samples: &[f64; SAMPLES_DIAG],
) -> V2 {
    let (e1, e2) = EDGES_DIAG[pattern];
    let mut p1 = line.0;
    let mut p2 = add(line.1, [d, d]);
    if e1 > 0 {
        p1 = add(p1, [offset.0, offset.1]);
    }
    if e2 > 0 {
        p2 = add(p2, [offset.0, offset.1]);
    }
    let a1 = diag_area_1(p1, p2, add([1.0, 0.0], [left, left]), samples);
    let a2 = diag_area_1(p1, p2, add([1.0, 1.0], [left, left]), samples);
    [1.0 - a1, a2]
}

/// The area for a diagonal `pattern`. Unlike the orthogonal ones, the null pattern is filtered too,
/// and the ends of the null and L patterns are unknown, so both possibilities are blended.
fn area_diag(
    pattern: usize,
    left: usize,
    right: usize,
    offset: (f64, f64),
    samples: &[f64; SAMPLES_DIAG],
) -> V2 {
    let d = (left + right + 1) as f64;
    let left = left as f64;
    let lines = DIAG_LINES[pattern];
    let first = diag_area(pattern, lines[0], d, left, offset, samples);
    match lines.get(1) {
        Some(&second) => half(add(
            first,
            diag_area(pattern, second, d, left, offset, samples),
        )),
        None => first,
    }
}

fn to_byte(a: f64) -> u8 {
    (255.0 * a) as u8
}

/// The AreaTex: `AREA_TEX_SIZE`, RG8, row-major — 179 200 bytes.
pub(crate) fn area_tex() -> Vec<u8> {
    let (width, height) = (AREA_TEX_SIZE.0 as usize, AREA_TEX_SIZE.1 as usize);
    let mut tex = vec![0u8; width * height * 2];
    let mut put = |x: usize, y: usize, v: V2| {
        let at = (y * width + x) * 2;
        tex[at] = to_byte(v[0]);
        tex[at + 1] = to_byte(v[1]);
    };

    // Orthogonal patterns compress their distances quadratically, to reach farther per texel.
    for (row, &offset) in SUBSAMPLE_OFFSETS_ORTHO.iter().enumerate() {
        for (pattern, &(ex, ey)) in EDGES_ORTHO.iter().enumerate() {
            for left in 0..SIZE_ORTHO {
                for right in 0..SIZE_ORTHO {
                    let area = area_ortho(
                        pattern,
                        (left * left) as f64,
                        (right * right) as f64,
                        offset,
                    );
                    put(
                        left + SIZE_ORTHO * ex,
                        5 * SIZE_ORTHO * row + right + SIZE_ORTHO * ey,
                        area,
                    );
                }
            }
        }
    }

    let mut samples = [0.0; SAMPLES_DIAG];
    for (i, s) in samples.iter_mut().enumerate() {
        *s = i as f64 / (SAMPLES_DIAG - 1) as f64;
    }
    for (row, &offset) in SUBSAMPLE_OFFSETS_DIAG.iter().enumerate() {
        for (pattern, &(ex, ey)) in EDGES_DIAG.iter().enumerate() {
            for left in 0..SIZE_DIAG {
                for right in 0..SIZE_DIAG {
                    let area = area_diag(pattern, left, right, offset, &samples);
                    put(
                        5 * SIZE_ORTHO + left + SIZE_DIAG * ex,
                        4 * SIZE_DIAG * row + right + SIZE_DIAG * ey,
                        area,
                    );
                }
            }
        }
    }
    tex
}

/// The SearchTex: `SEARCH_TEX_SIZE`, R8, row-major — 1 024 bytes.
pub(crate) fn search_tex() -> Vec<u8> {
    // The bilinear fetch at (-0.25, -0.125) over four edges weighs them 1, 3, 7 and 21 over 32, so
    // each of the 16 edge combinations lands on its own multiple of 1/32.
    let mut edges: [Option<[u8; 4]>; 33] = [None; 33];
    for mask in 0..16u8 {
        let e = [mask >> 3 & 1, mask >> 2 & 1, mask >> 1 & 1, mask & 1];
        let at = e[0] + 3 * e[1] + 7 * e[2] + 21 * e[3];
        edges[at as usize] = Some(e);
    }

    // The delta distance to add in the last step of a search to the left.
    let delta_left = |left: [u8; 4], top: [u8; 4]| {
        let mut d = 0;
        if top[3] == 1 {
            d += 1;
        }
        if d == 1 && top[2] == 1 && left[1] != 1 && left[3] != 1 {
            d += 1;
        }
        d
    };
    // And to the right.
    let delta_right = |left: [u8; 4], top: [u8; 4]| {
        let mut d = 0;
        if top[3] == 1 && left[1] != 1 && left[3] != 1 {
            d += 1;
        }
        if d == 1 && top[2] == 1 && left[0] != 1 && left[2] != 1 {
            d += 1;
        }
        d
    };

    // The upstream builds a 66×33 image (left deltas, then right deltas), crops rows 17..33 and
    // flips them vertically.
    let mut image = vec![0u8; 66 * 33];
    for x in 0..33 {
        for y in 0..33 {
            if let (Some(left), Some(top)) = (edges[x], edges[y]) {
                image[y * 66 + x] = 127 * delta_left(left, top);
                image[y * 66 + 33 + x] = 127 * delta_right(left, top);
            }
        }
    }
    let mut tex = Vec::with_capacity(64 * 16);
    for y in 0..16 {
        let source = 17 + (15 - y);
        tex.extend_from_slice(&image[source * 66..source * 66 + 64]);
    }
    tex
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64-bit FNV-1a.
    fn fnv1a(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// The hash is of the byte array in the upstream `Textures/AreaTex.h`.
    #[test]
    fn area_tex_matches_the_reference() {
        let tex = area_tex();
        assert_eq!(tex.len(), 160 * 560 * 2);
        assert_eq!(fnv1a(&tex), 0x247a_58bb_ba65_292d);
        // Subsample 0, pattern 0 at distance 0 on both sides is the null pattern: no area.
        assert_eq!(&tex[..2], &[0, 0]);
    }

    /// `smaa_weights.wgsl` declares the tables' geometry as its own consts; the generator's sizes are
    /// the source they must equal.
    #[test]
    fn the_weights_shader_agrees_with_the_tables() {
        use crate::shader_gen as shader;
        assert_eq!(shader::SMAA_AREATEX_MAX_DISTANCE as usize, SIZE_ORTHO);
        assert_eq!(shader::SMAA_AREATEX_MAX_DISTANCE_DIAG as usize, SIZE_DIAG);
        assert_eq!(
            shader::SMAA_AREATEX_SUBTEX_COUNT as usize,
            SUBSAMPLE_OFFSETS_ORTHO.len()
        );
        assert_eq!(
            (shader::SMAA_AREATEX_WIDTH, shader::SMAA_AREATEX_HEIGHT),
            AREA_TEX_SIZE
        );
        assert_eq!(
            (shader::SMAA_SEARCHTEX_WIDTH, shader::SMAA_SEARCHTEX_HEIGHT),
            SEARCH_TEX_SIZE
        );
    }

    /// The hash is of the byte array in the upstream `Textures/SearchTex.h`.
    #[test]
    fn search_tex_matches_the_reference() {
        let tex = search_tex();
        assert_eq!(tex.len(), 64 * 16);
        assert_eq!(fnv1a(&tex), 0x21c1_fcf0_aa63_1065);
        assert!(tex.iter().any(|&b| b != 0));
        assert!(tex.iter().all(|&b| b == 0 || b == 127 || b == 254));
    }
}
