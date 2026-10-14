import { describe, expect, it } from 'vitest';
import {
  applyWorldAxisRotation,
  buildViewProj,
  eulerXyzDegToQuat,
  parseCamera,
  parseTransform,
  quatToEulerXyzDeg,
  screenToRay,
  worldToScreen,
  worldLengthForPixels,
  type CameraParams,
  type Transform,
  type Viewport,
} from './camera';
import { vDot, vSub, type Vec3 } from './vecmath';

const SIZE: Viewport = { width: 800, height: 600 };
const IDENTITY_CAM_XFORM: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
const PERSP: CameraParams = { projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 };
const ORTHO: CameraParams = { projection: { kind: 'orthographic', height: 10 }, near: 0.1, far: 1000 };

/** Distance between two quaternions treating q and −q as equal (rotation-space distance). */
function quatSame(a: readonly number[], b: readonly number[]): number {
  const plus = Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]);
  const minus = Math.hypot(a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]);
  return Math.min(plus, minus);
}

describe('eulerXyzDegToQuat', () => {
  it('maps zero to the identity quaternion', () => {
    expect(eulerXyzDegToQuat([0, 0, 0])).toEqual([0, 0, 0, 1]);
  });

  it('matches a single-axis rotation (90° about Z)', () => {
    const q = eulerXyzDegToQuat([0, 0, 90]);
    expect(q[0]).toBeCloseTo(0, 9);
    expect(q[1]).toBeCloseTo(0, 9);
    expect(q[2]).toBeCloseTo(Math.SQRT1_2, 9);
    expect(q[3]).toBeCloseTo(Math.SQRT1_2, 9);
  });

  it('matches the hand-computed composed case [90, 90, 0] → [0.5,0.5,0.5,0.5]', () => {
    const q = eulerXyzDegToQuat([90, 90, 0]);
    expect(q[0]).toBeCloseTo(0.5, 9);
    expect(q[1]).toBeCloseTo(0.5, 9);
    expect(q[2]).toBeCloseTo(0.5, 9);
    expect(q[3]).toBeCloseTo(0.5, 9);
  });
});

describe('quatToEulerXyzDeg round-trip', () => {
  const cases: Vec3[] = [
    [0, 0, 0],
    [30, 0, 0],
    [0, 45, 0],
    [0, 0, 90],
    [45, 30, 60],
    [10, 89.5, -20], // gimbal-adjacent
    [0, 90, 0], // exact gimbal +90
    [0, -90, 0], // exact gimbal −90
    [12, -89.9, 34],
  ];
  for (const euler of cases) {
    it(`re-encodes ${JSON.stringify(euler)} to the same rotation`, () => {
      const q = eulerXyzDegToQuat(euler);
      const back = eulerXyzDegToQuat(quatToEulerXyzDeg(q));
      expect(quatSame(q, back)).toBeLessThan(1e-6);
    });
  }
});

describe('applyWorldAxisRotation', () => {
  it('rotating 90° about Z from identity yields [0,0,90]', () => {
    const e = applyWorldAxisRotation([0, 0, 0], [0, 0, 1], 90);
    expect(e[0]).toBeCloseTo(0, 6);
    expect(e[1]).toBeCloseTo(0, 6);
    expect(e[2]).toBeCloseTo(90, 6);
  });

  it('composes in world frame: two 45° Z rotations equal one 90°', () => {
    const once = applyWorldAxisRotation([0, 0, 0], [0, 0, 1], 45);
    const twice = applyWorldAxisRotation(once, [0, 0, 1], 45);
    expect(quatSame(eulerXyzDegToQuat(twice), eulerXyzDegToQuat([0, 0, 90]))).toBeLessThan(1e-6);
  });
});

