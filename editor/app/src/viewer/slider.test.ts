// Tests for the slider arithmetic. The magnitudes exercised are the ones the engine's own annotated
// fields really carry — `grain_size` travels `[0, 0.01]`, `ssao_bias` sits around `0.02`, `fade` is
// `[0, 1]` — because a step chosen for `[0, 1]` alone is useless on the first two.

import { describe, expect, it } from 'vitest';
import type { FieldRange } from './inspect-fields';
import { beginCompare, endCompare, type CompareSession } from '../editor/compare';
import { neutralPatch } from '../editor/art/chain';
import { POST_PROCESS } from '../editor/art/target';
import type { EntityId } from '../editor/undo';
import { clampToLegal, dragValue, niceStep, quantize, SliderDrag, widen } from './slider';

function range(min: number, max: number, hardMin: number | null = null, hardMax: number | null = null): FieldRange {
  return { min, max, hardMin, hardMax };
}

describe('quantize', () => {
  it('strips picker noise a shortest-round-trip writer would otherwise carry into the file', () => {
    // 128/255 decoded to linear — the number an untouched color picker hands over.
    expect(quantize(0.21586050011389923)).toBe(0.216);
    expect(quantize(0.15000000000000002)).toBe(0.15);
  });

  it('leaves an authored value alone', () => {
    for (const value of [0.15, 0.2, 0.95, 1, 5.5, 0]) {
      expect(quantize(value)).toBe(value);
    }
  });

  it('keeps significant digits, not decimal places, so near-black survives', () => {
    // The voxel palette's darkest authored range (creature flesh 0.03–0.08). Three decimals would
    // round 0.0137 to 0.014 — a visible step where the sRGB curve is steepest. The same property is
    // what keeps a slider knob at 0.002 from quantizing to 0.
    expect(quantize(0.013702239617988)).toBe(0.0137);
    expect(quantize(0.0020000000000000005)).toBe(0.002);
  });

  it('passes a non-finite value through rather than inventing one', () => {
    expect(Number.isNaN(quantize(Number.NaN))).toBe(true);
    expect(quantize(Number.POSITIVE_INFINITY)).toBe(Number.POSITIVE_INFINITY);
  });
});

describe('widen', () => {
  it('extends travel to contain a value outside it, in either direction', () => {
    expect(widen(range(0, 1), 5.5)).toEqual(range(0, 5.5));
    expect(widen(range(0, 1), -0.2)).toEqual(range(-0.2, 1));
  });

  it('leaves travel alone for a value already inside it', () => {
    expect(widen(range(0, 8), 5.5)).toEqual(range(0, 8));
    expect(widen(range(0, 1), 0)).toEqual(range(0, 1));
    expect(widen(range(0, 1), 1)).toEqual(range(0, 1));
  });

  it('never touches the legal bounds — widening the track does not make a value legal', () => {
    expect(widen(range(0, 1, 0, 1), 4)).toEqual(range(0, 4, 0, 1));
  });

  it('falls back to the declared travel for a value that is not a number', () => {
    expect(widen(range(0, 1), Number.NaN)).toEqual(range(0, 1));
    expect(widen(range(0, 1), Number.POSITIVE_INFINITY)).toEqual(range(0, 1));
  });
});

describe('niceStep', () => {
  it('is a 1/2/5×10ⁿ number at every magnitude this editor edits', () => {
    expect(niceStep(0, 1, false)).toBe(0.01); // Fade.fade, Mesh.roughness
    expect(niceStep(0, 0.5, false)).toBe(0.005); // bloom_strength, halation
    expect(niceStep(0, 0.1, false)).toBe(0.001); // fog_density
    expect(niceStep(0, 0.01, false)).toBe(0.0001); // grain_size — 1e-4, not 0
    expect(niceStep(0, 0.05, false)).toBe(0.0005); // ssao_bias's neighbourhood
    expect(niceStep(0, 8, false)).toBe(0.1); // emissive
    expect(niceStep(-1, 1, false)).toBe(0.02);
  });

  it('is free of the float noise the magnitude multiply would otherwise leave', () => {
    for (const [min, max] of [
      [0, 1],
      [0, 0.5],
      [0, 0.1],
      [0, 0.01],
      [0, 0.02],
      [0, 3],
      [0.1, 0.3],
    ]) {
      const step = niceStep(min, max, false);
      expect(Number(step.toPrecision(1)), `${min}..${max}`).toBe(step);
    }
  });

  it('never steps an integer field below 1, and stays nice on a wide one', () => {
    expect(niceStep(0, 10, true)).toBe(1);
    expect(niceStep(0, 1, true)).toBe(1);
    expect(niceStep(0, 150, true)).toBe(2);
    expect(niceStep(0, 1000, true)).toBe(10);
  });

  it('is deterministic — the same span always yields the same step', () => {
    expect(niceStep(0, 0.01, false)).toBe(niceStep(0, 0.01, false));
    expect(niceStep(2, 2.01, false)).toBe(niceStep(0, 0.01, false));
  });

  it('falls back rather than returning 0 or NaN for a span that cannot be subdivided', () => {
    expect(niceStep(1, 1, false)).toBe(0.01);
    expect(niceStep(1, 1, true)).toBe(1);
    expect(niceStep(1, 0, false)).toBe(0.01);
    expect(niceStep(0, Number.POSITIVE_INFINITY, false)).toBe(0.01);
    expect(niceStep(Number.NaN, 1, true)).toBe(1);
  });
});

