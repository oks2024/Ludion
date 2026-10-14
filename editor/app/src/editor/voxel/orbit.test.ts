import { describe, expect, it } from 'vitest';
import { eulerXyzDegToQuat } from '../gizmo/camera';
import { mat4FromRotationTranslation, vNormalize, type Vec3 } from '../gizmo/vecmath';
import {
  DOLLY_PER_STEP,
  ELEVATION_LIMIT_DEG,
  MAX_ORBIT_DISTANCE,
  MIN_ORBIT_DISTANCE,
  PRESETS,
  boxSphere,
  dollyBy,
  fitDistance,
  framing,
  lookAtPose,
  mergeSpheres,
  modelSphere,
  orbitDirection,
  orbitDrag,
  orbitFromDirection,
  orbitPose,
  panDrag,
  preset,
  presetOrbit,
  presetPose,
  turntableStep,
  type CameraPose,
  type PresetId,
} from './orbit';

const SQUARE = { width: 512, height: 512 };

/** Where the camera looks, rebuilt from a pose through the engine's own Euler convention
 *  (`camera.ts`'s `eulerXyzDegToQuat`) — the camera's local −Z in world space. */
function forwardOf(pose: CameraPose): Vec3 {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), pose.translation);
  return vNormalize([-model[8], -model[9], -model[10]]);
}

/** The camera's world-space up (column 1 of the model matrix). */
function upOf(pose: CameraPose): Vec3 {
  const model = mat4FromRotationTranslation(eulerXyzDegToQuat(pose.rotation), pose.translation);
  return vNormalize([model[4], model[5], model[6]]);
}

function expectVec(actual: Vec3, expected: Vec3, digits = 6): void {
  for (const [index, value] of expected.entries()) {
    expect(actual[index]).toBeCloseTo(value, digits);
  }
}

describe('presets — the contact sheet, reproduced', () => {
  it('carry the sheet cells\' own burned-in labels', () => {
    expect(PRESETS.map((p) => [p.id, p.label])).toEqual([
      ['front', 'FRONT +Z'],
      ['left', 'LEFT -X'],
      ['top', 'TOP Y'],
      ['iso', 'ISO'],
      ['game', 'GAME 58'],
    ]);
  });

  it('carry `ModelView::direction_and_up`\'s directions, not intuition\'s', () => {
    expectVec(preset('front').direction, [0, 0, -1]);
    expectVec(preset('left').direction, [-1, 0, 0]);
    expectVec(preset('top').direction, [0, 1, 0]);
    // Iso is a normalized (1, 0.75, 1).
    expectVec(preset('iso').direction, vNormalize([1, 0.75, 1]));
    // Game is a 58°-down pitch over a horizontal aim at the authored front with a +X three-quarter.
    const pitch = (58 * Math.PI) / 180;
    const horizontal = vNormalize([0.45, 0, -1]);
    expectVec(preset('game').direction, [
      horizontal[0] * Math.cos(pitch),
      Math.sin(pitch),
      horizontal[2] * Math.cos(pitch),
    ]);
    // The steep pitch IS the elevation, by construction.
    expect((Math.asin(preset('game').direction[1]) * 180) / Math.PI).toBeCloseTo(58, 9);
  });

  it('TOP frames with +Z up — the floor-plan convention, not a degenerate +Y', () => {
    expectVec(preset('top').up, [0, 0, 1]);
    // The authored front (world −Z) reads toward the bottom of the frame.
    expectVec(upOf(presetPose('top', [0, 0, 0], 5)), [0, 0, 1]);
  });

  it('every preset pose looks back down its own direction', () => {
    for (const entry of PRESETS) {
      const pose = presetPose(entry.id, [0, 1, 0], 6);
      expectVec(forwardOf(pose), [-entry.direction[0], -entry.direction[1], -entry.direction[2]], 5);
    }
  });

  it('no preset but TOP rolls the camera', () => {
    for (const id of ['front', 'left', 'iso', 'game'] as PresetId[]) {
      // A roll-free camera keeps its up vector in the plane of world up and the view direction, so
      // its X component is zero exactly when the view has no sideways tilt.
      expect(upOf(presetPose(id, [0, 0, 0], 5))[1]).toBeGreaterThan(0);
      expect(Math.abs(presetPose(id, [0, 0, 0], 5).rotation[2])).toBeLessThanOrEqual(90);
    }
  });
});

