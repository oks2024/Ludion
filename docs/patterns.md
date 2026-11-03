---
description: the kind: pattern document, Volley/PatternPlayer, and the engine/game boundary
---

# Projectile patterns: `kind: pattern`

**Scope:** Authoring closed-form bullet volleys as a `kind: pattern` document, and the two
components that fire and hold them (`Volley`, `PatternPlayer`). The doc envelope (`format:`/`kind:`,
registering a kind, hot reload) is `ludion docs data-docs`; the FX-only sibling with no gameplay hit
test is `ludion docs particles`; which two world axes a bullet's heading lives in is
`ludion docs collision`.

A gameplay bullet is not an entity — it is index `i` of a fired `Volley`, a pure function of
`(volley, bullet kind, i, gameplay time)`. The same closed form the sim's hit test evaluates is what
draws the bullet, so a swarm of thousands costs a few hundred hashed bytes per volley and cannot
drift between hit and drawn.

## The loop

Author the doc, opt the game in, place a `PatternPlayer`, then watch a volley stand up:

```
cargo run -- --scene scenes/boss.yaml --render-out target/pattern-120.png --frames 120
```

The game binary plays the volleys because its schedule places `play_patterns`; `ludion render` links
no game code and plays none (`ludion docs running`).

A `PatternPlayer` fires nothing until its `pattern` names a loaded doc, so the doc must live under
the content root and the game must call `.patterns()` on the builder — the doc kind is otherwise
unregistered and the file is inert data. Editing the doc while the game runs re-shapes every bullet
already in flight (Gotchas).

## Format

The envelope is the shared data-doc one (`ludion docs data-docs`): `format: 1`, `kind: pattern`, then
the pattern's own fields. **Write every collection in block style — never `{ }` or `[ ]` inline —
the strict YAML reader refuses a non-empty flow collection as `L1005`, and the content walk that discovers `kind:` documents reports that file instead of loading it.**

```yaml
format: 1
kind: pattern
name: boss-phase-1
bullets:
  needle:
    atlas: art/bullets.atlas.yaml
    frame: needle
    size:
      - 0.3
      - 0.3
    color:
      - 1.0
      - 0.4
      - 0.8
      - 1.0
    blend: additive
    hit_radius: 0.12
    life: 240
    motion:
      - ramp:
          frames: 30
          from: 6.0
          to: 2.0
      - turn:
          frames: 90
          rate_deg: 45.0
timeline:
  - at: 0
    repeat: 8
    every: 12
    shape: ring
    count: 24
    bullet: needle
    speed: 4.0
    angle_step_deg: 7.5
  - at: 60
    shape: arc
    center: aim
    spread_deg: 40.0
    count: 5
    bullet: needle
    speed: 6.0
  - at: 120
    play: patterns/sweep-left.yaml
loop: 240
```

