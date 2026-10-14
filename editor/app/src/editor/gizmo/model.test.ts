import { describe, expect, it } from 'vitest';
import { buildViewProj, worldToScreen, type CameraParams, type Transform, type Viewport } from './camera';
import {
  HANDLE_PIXELS,
  buildGizmo,
  hitTest,
  pointSegmentDistance,
  type GizmoHandle,
  type GizmoLayout,
} from './model';

const SIZE: Viewport = { width: 800, height: 600 };
const CAM_XFORM: Transform = { translation: [0, 0, 0], rotation: [0, 0, 0], scale: [1, 1, 1] };
const PERSP: CameraParams = { projection: { kind: 'perspective', fovYDeg: 60 }, near: 0.1, far: 1000 };
const KIT = buildViewProj(CAM_XFORM, PERSP, SIZE);

function entityAt(z: number): Transform {
  return { translation: [0, 0, z], rotation: [0, 0, 0], scale: [1, 1, 1] };
}

/** Finds a handle by kind+axis in a layout. */
function handle(layout: GizmoLayout, kind: string, axis?: string): GizmoHandle | undefined {
  return layout.handles.find((h) => h.id.kind === kind && h.id.axis === axis);
}

describe('buildGizmo layout', () => {
  it('hides the gizmo when the origin is behind the camera', () => {
    expect(buildGizmo(entityAt(5), KIT, SIZE, 'translate')).toBeNull();
  });

  it('translate mode emits three axis arms and three plane quads', () => {
    const layout = buildGizmo(entityAt(-10), KIT, SIZE, 'translate')!;
    expect(layout.handles.filter((h) => h.id.kind === 'axis')).toHaveLength(3);
    expect(layout.handles.filter((h) => h.id.kind === 'plane')).toHaveLength(3);
  });

  it('scale mode emits three axis arms and a center handle', () => {
    const layout = buildGizmo(entityAt(-10), KIT, SIZE, 'scale')!;
    expect(layout.handles.filter((h) => h.id.kind === 'axis')).toHaveLength(3);
    expect(layout.handles.filter((h) => h.id.kind === 'center')).toHaveLength(1);
  });

  it('rotate mode emits three rings', () => {
    const layout = buildGizmo(entityAt(-10), KIT, SIZE, 'rotate')!;
    expect(layout.handles.filter((h) => h.id.kind === 'ring')).toHaveLength(3);
  });

  it('keeps the X arm at a constant pixel length regardless of depth (perspective)', () => {
    for (const z of [-10, -20, -40]) {
      const layout = buildGizmo(entityAt(z), KIT, SIZE, 'translate')!;
      const arm = handle(layout, 'axis', 'x')!;
      expect(arm.geometry.type).toBe('segment');
      if (arm.geometry.type === 'segment') {
        const { a, b } = arm.geometry.segment;
        expect(Math.hypot(b.x - a.x, b.y - a.y)).toBeCloseTo(HANDLE_PIXELS, 2);
      }
    }
  });

  it('the Z ring projects to a circle of the expected screen radius about the origin', () => {
    const layout = buildGizmo(entityAt(-10), KIT, SIZE, 'rotate')!;
    const ring = handle(layout, 'ring', 'z')!;
    expect(ring.geometry.type).toBe('polyline');
    if (ring.geometry.type === 'polyline') {
      // The Z ring lies in the XY plane (parallel to the image plane) so it projects to a circle;
      // the radius equals the arm length ⇒ HANDLE_PIXELS (~80) px from the projected origin.
      const origin = worldToScreen([0, 0, -10], KIT.viewProj, SIZE)!;
      for (const p of ring.geometry.polyline.points) {
        expect(Math.hypot(p.x - origin.x, p.y - origin.y)).toBeCloseTo(HANDLE_PIXELS, 1);
      }
    }
  });
});