describe('lookAtPose — the engine\'s rotation convention', () => {
  it('reproduces the committed model-stage.yaml camera exactly', () => {
    // `editor/app/fixtures/stage/scenes/model-stage.yaml`: translation [0, 2, -7], rotation [0, 180, 0] — the
    // sheet's FRONT cell, dead level on −Z, raised to half the reference figure's height.
    const pose = lookAtPose([0, 2, -7], [0, 2, 0], [0, 1, 0]);
    expectVec(pose.translation, [0, 2, -7]);
    expectVec(pose.rotation, [0, 180, 0]);
  });

  it('an axis-aligned side view is a pure yaw', () => {
    expectVec(lookAtPose([-5, 0, 0], [0, 0, 0], [0, 1, 0]).rotation, [0, -90, 0]);
    expectVec(lookAtPose([5, 0, 0], [0, 0, 0], [0, 1, 0]).rotation, [0, 90, 0]);
  });

  it('a level camera on +Z needs no rotation at all', () => {
    expectVec(lookAtPose([0, 0, 5], [0, 0, 0], [0, 1, 0]).rotation, [0, 0, 0]);
  });

  it('a degenerate look (target on the camera) yields a finite pose, not NaN', () => {
    const pose = lookAtPose([1, 2, 3], [1, 2, 3], [0, 1, 0]);
    expect(pose.rotation.every(Number.isFinite)).toBe(true);
  });

  it('a view straight down the up axis yields a finite pose', () => {
    const pose = lookAtPose([0, 5, 0], [0, 0, 0], [0, 1, 0]);
    expect(pose.rotation.every(Number.isFinite)).toBe(true);
    expectVec(forwardOf(pose), [0, -1, 0], 5);
  });
});

describe('orbit state', () => {
  it('azimuth 0 is +Z, 180 is −Z, −90 is −X, elevation 90 is straight up', () => {
    expectVec(orbitDirection(0, 0), [0, 0, 1]);
    expectVec(orbitDirection(180, 0), [0, 0, -1]);
    expectVec(orbitDirection(-90, 0), [-1, 0, 0]);
    expectVec(orbitDirection(0, 90), [0, 1, 0]);
  });

  it('orbitFromDirection inverts orbitDirection', () => {
    for (const [az, el] of [
      [0, 0],
      [180, 0],
      [-90, 30],
      [45, -60],
      [155.78, 58],
    ]) {
      const state = orbitFromDirection(orbitDirection(az, el), [0, 0, 0], 5);
      expect(state.azimuthDeg).toBeCloseTo(az, 6);
      expect(state.elevationDeg).toBeCloseTo(el, 6);
    }
  });

  it('the pose an orbit state produces looks at its own target', () => {
    const state = { target: [1, 2, 3] as Vec3, distance: 4, azimuthDeg: 37, elevationDeg: 21 };
    const pose = orbitPose(state);
    const toTarget = vNormalize([
      state.target[0] - pose.translation[0],
      state.target[1] - pose.translation[1],
      state.target[2] - pose.translation[2],
    ]);
    expectVec(forwardOf(pose), toTarget, 5);
  });

  it('a preset hands the mouse a matching orbit state', () => {
    for (const entry of PRESETS) {
      const state = presetOrbit(entry.id, [0, 1, 0], 6);
      // TOP is the one that cannot survive the handover: you cannot orbit through a pole.
      const expectedElevation = Math.min(
        ELEVATION_LIMIT_DEG,
        (Math.asin(entry.direction[1]) * 180) / Math.PI,
      );
      expect(state.elevationDeg).toBeCloseTo(expectedElevation, 6);
      expect(state.distance).toBe(6);
    }
  });
});

