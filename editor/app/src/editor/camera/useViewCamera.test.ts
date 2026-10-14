// The hook's non-React half: the persisted pose and the `render.view` payload. Both are pure, and
// both are places where a wrong answer is invisible until a human notices their view opened in the
// wrong place or their FOV moved on its own.

import { describe, expect, it } from 'vitest';
import { eulerXyzDegToQuat } from '../gizmo/camera';
import { mat4FromRotationTranslation, vNormalize, type Vec3 } from '../gizmo/vecmath';
import { STAGE_FOV_Y_DEG, type CameraPose } from '../voxel/orbit';
import { MAX_FOV_DELTA_DEG, type FlyState } from './fly';
import { NO_FLY_KEYS } from './gestures';
import {
  DEFAULT_VIEW_POSE,
  VIEW_CAMERA_KEY_PREFIX,
  parseStoredView,
  serializeStoredView,
  viewCameraStorageKey,
  viewPoseOf,
} from './useViewCamera';

const FLY: FlyState = { keys: NO_FLY_KEYS, shift: false, speedMetersPerSecond: 4, fovDeltaDeg: 0 };
const POSE: CameraPose = { translation: [1, 2, 3], rotation: [-15, 180, 0] };

describe('viewCameraStorageKey', () => {
  it('names the view and the scene, folding separators', () => {
    expect(viewCameraStorageKey('models', 'scenes/model-stage.yaml')).toBe(
      `${VIEW_CAMERA_KEY_PREFIX}.models.scenes/model-stage.yaml`,
    );
    expect(viewCameraStorageKey('models', 'scenes\\model-stage.yaml')).toBe(
      viewCameraStorageKey('models', 'scenes/model-stage.yaml'),
    );
  });

  it('keeps two views of one scene, and two scenes of one view, apart', () => {
    expect(viewCameraStorageKey('models', 's.yaml')).not.toBe(viewCameraStorageKey('effects', 's.yaml'));
    expect(viewCameraStorageKey('models', 'a.yaml')).not.toBe(viewCameraStorageKey('models', 'b.yaml'));
  });
});

describe('stored view', () => {
  it('round-trips a pose and a mode', () => {
    for (const mode of ['view', 'game'] as const) {
      expect(parseStoredView(serializeStoredView({ pose: POSE, mode }))).toEqual({ pose: POSE, mode });
    }
  });

  it('refuses anything it did not write, rather than opening on a NaN pose', () => {
    expect(parseStoredView(null)).toBeNull();
    expect(parseStoredView('')).toBeNull();
    expect(parseStoredView('not json')).toBeNull();
    expect(parseStoredView('null')).toBeNull();
    expect(parseStoredView('[1,2,3]')).toBeNull();
    expect(parseStoredView('{"mode":"view"}')).toBeNull();
    expect(parseStoredView(JSON.stringify({ pose: POSE, mode: 'fly' }))).toBeNull();
    // A pose that is the wrong shape, or carries a number no camera can be at.
    expect(parseStoredView(JSON.stringify({ pose: { translation: [1, 2], rotation: [0, 0, 0] }, mode: 'view' }))).toBeNull();
    expect(parseStoredView(JSON.stringify({ pose: { translation: [1, 2, '3'], rotation: [0, 0, 0] }, mode: 'view' }))).toBeNull();
    expect(parseStoredView('{"pose":{"translation":[1,2,null],"rotation":[0,0,0]},"mode":"view"}')).toBeNull();
    expect(parseStoredView(JSON.stringify({ pose: { rotation: [0, 0, 0] }, mode: 'view' }))).toBeNull();
  });

  it('stores geometry only — no held FOV, no fly speed, nothing a session owns', () => {
    expect(JSON.parse(serializeStoredView({ pose: POSE, mode: 'view' }))).toEqual({
      pose: { translation: [1, 2, 3], rotation: [-15, 180, 0] },
      mode: 'view',
    });
  });
});

describe('viewPoseOf', () => {
  it('omits the FOV when the view named no base and nothing is held', () => {
    expect(viewPoseOf(POSE, FLY)).toEqual({ translation: [1, 2, 3], rotation: [-15, 180, 0] });
  });

  it('sends the view s base FOV when it named one', () => {
    expect(viewPoseOf(POSE, FLY, 60).fov_y_deg).toBe(60);
  });

  it('sends a held Z/C delta even with no base, against the stage default', () => {
    const narrowed: FlyState = { ...FLY, fovDeltaDeg: -10 };
    expect(viewPoseOf(POSE, narrowed).fov_y_deg).toBe(STAGE_FOV_Y_DEG - 10);
    expect(viewPoseOf(POSE, narrowed, 60).fov_y_deg).toBe(50);
  });

  it('clamps through `currentFovYDeg` rather than trusting the delta', () => {
    const wide: FlyState = { ...FLY, fovDeltaDeg: MAX_FOV_DELTA_DEG };
    expect(viewPoseOf(POSE, wide, 120).fov_y_deg).toBeLessThanOrEqual(130);
  });

  it('hands the wire plain mutable triples, not the pose objects', () => {
    const sent = viewPoseOf(POSE, FLY);
    expect(sent.translation).not.toBe(POSE.translation);
    expect(sent.rotation).not.toBe(POSE.rotation);
  });
});

describe('DEFAULT_VIEW_POSE', () => {
  it('looks at a subject standing on the origin', () => {
    const model = mat4FromRotationTranslation(
      eulerXyzDegToQuat(DEFAULT_VIEW_POSE.rotation),
      DEFAULT_VIEW_POSE.translation,
    );
    const forward: Vec3 = vNormalize([-model[8], -model[9], -model[10]]);
    // From [0, 2, 6] toward [0, 1, 0]: down the −Z axis, tipped slightly down.
    expect(forward[0]).toBeCloseTo(0, 6);
    expect(forward[1]).toBeLessThan(0);
    expect(forward[2]).toBeLessThan(-0.9);
  });
});
