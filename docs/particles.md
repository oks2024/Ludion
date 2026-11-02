---
description: ParticleEmitter — the analytic particle effect, its fields, firing a burst, the trail idiom
---

# Particles: `ParticleEmitter`

**Scope:** `ParticleEmitter` — a visual effect described by numbers, not entities: a plume, a burst, a
spill of sparks. Registering/defaulting components in general is `ludion docs components`; the other
scene-pass analytic primitive it draws alongside is `ludion docs ground-telegraphs`; HDR colour and
bloom are `ludion docs post-processing`.

## The loop

Author or edit the component in a scene, then look at it without writing a test:

```
ludion render --scene scenes/plume.yaml --frames 30 --out target/particles.png
ludion render --scene scenes/plume.yaml --frames 0,30,60 --out target/particles-strip.png
```

A single `--frames N` ticks the built-in schedule (no game systems run) `N` times and renders one PNG
at that tick — the only way to see a burst mid-flight, since a burst is invisible at tick 0. Two or
more ascending, comma-separated values (`--out` required) stitch a labelled `frame N` contact-sheet
strip, one row, so a burst's whole arc is one image. With no `--frames`, `render` still renders tick
0 exactly as before `ParticleEmitter` existed — an endless emitter (see Rules) is already full there.

## Format

```yaml
ParticleEmitter:
  shape: disc                        # point | sphere | box | disc (default point)
  extent:  # spawn volume, per shape (default [0.5, 0.5, 0.5]; >=0, L2107)
    - 0.3
    - 0.0
    - 0.0
  rate: 35.0                         # particles/second, continuous (default 40.0; >=0, L1014; non-finite L2107)
  burst: 0                           # particles released at once at `started` (default 0)
  started: 0                         # the frame the effect starts at (default 0)
  duration: 0.0                      # seconds of emission after `started` (default 0.0 = endless; >=0, L1014)
  life:  # seconds, uniform [min, max] (default [0.8, 1.4]; L2107)
    - 1.2
    - 2.0
  speed:  # units/s at spawn, uniform [min, max] (default [1.0, 2.5])
    - 0.4
    - 0.9
  spread_deg: 20.0                   # emission-cone half-angle about local +Y (default 30.0, [0,180]: L1014)
  gravity: -1.2                      # world -Y accel, units/s^2 (default 0.0; negative lifts)
  drag: 0.6                          # linear drag, 1/s (default 0.0 = none; >=0, L1014; non-finite L2107)
  floor: -1000.0                     # world Y below which a particle isn't drawn (default -f32::MAX)
  start_size: 0.18                   # disc diameter at age zero (default 0.25; >=0, L1014; non-finite L2107)
  end_size: 0.02                     # disc diameter at end of life (default 0.0 = shrinks away; >=0, L1014; non-finite L2107)
  start_color:  # linear RGBA at age zero, HDR-capable (default opaque white)
    - 3.0
    - 1.4
    - 0.4
    - 1.0
  end_color:  # linear RGBA at end of life (default transparent white)
    - 0.6
    - 0.1
    - 0.0
    - 0.0
  blend: additive                    # alpha | additive (default alpha)
  softness: 0.8                      # 0 = hard circle, 1 = gaussian smudge (default 0.5, [0,1]: L1014)
  lit: false                         # true: colour is albedo x the light at the particle's centre (default false; omitted from text when false)
  seed: 101                          # names this emitter's random stream (default 0)
```

An empty `ParticleEmitter: {}` is a live, endless white plume — every field defaults to something
that draws, the `Sprite`/`GroundMarker` posture. `shape` picks where inside `extent` a particle
spawns: `point` ignores `extent` and releases from the entity's origin; `sphere` releases uniformly
through the volume of a sphere of radius `extent.x`; `box` releases uniformly through an
axis-aligned box of half-extents `extent`; `disc` releases uniformly across a disc of radius
`extent.x` in the local XZ plane. See `ludion docs components` for the built-in-component table's
one-line row form.

## Rules

- **A particle is a pure function of `(emitter, gameplay time)` — never an entity, a pool, or a GPU
  sim.** Every frame recomputes the live set analytically: continuous particle *i* releases at
  `started + i / rate`, burst particle *j* at `started`, and each one's position/size/colour follow
  in closed form from its age. Nothing is spawned or stepped, so a particle is never a `WorldHash`
  term, never a replay record, and never an inspector row — a paused engine renders the same frame
  twice, and a restored snapshot renders what the recorded session rendered. The evaluator reads the
  world's `GameClock` like every other closed-form evaluator, so a held game's particles freeze too
  (`ludion docs game-clock`). The evaluation cost rides inside the existing `extract_ms` measurement
  (`ludion devdocs profiling`); adding an emitter costs no new profile-report section.
- **Particles live in the emitter's `Transform`, not world space.** Every offset anchors at the
  entity's *current* translation, so a moving emitter carries its plume with it. `Transform.rotation`
  orients the emission cone (axis local **+Y**); `gravity` always pulls world −Y regardless of
  rotation. The uniform `Transform.scale.x` multiplies `extent`, both `speed` bounds, and both size
  fields — the `Sprite`/`GroundMarker` "authored × entity scale" convention.
- **The trail idiom: one short-life burst emitter spawned where the thing was.** There is no
  `space: world` option and none is coming — a trail behind a moving object is a fresh burst emitter
  (or a re-stamped `started` on a small `duration`) placed at the object's position each time a
  segment is due, left to finish its own `life` and then reaped by a countdown-`f32` despawn, the
  same idiom a `GroundMarker` warmup uses.
