import { describe, expect, it } from 'vitest';
import { buildViewProj, eulerXyzDegToQuat, worldToScreen, type CameraParams, type Transform, type Viewport } from './camera';
import { beginDrag, dragWorld, endDrag, shouldSendUpdate, snapValue, updateDrag, type GizmoFrame, type SnapSettings } from './drag';
import type { HandleId } from './model';
import { quatFromAxisAngle, quatMul, vFinite, type Quat } from './vecmath';

const SIZE: Viewport = { width: 800, height: 600 };
const CAM_XFORM: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
const PERSP: CameraParams = { projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 };
const KIT = buildViewProj(CAM_XFORM, PERSP, SIZE);
const NO_SNAP: SnapSettings = { translateStep: 0, rotateStepDeg: 0, scaleStep: 0 };

/** The base entity: at (0,0,−10), identity rotation, unit scale — straight in front of the camera. */
const BASE: Transform = { translation: [0, 0, -10], rotation: [0, 0, 0], scale: [1, 1, 1] };

/** The engine pixel a world point projects to (must be in front of the camera). */
function px(world: [number, number, number]) {
  return worldToScreen(world, KIT.viewProj, SIZE)!;
}

describe('translate-axis drag', () => {
  const id: HandleId = { kind: 'axis', axis: 'x' };

  it('translates along X by the world amount the pointer traces', () => {
    // Mousedown at the origin's pixel, drag to where (2,0,−10) projects → +2 along X.
    const session = beginDrag(id, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    expect(session).not.toBeNull();
    expect(session.kind).toBe('translate-axis');
    const out = updateDrag(session, px([2, 0, -10]), NO_SNAP);
    expect(out.translation[0]).toBeCloseTo(2, 4);
    expect(out.translation[1]).toBeCloseTo(0, 4);
    expect(out.translation[2]).toBeCloseTo(-10, 4);
    expect(out.rotation).toEqual([0, 0, 0]);
    expect(out.scale).toEqual([1, 1, 1]);
  });

  it('snaps the delta to translateStep', () => {
    const session = beginDrag(id, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([2.2, 0, -10]), { ...NO_SNAP, translateStep: 1 });
    expect(out.translation[0]).toBeCloseTo(2, 6); // 2.2 → nearest 1.0
  });

  it('returns null when the axis is viewed edge-on (ray parallel to axis)', () => {
    // The Z axis is parallel to the straight-on camera's pick ray → no meaningful drag.
    expect(beginDrag({ kind: 'axis', axis: 'z' }, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)).toBeNull();
  });
});

describe('translate-plane drag', () => {
  const id: HandleId = { kind: 'plane', axis: 'z' }; // XY plane

  it('translates in the plane by the traced delta', () => {
    const session = beginDrag(id, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([2, 3, -10]), NO_SNAP);
    expect(out.translation[0]).toBeCloseTo(2, 4);
    expect(out.translation[1]).toBeCloseTo(3, 4);
    expect(out.translation[2]).toBeCloseTo(-10, 4);
  });

  it('snaps each in-plane component independently', () => {
    const session = beginDrag(id, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([2.4, 2.6, -10]), { ...NO_SNAP, translateStep: 1 });
    expect(out.translation[0]).toBeCloseTo(2, 5);
    expect(out.translation[1]).toBeCloseTo(3, 5);
  });
});

describe('rotate drag', () => {
  const id: HandleId = { kind: 'ring', axis: 'z' };

  it('produces a positive angle about +Z from +X toward +Y (right-hand rule)', () => {
    // Mousedown on the +X point of the ring, drag to the +Y point → +90° about Z.
    const session = beginDrag(id, 'rotate', BASE, px([1, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([0, 1, -10]), NO_SNAP);
    expect(out.rotation[0]).toBeCloseTo(0, 4);
    expect(out.rotation[1]).toBeCloseTo(0, 4);
    expect(out.rotation[2]).toBeCloseTo(90, 4);
  });

  it('gives a negative angle for the opposite sweep', () => {
    const session = beginDrag(id, 'rotate', BASE, px([1, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([0, -1, -10]), NO_SNAP);
    expect(out.rotation[2]).toBeCloseTo(-90, 4);
  });

  it('snaps the angle to rotateStepDeg', () => {
    const session = beginDrag(id, 'rotate', BASE, px([1, 0, -10]), KIT, SIZE)!;
    // A 40° sweep with a 30° grid snaps to 30°.
    const dir: [number, number, number] = [Math.cos((40 * Math.PI) / 180), Math.sin((40 * Math.PI) / 180), -10];
    const out = updateDrag(session, px(dir), { ...NO_SNAP, rotateStepDeg: 30 });
    expect(out.rotation[2]).toBeCloseTo(30, 3);
  });

  it('holds the base value when the pointer is at the gizmo origin (zero-length direction)', () => {
    const session = beginDrag(id, 'rotate', BASE, px([1, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([0, 0, -10]), NO_SNAP); // ray hits the origin → degenerate
    expect(out).toEqual(BASE);
    expect(vFinite(out.rotation)).toBe(true);
  });
});

describe('scale drag', () => {
  it('scale-axis multiplies the base component by the parameter ratio', () => {
    // Same axis-arm HandleId as translate, but scale mode → a scale-axis session.
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'scale', BASE, px([1, 0, -10]), KIT, SIZE)!;
    expect(session.kind).toBe('scale-axis');
    const out = updateDrag(session, px([2, 0, -10]), NO_SNAP);
    expect(out.scale[0]).toBeCloseTo(2, 4); // param 1 → 2 ⇒ ratio 2 on base 1
    expect(out.scale[1]).toBeCloseTo(1, 6);
    expect(out.scale[2]).toBeCloseTo(1, 6);
    expect(out.translation).toEqual([0, 0, -10]);
  });

  it('scale-axis respects a non-unit base component', () => {
    const base: Transform = { ...BASE, scale: [3, 1, 1] };
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'scale', base, px([1, 0, -10]), KIT, SIZE)!;
    const out = updateDrag(session, px([2, 0, -10]), NO_SNAP); // ratio 2 on base 3
    expect(out.scale[0]).toBeCloseTo(6, 4);
  });

  it('scale-uniform ratios by screen distance from the origin', () => {
    const origin = px([0, 0, -10]);
    const session = beginDrag({ kind: 'center' }, 'scale', BASE, { x: origin.x + 50, y: origin.y }, KIT, SIZE)!;
    expect(session.kind).toBe('scale-uniform');
    const out = updateDrag(session, { x: origin.x + 100, y: origin.y }, NO_SNAP); // 2× the distance
    expect(out.scale[0]).toBeCloseTo(2, 6);
    expect(out.scale[1]).toBeCloseTo(2, 6);
    expect(out.scale[2]).toBeCloseTo(2, 6);
  });

  it('snaps the uniform scale result', () => {
    const origin = px([0, 0, -10]);
    const session = beginDrag({ kind: 'center' }, 'scale', BASE, { x: origin.x + 50, y: origin.y }, KIT, SIZE)!;
    const out = updateDrag(session, { x: origin.x + 108, y: origin.y }, { ...NO_SNAP, scaleStep: 0.5 });
    expect(out.scale[0]).toBeCloseTo(2, 6); // ratio ≈ 2.16 → nearest 0.5 = 2.0
  });
});

describe('finiteness under degenerate pointers', () => {
  it('never emits a non-finite translate value', () => {
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    for (const p of [
      { x: 0, y: 0 },
      { x: -1e6, y: 5e5 },
      { x: 400, y: 300 },
      px([50, 0, -10]),
    ]) {
      const out = updateDrag(session, p, NO_SNAP);
      expect(vFinite(out.translation)).toBe(true);
      expect(vFinite(out.rotation)).toBe(true);
      expect(vFinite(out.scale)).toBe(true);
    }
  });

  it('cannot begin a center drag when the entity is behind the camera', () => {
    const behind: Transform = { translation: [0, 0, 5], rotation: [0, 0, 0], scale: [1, 1, 1] };
    expect(beginDrag({ kind: 'center' }, 'scale', behind, { x: 400, y: 300 }, KIT, SIZE)).toBeNull();
  });

  it('cannot begin a center drag when the pointer sits on the gizmo origin', () => {
    const origin = px([0, 0, -10]);
    expect(beginDrag({ kind: 'center' }, 'scale', BASE, origin, KIT, SIZE)).toBeNull();
  });
});

describe('endDrag', () => {
  it('returns the base value and the last good value as one undo pair', () => {
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', BASE, px([0, 0, -10]), KIT, SIZE)!;
    const last = updateDrag(session, px([3, 0, -10]), NO_SNAP);
    const undo = endDrag(session, last);
    expect(undo.before).toEqual(BASE);
    expect(undo.after).toBe(last);
    expect(undo.after.translation[0]).toBeCloseTo(3, 4);
  });
});

describe('snapValue', () => {
  it('rounds to the nearest multiple', () => {
    expect(snapValue(2.2, 1)).toBeCloseTo(2, 9);
    expect(snapValue(2.7, 1)).toBeCloseTo(3, 9);
    expect(snapValue(47, 15)).toBeCloseTo(45, 9);
  });

  it('is a no-op for step ≤ 0', () => {
    expect(snapValue(2.345, 0)).toBe(2.345);
    expect(snapValue(2.345, -1)).toBe(2.345);
  });
});

describe('shouldSendUpdate throttle', () => {
  it('sends once the interval has elapsed and holds otherwise', () => {
    expect(shouldSendUpdate(0, 66, 66)).toBe(true);
    expect(shouldSendUpdate(0, 65, 66)).toBe(false);
    expect(shouldSendUpdate(100, 100, 66)).toBe(false);
  });
});

describe('parented drag (gizmo at the world pose, writes the local Transform)', () => {
  const WORLD: Transform = { translation: [0, 0, -10], rotation: [0, 0, 0], scale: [1, 1, 1] };
  const LOCAL: Transform = { translation: [1, 2, 3], rotation: [10, 20, 30], scale: [1.5, 0.5, 2] };
  const frameOf = (rotationEulerDeg: [number, number, number], scale: [number, number, number]): GizmoFrame => ({
    world: WORLD,
    parent: { rotation: eulerXyzDegToQuat(rotationEulerDeg), scale },
  });
  const sameRotation = (a: Quat, b: Quat): void => {
    const sign = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3] < 0 ? -1 : 1;
    for (let i = 0; i < 4; i++) expect(a[i]).toBeCloseTo(sign * b[i]!, 6);
  };

  it('a +1 world-X translate through a parent turned 90° about Y with scale 2 moves local Z by 0.5', () => {
    const frame = frameOf([0, 90, 0], [2, 2, 2]);
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', LOCAL, px([0, 0, -10]), KIT, SIZE, frame)!;
    const out = updateDrag(session, px([1, 0, -10]), NO_SNAP);
    expect(out.translation[0]).toBeCloseTo(1, 4);
    expect(out.translation[1]).toBeCloseTo(2, 4);
    expect(out.translation[2]).toBeCloseTo(3.5, 4);
    // Untouched channels keep their exact bits.
    expect(out.rotation).toEqual(LOCAL.rotation);
    expect(out.scale).toEqual(LOCAL.scale);
  });

  it('divides by a non-uniform parent scale per local axis', () => {
    const frame = frameOf([0, 0, 0], [2, 4, 1]);
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', LOCAL, px([0, 0, -10]), KIT, SIZE, frame)!;
    const out = updateDrag(session, px([1, 0, -10]), NO_SNAP);
    expect(out.translation[0]).toBeCloseTo(1.5, 4); // 1 + 1/2
  });

  it('the overlay pose follows the drag in world space', () => {
    const frame = frameOf([0, 90, 0], [2, 2, 2]);
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', LOCAL, px([0, 0, -10]), KIT, SIZE, frame)!;
    expect(dragWorld(session).translation).toEqual(WORLD.translation);
    updateDrag(session, px([1, 0, -10]), NO_SNAP);
    const w = dragWorld(session).translation;
    expect(w[0]).toBeCloseTo(1, 4);
    expect(w[1]).toBeCloseTo(0, 4);
    expect(w[2]).toBeCloseTo(-10, 4);
  });

  it('a world-Z ring under a parent turned 90° about X rotates the local frame about its Y', () => {
    const frame = frameOf([90, 0, 0], [1, 1, 1]);
    const base: Transform = { ...LOCAL, rotation: [0, 0, 0] };
    const a = (60 * Math.PI) / 180;
    const session = beginDrag({ kind: 'ring', axis: 'z' }, 'rotate', base, px([1, 0, -10]), KIT, SIZE, frame)!;
    const out = updateDrag(session, px([Math.cos(a), Math.sin(a), -10]), NO_SNAP);
    expect(out.rotation[0]).toBeCloseTo(0, 3);
    expect(out.rotation[1]).toBeCloseTo(60, 3);
    expect(out.rotation[2]).toBeCloseTo(0, 3);
    // World composition: P · local' == Rz(60°) · P · local.
    const p = frame.parent.rotation;
    const worldAfter = quatMul(p, eulerXyzDegToQuat(out.rotation));
    sameRotation(worldAfter, quatMul(quatFromAxisAngle([0, 0, 1], a), p));
    expect(out.translation).toEqual(base.translation);
    expect(out.scale).toEqual(base.scale);
  });

  it('the overlay rotation is the parent world rotation composed with the dragged local rotation', () => {
    const frame = frameOf([0, 90, 0], [1, 1, 1]);
    const base: Transform = { ...LOCAL, rotation: [0, 0, 0] };
    const a = (40 * Math.PI) / 180;
    const session = beginDrag({ kind: 'ring', axis: 'z' }, 'rotate', base, px([1, 0, -10]), KIT, SIZE, frame)!;
    const out = updateDrag(session, px([Math.cos(a), Math.sin(a), -10]), NO_SNAP);
    const expected = quatMul(frame.parent.rotation, eulerXyzDegToQuat(out.rotation));
    sameRotation(eulerXyzDegToQuat(dragWorld(session).rotation), expected);
    expect(dragWorld(session).rotation).not.toEqual(WORLD.rotation);
  });

  it('a root is untouched by the frame plumbing', () => {
    const session = beginDrag({ kind: 'axis', axis: 'x' }, 'translate', BASE, px([0, 0, -10]), KIT, SIZE, null)!;
    const out = updateDrag(session, px([2, 0, -10]), NO_SNAP);
    expect(dragWorld(session)).toBe(out);
  });
});
