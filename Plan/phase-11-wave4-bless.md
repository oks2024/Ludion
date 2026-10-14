# Phase 11 Wave 4 — the swap: bless event evidence

The BRDF swap (`mesh.wgsl` GGX/Cook-Torrance), the smooth `sphere(24, 16)` primitive, and the new
`material-park` gallery content land here. Per Plan/phase-11-pbr.md decision 9 this is the phase's
one pre-authorized golden-moving wave. Rendered on the pinned RTX 3090 (Vulkan). All SSIM values
below are the tiled testkit metric unless marked *global*.

## Movers — re-blessed once (old golden vs new render, pre-bless)

| Golden | pre-bless SSIM | MAE (0–255) | what moved |
|---|---|---|---|
| gallery `meshes` | 0.9352 | 1.86 | sphere → smooth + specular; cube/plane/ground unchanged |
| gallery `lights` | 0.9765 | 1.53 | neutral sphere gained a specular sheen; cubes + ground at parity |
| gallery `billboards` | 0.9992 | — | ground plane picked up the BRDF (still passed threshold, but not byte-identical → re-blessed) |
| gallery `mesh-asset` | 0.9991 | — | crystal asset re-shaded (passed threshold, re-blessed for byte-identity) |
| gallery `voxel-model` | 0.9990 | — | lantern/tree quads re-shaded (passed threshold, re-blessed for byte-identity) |
| import `scene` | 0.74 *global* | 3.46 *gray* | crystal mesh: faceted dark/light split → smooth even blue; cones unchanged. Re-blessed out-of-band via `render examples/import/expected/scene.yaml -o examples/import/goldens/scene.png` (960×540). |
| gallery `material-park` | — (new) | — | new exit-gate scene, blessed for the first time |

Raw testkit `DiffReport`s were only written for the two below-threshold goldens (`lights`,
`meshes`); the three that stayed above 0.99 print their SSIM but produce no report file.

### Brightness parity (the π-convention check — decision 3)

`lights` and `meshes` damage is **localized to the sphere centers** (the added specular
highlights), not spread across the frame:

- `lights`: MAE 1.53. Worst tiles are the two dead-center sphere tiles (r4c3/r4c4, ssim 0.33/0.34);
  every other tile ≥ 0.94. Visual A/B confirmed: cubes and ground are unchanged; the sphere kept
  its brightness and gained the RGB-light specular sheen. No wholesale darkening.
- `meshes`: MAE 1.86. Worst tiles are the sphere face (r3c5/r4c5/r1c5, ssim 0.08–0.15) where the
  faceted normals became smooth + a specular hotspot appeared. Cube/plane/ground tiles unchanged.

A wholesale 3× darkening (the failure the π-scaling guards against) would have produced a large MAE
across every tile; MAE ≈ 1.5–1.9 with damage confined to highlight tiles is the expected signature.

## Non-movers — asserted byte-identical (decision 9)

The 7 goldens that must not move did not. Verified two ways:

1. **Pre-bless compare run** (`ludion test --dir examples/gallery`) reported each at **SSIM
   1.0000** — pinned-GPU byte-identity — for `sprites`, `text`, `tilemap`, `breakout-mock`.
2. **git status**: after blessing only the 6 movers + writing `material-park`, the non-mover PNGs
   (`sprites.png`, `text.png`, `tilemap.png`, `breakout-mock.png`, `games/breakout/goldens/main.png`,
   `games/hello-paddle/goldens/paddle.png`, `games/sokoban/goldens/main.png`) are untouched on disk
   (not in `git status`). The 3 game goldens use game-registry components (CLI render is
   builtins-only) and are covered byte-identical by their own workspace golden tests, which stay
   green.

The sprite/text/tilemap path shares only the `Camera`/`Lights` uniform *layouts* with the mesh
shader; those structs were not changed, so the shared path came back byte-identical as designed.

## Post-bless verification

- `ludion test --dir examples/gallery`: **10/10 pass at SSIM 1.0000**.
- Import golden byte-stable across two renders (sha256 match).
- `node scripts/check.mjs --rust-only`: **ALL GREEN** (fmt, clippy, `cargo test --workspace`
  incl. renderer, gallery suite, import exit-gate, game goldens, replay/WorldHash; explain; docs).
- `ludion profile --scene examples/gallery/material-park.yaml`: exit 0; scene pass median 0.028 ms.
