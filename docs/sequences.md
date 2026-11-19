---
description: Sequences: kind sequence timelines of camera, animation, ui, audio, dialogue, event keys; skip
---

# Sequences (a keyed timeline, skip, cancel)

**Scope:** `kind: sequence` documents, `sequences::play` with roles bound to entities, the six W1 tracks,
`SequenceEvents`, skip and cancel. Clocks are `ludion docs game-clock`; lines are `ludion docs dialogue`; the camera
rig is `ludion docs camera`; UI tweens are `ludion docs ui-tweens`.

## The loop

1. `Engine::builder().sequences()` opts in (add `.dialogue(..)` if a key says a line).
2. Write `sequences/<name>.yaml` under the content root; `build` and `check-scene` validate it (`L1730`–`L1733`).
3. Schedule `.system("sequence_update", sequence_update)` **before** `dialogue_update` (a dialogue key then starts the
   same tick) and before systems that read `SequenceEvents`.
4. `sequences::play(world, "sequences/radar.yaml", &[("camera", cam), ("radar", panel)])` returns a `SequenceHandle`.
   `skip(world, handle)` and `cancel(world, handle)` mark state; the next `sequence_update` applies it.
5. Read `SequenceEvents` after the system: `fired(handle, "radar_online")`, `ended(handle)`.

## Format

```yaml
format: 1
kind: sequence
roles:
  - camera
  - radar
length_ticks: 120
clock: game          # game (default) | sim
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
| `camera` | `role`, exactly one of `rig` / `restore: true`, `snap` (false) | Writes `CameraRig.rig`; the first `rig` key saves the old one for `restore`. A write that differs blends over its `blend_in`. |
| `animation` | `role`, `clip`, `fade` (0 s) | `crossfade`; `fade: 0` is a hard cut. |
| `ui` | `role`, `tracks` | Writes `UiTween`; each track's `start` is a frame offset after the key. |
| `audio` | `play`, `gain` (1), `role?`, `range` (10) | A one-shot; with `role`, positional at that entity. |
| `dialogue` | `line`, `speaker?` | `dialogue::say`; refused lines warn `L4353`. |
| `event` | `name`, `on_skip` (`fire`) | `Fired` in `SequenceEvents`. `name` is not empty or `end`. |

Every list is in ascending `at`, each `at` in `0..=length_ticks`; an unknown field is `L1120`. Block-style YAML only: a non-empty `[..]` or `{..}` is `L1005`.

## Clocks

The playhead is `now − started`. `clock: game` reads `ctx.game_frame`: a `GameClock` hold applies nothing and later keys
shift by the held length; `scale(0.5)` doubles the ticks. `clock: sim` runs through a hold. Animation keys always stamp
the gameplay frame. A sequence started on frame F ends `Completed` on F+`length_ticks`, keys at `at == length_ticks`
applying that tick. Order inside a tick: track (camera, animation, ui, audio, dialogue, event), `at`, document order;
players in ascending entity id.

## Skip and cancel

`skip` returns `false` when the handle is dead or the doc says `skippable: false`. Otherwise, per role, the last
camera, animation and ui key not yet applied takes its end form (camera: write plus a snap; animation: hard cut; ui:
`once` tracks jump to `to`), unplayed audio and dialogue keys are dropped, its lines are interrupted, `on_skip: fire`
events fire in order, then `Ended { Skipped }`. A skipped and a watched beat leave the same rig, clip and tween.
`cancel` applies no end state, interrupts its lines and ends `Cancelled`; so does a document unloaded mid-play.

## Events

`SequenceEvents` is rewritten each run: `KeyApplied { handle, track, at }` (the trace; a skip emits it per end form),
`Fired { handle, name }`, `Ended { handle, end }`. The handle is dead from `Ended` on.

## Diagnostics

| Code | Meaning |
| --- | --- |
| `L1730` | Structural: length 0, `at` past the length or out of order, camera key without exactly one of `rig`/`restore`, non-finite number |
| `L1731` | Unknown, duplicate or missing role |
| `L1732` | Empty or reserved event name |
| `L1733` | Dialogue line unknown, or no `.dialogue()` |
| `L4350` / `L4351` | `play`: unknown document / role undeclared or bound twice |
| `L4352` | Role unbound or its entity dead (warning, once) |
| `L4353` | Key degraded: missing `CameraRig`/`Animation`, `say` refused, doc gone (warning, once) |
| `L4354` | Called without `.sequences()` |

## Gotchas

- State is entities (`SequencePlayer`, one `SequenceBinding` per role), so snapshots and checkpoints keep a beat.
  Entity refs in a list are refused (`L1392`), hence one binding entity per role.
- Scheduled after `dialogue_update`, a dialogue key starts a tick late.
- An opted-in game that never plays hashes as if it had not opted in.

## Next

- `ludion docs dialogue` — lines, speakers, holds.
- `ludion docs game-clock` — holds and time scale.
- `ludion docs camera`, `ludion docs ui-tweens` — the rig and tween a key writes.
