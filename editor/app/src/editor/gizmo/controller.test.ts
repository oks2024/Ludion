import { describe, expect, it } from 'vitest';
import { buildViewProj, worldToScreen, type CameraParams, type Transform, type Viewport } from './camera';
import type { SnapSettings } from './drag';
import type { HandleId } from './model';
import type { EntityId } from '../undo';
import { GizmoController } from './controller';

const SIZE: Viewport = { width: 800, height: 600 };
const CAM: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
const PERSP: CameraParams = { projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 };
const KIT = buildViewProj(CAM, PERSP, SIZE);
const NO_SNAP: SnapSettings = { translateStep: 0, rotateStepDeg: 0, scaleStep: 0 };
const BASE: Transform = { translation: [0, 0, -10], rotation: [0, 0, 0], scale: [1, 1, 1] };
const ENTITY: EntityId = { index: 5, generation: 1 };
const X_AXIS: HandleId = { kind: 'axis', axis: 'x' };

/** The engine pixel a world point (in front of the camera) projects to. */
function px(world: [number, number, number]) {
  return worldToScreen(world, KIT.viewProj, SIZE)!;
}

function down(c: GizmoController, handle: HandleId = X_AXIS) {
  return c.pointerDown(handle, 'translate', BASE, ENTITY, px([0, 0, -10]), KIT, SIZE);
}

describe('GizmoController pointerDown', () => {
  it('starts a drag and suppresses the pick click', () => {
    const c = new GizmoController();
    const instr = down(c);
    expect(instr).not.toBeNull();
    expect(instr?.suppressClick).toBe(true);
    expect(c.dragging).toBe(true);
  });

  it('returns null (no drag) when the pick is degenerate, so click-picking runs', () => {
    const c = new GizmoController();
    // The Z axis is edge-on to the straight-on camera → beginDrag fails.
    const instr = c.pointerDown({ kind: 'axis', axis: 'z' }, 'translate', BASE, ENTITY, px([0, 0, -10]), KIT, SIZE);
    expect(instr).toBeNull();
    expect(c.dragging).toBe(false);
  });

  it('ignores a move before any drag started', () => {
    const c = new GizmoController();
    expect(c.pointerMove(px([1, 0, -10]), 0, NO_SNAP)).toEqual({});
  });
});

describe('GizmoController drag lifecycle', () => {
  it('throttles preview sends but always yields a preview', () => {
    const c = new GizmoController(66);
    down(c);

    const m1 = c.pointerMove(px([1, 0, -10]), 0, NO_SNAP);
    expect(m1.preview?.translation[0]).toBeCloseTo(1, 4);
    expect(m1.send).toBeDefined(); // first changed move always sends (clock starts at -Infinity)

    const m2 = c.pointerMove(px([1.5, 0, -10]), 10, NO_SNAP);
    expect(m2.preview?.translation[0]).toBeCloseTo(1.5, 4);
    expect(m2.send).toBeUndefined(); // within the 66 ms window

    const m3 = c.pointerMove(px([2, 0, -10]), 100, NO_SNAP);
    expect(m3.preview?.translation[0]).toBeCloseTo(2, 4);
    expect(m3.send).toBeDefined(); // window elapsed
  });

  it('always sends the exact final value at release, even after a throttled move', () => {
    const c = new GizmoController(66);
    down(c);
    c.pointerMove(px([1, 0, -10]), 0, NO_SNAP); // sends V1
    c.pointerMove(px([3, 0, -10]), 5, NO_SNAP); // throttled: value never sent

    const up = c.pointerUp();
    expect(up.done).toBe(true);
    expect(up.send?.translation[0]).toBeCloseTo(3, 4); // the exact last pose, not the last SENT one
    expect(up.entry).toBeDefined();
    expect(up.entry).toMatchObject({ kind: 'set', entity: ENTITY, component: 'Transform' });
    expect((up.entry as { before: Transform }).before).toEqual(BASE);
    expect((up.entry as { after: Transform }).after.translation[0]).toBeCloseTo(3, 4);
    expect(c.dragging).toBe(false);
  });

  it('records nothing and sends nothing when the pose never left its mousedown value', () => {
    const c = new GizmoController();
    down(c);
    const move = c.pointerMove(px([0, 0, -10]), 0, NO_SNAP); // back at the origin → no change
    expect(move.send).toBeUndefined();

    const up = c.pointerUp();
    expect(up).toEqual({ done: true });
  });

  it('treats an immediate release (no move at all) as a no-op', () => {
    const c = new GizmoController();
    down(c);
    expect(c.pointerUp()).toEqual({ done: true });
  });

  it('restores the engine when the drag returns exactly to base after previews went out', () => {
    const c = new GizmoController(66);
    down(c);
    c.pointerMove(px([2, 0, -10]), 0, NO_SNAP); // sends a real preview
    const back = c.pointerMove(px([0, 0, -10]), 100, NO_SNAP); // back onto base — must still send
    expect(back.send).toEqual(BASE);

    const c2 = new GizmoController(66);
    down(c2);
    c2.pointerMove(px([2, 0, -10]), 0, NO_SNAP); // sends
    c2.pointerMove(px([0, 0, -10]), 10, NO_SNAP); // back onto base, throttled: NOT sent
    const up = c2.pointerUp();
    expect(up.send).toEqual(BASE); // release must restore base, or the engine stays at [2,0,-10]
    expect(up.entry).toBeUndefined(); // still not an edit
  });
});

describe('GizmoController escape', () => {
  it('restores the mousedown value and records no entry', () => {
    const c = new GizmoController();
    down(c);
    c.pointerMove(px([2, 0, -10]), 0, NO_SNAP); // a real move → dirtied

    const esc = c.escape();
    expect(esc.done).toBe(true);
    expect(esc.send).toEqual(BASE); // restore
    expect(esc.entry).toBeUndefined();
    expect(c.dragging).toBe(false);
    // A second escape has no active drag.
    expect(c.escape()).toEqual({});
  });

  it('sends nothing on cancel when no preview ever went out', () => {
    const c = new GizmoController();
    down(c);
    expect(c.escape()).toEqual({ done: true });
  });
});

describe('GizmoController with a parent frame', () => {
  const FRAME = {
    world: BASE,
    parent: { rotation: [0, 0, 0, 1] as const, scale: [2, 2, 2] as const },
  };
  const LOCAL: Transform = { translation: [1, 1, 1], rotation: [0, 0, 0], scale: [1, 1, 1] };

  it('sends and undoes the LOCAL value while handing the overlay the world pose', () => {
    const c = new GizmoController(0);
    c.pointerDown(X_AXIS, 'translate', LOCAL, ENTITY, px([0, 0, -10]), KIT, SIZE, FRAME);
    const move = c.pointerMove(px([2, 0, -10]), 0, NO_SNAP);
    expect(move.send!.translation[0]).toBeCloseTo(2, 4); // 1 + 2/2 under a scale-2 parent
    expect(move.world!.translation[0]).toBeCloseTo(2, 4);
    const up = c.pointerUp();
    expect((up.entry as { before: Transform }).before).toEqual(LOCAL);
    expect(up.world!.translation[0]).toBeCloseTo(2, 4);
  });

  it('a cancel restores the local base and the frame world pose', () => {
    const c = new GizmoController(0);
    c.pointerDown(X_AXIS, 'translate', LOCAL, ENTITY, px([0, 0, -10]), KIT, SIZE, FRAME);
    c.pointerMove(px([2, 0, -10]), 0, NO_SNAP);
    const esc = c.escape();
    expect(esc.send).toEqual(LOCAL);
    expect(esc.world).toEqual(BASE);
  });
});
