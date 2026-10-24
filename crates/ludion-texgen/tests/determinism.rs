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
  nrm:
    op: normal_from_height
    input: high
    depth: 0.08
outputs:
  base_color: albedo
  roughness: soft
  metallic: flat
  normal: nrm
  occlusion: low
  emissive: tone
  height: high
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
    0x7099d73beebb4a5d,
    0x83b337890a9fff8e,
    0x6c2338e43f463f6e,
    0xddd4cb779acb745f,
    0x82e8bd780c579d21,
    0x7ddfc8be0c23cf19,
];
const PINNED_128: [u64; 6] = [
    0x7f21197f3734f7b8,
    0xc1193c34f638e6ce,
    0x9b289587ed972a2c,
    0xb7c31384ed55cab4,
    0xa8cd0c1ec1434a8a,
    0x559c2d7dcaaa1e5b,
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
