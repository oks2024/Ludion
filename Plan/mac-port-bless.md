# macOS port Wave 1 — per-OS goldens: bless event evidence

Modeled on `Plan/phase-11-wave4-bless.md`. This wave replaces the single flat golden sets with **per-OS
subdirectories**: pinned-tier byte-identity (SSIM == 1.0) is a per-(OS, GPU) guarantee and can
never hold across two GPUs, so each pinned platform now keeps its own blessed set —
`goldens/windows/<stem>.png` (pinned RTX 3090, Vulkan) and `goldens/macos/<stem>.png` (pinned
Apple M4, Metal) — resolved at runtime by `ludion_testkit::platform_dir()` from
`std::env::consts::OS` (any other OS is the new `L2205`). `goldens/manifest.json` stays at the
`goldens/` root: thresholds/resolution describe scene content, not a platform.

## Machine

Apple M4 (Metal 4), wgpu Metal backend, macOS (Darwin 25.5.0). Renders verified byte-deterministic
on this machine. All images 960×540 except the lantern contact sheet (900×600, its committed
resolution — six views, 3×2 grid of 300×300 cells).

## The 17 windows files — pure moves, asserted byte-identical

Every existing PNG was `mv`ed (never re-encoded) from `goldens/<stem>.png` to
`goldens/windows/<stem>.png`; sha256 recorded before the move and re-checked after — all 17 OK:

- `examples/gallery/goldens/windows/` — `billboards`, `breakout-mock`, `colliders`, `lights`,
  `material-park`, `mesh-asset`, `meshes`, `sprites`, `text`, `tilemap`, `voxel-model`,
  `lantern-sheet` (12)
- `examples/import/goldens/windows/scene.png`
- `games/breakout/goldens/windows/main.png`
- `games/sokoban/goldens/windows/main.png`
- `games/hello-paddle/goldens/windows/paddle.png`
- `games/vigil/goldens/windows/district-plaza.png`

## The 17 new macos files

Blessed on this machine through the same paths that produced the windows originals:

- 11 gallery scenes: `ludion test --dir examples/gallery --bless` → `goldens/macos/*.png`
- `lantern-sheet.png`: `LUDION_BLESS=1 cargo test -p ludion-testkit --test gallery
  lantern_contact_sheet` (the test that owns the sheet; 900×600 via
  `ludion_engine::render_model_sheet`, all six views)
- import `scene.png`: `ludion render --scene examples/import/expected/scene.yaml --width 960
  --height 540 --out examples/import/goldens/macos/scene.png` (explicit 960×540 — the testkit
  default; `render`'s own default is 1920×1080)
- 4 games: `LUDION_BLESS=1 cargo test -p <game> --test golden` — scoped to `--test golden` so the
  committed `run.lrp` session replays were **not** re-recorded (`lore status --scan` confirms: no
  `.lrp` changes, only the new PNGs)

## Pre-bless SSIM — macOS render vs the Windows goldens

Tiled testkit metric (8×8 grid, pixel-weighted). The macOS render is byte-deterministic here, so
the committed macos-vs-windows golden pair is the same comparison; values re-verified post-bless
over the committed pairs to 6 decimal places.

| Golden | SSIM vs windows |
|---|---|
| gallery `billboards` | 0.9998 |
| gallery `breakout-mock` | 0.9999 |
| gallery `colliders` | 0.9998 |
| gallery `lights` | 0.9999 |
| gallery `material-park` | 1.0000 (0.999973) |
| gallery `mesh-asset` | 0.9999 |
| gallery `meshes` | 0.9999 |
| gallery `sprites` | 1.0000 (0.999999+) |
| gallery `text` | 1.0000 (0.999999+) |
| gallery `tilemap` | 1.0000 (0.999986) |
| gallery `voxel-model` | 0.9999 |
| gallery `lantern-sheet` | 0.999933 |
| import `scene` | 0.99999989 *global* |
| breakout `main` | 0.999962 |
| sokoban `main` | 0.999088 |
| hello-paddle `paddle` | 1.0000 (0.999999+) |
| vigil `district-plaza` | 0.999924 |

Every pair sits at or above 0.999 — the two GPUs agree to well within the 0.99 tier (the games'
values were not captured during the original scouting pass; the four rows above are the post-bless
re-measurement over the committed pairs). No pair is byte-identical (checked with `cmp`), which is
exactly why the tier is per-OS.

## Post-bless verification

- `ludion test --dir examples/gallery`: 11/11 PASS at ssim=1.0000.
- `cargo test --workspace`: all green on this machine (byte-identity gates now compare the
  macos-blessed set; the import exit gate renders vs the fresh macos golden at exactly 1.0).
- `ludion explain L2205` prints the new code; the codes-table bijection + reachability audit passes.
- `node scripts/check.mjs`: ALL GREEN.

## Human gate

**Passed, 2026-07-16.** The user reviewed all 17 macos/windows pairs side by side (960×540 at
full size; lantern sheet at 900×600) and approved the set: no structural differences — no missing
objects, no color/lighting errors, no text corruption. Review medium: a generated HTML page
showing each windows image beside its macos twin, viewed on the blessing machine.
