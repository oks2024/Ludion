// Pure geometry for the 3D pointer: turns a world-space picking ray (from `screenToRay` in
// gizmo/camera.ts) into a placement point. Raycasts against entity AABBs first (slab method) and
// falls back to the y=0 ground plane when nothing is hit. No DOM, no transport — the viewport wiring
// feeds this a `Ray` and the latest `EntityScreenBounds[]`, and gets back a placement or `null`.

import type { Ray } from '../gizmo/camera';
import { vAdd, vScale, type Vec3 } from '../gizmo/vecmath';
import type { EntityScreenBounds } from '../../viewer/picking';

/** Below this, a ray direction component is treated as parallel to that axis/plane, not just small. */
const EPSILON = 1e-12;

/**
 * Slab-method ray/AABB intersection: the entering `t` (`origin + t·dir` on the box surface), or
 * `null` on a miss or when the box lies entirely behind the origin (`t < 0` throughout). When the
 * origin is already inside the box, the "entering" t would be negative (the ray started past it) —
 * clamped to 0 rather than reported negative, since the placement point is the ray origin itself.
 */
export function rayAabbEnter(ray: Ray, min: Vec3, max: Vec3): number | null {
  let tmin = -Infinity;
  let tmax = Infinity;
  for (let i = 0; i < 3; i++) {
    const o = ray.origin[i];
    const d = ray.dir[i];
    const lo = min[i];
    const hi = max[i];
    if (Math.abs(d) < EPSILON) {
      // Parallel to this axis's slab: only a hit if the origin already lies within its bounds.
      if (o < lo || o > hi) return null;
      continue;
    }
    let t1 = (lo - o) / d;
    let t2 = (hi - o) / d;
    if (t1 > t2) {
      [t1, t2] = [t2, t1];
    }
    tmin = Math.max(tmin, t1);
    tmax = Math.min(tmax, t2);
    if (tmin > tmax) return null;
  }
  if (tmax < 0) return null; // the whole box is behind the origin
  return Math.max(tmin, 0);
}

/**
 * Intersection of `ray` with the world y=0 plane, at `t > 0`. `null` when the ray is parallel to the
 * plane (`dir.y ≈ 0`) or the plane is behind/at the origin (`t <= 0`).
 */
export function rayGroundY0(ray: Ray): Vec3 | null {
  const dy = ray.dir[1];
  if (Math.abs(dy) < EPSILON) return null;
  const t = -ray.origin[1] / dy;
  if (t <= 0) return null;
  return [ray.origin[0] + t * ray.dir[0], 0, ray.origin[2] + t * ray.dir[2]];
}

/**
 * Resolves where a click's world-space ray should place something: the nearest entity AABB it
 * enters (by entering `t`), or the y=0 ground plane when no AABB is hit, or `null` when neither
 * (e.g. an upward ray over an unbounded scene). `bounds` order does not matter — every candidate is
 * tested and the smallest non-negative `t` wins.
 */
export function resolvePlacement(
  ray: Ray,
  bounds: readonly EntityScreenBounds[],
): { position: Vec3; surface: 'entity' | 'ground' } | null {
  let bestT: number | null = null;
  for (const b of bounds) {
    const t = rayAabbEnter(ray, b.worldAabb.min, b.worldAabb.max);
    if (t === null) continue;
    if (bestT === null || t < bestT) {
      bestT = t;
    }
  }
  if (bestT !== null) {
    return { position: vAdd(ray.origin, vScale(ray.dir, bestT)), surface: 'entity' };
  }
  const ground = rayGroundY0(ray);
  if (ground !== null) {
    return { position: ground, surface: 'ground' };
  }
  return null;
}