describe('interaction', () => {
  const base = { target: [0, 1, 0] as Vec3, distance: 6, azimuthDeg: 180, elevationDeg: 10 };

  it('dragging right walks the camera to its own right', () => {
    const level = { ...base, azimuthDeg: 0, elevationDeg: 0 };
    const dragged = orbitDrag(level, 100, 0, 0.4);
    expect(dragged.azimuthDeg).toBeCloseTo(40, 9);
    // Azimuth +40 from +Z moves the camera toward +X, which is the camera's own right at az 0.
    expect(orbitPose(dragged).translation[0]).toBeGreaterThan(0);
  });

  it('elevation clamps just short of the poles, in both directions', () => {
    expect(orbitDrag(base, 0, 10000).elevationDeg).toBe(ELEVATION_LIMIT_DEG);
    expect(orbitDrag(base, 0, -10000).elevationDeg).toBe(-ELEVATION_LIMIT_DEG);
  });

  it('azimuth wraps into (-180, 180]', () => {
    expect(orbitDrag(base, 500, 0, 1).azimuthDeg).toBeCloseTo(-40, 9);
    expect(turntableStep({ ...base, azimuthDeg: 179 }, 2).azimuthDeg).toBeCloseTo(-179, 9);
    expect(turntableStep({ ...base, azimuthDeg: 0 }, 180).azimuthDeg).toBe(180);
  });

  it('the turntable moves azimuth only', () => {
    const stepped = turntableStep(base, 3);
    expect(stepped.elevationDeg).toBe(base.elevationDeg);
    expect(stepped.distance).toBe(base.distance);
    expect(stepped.target).toBe(base.target);
  });

  it('dolly is multiplicative and clamped', () => {
    expect(dollyBy(base, 1).distance).toBeCloseTo(base.distance * DOLLY_PER_STEP, 9);
    expect(dollyBy(base, -1).distance).toBeCloseTo(base.distance / DOLLY_PER_STEP, 9);
    expect(dollyBy(base, -1000).distance).toBe(MIN_ORBIT_DISTANCE);
    expect(dollyBy(base, 1000).distance).toBe(MAX_ORBIT_DISTANCE);
  });

  it('panning right slides the target left, so the subject follows the cursor', () => {
    const level = { ...base, azimuthDeg: 0, elevationDeg: 0 };
    const panned = panDrag(level, 100, 0, SQUARE);
    expect(panned.target[0]).toBeLessThan(level.target[0]);
    expect(panned.distance).toBe(level.distance);
    // One pixel is 2·d·tan(fov/2)/height at the target's depth.
    const perPixel = (2 * level.distance * Math.tan((22.5 * Math.PI) / 180)) / SQUARE.height;
    expect(level.target[0] - panned.target[0]).toBeCloseTo(100 * perPixel, 9);
  });

  it('panning down lifts the target, and a zero-height viewport is a no-op', () => {
    expect(panDrag(base, 0, 50, SQUARE).target[1]).toBeGreaterThan(base.target[1]);
    expect(panDrag(base, 10, 10, { width: 0, height: 0 })).toBe(base);
  });
});

