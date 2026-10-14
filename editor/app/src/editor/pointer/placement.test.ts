import { describe, expect, it } from 'vitest';
import type { Ray } from '../gizmo/camera';
import type { EntityScreenBounds } from '../../viewer/picking';
import { rayAabbEnter, rayGroundY0, resolvePlacement } from './placement';

/** A bounds entry with sensible defaults; override only the fields a case cares about. */
function box(
  min: readonly [number, number, number],
  max: readonly [number, number, number],
  index = 1,
): EntityScreenBounds {
  return {
    entity: { index, generation: 0 },
    onScreen: true,
    screenRect: { x: 0, y: 0, w: 10, h: 10 },
    worldCenter: [(min[0] + max[0]) / 2, (min[1] + max[1]) / 2, (min[2] + max[2]) / 2],
    worldAabb: { min, max },
  };
}

describe('rayAabbEnter', () => {
  it('returns the entering t for a direct hit', () => {
    const ray: Ray = { origin: [0, 0, -5], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBe(4);
  });

  it('returns null on a miss', () => {
    const ray: Ray = { origin: [5, 0, -5], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBeNull();
  });

  it('clamps to 0 when the origin is inside the box', () => {
    const ray: Ray = { origin: [0, 0, 0], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBe(0);
  });

  it('rejects a box entirely behind the origin', () => {
    const ray: Ray = { origin: [0, 0, 5], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBeNull();
  });

  it('hits when a zero direction component keeps the origin within that slab', () => {
    // dir.x is exactly 0 (parallel to the x slab); origin.x = 0.5 lies within [-1,1] so the other
    // two (non-degenerate) axes decide the hit.
    const ray: Ray = { origin: [0.5, 0, -5], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBe(4);
  });

  it('misses when a zero direction component puts the origin outside that slab', () => {
    const ray: Ray = { origin: [5, 0, -5], dir: [0, 0, 1] };
    expect(rayAabbEnter(ray, [-1, -1, -1], [1, 1, 1])).toBeNull();
  });
});

describe('rayGroundY0', () => {
  it('intersects the ground below the origin', () => {
    const ray: Ray = { origin: [2, 5, 3], dir: [0, -1, 0] };
    expect(rayGroundY0(ray)).toEqual([2, 0, 3]);
  });

  it('returns null when the ray is parallel to the ground', () => {
    const ray: Ray = { origin: [0, 5, 0], dir: [1, 0, 0] };
    expect(rayGroundY0(ray)).toBeNull();
  });

  it('returns null when the plane is behind the origin', () => {
    const ray: Ray = { origin: [0, -5, 0], dir: [0, -1, 0] };
    expect(rayGroundY0(ray)).toBeNull();
  });
});

describe('resolvePlacement', () => {
  it('hits the sole entity in the way', () => {
    const ray: Ray = { origin: [0, 0, -5], dir: [0, 0, 1] };
    const result = resolvePlacement(ray, [box([-1, -1, -1], [1, 1, 1])]);
    expect(result).toEqual({ position: [0, 0, -1], surface: 'entity' });
  });

  it('falls back to the ground when no box is hit', () => {
    const ray: Ray = { origin: [0, 5, 0], dir: [0, -1, 0] };
    const result = resolvePlacement(ray, []);
    expect(result).toEqual({ position: [0, 0, 0], surface: 'ground' });
  });

  it('returns null for an upward sky ray with no box and no ground', () => {
    const ray: Ray = { origin: [0, 0, 0], dir: [0, 1, 0] };
    expect(resolvePlacement(ray, [])).toBeNull();
  });

  it('picks the nearer of two overlapping boxes', () => {
    const ray: Ray = { origin: [0, 0, -10], dir: [0, 0, 1] };
    const near = box([-1, -1, -3], [1, 1, 3], 1); // entering t = 7
    const far = box([-1, -1, 1], [1, 1, 5], 2); // entering t = 11, overlaps `near`'s z range
    const result = resolvePlacement(ray, [far, near]);
    expect(result).toEqual({ position: [0, 0, -3], surface: 'entity' });
  });

  it('prefers an entity hit over a farther ground hit', () => {
    const ray: Ray = { origin: [0, 10, 0], dir: [0, -1, 0] }; // ground at t = 10
    const entity = box([-1, 3, -1], [1, 6, 1]); // entering t = 4
    const result = resolvePlacement(ray, [entity]);
    expect(result).toEqual({ position: [0, 6, 0], surface: 'entity' });
  });

  it('prefers an entity hit even when the box straddles the ground plane behind it', () => {
    const ray: Ray = { origin: [0, 10, 0], dir: [0, -1, 0] }; // ground at t = 10
    // The box spans y in [-2, 2] (it extends below the ground plane), but the ray enters its top
    // face at t = 8 — still nearer than the ground hit, so the entity must still win.
    const entity = box([-1, -2, -1], [1, 2, 1]);
    const result = resolvePlacement(ray, [entity]);
    expect(result).toEqual({ position: [0, 2, 0], surface: 'entity' });
  });
});
