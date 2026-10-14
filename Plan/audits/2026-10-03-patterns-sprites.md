# Audit: projectile patterns and the sprite engine

**Scope.** Report only, at rev `9d569267`. Covered: `crates/ludion-engine/src/patterns.rs` (the
`kind: pattern` document and its validator, `Volley`, `PatternPlayer`, `play_patterns`,
`advance_volleys`, `volley_hits`, the evaluator), the `Ramp`/`Turn` segments and `Motion` in
`crates/ludion-engine/src/particles.rs`, `extract_volleys` / `bullet_texture` / the sprite placement
(pivot, flip, flipbook) and atlas-frame resolution in `crates/ludion-engine/src/extract.rs`, the
sprite sort and batching in `crates/ludion-renderer/src/render.rs` (`build_sprite_draws`),
`ludion_scene::flipbook_frame`, the `ludion-sprite` crate (`bake`, `pack`, `import`, its clippy
denylist), `GameClock`, `games/shmup-stress` (scenes, `tests/stress.rs`) with
`budgets/pinned-rtx3090-shmup-stress.toml`, vigil's `enemy_volley_hits`, and `docs/patterns.md` /
`docs/sprites.md`. The external games' request folders are not in this repository and were not read.

## Overall assessment

The closed-form design holds up. Bullets are a pure function of `(volley, index, gameplay time)`.
Transcendentals go through `ludion_core::math`, ages come from integer tick differences rather than
accumulation, and spawn order follows ascending entity ids. Volley seeds don't depend on which
sibling entries were due that frame. The only state is two hashed components, so snapshot/restore
has nothing extra to carry. No High finding. The real defects are on the authoring and sprite side:
- timeline entries past `loop` are silently dropped;
- a pattern reload that the validator accepts can make every frame fail to extract;
- a flipped sprite with an off-centre pivot no longer holds its pivot;
- flipbooks at 15, 30 or 60 fps skip frames, because the frame index floors `ticks × (1/60) × fps`
  in floating point.

The shmup stress gate and its budget are sound.

## High

None.

## Medium

### B1 — Timeline entries (and repeats) at or past `loop` never fire, and nothing says so · Medium · [verified]

**Evidence.** `elapsed_frames` folds time with `elapsed % loop` (`crates/ludion-engine/src/patterns.rs:938-944`), so `t` is
always `< loop`. `Walk::consider` fires only when `t == at + repeat × every` (`patterns.rs:978`).
`validate_entry` (`patterns.rs:1247-1357`) and `validate_play` never read `loop_frames`: the only
uses of the field are its declaration and line 877. A played pattern's own `loop:` is also ignored
without a word, because `play` walks the leaf's timeline once, offset by `at` (`patterns.rs:917-919`).

**Why it matters.** This is silent content loss in the authoring path that the error-UX pillar
covers. Writing `at: 240` with `loop: 240` to mean "on the loop boundary" is a natural off-by-one,
as is a repeat train such as `at: 200, repeat: 8, every: 8` that runs past the loop. Either way part
of the pattern disappears, and the doc loads clean with no diagnostic. The pattern tab shows the
same: the doc reports no problem and the stage just plays fewer volleys.

**Failure scenario.** `loop: 240`, entry `at: 120, play: patterns/sweep.yaml`, where `sweep.yaml`'s
entries run to `at: 150`. Every volley after combined frame 239 is dropped, every loop, with no
`L1149`/`L1150`.

**Fix.** In `validate_entry`, when `doc.loop_frames > 0`, raise `L1149` at `/timeline/<i>/at` (or
`/repeat`) if `at + (repeat − 1) × every ≥ loop`, and at `/play` for a played leaf whose last fire
frame plus `at` reaches `loop`. Add a warning, or a doc line, saying that a played pattern's own
`loop` is not honoured.

### B2 — A reload the validator accepts can make every frame fail while a volley of that kind is live · Medium · [verified]

**Evidence.** `validate_bullet_atlases`' comment (`patterns.rs:1127-1129`) says it exists "so a kind
that loads clean cannot fail every frame once a volley of it is on the field". A reload, however,
is accepted with `L1151`/`L1152` reported rather than refused (pinned by
`a_bad_bullet_atlas_on_reload_is_reported_not_refused`, `patterns.rs:2141-2182`).

After that, `extract_volleys` calls `bullet_texture` → `resolve_atlas_frame` for every volley
(`extract.rs:1526-1535`, `1575-1596`). An unresolvable atlas, or an `atlas` with no `frame`
(`extract.rs:2722-2725`), pushes onto the frame's shared `errors`. `extract_inner` then returns
`Err` for the whole scene (`extract.rs:1176-1178`). That error reaches every frame request through
`Engine` (`engine.rs:3082-3090`).

