---
description: SMAA 1x edge anti-aliasing — the --aa flag, the runtime switch, goldens, and what it won't fix
---

# Anti-aliasing: `--aa`, `RenderSettings`

**Scope:** the single-frame edge anti-aliasing (SMAA 1x) a host or a game turns on, and how it relates
to supersampling. The imaging chain it joins is `ludion docs post-processing`; `--scale` and the
presets are `ludion docs quality`; the host flags are `ludion docs running`; goldens and hashes are
`ludion docs verify-loop`.

## The loop

Measure with grain and depth of field off — both lower `edge_aliasing` without anti-aliasing anything.

```
ludion render --scene examples/gallery/anti-aliasing.yaml --aa smaa --out target/aa-smaa.png
ludion render --scene examples/gallery/anti-aliasing.yaml --out target/aa-off.png
ludion render --scene examples/gallery/anti-aliasing.yaml --scale 2 --out target/aa-x2.png
ludion image-stats target/aa-off.png        # edge_aliasing: off, then smaa, then --scale 2
ludion dump-graph --scene examples/gallery/anti-aliasing.yaml --aa smaa    # +3 passes after composite
ludion profile --scene examples/gallery/anti-aliasing.yaml --aa smaa
```

Measured on `post-full` at 480x270 on the RTX 3090: `edge_aliasing` falls from 0.798 (off) to 0.464
(smaa); `--scale 2` reaches 0.492. SSIM against a `--scale 4` render rises (0.9858 → 0.9895), and
edge-free regions stay at 0.9999 of the off frame: it is not a blur. Cost at 1080p, same GPU, pass
medians: the three SMAA passes sum to 0.095 ms (edges 0.025, weights 0.045, blend 0.026), against the
scene pass going 0.125 ms → 0.257 (`--scale 1.5`) → 0.462 (`--scale 2`).

## Format

Three control surfaces; the first that has an opinion wins, top to bottom:

| Surface | Set by | Reach |
|---|---|---|
| Runtime choice | a game system, `RenderSettings::request_anti_aliasing` | the session, across world rebuilds |
| Host | `--aa off\|smaa`, or `Engine::set_anti_aliasing` | the process |
| Builder default | `EngineBuilder::anti_aliasing` | the game's default |
| (none) | — | `off` |

`--aa` is accepted by `ludion render`, `serve`, `dev` (forwarded to every child), `profile` and
`dump-graph`, and by a game binary through `ludion::run`. `profile --attach` refuses it; the replay,
profile-replay and probe-bake modes ignore it, as they ignore `--render-size`.

An options-menu row is one system:

```rust
use ludion::prelude::*;

fn options(world: &mut World, _ctx: &SystemCtx) {
    if let Some(settings) = world.resource_mut::<RenderSettings>() {
        settings.request_anti_aliasing(AntiAliasing::Smaa);
    }
}
```

The request applies at the next frame boundary. `settings.clear_anti_aliasing()` drops the game's
choice and returns to the host's `--aa`, else the builder default. The choice lives on the Engine: it
survives world rebuilds but not a `ludion dev` restart, and an `engine.restore` does not roll it back,
so a game re-requests from its own option state on boot and after a restore (every tick, or on change
plus on load). `engine.describe` and a profile report's
`quality.anti_aliasing` name the mode the host resolved.

A golden suite pins the mode per scene with the `goldens/manifest.json` key `anti_aliasing` (a name
`--aa` takes; anything else is `L2208`). Only `examples/gallery/anti-aliasing.yaml` uses it.

## Rules

- **Off by default, and off is the old frame.** The graph, every golden and every hash are unchanged
  until something turns it on.
- **Write-only on purpose.** The effective mode depends on the host flag, which differs per machine, so
  a sim read would leak machine state into replays. `RenderSettings` has no getter: keep the option in
  your own state (a profile, say). The choice is never hashed, snapshotted, recorded or on the wire.
- **Goldens pin the builder default, never the flag.** The golden harness reads no `--aa`; a scene
  that wants SMAA in its golden says so in the manifest.
- **A bad value is clap's refusal** (the possible values are listed), not an `L####`.
- **The graph:** `smaa_edges → smaa_weights → smaa_blend` after `composite`, before `resolve`/`upscale`
  and `overlay`. Luma detection at fixed HIGH constants (threshold 0.1); nothing is authorable. UI is
  drawn after, so UI pixels match off.
- **Chain order:** with SMAA on, composite writes grain 0 and dither 0, and `smaa_blend` applies grain
  then dither: `… vignette → SMAA → grain → dither`.
- **A budget refuses an SMAA report** with `L4020`, as it refuses a non-`full` preset: an SMAA capture
  pays passes the budget never measured. Re-measure with `--aa off`.

## Gotchas

- **Sub-pixel geometry stays lost.** A wire thinner than a pixel was never rasterized; SMAA cannot
  reconnect it. `--scale` is the fix and the ceiling.
- **Shimmer and crawl under motion remain.** A single-frame filter lowers each step's contrast; it has
  no history.
- **Grain and depth of field lower `edge_aliasing`** with no anti-aliasing at all — turn them off to
  measure.
- **Debug views skip it.** `--debug-view` schedules no SMAA passes.
- **No preset masks it.** At `low` it runs at the 0.75 render extent, before `upscale`.
- **Never compare profile reports across modes:** `quality.anti_aliasing` says which one a report is.

## Next

- `ludion docs post-processing` — the chain SMAA sits in.
- `ludion docs quality` — `--scale`, the presets and the override file.
- `ludion docs running` — the host flags and `profile`.
- `ludion docs verify-loop` — goldens and the suite manifest.
