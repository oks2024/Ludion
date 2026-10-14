import { describe, expect, it } from 'vitest';
import { eulerXyzDegToQuat } from '../gizmo/camera';
import { mat4FromRotationTranslation, vNormalize, vScale, type Vec3 } from '../gizmo/vecmath';
import { ELEVATION_LIMIT_DEG, STAGE_FOV_Y_DEG, type CameraPose } from '../voxel/orbit';
import {
  DEFAULT_FLY_SPEED,
  FLY_SPEED_PER_STEP,
  MAX_FLY_SPEED,
  MAX_FOV_DELTA_DEG,
  MIN_FLY_SPEED,
  MIN_FOV_DELTA_DEG,
  SHIFT_SPEED_MULTIPLIER,
  adjustFlySpeed,
  adjustFov,
  advance,
  currentFovYDeg,
  lookDrag,
  resetFov,
  type FlyKeys,
  type FlyState,
} from './fly';

/** Where a pose looks — rebuilt through the engine's own Euler convention, exactly as
 *  `orbit.test.ts`'s `forwardOf` does. */
function forwardOf(pose: CameraPose): Vec3 {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), pose.translation);
  return vNormalize([-model[8], -model[9], -model[10]]);
}

/** The camera's world-space right and up axes; a level horizon is `rightOf(pose)[1] === 0`. */
function rightOf(pose: CameraPose): Vec3 {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), pose.translation);
  return vNormalize([model[0], model[1], model[2]]);
}

function upOf(pose: CameraPose): Vec3 {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), pose.translation);
  return vNormalize([model[4], model[5], model[6]]);
}

function expectVec(actual: Vec3, expected: Vec3, digits = 6): void {
  for (const [index, value] of expected.entries()) {
    expect(actual[index]).toBeCloseTo(value, digits);
  }
}

const NO_KEYS: FlyKeys = {
  forward: false,
  back: false,
  left: false,
  right: false,
  down: false,
  up: false,
};

function input(keys: Partial<FlyKeys>, overrides: Partial<Omit<FlyState, 'keys'>> = {}): FlyState {
  return {
    keys: { ...NO_KEYS, ...keys },
    shift: false,
    speedMetersPerSecond: DEFAULT_FLY_SPEED,
    fovDeltaDeg: 0,
    ...overrides,
  };
}

const LEVEL: CameraPose = { translation: [0, 0, 0], rotation: [0, 0, 0] };

describe('advance — movement', () => {
  it('forward motion follows the view direction when pitched', () => {
    const pitched: CameraPose = { translation: [1, 2, 3], rotation: [30, 40, 0] };
    const moved = advance(pitched, input({ forward: true }), 1);
    expectVec(moved.translation, [
      pitched.translation[0] + forwardOf(pitched)[0] * DEFAULT_FLY_SPEED,
      pitched.translation[1] + forwardOf(pitched)[1] * DEFAULT_FLY_SPEED,
      pitched.translation[2] + forwardOf(pitched)[2] * DEFAULT_FLY_SPEED,
    ]);
    // Back is the exact opposite step.
    const backed = advance(pitched, input({ back: true }), 1);
    expectVec(backed.translation, [
      pitched.translation[0] - forwardOf(pitched)[0] * DEFAULT_FLY_SPEED,
      pitched.translation[1] - forwardOf(pitched)[1] * DEFAULT_FLY_SPEED,
      pitched.translation[2] - forwardOf(pitched)[2] * DEFAULT_FLY_SPEED,
    ]);
  });

  it('holding both forward and back cancels, as does left and right', () => {
    const pitched: CameraPose = { translation: [0, 0, 0], rotation: [15, 25, 0] };
    const both = advance(pitched, input({ forward: true, back: true, left: true, right: true }), 1);
    expectVec(both.translation, [0, 0, 0]);
  });

  it('Q/E move along WORLD up, not the camera\'s, even steeply pitched', () => {
    const steep: CameraPose = { translation: [0, 0, 0], rotation: [80, -55, 0] };
    const up = advance(steep, input({ up: true }), 1);
    expectVec(up.translation, [0, DEFAULT_FLY_SPEED, 0]);
    const down = advance(steep, input({ down: true }), 1);
    expectVec(down.translation, [0, -DEFAULT_FLY_SPEED, 0]);
  });

  it('Shift multiplies speed', () => {
    const plain = advance(LEVEL, input({ forward: true }), 1);
    const fast = advance(LEVEL, input({ forward: true }, { shift: true }), 1);
    expectVec(fast.translation, vScale(plain.translation, SHIFT_SPEED_MULTIPLIER));
  });

  it('a zero or negative dt is a no-op', () => {
    expect(advance(LEVEL, input({ forward: true }), 0)).toBe(LEVEL);
    expect(advance(LEVEL, input({ forward: true }), -1)).toBe(LEVEL);
  });

  it('two half-steps equal one whole step for pure translation', () => {
    const whole = advance(LEVEL, input({ forward: true, right: true, up: true }), 1);
    const half = advance(advance(LEVEL, input({ forward: true, right: true, up: true }), 0.5), input({ forward: true, right: true, up: true }), 0.5);
    expectVec(half.translation, whole.translation, 9);
  });
});

