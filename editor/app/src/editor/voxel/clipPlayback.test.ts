import { describe, expect, it } from 'vitest';
import { durationTicks, parseAnimDoc, phaseTick } from './anim';
import {
  ANIMATION,
  animationValue,
  barRatio,
  clampTick,
  crossfadeTo,
  fadeTicks,
  INITIAL_CLIP_STATE,
  loopRestartDue,
  pause,
  play,
  restamped,
  restart,
  scrubTo,
  selectClip,
  setLoopOneShot,
  setSpeed,
  shownTick,
  tickAtRatio,
  toRest,
  togglePlay,
  withPosedTick,
  withStamp,
  type ClipContext,
  type ClipState,
} from './clipPlayback';

// The two committed clips the stage rig binds — the same pair `anim.test.ts` names, and for the
// same reason: the anims folder also holds uncommitted work a glob would go red on.
const CLIPS = import.meta.glob(
  [
    '../../../fixtures/stage/scenes/voxels/anims/rig-walk.anim.yaml',
    '../../../fixtures/stage/scenes/voxels/anims/rig-wave.anim.yaml',
  ],
  { query: '?raw', import: 'default', eager: true },
) as Record<string, string>;

function clipDoc(name: string) {
  const key = Object.keys(CLIPS).find((path) => path.endsWith(`/${name}`));
  expect(key, `${name} is missing from the frozen stage fixture (editor/app/fixtures/stage)`).toBeDefined();
  return parseAnimDoc(CLIPS[key as string]);
}

const WALK = clipDoc('rig-walk.anim.yaml');
const WAVE = clipDoc('rig-wave.anim.yaml');

/** A warm session: far enough along that no scrub needs the step-forward guard. */
function ctxOf(doc: { duration: number; loop: boolean }, frame = 1000): ClipContext {
  return { frame, clipTicks: durationTicks(doc.duration), loops: doc.loop };
}

const WALKING = ctxOf(WALK);
const WAVING = ctxOf(WAVE);

/** `selectClip` from the initial state, the way every gesture below starts. */
function staged(name: string, ctx: ClipContext): ClipState {
  return selectClip(INITIAL_CLIP_STATE, name, ctx).state;
}

describe('staging a clip', () => {
  it('starts at the rest pose on a running session', () => {
    expect(INITIAL_CLIP_STATE.clip).toBeNull();
    expect(INITIAL_CLIP_STATE.playing).toBe(true);
    expect(INITIAL_CLIP_STATE.speed).toBe(1);
  });

  it('stamps the clip at its first tick, at the frame it was handed', () => {
    const { state, intents } = selectClip(INITIAL_CLIP_STATE, 'walk', WALKING);
    expect(intents).toEqual([
      { kind: 'set-animation', clip: 'walk', tick: 0, started: 1000, speed: 1 },
    ]);
    expect(state).toMatchObject({ clip: 'walk', tick: 0, started: 1000, playing: true });
  });

  it('resumes a stopped session, and only after the stamp', () => {
    const stopped = pause(staged('walk', WALKING), WALKING).state;
    const { intents } = selectClip(stopped, 'wave', WAVING);
    expect(intents.map((intent) => intent.kind)).toEqual(['set-animation', 'resume']);
  });

  it('rest removes the component and hands the session back running at ×1', () => {
    const slow = setSpeed(pause(staged('walk', WALKING), WALKING).state, 0.25).state;
    const { state, intents } = selectClip(slow, '', WALKING);
    expect(intents).toEqual([
      { kind: 'remove-animation' },
      { kind: 'resume' },
      { kind: 'set-speed', speed: 1 },
    ]);
    expect(state).toMatchObject({ clip: null, started: null, tick: 0, playing: true, speed: 1 });
  });

  it('never removes a component it did not put there — `component.remove` is L3006 on an absent one', () => {
    expect(toRest(INITIAL_CLIP_STATE).intents).toEqual([]);
    expect(selectClip(INITIAL_CLIP_STATE, null, WALKING).intents).toEqual([]);
  });

  it('writes the one component, always at speed 1.0', () => {
    expect(ANIMATION).toBe('Animation');
    const { intents } = selectClip(setSpeed(INITIAL_CLIP_STATE, 2).state, 'walk', WALKING);
    expect(intents[0]).toMatchObject({ kind: 'set-animation', speed: 1 });
  });
});