**Why it matters.** The editor's pattern tab writes the file and `asset.reload`s it on **every
committed field** (`Plan/projectile-patterns.md` decision 10). Adding art to a bullet kind is two
commits: `atlas` first, then `frame`. The intermediate state (atlas without frame) is exactly this
error. While the stage's `PatternPlayer` keeps firing that kind, every stage frame fails, not just
the bullets. A path typo has the same effect in a running game until the volleys age out or the doc
is fixed.

**Failure scenario.** In the pattern tab, set `bullets/needle/atlas` to `art/bullets.atlas.yaml`.
The reload succeeds with an `L1151` log, and the next extract fails until `frame` is committed. In
the shmup, one mistyped atlas path on reload blanks the game, not just one bullet.

**Fix.** Degrade per kind instead of per frame. When `bullet_texture` fails, log the diagnostic once
(latched per pattern key and kind) and draw the untextured quad (`FULL_UV`, `None`). Do not push it
onto the extract's hard `errors`. Then correct the `patterns.rs:1127` comment.

### B3 — `flip_x`/`flip_y` ignore the region pivot, so a flipped off-centre-pivot sprite jumps · Medium · [verified]

**Evidence.** `place` offsets the quad centre by the **unflipped** pivot fraction:
`center -= (fx − 0.5)·right + (0.5 − fy)·up` (`extract.rs:2085-2090`). `flip_x`/`flip_y` are applied
later, only to the UV rect (`extract.rs:697-711`). `docs/sprites.md:38-39` promises that "the
region's pivot is the point the entity's translation holds and rotation turns about".

**Why it matters.** The flip mirrors the art about the quad centre, not the pivot. A region with
pivot fraction `fx = 0.25` draws its pivot texel at the entity translation unflipped. Flipped, that
texel lands at `translation + 0.5 × width`. Any facing-flip on asymmetric art (a character with its
feet pivot off-centre, a gun pivoted at the grip) pops sideways by `(1 − 2·fx) × width` on every
turn.

The `sprite_w3.rs` flip test only uses the centre-pivoted region, so it cannot see this.

**Fix.** Mirror the pivot fraction with the flip before the offset (`fx → 1 − fx` under `flip_x`,
`fy → 1 − fy` under `flip_y`). Add a `sprite_w3` case with the corner-pivot fixture flipped,
asserting that the pivot texel stays at the translation.

### B4 — Flipbooks at 15, 30 or 60 fps show the wrong frame on some ticks (skipping frames) · Medium · [verified]

**Evidence.** `elapsed_seconds` returns `ticks × (1/60) × speed` (`extract.rs:3072-3075`).
`flipbook_frame` then takes `floor(elapsed × fps)` (`crates/ludion-scene/src/clip_eval.rs:248`).
`1/60` is inexact, so the product lands just below an integer on some exact boundaries. Reproducing
the same IEEE-754 double arithmetic over 3,600 ticks gives:

| fps | ticks one frame early |
|---|---|
| 8, 12, 20, 24 | none |
| 15 | 35 |
| 30 | 73 (first at tick 62: frame 30 instead of 31) |
| 60 | 150 |

**Why it matters.** The result is deterministic (the same on both architectures), so this is not a
determinism break. It is a frame-timing defect: at 60 fps (one region per tick, common for shmup
explosions and muzzle flashes) about 4 % of frames are never shown, and their predecessor holds two
ticks. At 30 fps some frames get one tick instead of two. It lands on specific ticks, so it reads as
an irregular hitch.

**Fix.** Compute the clip frame from integers where possible: `floor((ticks × speed × fps) / hz)`,
multiplying before the single division (that makes every exact boundary exact). That means passing
ticks and rate into the evaluator instead of pre-multiplied seconds. Add a test that sweeps fps in
{15, 30, 60} over a few hundred ticks against the integer formula.

## Low

### B5 — The sprite sort uses `partial_cmp(..).unwrap_or(Equal)`, the intransitive comparator the particle sort documents as wrong · Low · [verified]

**Evidence.** `build_sprite_draws` uses `.then(da.partial_cmp(&db).unwrap_or(Equal))`
(`crates/ludion-renderer/src/render.rs:1442-1453`). The particle sort beside it uses `total_cmp`, and
its comment (`render.rs:2110-2128`) explains that the fallback is intransitive under NaN and that
`sort_by` may panic on it. The sprite comparator also recomputes `view_depth` (a matrix–vector
product) twice per comparison: about 2 × n log n of them for roughly 3,500 sprites per frame in the
stress scene.

