# GI wave 4 — the sweeps that settled vigil's `GiVolume`

`Plan/global-illumination.md` §4 (W4) requires the shipped values to come off measurement with a
stated criterion, not off taste. This is the evidence behind the block `games/vigil/scenes/env-night.yaml`
now authors on the `environment` entity — the one volume all three districts, the E6 budget scene and
the stress arena share.

## The shipped values

| field | shipped | why, in one line |
|---|---|---|
| `cell_size` | `0.5` | plaza grid 264 × 30 × 288 = 2.28 M cells (7% of `L2103`'s 32 M), 283 k occupied; the two neighbours on the ladder move the frame by ≤ 0.81 of 255 and cost 8× the voxelize (0.25) or drop to one cell per world unit (1.0) |
| `probe_spacing` | `1.0` | 133 × 16 × 145 = 309 k probes, 7.7% of the 4 M probe cap; the thin-occluder rule is knowingly exceeded and the leak measured absent — see below |
| `normal_bias` | `0.5` | half the spacing, the committed `gi-interior` rule |
| `sky_visibility` | `1.0` | full strength; the cheap half here — it costs 0.80 of 255 of frame mean, because this night's IBL is small |
| `bounce` | `0.6` | the largest value at which the plaza frame's pool-lead contrast (`p95/p50` of luma) stays ≥ 2.0, so the torch pools still lead the read |

## The instrument, and why it is trustworthy

`ludion render` links the built-in component registry, so it refuses a district scene (`L1013` on
`Player`, `Hud`, `Gate`, …). Every sweep below was therefore rendered from a **scratch copy** of the
committed `district-plaza.yaml` with the 21 vigil-only component keys stripped out (they are gameplay
markers with no render side) and the `generated_from:` line dropped, written under `.build/` with the
`voxels/` directory copied beside it so asset paths resolve identically.

The copy is not assumed faithful, it is **proven**: the same strip applied to the *pre-wave*
`district-plaza.yaml` and rendered through `ludion render --width 960 --height 540` produces a PNG
**byte-identical** to the committed `games/vigil/goldens/windows/district-plaza.png` (607,675 bytes,
both). So the instrument renders exactly what the golden test renders, and a sweep cell differs from
the golden only by the value being swept.

All measurements are on the golden's own 960 × 540 frame; luma is `0.2126 R + 0.7152 G + 0.0722 B`
on the 0–255 scale.

## `bounce` — the sweep that needed a criterion (`gi-wave4-bounce.png` deleted before publication; in Lore history)

`bounce` defaults to `0.0`, and `0.0` also gates the per-frame dynamic injection off entirely
(`gi_inject.rs`'s `scheduled_lattice` requires `bounce > 0.0`), so W4 had to author it explicitly:
without it neither the lantern nor the bolt tracers are ever injected and the whole dynamic path is
dead. It is also the wave's loud knob — the moon-fill deletion and `sky_visibility` between them take
7.07 of 255 out of the frame, and `bounce` is what puts light back, honestly this time.

**Criterion.** On the plaza's golden frame — the district's densest torch cluster in view — the
frame's **pool-lead contrast**, the brightest 5% of luma over the median (`p95/p50`), must stay at or
above **2.0**. That number is read off the two ends of the change: the deleted `moon-fill` produced a
flat, floor-lit frame at 1.869, and the fill-free frame reads 2.021; 2.0 is the round value inside
that band, next to the fill-free ceiling. Take the largest bounce that clears it. The failure the
criterion catches is exactly the fill's own — a warm wash under every cobble at once, with the pools
no longer leading — arriving by a different door.

```
# plaza.yaml = district-plaza.yaml minus the 21 vigil component keys and `generated_from:`,
# with games/vigil/scenes/voxels/ copied beside it (a throwaway script, not committed)
ludion render --scene .build/gi-w4/plaza.yaml --out Plan/artifacts/gi-wave4-bounce.png \
  --width 1920 --height 1080 --sweep /entities/0/components/GiVolume/bounce=0.0,0.3,0.6,1.0
ludion render --scene .build/gi-w4/plaza.yaml --out <cell>.png --width 960 --height 540 \
  --set /entities/0/components/GiVolume/bounce=<v>
```

| `bounce` | frame mean | R − B | p50 | p95 | **p95/p50** |
|---|---|---|---|---|---|
| *pre-wave golden* (moon-fill 0.5, no volume) | 74.202 | 12.303 | 69.156 | 129.264 | **1.869** |
| *fill deleted, volume at sky 0 / bounce 0* | 67.931 | 14.457 | 61.643 | 124.129 | **2.014** |
| 0.0 (sky visibility on, bounce off) | 67.136 | 15.777 | 60.834 | 122.959 | **2.021** |
| 0.25 | 68.124 | 16.262 | 61.812 | 124.785 | 2.019 |
| 0.5 | 69.107 | 16.734 | 62.922 | 126.443 | 2.010 |
| **0.6 (shipped)** | **69.497** | **16.922** | **63.427** | **127.070** | **2.003** |
| 0.65 | 69.691 | 17.016 | 63.686 | 127.389 | 2.000 |
| 0.7 | 69.886 | 17.108 | 63.937 | 127.689 | 1.997 |
| 0.75 | 70.079 | 17.200 | 64.181 | 128.007 | 1.994 |
| 1.0 | 71.042 | 17.654 | 65.277 | 129.520 | 1.984 |
| 1.5 | 72.934 | 18.535 | 67.172 | 132.637 | 1.975 |
| 2.0 | 74.785 | 19.404 | 68.724 | 135.883 | 1.977 |

The crossing sits at 0.65, exactly on the threshold; `0.6` is the shipped value because it clears it
with margin and 0.05 steps are false precision on a taste knob. It buys **+2.36 of frame mean and
+1.15 of R − B** over `bounce: 0.0` — a bigger move than the halation the same file calls faint
(+1.3) — for 0.018 of contrast. On the sheet, 0.0 → 0.6 is bounced warmth appearing on the cobbles
around the fountain and down the avenue; 0.6 → 1.0 is the ground between the pools going broadly
amber.

**Checked on the other two districts** at the same value, from their own start cameras (each has
fewer pools in frame, so each starts flatter and the criterion is slack there — the plaza is the
binding case, which is why the number is set on it):

| district | `bounce` 0.0 | 0.6 | 1.0 |
|---|---|---|---|
| market ward — mean / contrast | 62.269 / 1.728 | 64.026 / 1.728 | 65.184 / 1.733 |
| cathedral quarter — mean / contrast | 64.882 / 1.696 | 66.758 / 1.682 | 67.988 / 1.672 |

## `probe_spacing` — the thin-occluder rule, knowingly exceeded, and the leak measured absent

`ludion docs global-illumination` states the calibration rule: `probe_spacing` must not exceed the
thinnest occluder, or a probe on the bright side and one on the dark side straddle the wall and leak
sky through it. **The city kit exceeds it and that is a measured call.**

The kit's thinnest occluder is the house shells' wall — `wallT: 2` cells in `scripts/gen-houses.mjs`
over models authored at 5 voxels per world unit, so **0.4 u**. (The perimeter rampart is 4.0 u; every
other kit occluder — tower, pillar, idol, crate, barrel, fountain — is 1.6 u or more.) Honouring
the rule would mean `probe_spacing` 0.4, which is ≈ 4.6 M probes: past the engine's 4 M cap, so not
merely expensive but **inexpressible**. There is no spacing reduction to pay.

So it was measured against the finest lattice that *is* expressible, `probe_spacing 0.5` /
`normal_bias 0.25` (265 × 31 × 289 = 2.37 M probes), both at `bounce: 0.6`:

| comparison | result |
|---|---|
| whole-frame mean, spacing 1.0 vs 0.5 | 69.497 vs 69.582 |
| mean \|Δluma\| / max | 0.519 / 11.16 of 255 |
| pixels ≥ 4 levels apart | 0.66% |
| **signed Δ (1.0 − 0.5) over the darkest 10% of the frame** | **+0.054 of 255** |
| signed Δ over the darkest 25% | +0.013 |
| signed Δ over the whole frame | −0.086 |

A leak makes *dark* pixels brighter at the coarser spacing. The darkest tenth of the plaza frame is
**0.054 of 255 brighter at spacing 1.0** — a twentieth of one quantization step. The difference that
does exist is diffuse low-amplitude mottle over open ground (the finer lattice resolving the bounce
blobs), with no bright band at any facade.

The reason the rule does not bite is geometric, and it is the condition to re-check if the content
changes: these houses are facade-correct **shells** around a ~1.2 u sooted alcove behind the doorway.
There is no enclosed interior for sky to leak *into*. Author a real interior and this call must be
re-taken.

## `cell_size` — why 0.5 and not its neighbours

| `cell_size` | grid | cells | occupied | frame vs shipped (mean \|Δluma\|) |
|---|---|---|---|---|
| 0.25 | 528 × 60 × 576 | 18.2 M (57% of the 32 M cap) | 2,151,710 | 0.520 |
| **0.5 (shipped)** | **264 × 30 × 288** | **2.28 M (7%)** | **283,234** | — |
| 1.0 | 132 × 15 × 144 | 285 k (0.9%) | 38,721 | 0.814 |

0.25 is expressible but costs ~8× the voxelize for a frame that moves by half a level; 1.0 drops the
grid to one cell per world unit, coarser than the 0.4 u walls it has to represent. 0.5 also sits at
or above every voxel prop's placement scale except the fountain's and the tower's (both 1.0), which
therefore deposit as a lattice rather than a solid — the documented voxel-scale gotcha, accepted here
because both are small props whose occlusion is not load-bearing.

## Load cost — the district's (voxelize, bake) pair

`Plan/gi-wave3-dynamic-injection.md` §6.4 records the escalation trigger as the measured pair
breaching ~500 ms. This is the first district data point, taken by process wall-clock differencing of
`ludion render --out` at 960 × 540 on the pinned RTX 3090 (medians of 9–15 interleaved runs), using
`probe_spacing 8.0` (612 probes ⇒ a bake of essentially nothing) to separate the two halves:

| configuration | median wall clock |
|---|---|
| no `GiVolume` | 1194 ms |
| `GiVolume`, `probe_spacing 8.0` (voxelize only) | 1297 ms |
| `GiVolume`, shipped `probe_spacing 1.0` | 1302 – 1317 ms |
| `GiVolume`, `probe_spacing 0.5` (2.37 M probes) | 1348 – 1372 ms |

**voxelize ≈ 100 ms, bake ≈ 12 ms, pair ≈ 112 ms** — a fifth of the ~500 ms trigger and well inside
the plan's ~200 ms district bake target. The split matches W3's finding that the CPU voxelizer is the
cost centre (it is ~90% of the bill), at a third of `post-full`'s 310–340 ms because the district grid
is 2.28 M cells against that scene's 8.52 M. The bake at the shipped lattice is small enough to sit
close to process-timing noise; it resolves cleanly only at 8× the probes (65 ms at spacing 0.5).