describe('run control', () => {
  it('pause freezes the bar on the tick last derived', () => {
    const state = withStamp(staged('walk', WALKING), {
      kind: 'set-animation',
      clip: 'walk',
      tick: 0,
      started: 980,
      speed: 1,
    });
    const { state: stopped, intents } = pause(state, WALKING);
    expect(intents).toEqual([{ kind: 'pause' }]);
    expect(stopped.playing).toBe(false);
    expect(stopped.tick).toBe(20);
  });

  it('the toggle is one gesture either way, and an idempotent one', () => {
    const running = staged('walk', WALKING);
    const stopped = togglePlay(running, WALKING).state;
    expect(stopped.playing).toBe(false);
    expect(togglePlay(stopped, WALKING).intents).toEqual([{ kind: 'resume' }]);
    expect(play(running).intents).toEqual([]);
    expect(pause(stopped, WALKING).intents).toEqual([]);
  });

  it('resuming from a scrub continues from the tick scrubbed to, with nothing re-written', () => {
    const scrubbed = scrubTo(staged('walk', WALKING), 27, WALKING).state;
    expect(play(scrubbed).intents).toEqual([{ kind: 'resume' }]);
    const started = scrubbed.started as number;
    // Ten frames later the clip is ten ticks further on — not back at its beginning.
    expect(phaseTick(WALKING.frame + 10, started, 1, WALKING.clipTicks, true)).toBe(37 % 36);
    expect(phaseTick(WALKING.frame, started, 1, WALKING.clipTicks, true)).toBe(27);
  });

  it('restart re-stamps at tick 0 and leaves the run state alone', () => {
    const stopped = scrubTo(staged('walk', WALKING), 27, WALKING).state;
    const { state, intents } = restart(stopped, WALKING);
    expect(intents).toEqual([
      { kind: 'set-animation', clip: 'walk', tick: 0, started: 1000, speed: 1 },
    ]);
    expect(state.playing).toBe(false);
    expect(state.tick).toBe(0);
  });

  it('a speed is validated before it can reach the wire', () => {
    expect(setSpeed(INITIAL_CLIP_STATE, 2).intents).toEqual([{ kind: 'set-speed', speed: 2 }]);
    for (const bad of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(setSpeed(INITIAL_CLIP_STATE, bad).intents).toEqual([]);
    }
  });

  it('every control is inert at the rest pose', () => {
    for (const transition of [
      restart(INITIAL_CLIP_STATE, WALKING),
      scrubTo(INITIAL_CLIP_STATE, 12, WALKING),
    ]) {
      expect(transition.intents).toEqual([]);
    }
  });
});

describe('scrubbing', () => {
  it('stops the clock, then writes the subtraction', () => {
    const { state, intents } = scrubTo(staged('walk', WALKING), 27, WALKING);
    expect(intents).toEqual([
      { kind: 'pause' },
      { kind: 'set-animation', clip: 'walk', tick: 27, started: 973, speed: 1 },
    ]);
    expect(state).toMatchObject({ playing: false, tick: 27, started: 973 });
  });

  it('an already-stopped session pauses nothing twice', () => {
    const stopped = scrubTo(staged('walk', WALKING), 27, WALKING).state;
    expect(scrubTo(stopped, 9, WALKING).intents.map((intent) => intent.kind)).toEqual([
      'set-animation',
    ]);
  });

  it('a session younger than the tick steps forward first — `started` is unsigned', () => {
    const cold: ClipContext = { ...WALKING, frame: 10 };
    const { state, intents } = scrubTo(staged('walk', cold), 27, cold);
    expect(intents).toEqual([
      { kind: 'pause' },
      { kind: 'step', ticks: 17 },
      { kind: 'set-animation', clip: 'walk', tick: 27, started: 0, speed: 1 },
    ]);
    // The whole point of the guard: at the stepped-to frame the subtraction reaches tick 27 exactly.
    expect(phaseTick(27, state.started as number, 1, cold.clipTicks, true)).toBe(27);
  });

  it('clamps to the clip, so the bar can never name a tick the clip has not got', () => {
    const state = staged('wave', WAVING);
    expect(scrubTo(state, 999, WAVING).state.tick).toBe(durationTicks(WAVE.duration));
    expect(scrubTo(state, -4, WAVING).state.tick).toBe(0);
    expect(scrubTo(state, Number.NaN, WAVING).state.tick).toBe(0);
  });

  it('re-stamps at the frame the write really lands on', () => {
    const [, intent] = scrubTo(staged('walk', WALKING), 27, WALKING).intents;
    expect(intent.kind).toBe('set-animation');
    if (intent.kind !== 'set-animation') {
      throw new Error('unreachable');
    }
    // Four frames slipped between the decision and the write; the tick is the authority, not the
    // predicted `started`.
    const landed = restamped(intent, 1004);
    expect(landed.started).toBe(977);
    expect(withStamp(staged('walk', WALKING), landed)).toMatchObject({ tick: 27, started: 977 });
  });
});