describe('worldToScreen (perspective, identity camera)', () => {
  const kit = buildViewProj(IDENTITY_CAM_XFORM, PERSP, SIZE);

  it('projects a point on the view axis to screen center', () => {
    expect(worldToScreen([0, 0, -5], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(400, 6),
      y: expect.closeTo(300, 6),
    });
  });

  it('projects the top frustum edge (y = tan(fov/2) at z=−1) to the top of the screen', () => {
    const top = Math.tan(Math.PI / 6); // tan(30°)
    expect(worldToScreen([0, top, -1], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(400, 6),
      y: expect.closeTo(0, 5),
    });
  });

  it('projects the right frustum edge (x = tan(fov/2)*aspect at z=−1) to the right of the screen', () => {
    const right = Math.tan(Math.PI / 6) * (800 / 600);
    expect(worldToScreen([right, 0, -1], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(800, 5),
      y: expect.closeTo(300, 6),
    });
  });

  it('returns null for a point behind the camera', () => {
    expect(worldToScreen([0, 0, 5], kit.viewProj, SIZE)).toBeNull();
  });
});

describe('worldToScreen (orthographic, identity camera)', () => {
  const kit = buildViewProj(IDENTITY_CAM_XFORM, ORTHO, SIZE);

  it('projects the view-axis point to center regardless of depth', () => {
    expect(worldToScreen([0, 0, -5], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(400, 6),
      y: expect.closeTo(300, 6),
    });
  });

  it('maps the half-extents to the screen corner', () => {
    const halfH = 5;
    const halfW = halfH * (800 / 600);
    expect(worldToScreen([halfW, halfH, -5], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(800, 6),
      y: expect.closeTo(0, 6),
    });
  });

  it('still projects a point behind the origin (orthographic w == 1, never behind)', () => {
    expect(worldToScreen([0, 0, 5], kit.viewProj, SIZE)).not.toBeNull();
  });
});

describe('worldToScreen (perspective, moved camera)', () => {
  // Five units out on +X, yawed 90° about Y: it looks down −X, so its right is −Z. The identity
  // camera cannot tell the view matrix from its inverse; a translated, rotated one can.
  const cam: Transform = { translation: [5, 0, 0], rotation: [0, 90, 0], scale: [1, 1, 1] };
  const kit = buildViewProj(cam, PERSP, SIZE);

  it('projects the look-at point to the screen centre', () => {
    expect(worldToScreen([0, 0, 0], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(400, 5),
      y: expect.closeTo(300, 5),
    });
  });

  it('projects the right frustum edge at distance 5 to the right of the screen', () => {
    const right = Math.tan(Math.PI / 6) * (800 / 600) * 5;
    expect(worldToScreen([0, 0, -right], kit.viewProj, SIZE)).toEqual({
      x: expect.closeTo(800, 5),
      y: expect.closeTo(300, 5),
    });
  });
});

describe('screenToRay consistency', () => {
  for (const [label, cam] of [
    ['perspective', PERSP],
    ['orthographic', ORTHO],
  ] as const) {
    it(`a projected point's pixel unprojects to a ray through it (${label})`, () => {
      const kit = buildViewProj(IDENTITY_CAM_XFORM, cam, SIZE);
      const world: Vec3 = [1.5, -0.8, -7];
      const pixel = worldToScreen(world, kit.viewProj, SIZE)!;
      const ray = screenToRay(pixel, SIZE, kit.invViewProj)!;
      // Distance from the world point to the ray line must be ~0.
      const rel = vSub(world, ray.origin);
      const along = vDot(rel, ray.dir);
      const perp = vSub(rel, [ray.dir[0] * along, ray.dir[1] * along, ray.dir[2] * along]);
      expect(Math.hypot(perp[0], perp[1], perp[2])).toBeLessThan(1e-4);
    });
  }

  it('a moved camera still round-trips (view matrix path)', () => {
    const xform: Transform = { translation: [3, 2, 5], rotation: [15, -40, 0], scale: [1, 1, 1] };
    const kit = buildViewProj(xform, PERSP, SIZE);
    const world: Vec3 = [1, 1, 1];
    const pixel = worldToScreen(world, kit.viewProj, SIZE)!;
    expect(pixel).not.toBeNull();
    const ray = screenToRay(pixel, SIZE, kit.invViewProj)!;
    const rel = vSub(world, ray.origin);
    const along = vDot(rel, ray.dir);
    const perp = vSub(rel, [ray.dir[0] * along, ray.dir[1] * along, ray.dir[2] * along]);
    expect(Math.hypot(perp[0], perp[1], perp[2])).toBeLessThan(1e-3);
  });
});

