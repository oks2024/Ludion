---
description: Driving a Sprite's frame with an atlas flipbook and Animation, and scrolling or tiling a band
---

# Sprite animation

**Scope:** playing one of an atlas's `flipbooks:` on a `Sprite` — the same `Animation` component that
poses a voxel assembly's clips (`ludion docs voxel-animation`), read against a `Sprite` instead of a
`Mesh`. Separately, the `Sprite` fields `uv_repeat`/`uv_scroll` tile and scroll a band. Declaring the flipbook itself is `ludion docs sprite-sheets`; authoring the frames it cycles
is `ludion docs sprite-format`; the component's full field table is `ludion docs components`;
the hub over the sprite layer is `ludion docs sprites`.

## The loop

1. **Declare the flipbook** on the `kind: atlas-source` document, beside the inputs it draws frames
   from (`ludion docs sprite-sheets`), and bake it:
   ```
   ludion generate-atlas sprites/gallery.atlas-source.yaml
   ```
2. **Look at the clip** before wiring any code — three instruments, CPU-only, over the committed
   `kind: atlas`:
   ```
   ludion render --atlas sprites/gallery.atlas.yaml --clip ship/bank --time 0.25 --scale 8
   ludion render --atlas sprites/gallery.atlas.yaml --clip ship/bank --strip 6 --scale 8
   ludion render --atlas sprites/gallery.atlas.yaml --clip explosion/burst --onion --scale 8
   ```
   **Always pass `--scale`.** Under `--atlas --clip` it is whole sheet pixels per texel and it
   defaults to `1`, so a 16-texel frame comes out 16 px wide and nothing in it is legible; `8` is a
   good first look. (The plain `--atlas` packing overview derives its own upscale and ignores it.)
3. **Place the `Animation`** on the same entity as the `Sprite`, naming the flipbook:
   ```yaml
   Sprite:
     atlas: sprites/gallery.atlas.yaml
     frame: ship/idle          # the rest frame — shown until an Animation names a bound clip
   Animation:
     clip: ship/bank
   ```
4. **Drive `clip`/`started` from a game system, only when the choice changes** — the same discipline
   `ludion docs voxel-animation` states for a `Mesh`: re-stamping `started` every frame restarts the
   clip every frame and churns the `WorldHash` for a `String` that did not change.
5. **Land a mid-clip frame on a golden** by ticking before extracting (`ludion render --frames N`, or
   a suite's `goldens/manifest.json` `frames` override — `ludion docs verify-loop`); an un-ticked
   golden extracts every clip at its first frame.

## Format

A flipbook is declared on the `kind: atlas-source`/`kind: atlas` documents, not on `Animation` —
`ludion docs sprite-sheets` owns the `flipbooks:` key. The fields that drive playback:

| Key | Rule |
|---|---|
| `name` | unique within the document — what `Animation.clip` names |
| `frames` | non-empty region names, in show order |
| `fps` | finite, `> 0` |
| `loop` | default `true`; `false` is a one-shot that clamps to its last frame and holds it |
| `hold` | optional, one integer `>= 1` per entry of `frames`; absent means one clip frame each |

**The timing arithmetic.** A flipbook's length in **clip frames** is the sum of `hold` (or one per
`frames` entry with no `hold`); its length in **seconds** is that count divided by `fps`. At an
instant `ticks` clip ticks into playback (the game counts them: each played tick adds the `Animation`'s
`speed`, continued across `set_clip_speed`), the clip frame showing is `floor(ticks × fps / rate)` with
the division last — counted from zero on every call, never accumulated, so two hosts reaching the
same tick by different routes show the same frame, and 15, 30 and 60 fps clips at 60 Hz turn over
exactly on their tick — then wrapped into `[0, total)` for a looping clip or clamped to `total - 1` for
a one-shot. `hold` turns that clip frame into a frame index by walking the cumulative counts. The
`--time` instrument takes seconds and reads `floor(seconds × fps)`.

Worked example: `fps: 10.0`, three `frames` `a`/`b`/`c`, `hold` `1`/`3`/`1`. Total clip frames
`1 + 3 + 1 = 5`, duration `5 / 10 = 0.5` s. `a` shows for clip frame `0` (`t` in `[0.0, 0.1)`), `b` for
clip frames `1`–`3` (`t` in `[0.1, 0.4)`), `c` for clip frame `4` (`t` in `[0.4, 0.5)`); past `0.5` s a
one-shot holds `c`, a loop wraps back to `a`.