describe('the tick on screen', () => {
  it('is derived from the session frame while it runs, and held while it is stopped', () => {
    const running = withStamp(staged('walk', WALKING), {
      kind: 'set-animation',
      clip: 'walk',
      tick: 0,
      started: 991,
      speed: 1,
    });
    expect(shownTick(running, WALKING)).toBe(9);
    // Twelve frames on, twelve ticks on — the frame is the only clock.
    expect(shownTick(running, { ...WALKING, frame: 1012 })).toBe(21);

    const stopped = scrubTo(running, 27, WALKING).state;
    expect(shownTick(stopped, { ...WALKING, frame: 5000 })).toBe(27);
  });

  it('is 0 at the rest pose, whatever the frame', () => {
    expect(shownTick(INITIAL_CLIP_STATE, { ...WALKING, frame: 5000 })).toBe(0);
  });

  it('a one-shot holds its last tick; a loop wraps', () => {
    const wave = withStamp(staged('wave', WAVING), {
      kind: 'set-animation',
      clip: 'wave',
      tick: 0,
      started: 900,
      speed: 1,
    });
    expect(shownTick(wave, WAVING)).toBe(durationTicks(WAVE.duration));
    const walk = withStamp(staged('walk', WALKING), {
      kind: 'set-animation',
      clip: 'walk',
      tick: 0,
      started: 900,
      speed: 1,
    });
    expect(shownTick(walk, WALKING)).toBe(100 % 36);
  });

  it('adopts the pose the engine really came to rest on', () => {
    const state: ClipState = { ...staged('walk', WALKING), playing: false, started: 991, tick: 9 };
    // `sim.pause` acked two frames later than the poll had seen.
    expect(withPosedTick(state, { ...WALKING, frame: 1002 }).tick).toBe(11);
    expect(withPosedTick(INITIAL_CLIP_STATE, WALKING)).toBe(INITIAL_CLIP_STATE);
  });
});

describe('the one-shot loop', () => {
  const held: ClipState = {
    ...staged('wave', WAVING),
    started: 900,
    tick: 0,
  };

  it('fires once the clip has run out', () => {
    expect(loopRestartDue(setLoopOneShot(held, true).state, WAVING)).toBe(true);
    const fresh = { ...WAVING, frame: 900 + durationTicks(WAVE.duration) - 1 };
    expect(loopRestartDue(setLoopOneShot(held, true).state, fresh)).toBe(false);
  });

  it('is off unless asked for, and never applies to a clip that loops in the engine', () => {
    expect(loopRestartDue(held, WAVING)).toBe(false);
    const walk = { ...setLoopOneShot(held, true).state, clip: 'walk' };
    expect(loopRestartDue(walk, WALKING)).toBe(false);
  });

  it('never fires on a stopped session or at the rest pose', () => {
    const stopped = { ...setLoopOneShot(held, true).state, playing: false };
    expect(loopRestartDue(stopped, WAVING)).toBe(false);
    expect(loopRestartDue(setLoopOneShot(INITIAL_CLIP_STATE, true).state, WAVING)).toBe(false);
  });
});

describe('the bar’s geometry', () => {
  it('maps a fraction of the bar to a tick and back', () => {
    expect(tickAtRatio(0, 36)).toBe(0);
    expect(tickAtRatio(0.75, 36)).toBe(27);
    expect(tickAtRatio(1, 36)).toBe(36);
    // A pointer dragged past either end of the bar.
    expect(tickAtRatio(-0.4, 36)).toBe(0);
    expect(tickAtRatio(1.4, 36)).toBe(36);
    expect(barRatio(27, 36)).toBe(0.75);
    expect(barRatio(99, 36)).toBe(1);
  });

  it('a clip with no length draws everything at its start rather than dividing by zero', () => {
    expect(barRatio(5, 0)).toBe(0);
    expect(tickAtRatio(0.5, 0)).toBe(0);
    expect(clampTick(5, 0)).toBe(0);
  });
});