**Why it matters.** No NaN sprite centre is known to be reachable today, because volley inputs are
validated finite. The two sorts in the same file follow different rules, though, and only one of
them is panic-safe.

**Fix.** Precompute `(layer, depth)` keys once into a vector and sort indices with
`layer.cmp().then(depth.total_cmp()).then(index)`.

### B6 — An `arc` with `spread_deg: 360` puts its first and last bullet on the same heading · Low · [verified]

**Evidence.** The offset `-0.5·spread + spread·i/(count−1)` (`patterns.rs:641-647`) spans the closed
interval, so 360° gives −180° and +180° for the first and last bullet.

**Why it matters.** It is the classic duplicate: an author who wants "a full circle aimed at the
player" writes `arc, center: aim, spread_deg: 360` and gets `count − 1` visible directions, with two
bullets stacked (double damage on one line). `ring` is the right shape, but nothing points the
author to it.

**Fix.** Add an `L1149` warning or lint for `arc` with `|spread_deg| ≥ 360` ("use `shape: ring`").
Alternatively, treat a full-turn arc as a ring.

### B7 — `speed_step` can drive a repeat's speed negative, which the validator forbids for authored speeds · Low · [verified]

**Evidence.** `check_speed` covers `speed` and `speed_range` (`patterns.rs:1337-1342`). `speed_step`
is only checked for finiteness (`patterns.rs:1343-1356`). `speed_range_at` adds
`speed_step × repeat` unchecked (`patterns.rs:376-380`). `docs/patterns.md` lists a negative speed as
`L1149`.

**Failure scenario.** `speed: 4, speed_step: -1, repeat: 8` fires repeats 5–7 at −1, −2 and −3 u/s.
Those bullets fly backwards through the shooter, with no diagnostic.

**Fix.** Validate both ends at the last repeat:
`range[k] + speed_step × (repeat − 1) ≥ 0`.

### B8 — `play_patterns` loops over every repeat of every entry, every frame · Low · [verified]

**Evidence.** `Walk::consider` runs `for repeat in 0..entry.repeat` on every frame for every player
(`patterns.rs:975-980`), and `repeat` has no upper bound (`patterns.rs:1319-1325` only rejects 0). A
sustained stream authored as `repeat: 100000, every: 2` costs 100,000 iterations per player per
frame, before anything is due. Vigil's permanent `PatternPlayer`s also walk their finished `loop: 0`
timelines forever.

**Fix.** Make it O(1) per entry. A repeat is due iff `t ≥ at`, `(t − at) % every == 0` and
`(t − at) / every < repeat`. Keep the seed's ordinal semantics by adding `entry.repeat` to the
running ordinal and using `base + r` for the fired repeat.

### B9 — Bullets never orient to their heading · Low · [verified]

**Evidence.** A non-billboard bullet quad always uses the plane's fixed axes:
`(plane_right, plane_up) × size` (`extract.rs:1499`, `1521-1525`). There is no rotation term from
`Motion`'s current heading. The plan's and the doc's showcase kind is a `needle` (`patterns.rs:84-117`,
`docs/patterns.md` Format), which reads as an oriented sprite. Neither the plan nor the doc says that
a bullet keeps a fixed orientation.

**Why it matters.** In a ring of 24 needles, all 24 point the same way. Elongated bullets are a
genre staple, and the external shmup's first boss pattern will hit this.

**Fix.** Either document "bullet quads are axis-aligned in the plane; use round art", or add
`BulletKind.orient: bool` that rotates `right`/`up` by the heading. That could be either a
`Motion::heading_at(t)` beside `position_at`, or a finite difference of the closed form.

### B10 — The tick rate is hard-coded on one side of the age-versus-life comparison · Low · [verified]

**Evidence.** `frames_seconds` converts `life` and segment `frames` with `TickRate::DEFAULT`
(`patterns.rs:598-601`). `seconds_since` measures age with `now.rate()` (`patterns.rs:605-613`).

**Why it matters.** This is latent today, because `Engine` always builds the driver with
`SimConfig::default()` (`engine.rs:625`, `685`). The day a non-60 Hz rate exists, life and segment
boundaries silently stop meaning "frames".

**Fix.** Compare in ticks: alive iff `now_ticks − started < life`. Pass the rate into `bullet_motion`
for the segment durations.

### B11 — Stale docs and comments · Low · [verified]

