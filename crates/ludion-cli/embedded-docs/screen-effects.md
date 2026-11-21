---
description: ScreenEffect: CRT, analog-video and scan filters over the frame, a region or a view, in UI order
---

# Screen effects: `ScreenEffect`

**Scope:** `ScreenEffect` filters what is already drawn: the 3D frame and every UI element with a lower `order`, over
the whole output (`target: fill`), a rectangle of it (`target: region`) or a secondary view's texture (`target: view`). It
is a backdrop filter in UI order, a look for a radar panel, a terminal or a whole retro screen. It is render-only: on a presentation entity it is never hashed,
and a scene without one keeps the identical frame graph and the identical pixels. The imaging chain on the 3D frame is
`ludion docs post-processing`; the UI it filters is `ludion docs screen-ui`.

## Format

```yaml
- name: radar-feed-fx
  components:
    ScreenEffect:
      target:
        region:
          anchor: top_right        # laid out like a UiRect, in canvas units
          offset: [-32.0, 32.0]
          size: [320.0, 320.0]
      order: 10                    # UI order: lower-order elements draw under the effect
      preset: crt
      scanline_count: 240          # any stage overrides the preset
      curvature: 0.06
```

A scene file writes the lists block-style; flow style is shown here for brevity.

- **`target`**: `fill` (the default, the whole frame), `region { anchor, offset, size }`, laid out exactly like a
  `UiRect` (`ludion docs screen-ui`), or `view: <name>`, a `RenderView`'s texture (see On a view). A non-positive or
  non-finite size, an empty view name, or a view named by no `RenderView` is `L1761`.
- **`order`** (default 0): the effect filters the 3D frame and every UI element with a **lower** `order`; elements at
  the same `order` or above draw clean over it. Ties between effects break by entity position.
- **`preset`**: the starting look; every unset stage takes the preset's value, so the scene text carries only what the
  author overrode. An effect with every stage off is not applied and costs nothing.
- **`seed`**: varies the noise and jitter hash (default 0).

## On a view

`target: { view: radar }` runs the effect as the last step of that `RenderView`'s own frame, over its whole texture, so
it is baked in before any `UiRect` shows the view (`ludion docs render-views`). The effect's `order` only sorts it among
the effects on that view; it has no UI to sit under. The rest of the frame is unchanged. A view that exists but does not
render (`enabled: false`, or past the view budget) drops its effects silently.

## Presets

| `preset` | Look |
| --- | --- |
| `none` | every stage off |
| `crt` | scanlines 0.35, aperture mask 0.2, curvature 0.06, green phosphor tint, glow 0.3, noise 0.04, flicker 0.02 |
| `analog_video` | chroma bleed 0.4, tracking 0.15, jitter 0.2, noise 0.08, flicker 0.03, amber tint, interlace, scanlines 0.1 |
| `scan` | scanlines 0.2 at 540 lines, a sweep bar 0.25 every 4 s, posterize 24, noise 0.02 |

## Stages

Evaluated in this order, each skipped when it is zero or off. The picture is filtered in display (sRGB-encoded) values,
because the presets are display looks.

| Stage | Range | What it does |
| --- | --- | --- |
| `roll` | -4..4 | vertical hold roll, regions per second |
| `curvature` | 0..0.3 | barrel warp; outside the warped image is black |
| `tracking` | 0..1 | horizontal tracking-error bands that shift every few ticks |
| `jitter` | 0..1 | per-line horizontal jitter |
| `chroma_bleed` | 0..1 | red and blue fringe sideways from green |
| `glow` | 0..1 | bloom of the bright parts, a small blur added back |
| `phosphor` | each >= 0 | mono tint: the luma times the colour; `[0, 0, 0]` keeps the colour |
| `posterize` | 0..256 | colour levels per channel; 0 or 1 is off |
| `scanlines`, `scanline_count` | 0..1, 1..4320 | dark bands across the region's height |
| `interlace` | bool | alternate lines darken each tick |
| `mask`, `mask_strength` | `aperture`, `slot`, `shadow`; 0..1 | RGB stripe, slot or dot-triad mask in output pixels |
| `sweep`, `sweep_period` | 0..1; 0.1..600 s | a bright bar crossing the region |
| `noise` | 0..1 | per-pixel animated noise |
| `flicker` | 0..1 | whole-region brightness flicker |

## Animation

Every animated stage (noise, jitter, tracking, roll, sweep, flicker, interlace) is a function of the game tick, the
game clock, the pixel and `seed`, evaluated from integer hashes and polynomials. A paused game holds the picture, a
replay reproduces it, and two processes render the same frame. `ludion render --frames 0,1,2` shows the motion.

## The cap and the cost

At most **4** effects are applied per frame, fill, region and view together: the lowest `(order, entity)` among those
that change a pixel. Each one past
the cap is not applied and raises `L2392` once, never silently (`ludion explain L2392`). Each applied effect adds one
`screen_fx_k` pass: the overlay writes a whole-frame layer instead of the output, and each pass copies the previous
layer, draws its effect over its rectangle and then the next slice of UI. Pixels outside every rectangle are the plain
frame. The `[post_groups] screen_fx` group in a profile budget sums the passes; the target is 0.15 ms at 1080p for a
320x320 region plus a fullscreen `scan`.

## No shader hook

There is no custom-shader slot: the presets and their stages are the whole surface, so one look has one spelling
(`ludion docs post-processing` for the full-frame grade). Faking scanlines with tiled `UiRect`s is not needed.

## Next

- `ludion docs screen-ui` — the UI elements an effect filters and the order they draw in.
- `ludion docs render-views` — the views a `view` target filters.
- `ludion docs post-processing` — the imaging chain on the 3D frame.
- `ludion docs components` — the built-in component set.