describe('an overlay rate', () => {
  const FAST: ClipContext = { frame: 1000, clipTicks: 60, loops: true, rate: 1.2 };

  it('a scrub poses the tick it names, to within the rounding of the sim-tick offset', () => {
    const state = staged('run', FAST);
    for (const tick of [0, 12, 30, 31, 59]) {
      const landed = scrubTo(state, tick, FAST).state;
      const posed = phaseTick(FAST.frame, landed.started as number, 1.2, 60, true);
      expect(Math.abs(posed - tick)).toBeLessThanOrEqual(1);
    }
    expect(scrubTo(state, 30, FAST).state.started).toBe(975);
  });

  it('advances 1.2 clip ticks per sim frame', () => {
    const running = withStamp(staged('run', FAST), {
      kind: 'set-animation',
      clip: 'run',
      tick: 0,
      started: 900,
      speed: 1,
    });
    const long = { ...FAST, clipTicks: 1000, loops: false };
    expect(shownTick(running, long)).toBe(120);
    expect(shownTick(running, { ...long, frame: 1010 })).toBe(132);
  });

  it('steps a cold session by sim ticks, not clip ticks', () => {
    const cold = { ...FAST, frame: 10 };
    const { intents } = scrubTo(staged('run', cold), 60, cold);
    expect(intents).toContainEqual({ kind: 'step', ticks: 40 });
  });

  it('leaves a rate-less context untouched', () => {
    expect(scrubTo(staged('walk', WALKING), 27, WALKING).state.started).toBe(973);
  });
});

describe('a crossfade', () => {
  const at = (frame: number): ClipContext => ({ ...WALKING, frame });

  it('stamps the incoming clip at tick 0 with the outgoing clip’s elapsed time behind it', () => {
    const { state, intents } = crossfadeTo(staged('walk', WALKING), 'wave', 0.3, at(1040));
    expect(intents).toEqual([
      {
        kind: 'set-animation',
        clip: 'wave',
        tick: 0,
        started: 1040,
        speed: 1,
        fade: { from: 'walk', fromElapsed: 40, seconds: 0.3 },
        fromStarted: 1000,
        fadeStarted: 1040,
      },
    ]);
    expect(state).toMatchObject({
      clip: 'wave',
      playing: true,
      fade: { from: 'walk', fromElapsed: 40, seconds: 0.3 },
    });
    const [intent] = intents;
    expect(animationValue(intent as never)).toEqual({
      clip: 'wave',
      started: 1040,
      speed: 1,
      from: 'walk',
      from_started: 1000,
      from_speed: 1,
      fade_started: 1040,
      fade: 0.3,
    });
  });

  it('resumes a stopped session after the stamp', () => {
    const stopped = pause(staged('walk', WALKING), WALKING).state;
    const { intents } = crossfadeTo(stopped, 'wave', 0.2, WALKING);
    expect(intents.map((intent) => intent.kind)).toEqual(['set-animation', 'resume']);
  });

  it('is nothing at the rest pose, and a hard cut for a length the engine cannot blend over', () => {
    expect(crossfadeTo(INITIAL_CLIP_STATE, 'walk', 0.2, WALKING).intents).toEqual([]);
    for (const seconds of [0, -1, Number.NaN]) {
      const { state, intents } = crossfadeTo(staged('walk', WALKING), 'wave', seconds, WALKING);
      expect(intents).toEqual([
        { kind: 'set-animation', clip: 'wave', tick: 0, started: 1000, speed: 1 },
      ]);
      expect(state.fade).toBeNull();
    }
  });

  it('is kept by restart and scrub, and cleared by a plain select and by rest', () => {
    const faded = crossfadeTo(staged('walk', WALKING), 'wave', 0.3, at(1040)).state;
    const c = at(1100);
    expect(restart(faded, c).state.fade).toEqual(faded.fade);
    const [restartIntent] = restart(faded, c).intents;
    expect(restartIntent).toMatchObject({ started: 1100, fromStarted: 1060, fadeStarted: 1100 });
    expect(scrubTo(faded, 10, c).state.fade).toEqual(faded.fade);
    expect(selectClip(faded, 'walk', c).state.fade).toBeNull();
    expect(toRest(faded).state.fade).toBeNull();
  });

  it('moves all three stamps together when scrubbed inside the window', () => {
    const faded = crossfadeTo(staged('walk', WALKING), 'wave', 0.3, at(1040)).state;
    const { intents } = scrubTo(faded, 9, at(1100));
    const stamp = intents[intents.length - 1];
    expect(stamp).toMatchObject({ started: 1091, fadeStarted: 1091, fromStarted: 1051 });
  });

  it('steps a young session far enough for the outgoing clip’s offset', () => {
    const faded: ClipState = {
      ...INITIAL_CLIP_STATE,
      clip: 'wave',
      started: 5,
      fade: { from: 'walk', fromElapsed: 40, seconds: 0.3 },
    };
    const { intents } = scrubTo(faded, 0, at(5));
    expect(intents).toEqual([
      { kind: 'pause' },
      { kind: 'step', ticks: 35 },
      expect.objectContaining({ started: 40, fromStarted: 0, fadeStarted: 40 }),
    ]);
  });

  it('sizes the bar’s band in ticks', () => {
    expect(fadeTicks({ from: 'walk', fromElapsed: 0, seconds: 0.3 })).toBe(18);
  });
});
