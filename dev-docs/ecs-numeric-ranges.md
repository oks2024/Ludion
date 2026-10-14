---
description: Legal (minimum/maximum) vs useful (x-ui-range) ranges on numeric component fields
---

# ludion-ecs: numeric field ranges

**Scope:** Annotating a component field with a legal or UI range. Hub: `ludion devdocs ludion-ecs`; validator: `ludion devdocs ludion-scene`.

## Rules

### Two ranges on a numeric field

A numeric component field can declare two different ranges, and they are not the same statement:

| | keyword | meaning | who enforces it |
|---|---|---|---|
| **legal** | `minimum` / `maximum` | outside this the scene is invalid | `crates/ludion-scene/src/validate.rs` → `L1014` |
| **useful** | `x-ui-range: [lo, hi]` | how far a slider handle travels | nobody — it is inert to every validator |

`minimum`/`maximum` come from `#[schemars(range(min = …, max = …))]`. `x-ui-range` comes from
`#[schemars(extend("x-ui-range" = [lo, hi]))]` on the same field, and the two coexist: the extension is
a post-mutator on the field's own subschema. It is an unknown JSON Schema keyword, so every validator
ignores it by construction; it reaches clients on `engine.describe` with the rest of the component
schema. Annotate a field where `minimum`/`maximum` do not already give an honest travel range — either
because there is no ceiling, or because the legal one is far past the useful one (`dof` is legal to
`0.25`, where the gather's disc stops being smooth, but travels to `0.15`, which is where it stops
looking good). Take the numbers from the field's own doc comment rather than inventing them.

**A colour triple deliberately carries none.** Three narrow tracks are a worse control than the colour
picker that belongs there, so `fog_color` and `Environment`'s four colour fields state no travel and
render as plain number boxes. The absence is the mechanism: no client special-cases colours, they
simply have nothing to draw a track from.

**`GiVolume.cell_size` and `probe_spacing` deliberately carry none either, for a different reason.**
Every distinct value re-runs a real build (re-voxelize ~300 ms / re-bake), not a shader uniform, so a
generic live-preview slider is the wrong control for either field; the view that edits them owns a
stepped control instead. The absence is the mechanism, same as the colour ruling above.

**`ParticleEmitter.started` and `seed` deliberately carry none, for a third reason.** `started` is a
frame stamp — the Effects view's restart writes it from a reply envelope's `frame`, and a slider over it
would be the time scrubber that plan rules out; `seed` names a random stream, where adjacent values
mean nothing adjacent. Every other numeric field of that component travels, the `[f32; 2]` pairs and
the `extent` triple included (read per element, as below).

On a fixed numeric array (`[f32; 3]`) the extension sits on the array field itself and is understood
**per element**. Do not reach for `#[schemars(inner(range(…)))]` there: that emits real
`items.minimum`/`maximum` and turns an existing element-wise semantic diagnostic (`L1130` on
`PostProcess`, `L1128` on `Environment`) into a competing `L1014` — two diagnostics for one mistake.

## Verify

```
cargo test -p ludion-ecs
```