## Scrolling and tiling

`uv_repeat` repeats the frame across the quad; `uv_scroll` slides it, in UV units per second (`1.0` is one frame width). An
endless parallax band is then one entity — no wrap point, no second copy. Either field off its
default wraps the quad's region instead of clamping it, and the offset is derived from sim time at
extract, so nothing enters the world hash. A linear filter blends the column where a tile restarts
toward the atlas neighbour: pad the frame, or pack it alone.

```yaml
Sprite:
  atlas: sprites/sky.atlas.yaml
  frame: band/zodiac
  uv_repeat:
    - 8.0
    - 1.0
  uv_scroll:
    - 0.05
    - 0.0
```

## Rules

- **`Animation.clip` resolves against whatever the entity renders.** On a `Mesh` whose `shape` is an
  assembly it names an entry of that assembly's `clips:`; on a `Sprite` drawn from an atlas (`atlas` +
  `frame` both set) it names one of that atlas's `flipbooks:`. An entity carrying both reads the same
  name on each.
- **`frame` stays required beside `atlas`.** It is the rest frame: what shows with no `Animation`, an
  empty `clip`, or a clip name the atlas binds no flipbook to — and what a `texture:` sprite (no atlas
  at all) always shows, `Animation` or not.
- **An unbound clip name is a silent degrade, never a diagnostic** — no `Animation`, an empty `clip`,
  a name the atlas has no flipbook for, or a `texture:` sprite all collapse to the authored `frame`.
  The name is chosen by game logic no document-time check can see, and a complaint would fire sixty
  times a second.
- **Size derives from the frame currently showing**, not the authored one, unless `size:` is
  authored: a flipbook whose regions differ in extent changes the drawn quad's size as it plays, the
  same as its UV rect.
- **Playback is visual-only**, exactly as for a `Mesh` clip: the pose is a pure function of `(clip,
  gameplay time)` evaluated at extract, written back nowhere — the evaluator reads the world's
  `GameClock`, so a held game's animation freezes too (`ludion docs game-clock`). An animated run's
  `WorldHash` trace and its recorded replay are byte-identical to a run that never extracts a frame.

## Gotchas

- **Restarting a one-shot needs a fresh `started`, not a re-write of `clip`.** Writing the same clip
  name again with `started` unchanged is a no-op — the choice did not change. To play the strike
  again, stamp `started` to the current gameplay frame (`ctx.game_frame`, never `ctx.frame`) even
  though `clip` stays the same (a game's `trigger_attack` does
  this unconditionally on every committed shot). Under a held `GameClock` a `ctx.frame` stamp starts
  late, not wrong: `ludion docs game-clock`.
- **`speed: 0.0` freezes on the first frame**, not the frame showing when it was set — elapsed time is
  always measured from `started`, so a speed dropped to zero mid-clip jumps back to frame zero.
  `set_clip_speed(&mut animation, 0.0, ctx.game_time)` freezes on the current frame instead; a raw write
  still jumps.
- **A `started` frame in the future shows the first frame**, never a negative or wrapped time.
- **`hold` counts clip frames, not seconds** — divide by `fps` to read one in real time.
- **A golden of a mid-clip needs the tick count recorded somewhere.** The scene's own `--frames N`
  render is one call; a directory suite needs the stem's `frames` key in `goldens/manifest.json`
  (`ludion docs verify-loop`), or the golden re-blesses at tick 0 and looks static forever.

## Next

- `ludion docs sprites` — the hub: the sprite layer's documents, verbs and components at a glance.
- `ludion docs sprite-sheets` — declaring `flipbooks:` on an atlas source, and the `atlas-drift` gate.
- `ludion docs sprite-format` — the `kind: sprite` frames a flipbook cycles through.
- `ludion docs voxel-animation` — the same `Animation` component over a voxel assembly's clips.
- `ludion docs components` — `Sprite` and `Animation`'s full field tables.
- `ludion docs verify-loop` — ticking before extracting, and the golden manifest's overrides.
- `ludion docs game-clock` — `GameClock`, and freezing playback on demand.