describe('clampToLegal', () => {
  it('clamps to a declared bound on either side', () => {
    expect(clampToLegal(1.4, range(0, 1, 0, 1))).toBe(1);
    expect(clampToLegal(-0.3, range(0, 1, 0, 1))).toBe(0);
    expect(clampToLegal(0.4, range(0, 1, 0, 1))).toBe(0.4);
  });

  it('clamps only where a bound is declared', () => {
    expect(clampToLegal(-2, range(0, 0.5, 0, null))).toBe(0);
    expect(clampToLegal(40, range(0, 0.5, 0, null))).toBe(40);
    expect(clampToLegal(-40, range(0, 0.5, null, null))).toBe(-40);
  });

  it('lets a widened track write past the travel it was widened to, but never past the legal bound', () => {
    // `widen` opened the track to an authored 5.5 on a field the schema caps at 1: dragging to the
    // end of that track must still land on the legal maximum.
    const widened = widen(range(0, 1, 0, 1), 5.5);
    expect(clampToLegal(widened.max, widened)).toBe(1);
  });
});

describe('dragValue', () => {
  it('strips the float noise a track position carries, so a drag authors a readable number', () => {
    expect(dragValue(0.15000000000000002, range(0, 1), false)).toBe(0.15);
    expect(dragValue(0.30000000000000004, range(0, 0.5), false)).toBe(0.3);
  });

  it('never writes past a legal bound, however far the track was widened', () => {
    const widened = widen(range(0, 1, 0, 1), 5.5);
    expect(dragValue(5.5, widened, false)).toBe(1);
    expect(dragValue(-2, widened, false)).toBe(0);
  });

  it('quantizes before it clamps, so rounding cannot push a value back off its bound', () => {
    // 3 significant digits would round 0.9999 up to 1 — past a maximum of 0.9999 — if the clamp ran
    // first. It runs second, so the bound holds.
    expect(dragValue(0.99994, range(0, 1, 0, 0.9999), false)).toBe(0.9999);
  });

  it('rounds an integer field rather than quantizing it', () => {
    expect(dragValue(3.5, range(0, 10), true)).toBe(4);
    expect(dragValue(1234.6, range(0, 2000), true)).toBe(1235); // not 1230
  });

  it('leaves a value inside the bounds exactly as the track reported it', () => {
    expect(dragValue(0.4, range(0, 1, 0, 1), false)).toBe(0.4);
    expect(dragValue(0.002, range(0, 0.01, 0, 1), false)).toBe(0.002);
  });
});

// --- the gesture ---------------------------------------------------------------------------------
//
// `bloom_strength`'s real travel: legal `[0, ∞)`, useful `[0, 0.5]`.
const BLOOM = range(0, 0.5, 0, null);

/** A gesture opened on `base`, ready to move. */
function begin(base: number, intervalMs = 66): SliderDrag {
  const drag = new SliderDrag(intervalMs);
  drag.begin(base, BLOOM, false);
  return drag;
}

describe('SliderDrag cadence', () => {
  it('throttles previews latest-wins but tracks the handle on every move', () => {
    const drag = begin(0.1);

    const m1 = drag.move(0.2, 0);
    expect(m1.handle).toBe(0.2);
    expect(m1.preview).toBe(0.2); // the first changed move always goes out

    const m2 = drag.move(0.25, 10);
    expect(m2.handle).toBe(0.25);
    expect(m2.preview).toBeUndefined(); // inside the 66 ms window

    const m3 = drag.move(0.3, 100);
    expect(m3.handle).toBe(0.3);
    expect(m3.preview).toBe(0.3);
  });

  it('never lets the throttle drop the value the gesture ended on', () => {
    const drag = begin(0.1);
    drag.move(0.2, 0); // previewed
    drag.move(0.42, 5); // throttled away — this value has never reached the engine

    expect(drag.end(0.42)).toEqual({ commit: 0.42, done: true });
    expect(drag.dragging).toBe(false);
  });

  it('quantizes and clamps what it writes, at both cadences', () => {
    const drag = begin(0.1);
    expect(drag.move(0.30000000000000004, 0).preview).toBe(0.3);
    expect(drag.end(-2)).toEqual({ commit: 0, done: true }); // legal minimum, not the travel minimum
  });

  it('writes nothing for a gesture that never moved', () => {
    expect(begin(0.1).end(0.1)).toEqual({ done: true });
    const clicked = begin(0.1);
    expect(clicked.move(0.1, 0)).toEqual({ handle: 0.1 });
    expect(clicked.end(0.1)).toEqual({ done: true });
  });

  it('ignores events outside a gesture', () => {
    const drag = new SliderDrag();
    expect(drag.move(0.4, 0)).toEqual({});
    expect(drag.end(0.4)).toEqual({});
    expect(drag.escape()).toEqual({});
  });

  it('keeps the base it opened on however far the committed value composes forward', () => {
    // Previews compose into the value the widget is handed, so `begin` runs again on every event.
    const drag = begin(0.1);
    drag.move(0.4, 0);
    drag.begin(0.4, BLOOM, false); // the re-render's value — must not become the new base
    expect(drag.escape()).toEqual({ commit: 0.1, done: true });
  });
});

