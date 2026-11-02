// Tests for the grade band control's decisions: which fields become a split control, what travel its
// rows get, and — the one that costs something to get wrong — which split a gesture's writes are
// rebuilt from.
//
// `GradeBandControl.tsx` is a renderer with no logic of its own. This repo runs vitest with
// `environment: 'node'` and collects `*.test.ts` only, so everything a wrong answer could hide in
// lives in `gradeControl.ts` and is exercised here directly.

import { describe, expect, it } from 'vitest';
import { CHROMA_TRAVEL, decompose, GRADE_BANDS, recomposeForWrite, type GradeSplit } from './grade';
import { gradeControl, GradeGesture } from './gradeControl';
import type { FieldDescriptor, FieldRange } from '../../viewer/inspect-fields';

const LIFT = GRADE_BANDS[0];
const GAIN = GRADE_BANDS[2];

/** `x-ui-range` as the engine declares it for `gain`: travel only, no legal bound on the array. */
const GAIN_RANGE: FieldRange = { min: 0, max: 2, hardMin: null, hardMax: null };

function vectorField(name: string, range: FieldRange | null, length = 3): FieldDescriptor {
  return { kind: 'vector', path: [name], label: name, length, integer: false, wide: false, color: false, range };
}

// --- which fields get the control ------------------------------------------------------------------

describe('gradeControl', () => {
  it('claims the three graded triples and no other field', () => {
    for (const band of GRADE_BANDS) {
      expect(gradeControl(vectorField(band.id, GAIN_RANGE))?.band).toBe(band);
    }
    // The Grade row's other four fields are scalars, and every one of them keeps the generic widget.
    for (const scalar of ['contrast', 'saturation', 'temperature', 'tint']) {
      const field: FieldDescriptor = {
        kind: 'number',
        path: [scalar],
        label: scalar,
        integer: false,
        wide: false,
        range: { min: -1, max: 1, hardMin: -1, hardMax: 1 },
      };
      expect(gradeControl(field)).toBeNull();
    }
  });

  it('takes the master travel from the field, never from a table of its own', () => {
    const odd: FieldRange = { min: -9, max: 9, hardMin: -12, hardMax: 12 };
    const rows = gradeControl(vectorField('gamma', odd))?.rows ?? [];
    expect(rows.map((row) => row.axis)).toEqual(['master', 'temp', 'tint']);
    expect(rows[0].range).toBe(odd);
    expect(rows[1].range).toBe(CHROMA_TRAVEL);
    expect(rows[2].range).toBe(CHROMA_TRAVEL);
  });

  it('falls back to the generic widget for a band the schema gives no travel', () => {
    // Nothing else declares how far an intensity should throw, and a track may not invent one.
    expect(gradeControl(vectorField('lift', null))).toBeNull();
  });

  it('falls back for a band-named field of the wrong shape', () => {
    expect(gradeControl(vectorField('gain', GAIN_RANGE, 4))).toBeNull();
    expect(gradeControl({ ...vectorField('gain', GAIN_RANGE), path: ['nested', 'gain'] })).toBeNull();
    expect(gradeControl({ kind: 'json', path: ['lift'], label: 'lift' })).toBeNull();
  });
});

// --- the gesture -----------------------------------------------------------------------------------

describe('GradeGesture', () => {
  it('rebuilds every preview from the split the gesture opened on, not from what it just wrote', () => {
    const gesture = new GradeGesture();
    const opening: GradeSplit = { master: 1.4, temp: 0, tint: 0.2 };
    let live = opening;
    let reDerivedDrift = 0;

    for (const temp of [0.05, 0.11, 0.18, 0.26, 0.35, 0.44, 0.5, 0.44, 0.31, 0.17]) {
      const written = gesture.write(GAIN, 'temp', temp, live, 'preview');
      // Only the dragged axis moves; master and tint are the ones the gesture captured.
      expect(written).toEqual(recomposeForWrite(GAIN, { ...opening, temp }));
      // What the widget will show on the next render — and what must NOT feed the next write.
      live = decompose(GAIN, written);
      reDerivedDrift = Math.max(
        reDerivedDrift,
        Math.abs(live.master - opening.master),
        Math.abs(live.tint - opening.tint),
      );
    }

    // The reason the capture exists: the write path quantizes, so the re-derived split really is a
    // different split. Were it fed back in, that difference would be the other two handles moving.
    expect(reDerivedDrift).toBeGreaterThan(0);
  });

  it('holds the split only while previews are landing', () => {
    const gesture = new GradeGesture();
    const opening: GradeSplit = { master: 1, temp: 0, tint: 0 };
    expect(gesture.holding).toBe(false);

    gesture.write(GAIN, 'temp', 0.3, opening, 'preview');
    expect(gesture.holding).toBe(true);

    gesture.write(GAIN, 'temp', 0.4, opening, 'commit');
    expect(gesture.holding).toBe(false);

    // The next gesture opens on the live split, whatever the last one was holding.
    const later: GradeSplit = { master: 1.9, temp: -0.1, tint: 0.05 };
    expect(gesture.write(GAIN, 'temp', 0, later, 'preview')).toEqual(
      recomposeForWrite(GAIN, { ...later, temp: 0 }),
    );
  });

  it('treats a cadence-less write (a typed readout) as a whole gesture', () => {
    const gesture = new GradeGesture();
    const live: GradeSplit = { master: 0.2, temp: 0.4, tint: -0.3 };
    expect(gesture.write(LIFT, 'master', 0.05, live)).toEqual(
      recomposeForWrite(LIFT, { ...live, master: 0.05 }),
    );
    expect(gesture.holding).toBe(false);
  });

  it('opens a new gesture when a different row is written', () => {
    const gesture = new GradeGesture();
    const opening: GradeSplit = { master: 1, temp: 0, tint: 0 };
    gesture.write(GAIN, 'temp', 0.6, opening, 'preview');
    // Two handles cannot be under one pointer, so the second row starts from the live split — which
    // by then already carries the first row's write.
    const live: GradeSplit = { master: 1, temp: 0.6, tint: 0 };
    expect(gesture.write(GAIN, 'tint', 0.25, live, 'preview')).toEqual(
      recomposeForWrite(GAIN, { master: 1, temp: 0.6, tint: 0.25 }),
    );
  });

  it('writes the recomposed triple unclamped — the master is what was bounded', () => {
    const gesture = new GradeGesture();
    // The master row's track stops at `gain`'s declared 2.0; the channels a warm chroma puts that
    // master on are legal (the schema declares no bound on the array) and clamping them back would
    // silently cool the colour the human set.
    const written = gesture.write(GAIN, 'master', 2, { master: 1, temp: 0.9, tint: 0 }, 'commit');
    expect(written[0]).toBeGreaterThan(GAIN_RANGE.max);
    expect(written[0]).toBeCloseTo(3.37, 2);
    expect(decompose(GAIN, written).temp).toBeCloseTo(0.9, 2);
  });

  it('round-trips the singular pure-green chroma without a special case', () => {
    // At `tint = ±1` there is no warmth left to name, so `decompose` reports `temp: 0`; the triple
    // still rebuilds exactly, which is why the control needs no branch for it.
    const gesture = new GradeGesture();
    const written = gesture.write(GAIN, 'tint', 1, { master: 1, temp: 0.5, tint: 0 }, 'commit');
    expect(decompose(GAIN, written).tint).toBeCloseTo(1, 6);
    expect(decompose(GAIN, written).temp).toBe(0);
  });
});
