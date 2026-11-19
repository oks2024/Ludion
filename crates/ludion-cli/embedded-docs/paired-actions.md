---
description: The kind: paired document and check-scene: roles, align offsets, events, contact gaps on posed parts
---

# Paired actions

**Scope:** a two-or-more-role choreography authored as text (`kind: paired`): which assemblies take
part, where each ends up relative to the reference, the named moments, and the part contacts that must
meet. A game cannot play one yet; `ludion docs voxel-assemblies` §Ragdolls covers the switches a
finished action hands the target to. Clips are `ludion docs voxel-animation`.

## The loop

1. Write `voxels/choke.paired.yaml` beside the assemblies it names (format below).
2. `ludion check-scene voxels/choke.paired.yaml` prints each role, the align block, the events and
   each contact's gap, or `L1752`–`L1754` with JSON Pointers.
3. A contact over its `max_gap` is an `L1755` warning (exit 0): move the offset or the event time.
4. Add `--json` for the same fields as one object.

## Format

```yaml
format: 1
kind: paired
name: choke                  # required; events fire as `paired:<name>`
roles:                       # a mapping, two or more, sorted on emit
  instigator:
    assembly: humanoid.assembly.yaml   # relative to this file
    clip: carry              # one of the assembly's `clips:`
    scale: 0.125             # the role entity's uniform Transform scale (default 1)
  target:
    assembly: humanoid.assembly.yaml
    clip: idle
    scale: 0.125
    # align_only: true       # instead of `clip`: aligns, plays nothing
    # posture: standing      # a free posture name the game tests
align:
  reference: instigator      # the role every offset is relative to
  moves: instigator          # the role that closes the distance, or `split`
  ticks: 12                  # warp length in fixed ticks
  offsets:                   # one per non-reference role, metres, -Z forward
    target:
      translation:
        - 0.0
        - 0.0
        - -0.5
      yaw_deg: 180.0         # default 0
events:                      # strictly ascending t, unique names
  - name: grab
    t: 0.25                  # gameplay seconds since the start, at speed 1
contacts:
  - a: instigator/lower-arm-r
    b: target/head
    at: grab                 # an event name
    max_gap: 8.0             # cells of role `a`
```

Block sequences only. `clip` names a key of the assembly's `clips:`; a blend needs parameters the
document has no place for, and a prefab cannot be a role yet.

## Rules

- **`scale` is the role entity's uniform `Transform` scale.** An assembly cell is 1 unit and an
  assembly has no density of its own; the entity's scale maps a cell to metres, and `offsets` are
  metres. At 64 cells per metre the scale is `0.015625` (1/64): a 0.58 m offset is about 37 cells and
  a `max_gap: 1.0` is 1.6 cm. The gallery scenes place the humanoid at `0.125` (8 cells per metre).
  A wrong `scale` makes every contact gap wrong.
- A role entity sits at `T(offset) · R_y(yaw) · S(scale)`; the reference sits at the identity.
- Each contact poses both roles at its event's `t` (rest for an `align_only` role) and takes the
  nearest pair of cell centres between the two named parts. `gap = max(0, distance − (s_a + s_b) / 2)`
  in metres, reported also in role `a`'s cells (`gap / s_a`). 0 means touching or overlapping. The
  measure is cell-quantized.
- Every event `t` must be at most each clip role's clip `duration`.
- The warp report lists each role's aligned offset, yaw and planar distance from the reference; the
  start pose is runtime state, so it has no path from there yet.

## The check-scene report

```
examples/gallery/voxels/choke.paired.yaml: OK (paired choke, 2 roles, 3 events, 1 contact)
role instigator: assembly humanoid.assembly.yaml, clip carry (1 s), scale 0.125, reference
role target: assembly humanoid.assembly.yaml, clip idle (1.2 s), scale 0.125 | at [0, 0, -0.5] m yaw 180°, 0.500 m from instigator
align: 12 ticks, moves instigator
event grab @ 0.25
event hold @ 0.5
event release @ 0.9
contact instigator/lower-arm-r -> target/head @ hold (t 0.5): gap 6.828 cells (0.8535 m), max 8
```

## Codes

| Code | Raised for |
|---|---|
| `L1752` | empty or non-mapping root, envelope, wrong type, unknown key (nearest suggested), a role, offset, event or contact of the wrong shape |
| `L1753` | fewer than 2 roles, an unknown `reference`/`moves` role, a missing, extra or reference offset, `ticks: 0`, a non-finite number, `scale` of 0 or less, a role with both or neither of `clip` and `align_only`, an empty name, events out of order or repeated, a bad contact side, both sides on one role, an unknown `at`, a negative `max_gap` |
| `L1754` | a role's assembly does not load, an unknown clip (nearest suggested), an event past a clip's duration, a contact part the assembly lacks |
| `L1755` | warning: a contact's gap exceeds its `max_gap` |

`ludion explain L1752` and the others print the full text.
