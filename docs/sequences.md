---
description: Sequences: kind sequence timelines of camera, transform, animation, ui, audio, dialogue, events
---

# Sequences (a keyed timeline, skip, cancel)

**Scope:** `kind: sequence` documents, `sequences::play` with roles bound to entities, the tracks,
`SequenceEvents`, skip and cancel, `holds_gameplay`, waits, the transform track. Clocks are `ludion docs game-clock`; lines are `ludion docs dialogue`; the camera
rig is `ludion docs camera`; UI tweens are `ludion docs ui-tweens`.

## The loop

1. `Engine::builder().sequences()` opts in (add `.dialogue(..)` if a key says a line).
2. Write `sequences/<name>.yaml` under the content root; `build` and `check-scene` validate it (`L1730`–`L1735`).
3. `.sequences()` schedules the update itself, after `path_follow` and above every game system (a dialogue key starts
   the same tick; naming a system `sequence_update` is `L4018`).
4. `sequences::play(world, "sequences/radar.yaml", &[("camera", cam), ("radar", panel)])` returns a `SequenceHandle`.
   `skip`, `cancel` and `release` mark state; the next tick's update applies it, also when a game system called it.
5. Read `SequenceEvents` after the system: `fired(handle, "radar_online")`, `ended(handle)`.

## Format

```yaml
format: 1
kind: sequence
roles:
  - camera
  - radar
length_ticks: 120
clock: game          # game (default; sim when holds_gameplay) | sim
holds_gameplay: false  # true holds the gameplay clock while it plays
skippable: true      # default true
tracks:
  camera:
    - at: 0
      role: camera
      rig:
        focus_offset:
          - 0.0
          - 1.5
          - 0.0
    - at: 120
      role: camera
      restore: true
  transform:
    - at: 5
      role: radar
      over: 20
      ease: smooth
      to:
        anchor: camera
        offset:
          - 0.0
          - 0.0
          - -3.0
        yaw_deg: 180.0
    - at: 40
      role: radar
      over: 30
      path: paths/orbit.path.yaml
      orient: yaw
  animation:
    - at: 10
      role: radar
      clip: spin
      fade: 0.2
  ui:
    - at: 20
      role: radar
      tracks:
        - field: opacity
          from:
            - 0.0
          to:
            - 1.0
          duration: 0.5
  audio:
    - at: 30
      play: sounds/ping.wav
      gain: 0.8
      role: radar
      range: 20.0
    - at: 35
      music_crossfade:
        id: radar
        stems:
          - - music/radar.ogg
            - 1.0
        ticks: 30
    - at: 36
      duck:
        buses:
          - music
        depth_db: -9.0
        attack: 6
        release: 30
  dialogue:
    - at: 40
      line: radar_online
      speaker: radar
  event:
    - at: 60
      name: radar_online
      on_skip: fire
```

