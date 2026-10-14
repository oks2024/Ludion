# Volumetric medium, wave 4 — the exit gate's two contact sheets

The sitting judges the medium against these two 4×4 crosses. They are **judging instruments, not
goldens**: a `--sweep` cell is deliberately not byte-identical to the same `--set` render (the label
overlay schedules an extra pass), and nothing reads these PNGs at build or test time. Regenerate them
rather than trust the committed bytes — every command line is below.

## The scene: `examples/gallery/post-full.yaml`

Both sheets are swept on the gallery's all-on scene rather than on a vigil district, for two reasons.

- **`render --sweep` cannot drive a district at all.** A district scene names vigil's own components,
  so it renders only through `ludion_engine::render_scene_with_registry` with the game's registry —
  the CLI has the built-in registry and nothing else. A sheet the sitting cannot regenerate from one
  command line is not the instrument this gate asked for.
- **Both axes actually move pixels here.** `post-full` has a real 12→84-unit depth range and a moon
  that rakes *toward* the viewer, so the forward phase lobe points at the camera and `fog_anisotropy`
  is legible. Vigil's fixed follow camera sits 93° off its moon and never sees the sky, so the same
  sheet on a district would be sixteen near-identical thumbnails (measured: the whole phase axis moves
  the district's frame mean by 0.26 of 255, against 1.5 here).

## How a 4×4 cross is built from `--sweep`

`--sweep` takes **one** pointer and lays its values out near-square, so four values render as a 2×2
block, not a row. Each sheet is therefore four `--sweep` invocations — one per value of the second
axis — whose 2×2 blocks are re-cut into their four cells and laid out as one row, then stacked. The
re-cut copies pixels and changes none, and every cell keeps the sweep's own label for the swept axis;
the *rows* are named here.

## `volumetric-medium-wave4-density-shafts.png` — `fog_density` × `shafts` (deleted before publication; in Lore history)

Columns (labelled in each cell): `fog_density` 0.004 / 0.014 (authored) / 0.03 / 0.06.
Rows, top to bottom: `shafts` 0 / 0.02 (authored) / 0.05 / 0.12.

```
ludion render --scene examples/gallery/post-full.yaml --out a-s0.png    --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0    --sweep /entities/0/components/PostProcess/fog_density=0.004,0.014,0.03,0.06
ludion render --scene examples/gallery/post-full.yaml --out a-s0.02.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.02 --sweep /entities/0/components/PostProcess/fog_density=0.004,0.014,0.03,0.06
ludion render --scene examples/gallery/post-full.yaml --out a-s0.05.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.05 --sweep /entities/0/components/PostProcess/fog_density=0.004,0.014,0.03,0.06
ludion render --scene examples/gallery/post-full.yaml --out a-s0.12.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.12 --sweep /entities/0/components/PostProcess/fog_density=0.004,0.014,0.03,0.06
```

What the sheet is for: the merge means one density sets the haze, the beams' reach (3/σ_t) and how
much they are dimmed on the way back, so the two axes are not independent — the question the sitting
answers is whether the authored cell (row 2, column 2) is the right corner of that coupling.

## `volumetric-medium-wave4-albedo-anisotropy.png` — `fog_albedo` × `fog_anisotropy` (deleted before publication; in Lore history)

Columns (labelled in each cell): `fog_albedo` 0.15 / 0.4 / 0.7 / 1.0 (the default, and what the scene
inherits).
Rows, top to bottom: `fog_anisotropy` −0.3 / 0.0 / 0.4 / 0.7 (the default).

**Every cell pins `shafts` at 0.1**, five times the authored 0.02. At the authored intensity the whole
phase axis moves the frame by ±0.5 of 255 and the rows are indistinguishable — the axis cannot be
judged on a sheet that does not show the term it shapes. Albedo is unaffected by the pin (it scales
both in-scatter terms equally), so only the *relative* weight of beams against haze is exaggerated.

```
ludion render --scene examples/gallery/post-full.yaml --out b-g-0.3.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.1 --set /entities/0/components/PostProcess/fog_anisotropy=-0.3 --sweep /entities/0/components/PostProcess/fog_albedo=0.15,0.4,0.7,1.0
ludion render --scene examples/gallery/post-full.yaml --out b-g0.0.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.1 --set /entities/0/components/PostProcess/fog_anisotropy=0.0  --sweep /entities/0/components/PostProcess/fog_albedo=0.15,0.4,0.7,1.0
ludion render --scene examples/gallery/post-full.yaml --out b-g0.4.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.1 --set /entities/0/components/PostProcess/fog_anisotropy=0.4  --sweep /entities/0/components/PostProcess/fog_albedo=0.15,0.4,0.7,1.0
ludion render --scene examples/gallery/post-full.yaml --out b-g0.7.png --width 960 --height 540 --set /entities/0/components/PostProcess/shafts=0.1 --set /entities/0/components/PostProcess/fog_anisotropy=0.7  --sweep /entities/0/components/PostProcess/fog_albedo=0.15,0.4,0.7,1.0
```

What the sheet is for: neither field is authored by any scene in the repo, and both defaults (α = 1.0
purely scattering, g = 0.7 forward-biased) were inherited rather than chosen. Albedo runs left to
right from absorbing smoke to pure scattering; anisotropy runs top to bottom from a medium that
scatters the moon back toward it to one that throws it forward into the lens.
