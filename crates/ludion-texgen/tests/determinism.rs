//! The PNG bytes of one recipe using every op are pinned, and identical across thread counts.

mod common;

use common::MemResolver;
use ludion_core::par;

const ROOT: &str = "format: 1
kind: texture
seed: 7
params:
  scale: 5
  tint: \"#8a7a6a\"
steps:
  base:
    op: noise
    type: gradient
    scale: $scale
    octaves: 4
  cells:
    op: noise
    type: cellular
    scale: 6
    return: f2_minus_f1
    jitter: 0.9
    seed: 1
  grain:
    op: noise
    type: value
    scale: 9
    octaves: 2
    gain: 0.6
    seed: 2
  ramp:
    op: gradient
    axis: u
    shape: triangle
    repeat: 2
  sweep:
    op: gradient
    axis: v
    shape: sine
  tone:
    op: constant
    value: $tint
  flat:
    op: constant
    value: 0.5
  mixed:
    op: blend
    a: base
    b: cells
    mask: ramp
    mode: overlay
    opacity: 0.8
  lit:
    op: blend
    a: mixed
    b: sweep
    mode: screen
  deep:
    op: blend
    a: lit
    b: grain
    mode: difference
  lev:
    op: levels
    input: deep
    in_low: 0.1
    in_high: 0.9
    gamma: 1.4
    out_low: 0.05
    out_high: 0.95
  cur:
    op: curve
    input: lev
    points:
      - x: 0.0
        y: 0.1
      - x: 0.5
        y: 0.8
      - x: 1.0
        y: 0.3
  clp:
    op: clamp
    input: cur
    min: 0.1
    max: 0.9
  rmp:
    op: remap
    input: clp
    from_low: 0.1
    from_high: 0.9
    to_low: 0.0
    to_high: 1.0
  tinted:
    op: colorize
    input: rmp
    stops:
      - at: 0.0
        color: \"#201a14\"
      - at: 0.6
        color: $tint
      - at: 1.0
        color: \"#e8ddc8\"
  gray:
    op: grayscale
    input: tinted
  warped:
    op: warp
    input: rmp
    by: tinted
    amount: 0.04
  slid:
    op: warp
    input: gray
    by: base
    amount: 0.05
    angle: 30
  soft:
    op: blur
    input: slid
    radius: 0.01
  bricks:
    op: recipe
    path: sub/bricks.texture.yaml
    output: height
    params:
      scale: 3
      mortar: $tint
    seed: 4
  relief:
    op: blend
    a: warped
    b: bricks
    mode: add
    opacity: 0.5
  albedo:
    op: blend
    a: tinted
    b: tone
    mode: multiply
  low:
    op: blend
    a: soft
    b: flat
    mode: min
  high:
    op: blend
    a: low
    b: relief
    mode: max
  stamps:
    op: tile_sampler
    columns: 5
    rows: 6
    shape: bell
    scale: 1.3
    scale_jitter: 0.3
    position_jitter: 0.5
    rotation_jitter: 0.6
    value_jitter: 0.4
    offset: 0.5
    density: 0.8
    combine: add
    seed: 3
  blobs:
    op: noise
    type: value
    scale: 3
    octaves: 2
    seed: 5
  parts:
    op: flood_fill
    input: blobs
    threshold: 0.5
    output: gradient
    angle: 30
    angle_jitter: 0.5
    seed: 2
  regions:
    op: flood_fill
    input: blobs
    threshold: 0.45
    output: uv
  edge:
    op: distance
    input: blobs
    threshold: 0.5
    max: 0.15
  pic:
    op: image
    path: tex/pattern.png
  ao:
    op: ambient_occlusion
    input: high
    radius: 0.06
    depth: 0.08
    strength: 0.9
  crv:
    op: curvature
    input: high
    radius: 0.02
  detail:
    op: blend
    a: stamps
    b: edge
    mode: max
  detail2:
    op: blend
    a: detail
    b: parts
    mode: add
    opacity: 0.3
  detail3:
    op: blend
    a: detail2
    b: crv
    mode: overlay
    opacity: 0.5
  height_all:
    op: blend
    a: high
    b: detail3
    mode: add
    opacity: 0.25
  occl:
    op: blend
    a: low
    b: ao
    mode: multiply
  patina:
    op: blend
    a: albedo
    b: pic
    mode: overlay
    opacity: 0.5
  color_all:
    op: blend
    a: patina
    b: regions
    mode: screen
    opacity: 0.2
  nrm:
    op: normal_from_height
    input: height_all
    depth: 0.08
outputs:
  base_color: color_all
  roughness: soft
  metallic: flat
  normal: nrm
  occlusion: occl
  emissive: tone
  height: height_all
";

const BRICKS: &str = "format: 1
kind: texture
seed: 3
params:
  scale: 4
  mortar: \"#101010\"
steps:
  rows:
    op: gradient
    axis: v
    shape: ramp
    repeat: $scale
  inner:
    op: recipe
    path: inner.texture.yaml
    output: n
  height:
    op: blend
    a: rows
    b: inner
    mode: mix
    opacity: 0.4
outputs:
  base_color: height
  roughness: height
  height: height
";

const INNER: &str = "format: 1
kind: texture
steps:
  n:
    op: noise
    type: value
    scale: 7
outputs:
  base_color: n
  roughness: n
  height: n
";

fn resolver() -> MemResolver {
    MemResolver::with(&[
        ("sub/bricks.texture.yaml", BRICKS),
        ("sub/inner.texture.yaml", INNER),
    ])
    .with_image("tex/pattern.png", common::pattern_png(8))
}

fn hashes(size: u32) -> [u64; 6] {
    let set = ludion_texgen::build(ROOT, &resolver(), size).expect("recipe builds");
    let h = |bytes: &[u8]| ludion_assets::content_hash(bytes);
    [
        h(&set.base_color),
        h(&set.metallic_roughness),
        h(&set.normal),
        h(&set.occlusion),
        h(set.emissive.as_ref().expect("emissive is authored")),
        h(&set.height),
    ]
}

/// A change here is a reviewed re-pin: it re-textures every committed set built from the changed op.
const PINNED_64: [u64; 6] = [
    0x917ab525416d7237,
    0x83b337890a9fff8e,
    0xddcae2a51b3e19ed,
    0x19e7c3461387f679,
    0x82e8bd780c579d21,
    0x8a369b955759952e,
];
const PINNED_128: [u64; 6] = [
    0x1ae1ab0dd124bc6f,
    0xc1193c34f638e6ce,
    0x07a93e5573935d80,
    0x29026ad47936e570,
    0xa8cd0c1ec1434a8a,
    0xb22b2b73eafa28bb,
];

fn check(size: u32, pinned: [u64; 6]) {
    let got = hashes(size);
    assert_eq!(
        got, pinned,
        "texgen output at size {size} changed. If the change is intended, review the maps and re-pin:\n\
         const PINNED_{size}: [u64; 6] = {got:#018x?};"
    );
}

#[test]
fn pinned_hashes_at_64() {
    check(64, PINNED_64);
}

#[test]
fn pinned_hashes_at_128() {
    check(128, PINNED_128);
}

#[test]
fn bytes_are_identical_across_thread_counts() {
    let one = par::with_threads(1, || hashes(64));
    let eight = par::with_threads(8, || hashes(64));
    assert_eq!(one, eight);
}