- **`started` is a stamp, not a trigger — write it once, when the choice happens.** A burst releases
  its particles at `started` and is spent one `life`-max later, so firing an effect is a single write
  of the *current gameplay frame* (`ctx.game_frame`, never `ctx.frame`; over the wire, the
  response's `game_frame`) into that field, from a system reading an edge (`was_pressed`, never `is_held`). Re-stamping `started` every frame restarts
  the effect every frame and it never ages past zero — `Animation`'s identical rule. Under a held
  `GameClock` a `ctx.frame` stamp starts late, not wrong: `ludion docs game-clock`. The runnable pattern (a
  key-triggered burst beside an always-on plume): `crates/ludion/examples/particle_burst.rs`.
- **An endless emitter (`duration: 0.0`) is in steady state at every instant, including frame 0.**
  The live window admits negative particle indices — the emitter "has always been running" — so an
  authored ambient plume is full in a gallery golden and in the editor's first frame, with no
  warm-up. `started` therefore matters only to bursts and finite emitters.
- **Blend is `alpha` or `additive`, chosen once, never mixed mid-effect.** `alpha` (default) is
  ordinary coverage blending — smoke, dust, debris, a stack of them settles toward the topmost
  colour. `additive` only ever brightens what's behind it — sparks, embers, glows, a stack of them
  blows out. Colour is linear RGBA and HDR-capable: a channel above `1.0` rides the bloom chain
  exactly like an emissive mesh (`ludion docs post-processing`), which is how urgency or heat reads
  at a glance regardless of blend mode.
- **`floor` clips drawing, it does not stop the particle existing.** A particle whose current Y falls
  below `floor` (default `-f32::MAX`, i.e. no floor) is simply not drawn that frame — cheaper and
  steadier than shortening `life` to keep debris off a ground plane, and a particle whose motion
  carries it back above `floor` reappears.
- **The live-population cap is `PARTICLE_MAX_LIVE_PER_EMITTER` = 4096.** The *entire* live set is
  iterated every extracted frame with no pool to amortize against, so `rate × life[1] + burst` above
  the cap is `L2107` — almost always a mis-typed `rate` rather than an intended effect
  (`ludion explain L2107`).
- `--quality low` thins every emitter to a quarter of its `rate` and `burst` at extract
  (`particle_density`, see the `quality` topic) — a machine's lever, never content.
- **Draw order is fixed: after opaque meshes and sprites, before text.** A scene with no
  `ParticleEmitter` (or one drawing nothing) issues no particle draw at all — byte-identical to the
  pre-particle pipeline.
- **Particles are unlit unless `lit: true`.** Unlit, the authored colour is the look. Lit, a
  particle's colour is its albedo (`start_color`/`end_color`, lerped) times the light reaching its
  centre, evaluated once per particle with exactly the lit sprite's terms: the scene's flat ambient
  (`Environment.ambient`, else the default ambient), every point and spot light in range, and the
  sun, all two-sided. Shadow maps are ignored, and there is no IBL or probe term. Both blends take it
  (additive lit motes add albedo × light). Alpha is untouched.
### Recipe: motes in a lamp pool

Faint additive motes over the whole room with `lit: true` glow only inside a lamp's reach:

```yaml
ParticleEmitter:
  shape: box
  extent:  # spans depth through the lamp's plane
    - 10.0
    - 2.4
    - 8.0
  rate: 220.0
  life:
    - 3.0
    - 5.0
  speed:
    - 0.02
    - 0.12
  start_size: 0.09
  end_size: 0.09
  blend: additive
  lit: true
```

The pool's extent is the light's `range`; a lamp that toggles or moves just works. The darkness
outside needs `Environment.ambient` low (ambient lights every mote equally). See
`examples/gallery/particles-lit.yaml`.

## Gotchas

- A lit particle's light term is `|n·l|` with `n` the camera-facing normal, so a mote in the plane
  through the lamp perpendicular to the view dims toward zero. A mote field lying in the lamp's plane
  under a near-level camera shows a dim lane there: give the field depth or the camera some pitch.
- A lit particle reads the light cell at its centre. A centre off-screen reads the nearest edge tile's
  lights, so a large disc close to the camera can shade slightly differently from its visible part.
- Re-stamping `started` every frame (a per-frame write with no edge check) freezes the effect at age
  zero forever — see the `started`-is-a-stamp rule above.
- An emitter placed on a scaled model entity inherits that scale on `extent`, both `speed` bounds and
  both size fields, silently — an emitter authored against an unscaled reference and then dropped
  onto a `scale: 2.0` entity comes out twice as large and twice as fast.
- Two emitters sharing the same `seed` draw identical per-particle lives, speeds and directions — a
  feature for a rank of identical torches, a bug for a pair of impacts meant to look different from
  each other.
- A particle is never an entity: nothing can collide with it, query it, or read its position from
  game code. An effect a system needs to react to (a burst that should trigger damage) needs its own
  gameplay component alongside the `ParticleEmitter`, driven independently.
- `life`/`speed` are `[min, max]` pairs drawn uniformly; an inverted pair (`min` above `max`) is
  `L2107`, not silently clamped or swapped.

## Next

- `ludion docs components` — declaring components, drift safety, and the built-in visual set.
- `ludion docs screen-ui` — the sibling screen-space draw order (after the scene pass, not in it).
- `ludion docs ground-telegraphs` — `GroundMarker`, the other analytic scene-pass primitive.
- `ludion docs post-processing` — the bloom chain an HDR particle colour feeds.
- `ludion devdocs profiling` — where `extract_ms` and a profile capture come from.
- `ludion docs game-clock` — `GameClock`, and freezing an emitter's live window on demand.
