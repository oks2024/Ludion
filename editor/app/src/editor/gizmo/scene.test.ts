import { describe, expect, it } from 'vitest';
import { worldToScreen, type CameraKit, type Viewport } from './camera';
import type { Vec3 } from './vecmath';
import {
  OVERRIDE_FALLBACK_FOV_Y_DEG,
  readCameraKit,
  readSceneCamera,
  gizmoParentQuery,
  gizmoPoseQuery,
  mergeQueryBodies,
  readGizmoPose,
  viewOverrideKit,
} from './scene';

const SIZE: Viewport = { width: 800, height: 600 };

/** A `query.entities` match carrying a Transform + Camera. */
function cameraMatch(overrides: { transform?: unknown; camera?: unknown } = {}) {
  return {
    entity: { index: 0, generation: 0 },
    components: {
      Transform: overrides.transform ?? { translation: [0, 0, 5], rotation: [0, 0, 0], scale: [1, 1, 1] },
      Camera: overrides.camera ?? { projection: { type: 'perspective', fov_y_deg: 60 }, near: 0.1, far: 1000 },
    },
  };
}

function queryBody(entities: unknown[]) {
  return { kind: 'query_result', entities };
}

describe('readCameraKit', () => {
  it('projects a single camera into a usable kit', () => {
    const kit = readCameraKit(queryBody([cameraMatch()]), SIZE);
    expect(kit).not.toBeNull();
    expect(kit?.viewProj).toHaveLength(16);
    expect(kit?.cameraPos).toEqual([0, 0, 5]);
  });

  it('returns null with no camera (nothing to project through)', () => {
    expect(readCameraKit(queryBody([]), SIZE)).toBeNull();
  });

  it('returns null with two cameras (the active one is ambiguous)', () => {
    expect(readCameraKit(queryBody([cameraMatch(), cameraMatch()]), SIZE)).toBeNull();
  });

  it('returns null when the camera transform is malformed', () => {
    expect(readCameraKit(queryBody([cameraMatch({ transform: { translation: [0, 0] } })]), SIZE)).toBeNull();
  });

  it('returns null for a non-query body', () => {
    expect(readCameraKit({ kind: 'component_value', value: {} }, SIZE)).toBeNull();
    expect(readCameraKit(null, SIZE)).toBeNull();
  });
});

describe('the gizmo pose queries', () => {
  const E = (index: number) => ({ index, generation: 2 });
  const local = { translation: [1, 2, 3], rotation: [0, 0, 0], scale: [1, 1, 1] };
  const w = (translation: number[]) => ({ translation, rotation: [0, 0, 0], scale: [1, 1, 1], sheared: false });

  it('asks for the selection alone, with its world pose', () => {
    expect(gizmoPoseQuery(E(1))).toEqual({
      filter: [],
      include: ['Transform', 'Parent'],
      world: true,
      entities: [E(1)],
    });
  });

  it('asks for the parent only when the selection carries one', () => {
    const root = queryBody([{ entity: E(1), components: { Transform: local }, world: w([1, 2, 3]) }]);
    expect(gizmoParentQuery(root, E(1))).toBeNull();
    const child = queryBody([{ entity: E(1), components: { Transform: local, Parent: E(4) }, world: w([5, 2, 3]) }]);
    expect(gizmoParentQuery(child, E(1))).toEqual({ filter: [], include: [], world: true, entities: [E(4)] });
    expect(gizmoParentQuery(queryBody([]), E(1))).toBeNull();
  });

  it('merges the two replies so readGizmoPose finds the parent frame', () => {
    const child = queryBody([{ entity: E(1), components: { Transform: local, Parent: E(4) }, world: w([5, 2, 3]) }]);
    const parent = queryBody([{ entity: E(4), components: {}, world: { ...w([4, 0, 0]), scale: [2, 2, 2] } }]);
    const merged = mergeQueryBodies(child, parent);
    expect(merged.entities).toHaveLength(2);
    expect(readGizmoPose(merged, E(1))?.frame?.parent.scale).toEqual([2, 2, 2]);
    expect(mergeQueryBodies(child, null).entities).toHaveLength(1);
  });
});