describe('hitTest against a real translate gizmo', () => {
  const layout = buildGizmo(entityAt(-10), KIT, SIZE, 'translate')!;
  const origin = worldToScreen([0, 0, -10], KIT.viewProj, SIZE)!; // ≈ (400, 300)

  it('hits the X axis on its arm', () => {
    // A few px right of the origin along the +X arm (screen +x).
    expect(hitTest({ x: origin.x + 40, y: origin.y }, layout)).toEqual({ kind: 'axis', axis: 'x' });
  });

  it('misses the X axis just outside tolerance', () => {
    expect(hitTest({ x: origin.x + 40, y: origin.y + 15 }, layout)).toBeNull();
  });

  it('hits the Y axis on its arm (screen −y is +Y world)', () => {
    expect(hitTest({ x: origin.x, y: origin.y - 40 }, layout)).toEqual({ kind: 'axis', axis: 'y' });
  });

  it('hits the XY plane quad on its interior', () => {
    const quad = handle(layout, 'plane', 'z')!;
    expect(quad.geometry.type).toBe('quad');
    if (quad.geometry.type === 'quad') {
      const pts = quad.geometry.quad.points;
      const cx = (pts[0].x + pts[1].x + pts[2].x + pts[3].x) / 4;
      const cy = (pts[0].y + pts[1].y + pts[2].y + pts[3].y) / 4;
      expect(hitTest({ x: cx, y: cy }, layout)).toEqual({ kind: 'plane', axis: 'z' });
    }
  });
});

describe('hitTest priority (synthetic overlapping handles)', () => {
  const ring: GizmoHandle = {
    id: { kind: 'ring', axis: 'z' },
    geometry: { type: 'polyline', polyline: { points: [{ x: 0, y: 10 }, { x: 100, y: 10 }] } },
  };
  const axis: GizmoHandle = {
    id: { kind: 'axis', axis: 'x' },
    geometry: { type: 'segment', segment: { a: { x: 0, y: 10 }, b: { x: 100, y: 10 } } },
  };
  const plane: GizmoHandle = {
    id: { kind: 'plane', axis: 'z' },
    geometry: { type: 'quad', quad: { points: [{ x: 0, y: 0 }, { x: 100, y: 0 }, { x: 100, y: 20 }, { x: 0, y: 20 }] } },
  };
  const center: GizmoHandle = {
    id: { kind: 'center' },
    geometry: { type: 'center', disc: { center: { x: 50, y: 10 }, radius: 8 } },
  };

  function layoutOf(handles: GizmoHandle[]): GizmoLayout {
    return { mode: 'translate', origin: { x: 50, y: 10 }, handles };
  }

  it('plane beats axis beats ring at a shared point', () => {
    expect(hitTest({ x: 50, y: 10 }, layoutOf([ring, axis, plane]))).toEqual({ kind: 'plane', axis: 'z' });
    expect(hitTest({ x: 50, y: 10 }, layoutOf([ring, axis]))).toEqual({ kind: 'axis', axis: 'x' });
    expect(hitTest({ x: 50, y: 10 }, layoutOf([ring]))).toEqual({ kind: 'ring', axis: 'z' });
  });

  it('center outranks the axis it overlaps', () => {
    expect(hitTest({ x: 50, y: 10 }, layoutOf([axis, center]))).toEqual({ kind: 'center' });
  });

  it('returns null when nothing is within tolerance', () => {
    expect(hitTest({ x: 50, y: 200 }, layoutOf([ring, axis]))).toBeNull();
  });
});

describe('pointSegmentDistance', () => {
  it('is zero on the segment and clamps past the endpoints', () => {
    expect(pointSegmentDistance({ x: 5, y: 0 }, { x: 0, y: 0 }, { x: 10, y: 0 })).toBe(0);
    expect(pointSegmentDistance({ x: 5, y: 3 }, { x: 0, y: 0 }, { x: 10, y: 0 })).toBe(3);
    // Past the end: distance to the endpoint, not the infinite line.
    expect(pointSegmentDistance({ x: 20, y: 0 }, { x: 0, y: 0 }, { x: 10, y: 0 })).toBe(10);
  });

  it('handles a degenerate (zero-length) segment as distance to the point', () => {
    expect(pointSegmentDistance({ x: 3, y: 4 }, { x: 0, y: 0 }, { x: 0, y: 0 })).toBe(5);
  });
});