describe('lookDrag — mouse-look', () => {
  const level: CameraPose = { translation: [0, 0, 0], rotation: [0, 0, 0] };

  it('pitch clamps just short of straight up/down, in both directions', () => {
    expect(lookDrag(level, 0, -100000, 1).rotation[0]).toBeCloseTo(ELEVATION_LIMIT_DEG, 9);
    expect(lookDrag(level, 0, 100000, 1).rotation[0]).toBeCloseTo(-ELEVATION_LIMIT_DEG, 9);
  });

  it('keeps the horizon level while looking around from any yaw', () => {
    // Under Rx·Ry·Rz the X angle turns about WORLD X, so stepping it on a yawed camera rolls the view.
    let pose: CameraPose = { translation: [1, 2, 3], rotation: [0, 0, 0] };
    for (const [dx, dy] of [[300, 0], [0, -200], [250, 120], [-900, -60], [40, 400]]) {
      pose = lookDrag(pose, dx, dy);
      expect(rightOf(pose)[1]).toBeCloseTo(0, 9);
      expect(upOf(pose)[1]).toBeGreaterThan(0);
    }
    expectVec(pose.translation, [1, 2, 3]);
  });

  it('a vertical drag pitches by exactly the dragged angle without turning, at any yaw', () => {
    const yawed = lookDrag(level, 600, 0, 0.1);
    const pitched = lookDrag(yawed, 0, -300, 0.1);
    const before = forwardOf(yawed);
    const after = forwardOf(pitched);
    expect(Math.asin(after[1]) * (180 / Math.PI)).toBeCloseTo(30, 9);
    expect(Math.atan2(after[0], after[2])).toBeCloseTo(Math.atan2(before[0], before[2]), 9);
  });

  it('yaw wraps into (-180, 180]', () => {
    const base: CameraPose = { translation: [0, 0, 0], rotation: [0, 170, 0] };
    expect(lookDrag(base, -20, 0, 1).rotation[1]).toBeCloseTo(-170, 9);
  });

  it('dragging right turns the view right (toward the camera\'s own +X at yaw 0)', () => {
    const turned = lookDrag(level, 50, 0, 1);
    expectVec(forwardOf(turned), forwardOf({ translation: [0, 0, 0], rotation: [0, -50, 0] }));
  });

  it('drops any inherited roll — a fly pose is always roll-free', () => {
    const rolled: CameraPose = { translation: [0, 0, 0], rotation: [10, 20, 45] };
    expect(rightOf(lookDrag(rolled, 1, 1))[1]).toBeCloseTo(0, 9);
  });
});

describe('adjustFlySpeed — wheel', () => {
  const state = input({});

  it('is multiplicative, like orbit.ts\'s dolly', () => {
    expect(adjustFlySpeed(state, 1).speedMetersPerSecond).toBeCloseTo(
      state.speedMetersPerSecond * FLY_SPEED_PER_STEP,
      9,
    );
    expect(adjustFlySpeed(state, -1).speedMetersPerSecond).toBeCloseTo(
      state.speedMetersPerSecond / FLY_SPEED_PER_STEP,
      9,
    );
  });

  it('clamps to [MIN_FLY_SPEED, MAX_FLY_SPEED]', () => {
    expect(adjustFlySpeed(state, -1000).speedMetersPerSecond).toBe(MIN_FLY_SPEED);
    expect(adjustFlySpeed(state, 1000).speedMetersPerSecond).toBe(MAX_FLY_SPEED);
  });
});

describe('FOV — Z/C narrow/widen and reset', () => {
  const state = input({});

  it('narrowing and widening clamp to [MIN_FOV_DELTA_DEG, MAX_FOV_DELTA_DEG]', () => {
    expect(adjustFov(state, -1, 1000).fovDeltaDeg).toBe(MIN_FOV_DELTA_DEG);
    expect(adjustFov(state, 1, 1000).fovDeltaDeg).toBe(MAX_FOV_DELTA_DEG);
  });

  it('direction 0 (neither, or both, held) is a no-op', () => {
    expect(adjustFov(state, 0, 1)).toBe(state);
  });

  it('currentFovYDeg is the base plus the held delta', () => {
    const narrowed = adjustFov(state, -1, 0.1);
    expect(currentFovYDeg(narrowed)).toBeCloseTo(STAGE_FOV_Y_DEG + narrowed.fovDeltaDeg, 9);
  });

  it('resets to the base FOV on release', () => {
    const narrowed = adjustFov(state, -1, 1000);
    expect(currentFovYDeg(narrowed)).not.toBeCloseTo(STAGE_FOV_Y_DEG, 6);
    expect(currentFovYDeg(resetFov(narrowed))).toBeCloseTo(STAGE_FOV_Y_DEG, 9);
  });
});