describe('framing', () => {
  // A sample subject: a 9×9×9 grid at a declared scale of 0.24.
  const subject = modelSphere([9, 9, 9], 0.24);
  // The stage's reference figure: `adventurer` 60×60×60 at its declared 0.0675 (4.05 units tall),
  // standing 2.6 units to +X.
  const adventurer = modelSphere([60, 60, 60], 0.0675, [2.6, 0, 0]);

  it('a box sphere is the box\'s middle and half its diagonal — and agrees with modelSphere', () => {
    expect(boxSphere([-1, 0, -1], [1, 4, 1])).toEqual({ center: [0, 2, 0], radius: Math.hypot(2, 4, 2) * 0.5 });
    // A model sphere IS the sphere of the mesher's bottom-centre box, so the two must not drift.
    const box = boxSphere([-9 * 0.24 * 0.5, 0, -9 * 0.24 * 0.5], [9 * 0.24 * 0.5, 9 * 0.24, 9 * 0.24 * 0.5]);
    expect(box.radius).toBeCloseTo(subject.radius, 9);
    expectVec(box.center, subject.center);
  });

  it('a model sphere is the mesher\'s bottom-center box', () => {
    // The model stage's light rig derives from R = 0.5·|(9,9,9)|·0.24 = 1.871.
    expect(subject.radius).toBeCloseTo(1.871, 3);
    expectVec(subject.center, [0, 9 * 0.24 * 0.5, 0]);
  });

  it('fitDistance is the sheet\'s own fit formula at world scale', () => {
    // The sheet's unit-sphere distance is FIT_MARGIN/sin(22.5°) = 2.874; times R that is 5.377,
    // the number the stage's light positions derive from.
    expect(fitDistance(subject.radius, SQUARE)).toBeCloseTo(5.377, 3);
    expect(fitDistance(1, SQUARE)).toBeCloseTo(1.1 / Math.sin((22.5 * Math.PI) / 180), 9);
  });

  it('a wide viewport is limited vertically, a tall one horizontally', () => {
    const wide = fitDistance(1, { width: 1600, height: 900 });
    const square = fitDistance(1, SQUARE);
    const tall = fitDistance(1, { width: 400, height: 900 });
    expect(wide).toBeCloseTo(square, 9);
    expect(tall).toBeGreaterThan(square);
  });

  it('mergeSpheres contains both, and absorbs a contained one', () => {
    const merged = mergeSpheres(subject, adventurer);
    for (const sphere of [subject, adventurer]) {
      const gap = Math.hypot(
        sphere.center[0] - merged.center[0],
        sphere.center[1] - merged.center[1],
        sphere.center[2] - merged.center[2],
      );
      expect(gap + sphere.radius).toBeLessThanOrEqual(merged.radius + 1e-9);
    }
    const inner = modelSphere([1, 1, 1], 0.1);
    expect(mergeSpheres(subject, inner)).toBe(subject);
    expect(mergeSpheres(inner, subject)).toBe(subject);
  });

  it('fill-frame frames the subject alone; declared-scale keeps the reference in shot', () => {
    const fill = framing('fill-frame', subject, adventurer, SQUARE);
    const declared = framing('declared-scale', subject, adventurer, SQUARE);
    expect(fill.distance).toBeCloseTo(fitDistance(subject.radius, SQUARE), 9);
    expect(declared.distance).toBeGreaterThan(fill.distance);
    expect(declared.target[0]).toBeGreaterThan(fill.target[0]);
  });

  it('fill-frame makes a small model and a large one look the same size', () => {
    const small = modelSphere([4, 4, 4], 0.1);
    const large = modelSphere([32, 32, 32], 0.5);
    const ratio = (sphere: typeof small): number =>
      framing('fill-frame', sphere, null, SQUARE).distance / sphere.radius;
    expect(ratio(large)).toBeCloseTo(ratio(small), 9);
  });

  it('fitDistance stays inside the orbit range, so a huge model cannot leave the far plane', () => {
    expect(fitDistance(1000, SQUARE)).toBe(MAX_ORBIT_DISTANCE);
    expect(fitDistance(0, SQUARE)).toBe(MIN_ORBIT_DISTANCE);
  });

  it('declared-scale without a reference falls back to the subject', () => {
    expect(framing('declared-scale', subject, null, SQUARE)).toEqual(
      framing('fill-frame', subject, null, SQUARE),
    );
  });
});
