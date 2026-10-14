// Reads the two wire bodies the overlay needs — the camera and the selected entity's pose (both from
// `query.entities`) — into the typed shapes `camera.ts` and `model.ts` consume. Pure and
// transport-free like picking.ts: `unknown` in, typed-or-`null` out, so the gizmo's "quietly show
// nothing when the scene can't be projected" policy is a testable data decision rather than a tangle
// of React guards (see scene.test.ts).

import {
  buildViewProj,
  eulerXyzDegToQuat,
  parseCamera,
  parseTransform,
  type CameraKit,
  type CameraParams,
  type Transform,
  type Viewport,
} from './camera';
import type { GizmoFrame } from './drag';
import type { Vec3 } from './vecmath';

/** The scene's own camera, as the wire reports it: where it stands and how it projects. */
export interface SceneCamera {
  readonly transform: Transform;
  readonly camera: CameraParams;
}

/** What `render.view`'s override falls back to when the scene lends nothing — `Camera::default`'s
 *  60° perspective and clip planes (`crates/ludion-engine/src/components.rs`). */
export const OVERRIDE_FALLBACK_FOV_Y_DEG = 60;
const OVERRIDE_FALLBACK_NEAR = 0.1;
const OVERRIDE_FALLBACK_FAR = 1000;

/**
 * Reads the scene's camera out of a `query.entities` body filtered to `Camera` (including
 * `Transform` + `Camera`). Requires EXACTLY one camera — zero means nothing to project through, two
 * means the active one is ambiguous — and both components must parse. `null` otherwise.
 */
export function readSceneCamera(queryBody: unknown): SceneCamera | null {
  if (typeof queryBody !== 'object' || queryBody === null) {
    return null;
  }
  const b = queryBody as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities) || b.entities.length !== 1) {
    return null;
  }
  const match = b.entities[0];
  if (typeof match !== 'object' || match === null) {
    return null;
  }
  const components = (match as { components?: unknown }).components;
  if (typeof components !== 'object' || components === null) {
    return null;
  }
  const c = components as { Transform?: unknown; Camera?: unknown };
  const transform = parseTransform(c.Transform);
  const camera = parseCamera(c.Camera);
  if (transform === null || camera === null) {
    return null;
  }
  return { transform, camera };
}

/** The scene camera's projection kit: `readSceneCamera` built into matrices at `size`. */
export function readCameraKit(queryBody: unknown, size: Viewport): CameraKit | null {
  const scene = readSceneCamera(queryBody);
  return scene === null ? null : buildViewProj(scene.transform, scene.camera, size);
}

/**
 * The projection kit for a `render.view` override in force — what the human is actually looking at
 * while the view camera is showing.
 *
 * Mirrors `crates/ludion-engine/src/render_view.rs`'s `camera` exactly, which is the whole point: a
 * gizmo handle is drawn, hit-tested and solved through this, so any disagreement with the renderer
 * is a handle that does not land under the cursor. The override is always PERSPECTIVE; it takes its
 * clip planes from the scene's camera and its field of view from the pose, falling back to the
 * scene camera's own (an orthographic one has none to lend) and then to `Camera::default`'s.
 */
export function viewOverrideKit(
  pose: { readonly translation: Vec3; readonly rotation: Vec3 },
  fovYDeg: number | null,
  scene: SceneCamera | null,
  size: Viewport,
): CameraKit {
  const projection = scene?.camera.projection;
  const inherited =
    projection !== undefined && projection.kind === 'perspective'
      ? projection.fovYDeg
      : OVERRIDE_FALLBACK_FOV_Y_DEG;
  return buildViewProj(
    { translation: pose.translation, rotation: pose.rotation, scale: [1, 1, 1] },
    {
      projection: { kind: 'perspective', fovYDeg: fovYDeg ?? inherited },
      near: scene?.camera.near ?? OVERRIDE_FALLBACK_NEAR,
      far: scene?.camera.far ?? OVERRIDE_FALLBACK_FAR,
    },
    size,
  );
}

