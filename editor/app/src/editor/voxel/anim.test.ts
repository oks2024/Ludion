import { describe, expect, it } from 'vitest';
import {
  durationTicks,
  isOnTick,
  keyframeMarks,
  parseAnimDoc,
  phaseTick,
  scrubStarted,
  secondsOf,
  stepsNeeded,
  tickLabel,
  tickOf,
  TICK_HZ,
} from './anim';

// The two committed clips the stage rig binds. Named one by one rather than swept: the anims
// folder also holds work an author has not committed, and a test that globbed it would go red on
// someone else's half-finished clip.
const CLIPS = import.meta.glob(
  [
    '../../../fixtures/stage/scenes/voxels/anims/rig-walk.anim.yaml',
    '../../../fixtures/stage/scenes/voxels/anims/rig-wave.anim.yaml',
  ],
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;

function clip(name: string) {
  const key = Object.keys(CLIPS).find((path) => path.endsWith(`/${name}`));
  expect(key, `${name} is missing from the frozen stage fixture (editor/app/fixtures/stage)`).toBeDefined();
  return parseAnimDoc(CLIPS[key as string]);
}

const WALK = clip('rig-walk.anim.yaml');
const WAVE = clip('rig-wave.anim.yaml');

describe('parseAnimDoc — the shared humanoid walk', () => {
  it('reads the header past the clip’s comment block', () => {
    expect(WALK.name).toBe('rig-walk');
    expect(WALK.duration).toBe(0.6);
    expect(WALK.loop).toBe(true);
  });

  it('reads one track per part/channel pair, in authored order', () => {
    expect(WALK.tracks.map((track) => `${track.part}.${track.channel}`)).toEqual([
      'head.rotation',
      'lower-arm-l.rotation',
      'lower-arm-r.rotation',
      'lower-leg-l.rotation',
      'lower-leg-r.rotation',
      'torso.rotation',
      'torso.translation',
      'upper-arm-l.rotation',
      'upper-arm-r.rotation',
      'upper-leg-l.rotation',
      'upper-leg-r.rotation',
    ]);
  });

  it('reads each channel’s keyframe times, `t` living on the dash line', () => {
    const track = (id: string) => WALK.tracks.find((t) => `${t.part}.${t.channel}` === id);
    expect(track('head.rotation')?.times).toEqual([0.0, 0.3, 0.6]);
    // The swing leg keys twice as often: it is the one limb whose knee has to flex mid-stride.
    expect(track('lower-leg-l.rotation')?.times).toEqual([0.0, 0.15, 0.3, 0.45, 0.6]);
    expect(track('torso.translation')?.times).toEqual([0.0, 0.15, 0.3, 0.45, 0.6]);
  });

  it('the one-shot reads as one', () => {
    expect(WAVE.duration).toBe(0.35);
    expect(WAVE.loop).toBe(false);
  });

  it('is total: an empty and a half-typed document parse to nothing, never a throw', () => {
    expect(parseAnimDoc('')).toEqual({ name: null, duration: 0, loop: false, tracks: [] });
    const half = parseAnimDoc('kind: animation\nduration:\ntracks:\n  arm:\n    rotation:\n');
    expect(half.duration).toBe(0);
    expect(half.tracks).toEqual([{ part: 'arm', channel: 'rotation', times: [] }]);
  });
});

describe('keyframeMarks', () => {
  it('is the de-duplicated union of every track’s times, ascending', () => {
    expect(keyframeMarks(WALK).map((mark) => mark.t)).toEqual([0.0, 0.15, 0.3, 0.45, 0.6]);
    expect(keyframeMarks(WALK).map((mark) => mark.tick)).toEqual([0, 9, 18, 27, 36]);
    expect(keyframeMarks(WAVE).map((mark) => mark.tick)).toEqual([0, 6, 12, 21]);
  });

  it('every authored time in both shipped clips lands on a tick', () => {
    for (const doc of [WALK, WAVE]) {
      expect(keyframeMarks(doc).every((mark) => mark.onTick)).toBe(true);
    }
  });

  it('flags a time authored between ticks — the trap nothing else catches', () => {
    const marks = keyframeMarks(
      parseAnimDoc(
        'duration: 0.5\ntracks:\n  arm:\n    rotation:\n      - t: 0.11\n      - t: 0.25\n',
      ),
    );
    expect(marks.map((mark) => [mark.t, mark.tick, mark.onTick])).toEqual([
      [0.11, 7, false],
      [0.25, 15, true],
    ]);
  });
});

describe('tick arithmetic', () => {
  it('quantizes the way the engine does: nearest tick, non-positive and non-finite at 0', () => {
    expect(TICK_HZ).toBe(60);
    expect(tickOf(0.4)).toBe(24);
    // `--time 0.401` renders tick 24 and reports `t 0.4` (ludion docs voxel-animation).
    expect(tickOf(0.401)).toBe(24);
    expect(tickOf(0.35)).toBe(21);
    expect(tickOf(0)).toBe(0);
    expect(tickOf(-1)).toBe(0);
    expect(tickOf(Number.NaN)).toBe(0);
    expect(secondsOf(24)).toBe(0.4);
    expect(durationTicks(WALK.duration)).toBe(36);
    expect(durationTicks(WAVE.duration)).toBe(21);
  });

  it('isOnTick absorbs the float error an authored decimal carries', () => {
    // 0.45 * 60 is 27.000000000000004 in binary floating point.
    expect(isOnTick(0.45)).toBe(true);
    expect(isOnTick(0.6)).toBe(true);
    expect(isOnTick(0)).toBe(true);
    expect(isOnTick(0.11)).toBe(false);
    expect(isOnTick(1 / 120)).toBe(false);
    expect(isOnTick(Number.POSITIVE_INFINITY)).toBe(false);
  });

  it('tickLabel is the CLI’s phase-report wording, character for character', () => {
    expect(tickLabel(0)).toBe('t 0 (tick 0)');
    expect(tickLabel(24)).toBe('t 0.4 (tick 24)');
    expect(tickLabel(21)).toBe('t 0.35 (tick 21)');
    // Captured from `render --model rig.assembly.yaml --clip walk --strip 7`.
    expect(tickLabel(26)).toBe('t 0.43333333333333335 (tick 26)');
  });
});

describe('scrubbing', () => {
  it('started is the frame minus the tick to pose', () => {
    expect(scrubStarted(500, 27)).toBe(473);
    expect(stepsNeeded(500, 27)).toBe(0);
  });

  it('a fresh session must step forward before the deep end of a clip is reachable', () => {
    // `started` is unsigned, so frame 10 cannot show tick 27 — it would clamp to 0 and lie.
    expect(stepsNeeded(10, 27)).toBe(17);
    expect(scrubStarted(10, 27)).toBe(0);
    expect(stepsNeeded(10 + 17, 27)).toBe(0);
    expect(scrubStarted(27, 27)).toBe(0);
  });
});

describe('phaseTick', () => {
  const walk = durationTicks(WALK.duration);
  const wave = durationTicks(WAVE.duration);

  it('reads back exactly the tick a scrub wrote', () => {
    for (const tick of [0, 9, 18, 27, 35]) {
      expect(phaseTick(500, scrubStarted(500, tick), 1, walk, true)).toBe(tick);
    }
  });

  it('a loop wraps into [0, duration)', () => {
    expect(phaseTick(500, 500 - walk, 1, walk, true)).toBe(0);
    expect(phaseTick(500, 500 - walk - 5, 1, walk, true)).toBe(5);
  });

  it('a one-shot holds its last tick forever', () => {
    expect(phaseTick(500, 500 - wave, 1, wave, false)).toBe(wave);
    expect(phaseTick(500, 0, 1, wave, false)).toBe(wave);
  });

  it('a future started, a zero speed and a lengthless clip all read as the first instant', () => {
    expect(phaseTick(10, 40, 1, walk, true)).toBe(0);
    expect(phaseTick(500, 100, 0, walk, true)).toBe(0);
    expect(phaseTick(500, 100, 1, 0, true)).toBe(0);
  });

  it('a speed multiplier moves which tick is shown, without leaving the clip', () => {
    expect(phaseTick(500, 500 - 20, 0.5, walk, true)).toBe(10);
    expect(phaseTick(500, 500 - 20, 2, walk, true)).toBe(40 % walk);
  });
});

describe('an overlay rate', () => {
  it('offsets in sim ticks, rounded', () => {
    expect(scrubStarted(500, 30, 1.2)).toBe(475);
    expect(stepsNeeded(10, 60, 1.2)).toBe(40);
  });
});

describe('the scale channel', () => {
  it('is read, so an imported clip’s scale keys mark the bar', () => {
    const doc = parseAnimDoc(
      'kind: animation\nduration: 1\ntracks:\n  root:\n    scale:\n      - t: 0.5\n        value: [1, 1, 1]\n',
    );
    expect(doc.tracks).toEqual([{ part: 'root', channel: 'scale', times: [0.5] }]);
  });
});