| Track | Key fields | Effect |
| --- | --- | --- |
| `camera` | `role`, exactly one of `rig` / `restore: true`, `snap` (false) | Writes `CameraRig.rig`; a `rig` key with no saved rig saves the old one for `restore`; until `restore` the rig holds against camera zones (`ludion docs camera`). A write that differs blends over its `blend_in`. |
| `transform` | `role`, exactly one of `to` / `path`, `over` (0), `ease` (`linear`), `orient` (`none`, path only) | Moves the role's `Transform` over `over` ticks from the pose it had when the key applied. `to`: `anchor?` (a declared role), `offset` (the anchor's frame, else world), `yaw_deg?` (added to the anchor's yaw; absent leaves rotation alone). `path`: a `kind: path` in the role's parent frame. `at + over <= length_ticks`. |
| `animation` | `role`, `clip`, `fade` (0 s) | `crossfade`; `fade: 0` is a hard cut. |
| `ui` | `role`, `tracks` | Writes `UiTween`; each track's `start` is a frame offset after the key. |
| `audio` | exactly one of `play` (+ `gain` 1, `role?`, `range` 10), `music_crossfade` (`id`, `stems` as `[path, gain]`, `ticks` 0), `duck` (`buses`, `depth_db` <= 0, `attack` 0, `release` 0) | `play` is a voice (attached to `role`'s entity, else global); `music_crossfade` is `AudioQueue::music_crossfade`; `duck` is `duck_start` under one key per beat, a later key replaces it. |
| `dialogue` | `line`, `speaker?`, `wait?` (`line_end`) | `dialogue::say`; refused lines warn `L4353` (and set no wait). |
| `event` | `name`, `on_skip` (`fire`), `wait?` (`release`) | `Fired` in `SequenceEvents`. `name` is not empty or `end`. |

Every list is in ascending `at`, each `at` in `0..=length_ticks`; an unknown field is `L1120`. Block-style YAML only: a non-empty `[..]` or `{..}` is `L1005`.

## Clocks

The playhead is `now − started`. `clock: game` reads `ctx.game_frame`: a `GameClock` hold applies nothing and later keys
shift by the held length; `scale(0.5)` doubles the ticks. `clock: sim` runs through a hold. Animation keys always stamp
the gameplay frame. A sequence started on frame F ends `Completed` on F+`length_ticks`, keys at `at == length_ticks`
applying that tick. Order inside a tick: track (camera, transform, animation, ui, audio, dialogue, event), `at`, document order;
players in ascending entity id.

## Holds and waits

`holds_gameplay: true` holds the `GameClock` (the path dialogue's holding lines use) for as long as the beat is
live, except on its final tick, so gameplay resumes two sim frames after the beat ends. It implies `clock: sim`;
an explicit `clock: game` with it is `L1734`. `sequences::holding(world)` reports it.

A key can park the playhead: a dialogue key with `wait: line_end` (until the line it said has ended, however it
ended) or an event key with `wait: release` (the event fires, then the playhead waits for
`sequences::release(world, handle)`). The playhead is `now − started − waited`: while parked nothing applies and the
beat cannot complete, even at `at == length_ticks`; when the wait clears, later keys apply from the next clock frame,
so a longer voice line shifts everything after it. Keys at the wait's own `at`, on any track, apply that tick. One
wait at a time: two at one `at` is `L1735`. `release` is one latch, not a counter: called before the wait is
reached, it clears the next one, so an early release cannot stall the beat; it returns `false` for a dead handle.
`skip` and `cancel` ignore waits, and the hold ends with the beat.

## Bound actors

While a `holds_gameplay` beat plays, the entities bound to its roles and every `Parent` descendant of one (found when
the beat starts; a child spawned later is not bound) get a `SequenceBound` and run on their own gameplay instant,
`bound_game + (sim − bound_sim)`, in Q16 frames. Their animation poses, clip events, blend clocks, camera spring and
`PathFollower` keep moving through the hold; every unbound entity and every `ctx.game_*` stays frozen. A game system
reads a bound entity's time with `sequences::entity_clock(world, id, ctx)`, which returns the global `ctx.game_*`
values for an unbound one.

The bind is released on the tick **after** the beat ends (the two-frame rule), and a bound entity another holding
beat binds is handed to it instead. On release every gameplay stamp that reads the entity clock (`Animation`,
`AnimationLayers`, `BlendClock`, `CameraRig` blend) shifts back by the lead, so the actor does not jump. Stamps are
unsigned: a clip stamped inside a hold that outlasts the game's age saturates at frame 0. Particles, volleys,
patterns, ai and `AnimGraphState` follow the global clock. A checkpoint restore mid-bind rebases the global clock
but not `SequenceBound`.

## Preview

`ludion render --scene S --sequence sequences/radar.yaml --bind camera=cam --bind radar=panel --at 30 --out p.png`
plays the beat in the bare host and renders the world right after the update whose playhead is `--at` (`--at T` steps
`T + 1` ticks; default 0). A `--bind` value is `role=entity`, the entity found by its `Name`. The document must lie
under the scene's folder. It runs only the built-in schedule: no game systems, no dialogue (a dialogue key degrades,
`L4353`), and every `wait: release` clears as it is reached. The summary line names the key, the tick and a beat that
ended; `L4352`/`L4353` print as warnings. `L4355`: the doc is outside the folder, unreadable or not `kind: sequence`;
`L4356`: a bind name matches no entity or several. `--frames`, `--view` and the dumps conflict with it.
The gallery's `sequence-beat` scene is the worked example: `ludion render --scene examples/gallery/sequence-beat.yaml --sequence examples/gallery/sequences/demo.yaml --bind cam=camera --bind hero=hero --bind crate=crate --bind drone=drone --at 45 --out beat.png`.

## Trace

`ludion_testkit::sequence::sequence_trace(&mut engine, ticks) -> Vec<String>` steps the engine and returns one
line per key applied, event fired, end, wait and resume, per tick in event order then waits by player:
`"{tick:04} {player} key {track} at {at}"`, `fire {name}`, `end completed|skipped|cancelled`,
`wait line_end|release at {at}`, `resume at {at}`, e.g. `0050 e7v0 wait line_end at 50`. Two calls
concatenate to one, also across a snapshot restore. An engine without `.sequences()` returns nothing.

## Skip and cancel

`skip` returns `false` when the handle is dead or the doc says `skippable: false`. Otherwise, per role, the last
camera, transform, animation and ui key not yet applied takes its end form (camera: write plus a snap; transform:
the span's end pose, after every live span writes its end; animation: hard cut; ui: `once` tracks jump to `to`), the last unplayed `music_crossfade` applies, other unplayed audio and dialogue keys are dropped, its voices fade out over 6 ticks and its duck releases, its lines are interrupted, `on_skip: fire`
events fire in order, then `Ended { Skipped }`. A skipped and a watched beat leave the same rig, pose, clip and tween.
`cancel` applies no end state, stops its voices (as a skip does), interrupts its lines and ends `Cancelled`; a completed beat lets its voices play out. Every end releases the beat's duck; so does a document unloaded mid-play.

## Events

`SequenceEvents` is rewritten each run: `KeyApplied { handle, track, at }` (the trace; a skip emits it per end form),
`Fired { handle, name }`, `Ended { handle, end }`. The handle is dead from `Ended` on.

## Diagnostics

| Code | Meaning |
| --- | --- |
| `L1730` | Structural: length 0, `at` past the length or out of order, camera key without exactly one of `rig`/`restore`, audio key without exactly one of `play`/`music_crossfade`/`duck` or a bad stem, bus list or `depth_db`, transform key without exactly one of `to`/`path` or ending past the length, non-finite number |
| `L1731` | Unknown, duplicate or missing role (a transform `anchor` too) |
| `L1732` | Empty or reserved event name |
| `L1733` | Dialogue line unknown, or no `.dialogue()` |
| `L1734` | `holds_gameplay: true` with an explicit `clock: game` |
| `L1735` | Two `wait` keys at one `at` (dialogue and event tracks together) |
| `L4350` / `L4351` | `play`: unknown document / role undeclared or bound twice |
| `L4352` | Role unbound or its entity dead (warning, once) |
| `L4353` | Key degraded: missing `CameraRig`/`Animation`/`Transform`, a `path` that does not load, `say` refused, doc gone (warning, once) |
| `L4354` | Called without `.sequences()` |
| `L4355` / `L4356` | Preview: doc outside the scene folder or not a sequence / `--bind` name matches no entity or several |

## Gotchas

- State is entities (`SequencePlayer`, one `SequenceBinding` per role), so snapshots and checkpoints keep a beat.
  Entity refs in a list are refused (`L1392`), hence one binding entity per role.
- `play`, `skip`, `cancel` and `release` from a game system land at the next tick's update; called between ticks,
  on the next tick.
- An opted-in game that never plays hashes as if it had not opted in.
- A transform target is fixed when its key applies: a moving anchor is not tracked. A second key on a role starts
  from its current pose. Only a `kinematic` body follows `Transform`; a dynamic body is the solver's. Scale is never
  written.
- A `restore` inside a camera zone hands the rig back to the zone system, not to the saved rig; a `music_crossfade`
  is never undone. A checkpoint restore or world rebuild mid-beat forgets the voice ids (they play out, a later skip
  cannot stop them) and drops the duck.
- A game system that writes a bound actor's `Animation` during the hold stamps from
  `sequences::entity_clock(world, id, ctx).frame`, not `ctx.game_frame`.

## Next

- `ludion docs dialogue` — lines, speakers, holds.
- `ludion docs game-clock` — holds and time scale.
- `ludion docs camera`, `ludion docs ui-tweens` — the rig and tween a key writes.
- `examples/gallery/sequence-beat.yaml` — a beat to preview and trace.