/** An entity's local `Transform` and, when it has a `Parent`, where it stands in the world. */
export interface GizmoPose {
  readonly local: Transform;
  /** `null` for a root, whose local value IS its world pose. */
  readonly frame: GizmoFrame | null;
}

function entityMatches(body: unknown): { entity: unknown; components: Record<string, unknown>; world: unknown }[] | null {
  if (typeof body !== 'object' || body === null) {
    return null;
  }
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) {
    return null;
  }
  const out: { entity: unknown; components: Record<string, unknown>; world: unknown }[] = [];
  for (const raw of b.entities) {
    if (typeof raw !== 'object' || raw === null) {
      continue;
    }
    const m = raw as { entity?: unknown; components?: unknown; world?: unknown };
    const components = typeof m.components === 'object' && m.components !== null ? (m.components as Record<string, unknown>) : {};
    out.push({ entity: m.entity, components, world: m.world });
  }
  return out;
}

function sameHandle(a: unknown, b: unknown): boolean {
  if (typeof a !== 'object' || a === null || typeof b !== 'object' || b === null) {
    return false;
  }
  const x = a as { index?: unknown; generation?: unknown };
  const y = b as { index?: unknown; generation?: unknown };
  return x.index === y.index && x.generation === y.generation;
}

/** The `query.entities` params that read `entity`'s gizmo pose: its own match (`Transform`, `Parent`, world). */
export function gizmoPoseQuery(entity: { index: number; generation: number }) {
  return { filter: [], include: ['Transform', 'Parent'], world: true, entities: [entity] };
}

/**
 * The follow-up params for the parent's world pose, or `null` when the first reply's match for `entity`
 * carries no `Parent`.
 */
export function gizmoParentQuery(firstBody: unknown, entity: { index: number; generation: number }) {
  const me = entityMatches(firstBody)?.find((m) => sameHandle(m.entity, entity));
  const parent = me?.components.Parent;
  if (typeof parent !== 'object' || parent === null) {
    return null;
  }
  const { index, generation } = parent as { index?: unknown; generation?: unknown };
  if (typeof index !== 'number' || typeof generation !== 'number') {
    return null;
  }
  return { filter: [], include: [], world: true, entities: [{ index, generation }] };
}

/** Concatenates the matches of two `query_result` bodies, so `readGizmoPose` sees both entities. */
export function mergeQueryBodies(a: unknown, b: unknown) {
  const matches = (body: unknown) => entityMatches(body)?.map((m) => ({ entity: m.entity, components: m.components, world: m.world })) ?? [];
  return { kind: 'query_result', entities: [...matches(a), ...matches(b)] };
}

/**
 * Reads the selected entity's gizmo pose out of a `query.entities` body requested with
 * `include: ['Transform', 'Parent']` and `world: true`. `null` when the entity or its `Transform` is
 * absent or malformed. A root (no `Parent`) has `frame: null`; a parented entity's frame takes its
 * world pose from the entity and the parent's world rotation/scale from the parent's match — an identity
 * parent frame when the parent reports no `world`. An entity that reports no `world` is treated as a
 * root, so the gizmo stays usable at the local pose rather than disappearing.
 */
export function readGizmoPose(queryBody: unknown, entity: { index: number; generation: number }): GizmoPose | null {
  const matches = entityMatches(queryBody);
  const me = matches?.find((m) => sameHandle(m.entity, entity));
  if (matches === null || me === undefined) {
    return null;
  }
  const local = parseTransform(me.components.Transform);
  if (local === null) {
    return null;
  }
  const parentRef = me.components.Parent;
  const world = parseTransform(me.world);
  if (parentRef === undefined || parentRef === null || world === null) {
    return { local, frame: null };
  }
  const parent = matches.find((m) => sameHandle(m.entity, parentRef));
  const parentWorld = parent === undefined ? null : parseTransform(parent.world);
  return {
    local,
    frame: {
      world,
      parent:
        parentWorld === null
          ? { rotation: [0, 0, 0, 1], scale: [1, 1, 1] }
          : { rotation: eulerXyzDegToQuat(parentWorld.rotation), scale: parentWorld.scale },
    },
  };
}
