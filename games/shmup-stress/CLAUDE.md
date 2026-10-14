# shmup-stress

Not a game — a **measuring instrument**. Plans of record: `Plan/bullet-hell-stress.md` (the arena),
`Plan/projectile-patterns.md` Wave 2 (the volley migration) and `Plan/sprite-engine.md` Wave 4 (the
drawn scene). It asks one question: can the engine as shipped carry a Cave-class bullet hell
(~3,000 live bullets, one tiny player hitbox), and where does the cost land — ECS churn,
`spatial::rebuild`, or sprite extract? Vigil's proven ceilings are 300 projectiles / 500 enemies.

**Start at `src/lib.rs`: `game()`'s doc comment is the authoritative system order + rationale.**
There is no RNG anywhere: every bullet's existence, position, atlas region and blend is a closed-form
function of the frame index, so each scene's live census is a number a test asserts rather than one a
run produces.

## Two shapes, one census

Every scene holds the same field; what differs is **what a bullet is**.

- **Volleys** (`steady`, `burst`) — one hashed `Volley` entity per fired ring, each bullet a pure
  function of `(volley, index, now)` evaluated by `bullet_is_alive` / `bullet_position`. The engine
  ships `play_patterns`, `advance_volleys` and `volley_hits`; the pattern documents live in
  `scenes/patterns/` and are named by their content-root-relative key (`patterns/steady.yaml`).
- **Entities** (`steady-drawn`, `steady-indexed`) — one `Transform` + `Sprite` + `Bullet` per bullet,
  laid down by `Emitter` and integrated by `move_bullets`, exactly as this crate shipped in Wave 1.

`cancel_bullets` and `player_hit_test` see both shapes, so a cancel and a hit cost what they cost on
whichever shape a scene is made of. The game runs on `Plane::Xy` — headings, sprite quads and the
spatial index all read it, so it is a property of the arena, not of a scene.

## The four scenes

Each varies exactly one thing against its twin, with the closed form in its header:

- `steady.yaml` — **volleys**, the standing field. Four `PatternPlayer`s at `started` 0/1/2/3 play
  `patterns/steady.yaml` (a ring of 25 every 4 frames, `life: 119`).
  `live(N) = 25 · min(N + 1, 119)` → **2,975 live bullets in 119 volley entities**, first full at the
  end of frame 118. The A side.
- `steady-drawn.yaml` — **entities**, the baseline and the B side: the same census, art and framing
  made of one entity per bullet. A `BulletArt` singleton puts every bullet on one of 40 regions of
  `scenes/sprites/shmup.atlas.yaml`, 3 of every 25-bullet volley additive (357 live), behind a
  480-cell `Tilemap` a system scrolls. Its report against `steady`'s **is** the price of the
  entity-per-bullet shape.
- `burst.yaml` — **volleys** at `count: 50` plus `Arena.cancel_every: 60`: the whole field is
  despawned in one pass every 60 frames and rebuilt over the 59 after. Sawtooth
  `live(N) = 50 · (N % 60)` past frame 60, zero on every cancel frame, end-of-frame peak 2,950. The
  cancel destroys 3,000 bullets held in **60 entities** — the entity shape would destroy 3,000.
- `steady-indexed.yaml` — **entities**, `steady-drawn` + `Arena.bullets_collide: true`: every bullet
  carries a dynamic circle `Collider` and enters the spatial index. Same census. Its
  `spatial_rebuild` line minus `steady-drawn`'s **is** the price of indexing ~3,000 bullets. Volleys
  never enter the index by construction, which is the plan's decision, not a number.

Only `steady` draws bullets from a single atlas region: a region lookup is per *volley* at extract,
not per bullet, so variety there would cost nothing and prove nothing. Its field is all-alpha; the
additive share is an entity-side closed form (`BulletArt.additive_every`) with no volley counterpart.

The atlas under `scenes/sprites/` is generated content: `bullets.sprite.yaml` and
`backdrop.sprite.yaml` come from the `.gen.mjs` beside each, and the committed `shmup.png` +
`shmup.atlas.yaml` pair comes from `ludion generate-atlas` over `shmup.atlas-source.yaml` — never
hand-edited, and `node scripts/atlas-drift.mjs` gates the byte-identity.

## Measuring

Per scene, from the workspace root:

```
cargo run --release -p shmup-stress -- --scene games/shmup-stress/scenes/steady.yaml --profile 120
```

All four reports at once (the private CI dumps exactly these as artifacts):

```
cargo test -p shmup-stress --release --test profile -- --ignored --nocapture
```

**Two budgets are calibrated**, both at the workspace root beside the engine's own files and both on
the pinned RTX 3090: `budgets/pinned-rtx3090-shmup-stress.toml` over `steady` (the volley shape) and
`budgets/pinned-rtx3090-sprite-stress.toml` over `steady-drawn` (the entity baseline). Check either
in process — the CLI's `ludion profile` links none of the systems that build the field:

```
cargo run --release -p shmup-stress -- --scene games/shmup-stress/scenes/steady.yaml --profile 200 --profile-check budgets/pinned-rtx3090-shmup-stress.toml
```

The other two scenes stay deliberately ungated: `Plan/bullet-hell-stress.md` holds their sim numbers
as a measurement, not a tripwire. There is no macOS twin of either budget and no `budgets/` directory
in this crate. Numbers are read off the reference box, never off whichever machine happened to run
the suite.