describe('worldLengthForPixels', () => {
  const kit = buildViewProj(IDENTITY_CAM_XFORM, PERSP, SIZE);

  it('is null when the point is behind the camera', () => {
    expect(worldLengthForPixels([0, 0, 5], 80, kit, SIZE)).toBeNull();
  });

  it('scales with depth so apparent size stays constant (perspective)', () => {
    const near = worldLengthForPixels([0, 0, -10], 80, kit, SIZE)!;
    const far = worldLengthForPixels([0, 0, -20], 80, kit, SIZE)!;
    // Twice as far ⇒ ~twice the world length for the same pixel span.
    expect(far / near).toBeCloseTo(2, 3);
  });

  it('is constant with depth for orthographic', () => {
    const orthoKit = buildViewProj(IDENTITY_CAM_XFORM, ORTHO, SIZE);
    const a = worldLengthForPixels([0, 0, -10], 80, orthoKit, SIZE)!;
    const b = worldLengthForPixels([0, 0, -50], 80, orthoKit, SIZE)!;
    expect(a).toBeCloseTo(b, 9);
  });
});

describe('parseTransform', () => {
  it('fills all defaults for an empty object', () => {
    expect(parseTransform({})).toEqual({ translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] });
  });

  it('reads present fields and defaults absent ones', () => {
    expect(parseTransform({ translation: [1, 2, 3] })).toEqual({
      translation: [1, 2, 3],
      rotation: [0, 0, 0],
      scale: [1, 1, 1],
    });
  });

  it('returns null for a non-object', () => {
    expect(parseTransform(null)).toBeNull();
    expect(parseTransform('nope')).toBeNull();
  });

  it('returns null for a present-but-malformed field', () => {
    expect(parseTransform({ translation: [1, 2] })).toBeNull();
    expect(parseTransform({ scale: 'x' })).toBeNull();
    expect(parseTransform({ rotation: [1, 2, 'z'] })).toBeNull();
  });
});

describe('parseCamera', () => {
  it('defaults to 60° perspective, near 0.1, far 1000', () => {
    expect(parseCamera({})).toEqual({ projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 });
  });

  it('reads a perspective projection', () => {
    expect(parseCamera({ projection: { type: 'perspective', fov_y_deg: 45 }, near: 1, far: 500 })).toEqual({
      projection: { kind: 'perspective', fovYDeg: 45 },
      near: 1,
      far: 500,
    });
  });

  it('reads an orthographic projection', () => {
    expect(parseCamera({ projection: { type: 'orthographic', height: 12 } })).toEqual({
      projection: { kind: 'orthographic', height: 12 },
      near: 0.1,
      far: 1000,
    });
  });

  it('returns null for an unknown projection tag or non-object', () => {
    expect(parseCamera({ projection: { type: 'weird' } })).toBeNull();
    expect(parseCamera(42)).toBeNull();
    expect(parseCamera({ near: 'x' })).toBeNull();
  });

  it('returns null for a projection missing its payload field (no serde default in the Rust enum)', () => {
    expect(parseCamera({ projection: { type: 'perspective' } })).toBeNull();
    expect(parseCamera({ projection: { type: 'orthographic' } })).toBeNull();
    expect(parseCamera({ projection: { type: 'orthographic', height: 'tall' } })).toBeNull();
  });
});