**`bullets:`** — one `BulletKind` per name a timeline entry may fire under `bullet:`:
- `atlas` + `frame` — a `kind: atlas` document path and one of its regions, both or neither
  (`L1151`, `Sprite`'s own rule); omit both for an untextured quad tinted by `color`.
- `size` (default `[0.25, 0.25]`), `color` (default opaque white), `blend` (`alpha`\|`additive`,
  default `alpha`), `layer` (default `0`), `billboard` (default `false` — the quad lies in the
  game's `Plane` unless set).
- `hit_radius` (default `0.1`, world units) — added to whatever radius a `volley_hits` caller asks
  about.
- `life` (default `240` frames) — how long a bullet of this kind survives after its volley fires.
- `motion` — at most **four** phases, in order (default empty = straight forever).

**A motion segment** is exactly one of:
- `ramp { frames, from?, to }` — straight, speed sweeping linearly to `to`.
- `turn { frames, speed?, rate_deg }` — a circular arc at constant speed, heading turning `rate_deg`
  degrees per second (a positive rate always turns toward increasing angle).

Both, or neither, on one segment is `L1149` — a ramping turn has no elementary closed form. An
omitted `from`/`speed` carries the speed already in force: the volley's fire speed for the first
segment, the previous segment's end speed after. Past the authored segments the bullet continues
**straight at its final speed** until `life`.

**`timeline:`** — each entry is a volley or a `play:` of another pattern's timeline, never both:
- `at` — the fire frame, counted from the `PatternPlayer`'s `started` (and, inside a played pattern,
  offset again by the playing entry's own `at`).
- `play` — another doc's `DocStore` key (content-root-relative path). Nesting reaches exactly one
  level: a played pattern's own `play` entries are not followed (`L1150`).
- `bullet` — which of this document's `bullets:` fires (`L1148`, did-you-mean, when unknown).
- `repeat` (default `1`) / `every` (default `0`, frames between repeats — `0` with `repeat > 1` is
  `L1149`).
- `shape` — `ring` (count evenly over the full turn), `arc { center, spread_deg }` (count evenly
  over the spread, one bullet at the centre), or `random { spread_deg }` (each bullet's angle and
  speed independently drawn from a stream named by its own index).
- `center` — `absolute` (the authored `angle_deg`, default) or `aim` (the direction from the
  player to `PatternPlayer.target`, captured at fire time, plus `angle_deg`).
- `count` (default `1`, at most **256** — the `Volley` kill bitset's width, `L1149`/`L2108`).
- `speed` (units/s) or `speed_range: [min, max]`, which **replaces** `speed`: bullet `i` of `count`
  fires at `lerp(min, max, i / (count − 1))`, a single bullet at `min`.
- `angle_deg`, `angle_step_deg` (added once per repeat — the spiral), `speed_step` (added to both
  ends of the range once per repeat).

`loop:` (top level, default `0` = play once) is how many frames the whole timeline spans before
repeating — a boss phase. Each entry's last fire (`at + (repeat−1)×every`), and a played leaf's last
fire plus the `play`'s `at`, must land before `loop` (`L1149`: the timeline wraps first). A played
pattern's own `loop` is ignored (`L1265`). A phase change is the game writing a different `pattern`
onto the player.

The editor's Effects view has a **pattern tab** — a schema-driven form over one of these files that
writes the file on every committed field and then `asset.reload`s it, which is exactly the two steps
you take by hand (the file is the pattern's only truth; there is no wire method that saves one).

## Rules

- **Frames time it, units per second speed it.** Every `at`, `every`, `loop`, segment `frames` and
  bullet `life` is an exact integer tick (60 Hz by default); speeds are always units per second.
- **The plane decides where a heading points.** `Engine::builder().plane(Plane::Xy)` puts angle `0`
  along world X and `90°` along world Y — the plane `Sprite`/`Collider` already use for a top-down
  or side-on field; the default `Plane::Xz` is the ground plane (`ludion docs collision`). A pattern
  authored against one plane fires sideways on the other.
- **The engine ships the vocabulary; the game owns every rule.** `play_patterns` (stands volleys up
  on schedule) and `advance_volleys` (reaps a volley whose bullets are all dead or past `life`) are
  `pub fn`s a game registers in its own schedule order — the `spatial_rebuild` precedent
  (`ludion docs collision`). A world with a `PatternPlayer` or `Volley` where either fn never ran warns `L1394`; calling them from your own system is silent. `volley_hits(volley, origin, kind, now, plane, centre, radius) ->
  impl Iterator<Item = u32>` is the pure overlap query a hit system calls; setting a kill bit,
  scoring, damage and sides are entirely game code. **Cancel is a despawn over a `Volley` query** —
  what appears in a cancelled bullet's place (score items, an FX burst) is the game's own spawn at
  that frame:

  ```rust
  use ludion::prelude::*;

  fn cancel_into_items(world: &mut World, ctx: &SystemCtx, cancelled: &[EntityId]) {
      let plane = Plane::of(world);
      for &id in cancelled {
          let volley = world.component::<Volley>(id).unwrap().clone();
          let origin = *world.component::<Transform>(id).unwrap();
          let doc = world.resource::<DocStore<PatternDoc>>().unwrap().get(&volley.pattern).unwrap();
          let kind = doc.bullets[&volley.bullet].clone();
          for index in 0..volley.count {
              if bullet_is_alive(&volley, &kind, index, ctx.game_time) {
                  let at = bullet_position(&volley, &origin, &kind, index, ctx.game_time, plane);
                  // spawn a score item / an FX burst at `at` ([f32; 3], no `glam` needed)
              }
          }
          world.despawn(id).unwrap();
      }
  }
  ```
- **Every evaluator reads gameplay time, and `started` is a gameplay frame.** `bullet_age`,
  `bullet_is_alive`, `bullet_position` and `volley_hits` take `ctx.game_time`, never `ctx.time`;
  `started` (on `PatternPlayer` and on a fired `Volley`) comes from `ctx.game_frame`, never `ctx.frame`. A game holding its `GameClock` freezes the whole field:
  `play_patterns` returns on `!ctx.game_advanced()`, so a hold neither re-fires a timeline entry nor
  skips one whose frame passed while frozen (`ludion docs game-clock`).
- **Opt in through `.patterns()`.** `Engine::builder().patterns()` registers `PatternDoc` under
  `kind: pattern` and its validator — the same `doc_kind`/`validate_docs` mechanism any game data-doc
  uses (`ludion docs data-docs`). `ludion check-scene` registers the kind and validates pattern docs, but no CLI host
  (`serve`, `sim-hash`, `render --frames`) schedules either system or plays a volley: only a game
  binary does. `pattern:`/`play:` name documents by
  their `DocStore` key: a content-root-relative path, slash-normalized.
- **The codes.** `L1148` — a timeline entry's `bullet` names no kind in the document's own
  `bullets:` (did-you-mean). `L1149` — a bound the schema cannot express: over four motion segments,
  a segment with both or neither of `ramp`/`turn`, `count`/`repeat` of zero or `count` above 256,
  `every: 0` with `repeat > 1`, a `play` that also authors volley fields, a non-finite or negative
  speed or angle, a zero `life` or `hit_radius`, a fire at or past `loop`, a `speed_step` that
  drives a repeat's speed negative, an `angle_step_deg` that overflows. A bullet kind's `color` is
  `L1263`: RGB `>=0`, alpha `0..=1`. `L1150` — a `play` that does not resolve: an
  unloaded key, the document itself, or a document whose own timeline holds a `play`. `L2108` — a
  `Volley`/`PatternPlayer` that cannot be evaluated as authored (a `count` past the bitset, a
  non-finite angle/speed built in Rust, or a `center: aim` volley whose target is missing or dead);
  a **warning** — the sim degrades once per offender and keeps running, never a crash.
  `L1265` — a **warning**: a full-turn `arc` (it overlaps itself; use `shape: ring`) or a played
  pattern's `loop`. A bullet's `atlas`/`frame` is resolved at load like a `Sprite`'s
  (`L1151`/`L1152`). A reload that makes the validator report a new error — in the reloaded
  document or in another that plays it — is refused, the old doc kept; an unchanged reference that
  broke under a live doc is reported once and the kind draws untextured.

## Gotchas

- **A doc edit changes bullets already in flight.** A `Volley` stores its document and bullet-kind
  *names*, not their segments, so `asset.reload` of the doc re-shapes every live bullet fired from it
  on the very next frame — surprising mid-swarm, unlike a scene edit.
- **Bullet quads never turn to their heading.** They keep the plane's axes (or face the camera when
  `billboard`), so use round art.
- **The degenerate pattern.** A timeline firing `count: 1` every frame is entity-per-bullet again;
  a `Volley` only pays off when it holds more than a couple of bullets across more than a couple of frames.
- **`random`'s draw order is angle then speed**, per bullet index — reordering it would re-roll every
  random volley in the repo; treat the two as inseparable, not two independent knobs.

## Next

- `ludion docs particles` — `ParticleEmitter`, the FX-only sibling with no hit test.
- `ludion docs data-docs` — the `format`/`kind` envelope, registering a kind, hot reload.
- `ludion docs collision` — `Plane`, the spatial queries a hit system builds on.
- `ludion docs components` — the built-in component table, including `Volley` and `PatternPlayer`.
- `ludion docs game-clock` — `GameClock`, and freezing a whole field of bullets on demand.