describe('readGizmoPose', () => {
  const E = (index: number) => ({ index, generation: 0 });
  const local = { translation: [1, 2, 3], rotation: [0, 0, 0], scale: [1, 1, 1] };
  const world = (rotation: number[], scale: number[], translation = [5, 0, 0]) => ({ translation, rotation, scale, sheared: false });

  it('reads a root as local only, with no frame', () => {
    const body = queryBody([{ entity: E(1), components: { Transform: local }, world: world([0, 0, 0], [1, 1, 1]) }]);
    expect(readGizmoPose(body, E(1))).toEqual({
      local: { translation: [1, 2, 3], rotation: [0, 0, 0], scale: [1, 1, 1] },
      frame: null,
    });
  });

  it('applies field defaults to a sparse Transform', () => {
    const body = queryBody([{ entity: E(1), components: { Transform: { translation: [1, 2, 3] } } }]);
    expect(readGizmoPose(body, E(1))?.local).toEqual({ translation: [1, 2, 3], rotation: [0, 0, 0], scale: [1, 1, 1] });
  });

  it('takes a parented entity world pose from its match and the frame from the parent world', () => {
    const body = queryBody([
      { entity: E(0), components: { Transform: local }, world: world([0, 90, 0], [2, 2, 2], [9, 9, 9]) },
      { entity: E(1), components: { Transform: local, Parent: E(0) }, world: world([0, 0, 0], [1, 1, 1], [4, 5, 6]) },
    ]);
    const pose = readGizmoPose(body, E(1));
    expect(pose?.frame?.world.translation).toEqual([4, 5, 6]);
    expect(pose?.frame?.parent.scale).toEqual([2, 2, 2]);
    const q = pose!.frame!.parent.rotation;
    expect(q[1]).toBeCloseTo(Math.SQRT1_2, 6); // 90° about Y
    expect(q[3]).toBeCloseTo(Math.SQRT1_2, 6);
  });

  it('uses an identity parent frame when the parent reports no world', () => {
    const body = queryBody([
      { entity: E(0), components: { Transform: local } },
      { entity: E(1), components: { Transform: local, Parent: E(0) }, world: world([0, 0, 0], [1, 1, 1]) },
    ]);
    expect(readGizmoPose(body, E(1))?.frame?.parent).toEqual({ rotation: [0, 0, 0, 1], scale: [1, 1, 1] });
  });

  it('is null for a missing entity, a missing or malformed Transform, and a non-query body', () => {
    const body = queryBody([{ entity: E(1), components: {} }, { entity: E(2), components: { Transform: { scale: 'big' } } }]);
    expect(readGizmoPose(body, E(9))).toBeNull();
    expect(readGizmoPose(body, E(1))).toBeNull();
    expect(readGizmoPose(body, E(2))).toBeNull();
    expect(readGizmoPose({ kind: 'ack' }, E(1))).toBeNull();
    expect(readGizmoPose(null, E(1))).toBeNull();
  });
});

describe('viewOverrideKit', () => {
  const scene = readSceneCamera(queryBody([cameraMatch()]));

  it('reproduces the scene camera exactly when the pose is the scene camera pose', () => {
    // The property the gizmo depends on: the client's override kit and the renderer's camera are
    // the same projection, so a handle drawn through one lands under a cursor read through the other.
    const kit = viewOverrideKit({ translation: [0, 0, 5], rotation: [0, 0, 0] }, 60, scene, SIZE);
    const reference = readCameraKit(queryBody([cameraMatch()]), SIZE);
    expect(reference).not.toBeNull();
    for (const point of [
      [0, 0, 0],
      [1.5, -2, -3],
      [-4, 1, 2],
    ] as const) {
      const a = worldToScreen([point[0], point[1], point[2]], kit.viewProj, SIZE);
      const b = worldToScreen([point[0], point[1], point[2]], (reference as CameraKit).viewProj, SIZE);
      expect(a).not.toBeNull();
      expect(a?.x).toBeCloseTo(b?.x ?? NaN, 6);
      expect(a?.y).toBeCloseTo(b?.y ?? NaN, 6);
    }
  });

  it('stands where the pose says, whatever the scene camera is doing', () => {
    const kit = viewOverrideKit({ translation: [12, 30, -4], rotation: [-20, 45, 0] }, null, scene, SIZE);
    expect(kit.cameraPos).toEqual([12, 30, -4]);
  });

  it('takes the named field of view, else the scene camera’s own', () => {
    const narrow = viewOverrideKit({ translation: [0, 0, 5], rotation: [0, 0, 0] }, 20, scene, SIZE);
    const inherited = viewOverrideKit({ translation: [0, 0, 5], rotation: [0, 0, 0] }, null, scene, SIZE);
    const at: Vec3 = [1, 0, 0];
    const narrowPoint = worldToScreen(at, narrow.viewProj, SIZE);
    const inheritedPoint = worldToScreen(at, inherited.viewProj, SIZE);
    // A narrower angle magnifies: the same world point sits further from the centre.
    expect(Math.abs((narrowPoint?.x ?? 0) - SIZE.width / 2)).toBeGreaterThan(
      Math.abs((inheritedPoint?.x ?? 0) - SIZE.width / 2),
    );
  });

  it('falls back to the default framing for an orthographic scene camera and for no camera at all', () => {
    const ortho = readSceneCamera(queryBody([cameraMatch({ camera: { projection: { type: 'orthographic', height: 8 } } })]));
    const at: Vec3 = [1, 0, 0];
    const throughOrtho = viewOverrideKit({ translation: [0, 0, 5], rotation: [0, 0, 0] }, null, ortho, SIZE);
    const throughNothing = viewOverrideKit({ translation: [0, 0, 5], rotation: [0, 0, 0] }, null, null, SIZE);
    const explicit = viewOverrideKit(
      { translation: [0, 0, 5], rotation: [0, 0, 0] },
      OVERRIDE_FALLBACK_FOV_Y_DEG,
      null,
      SIZE,
    );
    expect(worldToScreen(at, throughOrtho.viewProj, SIZE)?.x).toBeCloseTo(
      worldToScreen(at, explicit.viewProj, SIZE)?.x ?? NaN,
      6,
    );
    expect(worldToScreen(at, throughNothing.viewProj, SIZE)?.x).toBeCloseTo(
      worldToScreen(at, explicit.viewProj, SIZE)?.x ?? NaN,
      6,
    );
  });
});
