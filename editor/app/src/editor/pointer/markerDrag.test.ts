import { describe, expect, it } from 'vitest';
import { buildViewProj, worldToScreen, type CameraParams, type Transform, type Viewport } from '../gizmo/camera';
import type { SnapSettings } from '../gizmo/drag';
import type { HandleId } from '../gizmo/model';
import { buildMarkerGizmo, markerTransform, MarkerDragController } from './markerDrag';

const SIZE: Viewport = { width: 800, height: 600 };
const CAM_XFORM: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
const PERSP: CameraParams = { projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 };
const KIT = buildViewProj(CAM_XFORM, PERSP, SIZE);
const NO_SNAP: SnapSettings = { translateStep: 0, rotateStepDeg: 0, scaleStep: 0 };

/** The marker's base position — straight in front of the camera, mirroring drag.test.ts's BASE entity. */
const BASE_POS: [number, number, number] = [0, 0, -10];

/** The engine pixel a world point projects to (must be in front of the camera). */
function px(world: [number, number, number]) {
  return worldToScreen(world, KIT.viewProj, SIZE)!;
}

describe('markerTransform', () => {
  it('is identity rotation and unit scale at the given position', () => {
    const t = markerTransform([1, 2, 3]);
    expect(t.translation).toEqual([1, 2, 3]);
    expect(t.rotation).toEqual([0, 0, 0]);
    expect(t.scale).toEqual([1, 1, 1]);
  });
});

describe('buildMarkerGizmo', () => {
  it('delegates to buildGizmo in translate mode', () => {
    const layout = buildMarkerGizmo(BASE_POS, KIT, SIZE)!;
    expect(layout).not.toBeNull();
    expect(layout.mode).toBe('translate');
  });

  it('returns null when the marker is behind the camera', () => {
    expect(buildMarkerGizmo([0, 0, 5], KIT, SIZE)).toBeNull();
  });
});

describe('MarkerDragController axis drag', () => {
  const id: HandleId = { kind: 'axis', axis: 'x' };

  it('moves only along the dragged axis', () => {
    const ctl = new MarkerDragController();
    const started = ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    expect(started).toBe(true);
    expect(ctl.dragging).toBe(true);

    const preview = ctl.pointerMove(px([2, 0, -10]), NO_SNAP)!;
    expect(preview).not.toBeNull();
    expect(preview[0]).toBeCloseTo(2, 4);
    expect(preview[1]).toBeCloseTo(0, 4);
    expect(preview[2]).toBeCloseTo(-10, 4);
  });
});

describe('MarkerDragController plane drag', () => {
  const id: HandleId = { kind: 'plane', axis: 'z' }; // XY plane

  it('stays in-plane', () => {
    const ctl = new MarkerDragController();
    ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    const preview = ctl.pointerMove(px([2, 3, -10]), NO_SNAP)!;
    expect(preview[0]).toBeCloseTo(2, 4);
    expect(preview[1]).toBeCloseTo(3, 4);
    expect(preview[2]).toBeCloseTo(-10, 4);
  });
});

describe('MarkerDragController escape', () => {
  it('returns the base position and clears the session', () => {
    const id: HandleId = { kind: 'axis', axis: 'x' };
    const ctl = new MarkerDragController();
    ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    ctl.pointerMove(px([5, 0, -10]), NO_SNAP);

    const restored = ctl.escape();
    expect(restored).toEqual(BASE_POS);
    expect(ctl.dragging).toBe(false);
    expect(ctl.escape()).toBeNull();
  });
});

describe('MarkerDragController degenerate pick', () => {
  it('pointerDown returns false and dragging stays false', () => {
    const ctl = new MarkerDragController();
    // The Z axis is parallel to the straight-on camera's pick ray → degenerate, per drag.test.ts.
    const started = ctl.pointerDown({ kind: 'axis', axis: 'z' }, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    expect(started).toBe(false);
    expect(ctl.dragging).toBe(false);
    expect(ctl.pointerMove(px([1, 0, -10]), NO_SNAP)).toBeNull();
    expect(ctl.pointerUp()).toBeNull();
  });
});

describe('MarkerDragController pointerUp', () => {
  it('returns the last previewed value and clears the session', () => {
    const id: HandleId = { kind: 'axis', axis: 'x' };
    const ctl = new MarkerDragController();
    ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    ctl.pointerMove(px([1, 0, -10]), NO_SNAP);
    const last = ctl.pointerMove(px([3, 0, -10]), NO_SNAP)!;

    const committed = ctl.pointerUp();
    expect(committed).toEqual(last);
    expect(committed![0]).toBeCloseTo(3, 4);
    expect(ctl.dragging).toBe(false);
    expect(ctl.pointerUp()).toBeNull();
  });
});

describe('MarkerDragController rotation/scale invariance', () => {
  it('keeps rotation identity and scale unit through a drag', () => {
    const id: HandleId = { kind: 'plane', axis: 'z' };
    const ctl = new MarkerDragController();
    ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    // Drive the drag and inspect the gizmo the marker would rebuild at the previewed position — its
    // transform must still carry identity rotation / unit scale, mirroring markerTransform's contract.
    const preview = ctl.pointerMove(px([4, -2, -10]), NO_SNAP)!;
    const t = markerTransform(preview);
    expect(t.rotation).toEqual([0, 0, 0]);
    expect(t.scale).toEqual([1, 1, 1]);

    const final = ctl.pointerUp()!;
    const finalT = markerTransform(final);
    expect(finalT.rotation).toEqual([0, 0, 0]);
    expect(finalT.scale).toEqual([1, 1, 1]);
  });
});

describe('reset', () => {
  it('abandons an in-flight drag', () => {
    const id: HandleId = { kind: 'axis', axis: 'x' };
    const ctl = new MarkerDragController();
    ctl.pointerDown(id, BASE_POS, px([0, 0, -10]), KIT, SIZE);
    expect(ctl.dragging).toBe(true);
    ctl.reset();
    expect(ctl.dragging).toBe(false);
    expect(ctl.pointerUp()).toBeNull();
    expect(ctl.escape()).toBeNull();
  });
});