describe('SliderDrag: one gesture, one undo entry', () => {
  it('reports exactly once however many previews went out', () => {
    const drag = begin(0.1);
    const reported: number[] = [];
    const previewed: number[] = [];
    for (const [value, at] of [
      [0.2, 0],
      [0.3, 10],
      [0.35, 80],
      [0.4, 90],
    ] as const) {
      const instr = drag.move(value, at);
      if (instr.preview !== undefined) {
        previewed.push(instr.preview);
      }
    }
    const up = drag.end(0.4);
    if (up.commit !== undefined) {
      reported.push(up.commit);
    }
    expect(previewed.length).toBeGreaterThan(1);
    expect(reported).toEqual([0.4]); // the frame moved four times; history moved once
  });

  it('still writes when the gesture ends back on its base, because previews went out', () => {
    const drag = begin(0.1);
    expect(drag.move(0.4, 0).preview).toBe(0.4);
    // Back onto base while the engine holds 0.4: the write is the restore, not a redundant set.
    expect(drag.move(0.1, 100)).toEqual({ handle: 0.1, preview: 0.1 });

    const throttled = begin(0.1);
    throttled.move(0.4, 0);
    expect(throttled.move(0.1, 5).preview).toBeUndefined(); // throttled away
    expect(throttled.end(0.1)).toEqual({ commit: 0.1, done: true }); // release restores it anyway
  });
});

describe('SliderDrag escape', () => {
  it('restores the value captured at the start of the gesture', () => {
    const drag = begin(0.1);
    drag.move(0.4, 0);
    expect(drag.escape()).toEqual({ commit: 0.1, done: true });
    expect(drag.dragging).toBe(false);
    expect(drag.escape()).toEqual({}); // a second Escape has no gesture
  });

  it('writes nothing when no preview ever went out', () => {
    expect(begin(0.1).escape()).toEqual({ done: true });
  });

  it('abandons a gesture outright on reset, writing nothing', () => {
    const drag = begin(0.1);
    drag.move(0.4, 0);
    drag.reset();
    expect(drag.dragging).toBe(false);
    expect(drag.end(0.4)).toEqual({});
  });
});

// --- a drag interleaved with a hold-to-compare ----------------------------------------------------
//
// `ArtDirectorView` refuses field writes while a comparison holds the component (its `commitField`).
// This exercises the rule against the real `compare.ts`: what makes the round trip net-zero is that
// nothing lands between the capture and the restore, and the slider is the one thing that could.

const TARGET: EntityId = { index: 3, generation: 0 };

/** The `component.set` value a compare patch carries. */
function patchValue(command: unknown): Record<string, unknown> {
  return (command as { params: { value: Record<string, unknown> } }).params.value;
}

describe('a slider drag and a hold-to-compare exclude each other', () => {
  it('leaves the compare net-zero when a drag is live under it', () => {
    // The engine's `PostProcess`, replaced wholesale by every `component.set`.
    let engine: Record<string, unknown> = { bloom_strength: 0.1, exposure: 1.3 };
    let held: CompareSession | null = null;
    const write = (next: number): void => {
      // The view's rule, verbatim: a held comparison owns the component.
      if (held !== null) {
        return;
      }
      engine = { ...engine, bloom_strength: next };
    };

    const drag = begin(0.1);
    const first = drag.move(0.4, 0);
    write(first.preview!);
    expect(engine.bloom_strength).toBe(0.4);

    // A/B pressed mid-gesture: the capture is the live value, previews and all.
    const start = beginCompare(POST_PROCESS, TARGET, engine, neutralPatch('bloom'), 'bloom')!;
    held = start.session;
    engine = patchValue(start.patch.command);
    expect(engine.bloom_strength).toBe(0); // the effect's neutral

    // Everything the gesture does under the hold is refused — including its final value.
    write(drag.move(0.45, 100).preview!);
    write(drag.end(0.45).commit!);
    expect(engine.bloom_strength).toBe(0);

    held = null;
    engine = patchValue(endCompare(start.session).command);
    expect(engine).toEqual({ bloom_strength: 0.4, exposure: 1.3 }); // exactly the captured object
  });

  it('lets the next gesture write once the comparison is released', () => {
    let engine: Record<string, unknown> = { bloom_strength: 0.4 };
    const start = beginCompare(POST_PROCESS, TARGET, engine, neutralPatch('bloom'), 'bloom')!;
    engine = patchValue(start.patch.command);
    engine = patchValue(endCompare(start.session).command);

    const drag = begin(0.4);
    const up = drag.end(0.2);
    engine = { ...engine, bloom_strength: up.commit! };
    expect(engine.bloom_strength).toBe(0.2);
  });
});