- `docs/patterns.md` (Rules, third bullet) gives the signature
  `volley_hits(volley, transform, doc, now, plane, centre, radius)`. The function takes the
  `&BulletKind`, not the doc (`patterns.rs:780-788`).
- The header of `budgets/pinned-rtx3090-shmup-stress.toml` still calls "a typed query instead of an
  `entities()` walk" an open item. Both systems already use `query_ids`
  (`patterns.rs:843-847`, `1054`).
- `patterns.rs:1127-1129` (see B2).

## Checked and healthy

- **Determinism.**
  - Angles go through `ludion_core::math::{sin, cos, atan2}` (`patterns.rs:39`, `703`, `950`), and
    the `Turn` closed form uses the same (`particles.rs` `offset_at`/`end_heading`).
  - Age is an integer tick difference converted once, never `t += dt`. `life` and age use the
    identical f64→f32 conversion, so alive iff `ticks < life` exactly.
  - Volley spawn order follows `query_ids` (ascending ids), and `extract_volleys` walks the
    presence cursor in ascending slot order.
  - `random` draws angle then speed from `derived_salted(seed, index, VOLLEY_SALT)`: no per-frame,
    culling-dependent or thread-dependent draw count.
  - The volley seed folds player id, gameplay frame and a walk ordinal that counts every candidate.
  - The `ludion-sprite` crate carries the full denylist and uses `math::sqrt`/`powf`, with a total,
    name-tie-broken pack order and an exact `ceil_sqrt`.
- **Snapshot/restore.**
  - Nothing per bullet or per tick is derived: `Volley` and `PatternPlayer` are the entire state,
    and `GameClock` is in the snapshot (`snapshot.rs:138-139`).
  - `play_patterns` short-circuits on `!game_advanced()`, so a held frame neither re-fires nor
    skips an entry. The `holding` subtraction keeps extract and sim in agreement through a hold.
  - Vigil's `VOLLEY_UNFIRED = u64::MAX` correctly makes `elapsed_frames` return `None`.
- **Kill bitset and bounds.**
  - `is_killed` treats indices ≥ 256 as dead.
  - `all_killed` is bounded by `min(count, 256)`.
  - `count` 0 or above 256, `repeat: 0`, `every: 0` with `repeat > 1`, `life: 0`, a non-positive
    `hit_radius`, a ramp+turn segment and over-four segments are all `L1149`, at
    `pattern/<key>/…` pointers with help.
  - `L1148` and `L1150` have did-you-mean help.
  - One-level nesting and self-play are enforced, and the runtime walk ignores nested `play` even
    if one loaded.
- **Motion kernel.**
  - The near-zero turn rate degrades to a straight line.
  - A zero-frame ramp has no division.
  - The tail slot is always reserved (`BULLET_MAX_SEGMENTS = MOTION_MAX_SEGMENTS − 1`).
  - Heading and speed carry across segments.
- **Atlas UVs.** Edge-exact rects over a gutter-extruded pack (`pack.rs` `compose`). No half-texel
  bleed under nearest or linear filtering, and `uv_scroll` is evaluated, not accumulated.
- **PNG import.** Lossless when the image fits the glyph budget, with a partial-alpha round-trip test
  (`crates/ludion-sprite/tests/import.rs`).
- **Gates.**
  - `games/shmup-stress/tests/stress.rs` asserts the closed-form census per frame, twin-run
    `WorldHash` identity, and A/B frame-by-frame agreement between `steady` and `steady-drawn`.
  - The budget's `extract_ms`, `[passes] scene` and `[systems]` lines are about 2× their medians.
  - An unknown system key in a budget is `L4006`, so a renamed system cannot pass silently.
  - `frame_total_ms = 16.6` is a deliberate fixed deadline (stated in the file), not the
    regression line.
- **Vigil.** `enemy_volley_hits` adopts fresh volleys in ascending order, lifts the origin before the
  first draw, and lands at most one hit per volley per frame on gameplay time.

## Suggested order of work

1. **B2:** degrade a bad bullet texture per kind instead of failing the frame. This unblocks the
   pattern tab's normal two-step art edit.
2. **B3:** mirror the pivot under flip, plus a corner-pivot flip test.
3. **B4:** integer-exact flipbook frame index, plus an fps sweep test.
4. **B1:** validate entries against `loop`.
5. **B7, B6:** the stepped-speed check and the full-turn arc lint, small validator additions in the
   same function as B1.
6. **B5, B8, B10:** comparator, O(1) timeline walk, tick-based age comparison.
7. **B9:** decide orientation (document it, or add `orient`) before the external shmup's first boss
   pattern.
8. **B11:** the doc and comment fixes.
