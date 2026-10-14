// Cross-validates the gizmo's TS camera math against the engine's own projection, over a *real*
// engine connection. `camera.ts` reimplements `crates/ludion-engine/src/extract.rs::build_camera`
// and `bounds.rs::project` in TS so gizmo handles land on the entity to the pixel; nothing but this
// test proves the two stay identical. `render.bounds` reports, per renderable, the world AABB and
// the engine's own screen-space projection of it — reprojecting that same AABB through
// `worldToScreen` and comparing rects pins TS math ≡ engine math, so any future drift on either side
// fails here rather than silently degrading handle placement.
//
// Opt-in like `viewer/integration.test.ts` (same `LUDION_WS_URL` gate, same reasoning: `npm test`
// must stay green with no engine around). To run it:
//   cargo build -p ludion-cli
//   target/debug/ludion serve --scene examples/gallery/sprites.yaml --addr 127.0.0.1:0 --paused
//   LUDION_WS_URL=ws://127.0.0.1:<port> npm test

import { afterAll, describe, expect, it } from 'vitest';
import { ProtocolClient } from '../../protocol/client';
import type { DecodedFrame } from '../../protocol/frames';
import { parseEntityBounds, type EntityId } from '../../viewer/picking';
import { worldToScreen, type Viewport } from './camera';
import { readCameraKit } from './scene';
import type { Vec3 } from './vecmath';

const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const url = env.LUDION_WS_URL;

/** The eight world-space corners of an AABB, matching `bounds.rs::Aabb::corners` (order is
 *  irrelevant here — only the min/max of their projections is compared). */
function aabbCorners(min: Vec3, max: Vec3): Vec3[] {
  const [ax, ay, az] = min;
  const [bx, by, bz] = max;
  return [
    [ax, ay, az],
    [bx, ay, az],
    [ax, by, az],
    [bx, by, az],
    [ax, ay, bz],
    [bx, ay, bz],
    [ax, by, bz],
    [bx, by, bz],
  ];
}

function entityKey(e: EntityId): string {
  return `${e.index}:${e.generation}`;
}

/** Raw `world_aabb.min`/`max` per entity, read straight from the wire body — `parseEntityBounds`
 *  only exposes the AABB center (the depth tie-break it needs), not the corners this test needs. */
function rawWorldAabbs(body: unknown): Map<string, { min: Vec3; max: Vec3 }> {
  const out = new Map<string, { min: Vec3; max: Vec3 }>();
  if (typeof body !== 'object' || body === null) return out;
  const entities = (body as { entities?: unknown }).entities;
  if (!Array.isArray(entities)) return out;
  for (const raw of entities) {
    if (typeof raw !== 'object' || raw === null) continue;
    const r = raw as { entity?: { index?: unknown; generation?: unknown }; world_aabb?: { min?: unknown; max?: unknown } };
    const index = r.entity?.index;
    const generation = r.entity?.generation;
    if (typeof index !== 'number' || typeof generation !== 'number') continue;
    const min = r.world_aabb?.min;
    const max = r.world_aabb?.max;
    if (!Array.isArray(min) || min.length !== 3 || !Array.isArray(max) || max.length !== 3) continue;
    out.set(`${index}:${generation}`, { min: [min[0], min[1], min[2]], max: [max[0], max[1], max[2]] });
  }
  return out;
}

describe.skipIf(url === undefined || url === '')('gizmo camera.ts against a live engine\'s render.bounds', () => {
  const client = new ProtocolClient();

  afterAll(() => {
    client.close();
  });

  it('reprojects render.bounds world AABBs and matches the engine screen_rect within 0.1px', async () => {
    await client.connect(url as string);
    await client.call({ method: 'sim.pause' }); // a stable world for the comparison

    // The engine render size isn't on the wire directly — read it off the first streamed frame, the
    // same path `useFrameStream` drives, then drop the subscription (this test doesn't watch frames).
    let subId: number | undefined;
    const size: Viewport = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('no frame arrived within 10s')), 10_000);
      const off = client.onFrame((frame: DecodedFrame) => {
        off();
        clearTimeout(timer);
        resolve({ width: frame.header.width, height: frame.header.height });
      });
      client
        .subscribe('frames')
        .then((id) => {
          subId = id;
        })
        .catch((err: unknown) => {
          clearTimeout(timer);
          reject(err instanceof Error ? err : new Error(String(err)));
        });
    });
    if (subId !== undefined) {
      await client.unsubscribe(subId);
    }

    const cameraBody = await client.call({
      method: 'query.entities',
      params: { filter: ['Camera'], include: ['Transform', 'Camera'] },
    });
    const kit = readCameraKit(cameraBody, size);
    expect(kit).not.toBeNull();
    if (kit === null) throw new Error('unreachable');

    const boundsBody = await client.call({ method: 'render.bounds', params: {} });
    expect(boundsBody.kind).toBe('render_bounds');
    const parsed = parseEntityBounds(boundsBody);
    expect(parsed).not.toBeNull();
    if (parsed === null) throw new Error('unreachable');

    const rawAabbs = rawWorldAabbs(boundsBody);

    const EPS = 0.1; // f32 (engine) vs f64 (TS) rounding at 960x540-ish targets stays well under this
    let compared = 0;
    let maxDelta = 0;

    for (const e of parsed) {
      if (e.screenRect === null) continue; // engine says behind camera: nothing to cross-check
      const aabb = rawAabbs.get(entityKey(e.entity));
      expect(aabb).toBeDefined();
      if (aabb === undefined) continue;

      // Exactly `bounds.rs::project`: project all 8 corners, skip behind-camera ones, bound the rest.
      let minX = Infinity;
      let minY = Infinity;
      let maxX = -Infinity;
      let maxY = -Infinity;
      let any = false;
      for (const corner of aabbCorners(aabb.min, aabb.max)) {
        const s = worldToScreen(corner, kit.viewProj, size);
        if (s === null) continue;
        any = true;
        minX = Math.min(minX, s.x);
        minY = Math.min(minY, s.y);
        maxX = Math.max(maxX, s.x);
        maxY = Math.max(maxY, s.y);
      }
      // The engine reported a non-null rect for this entity, so at least one corner must survive here too.
      expect(any).toBe(true);

      const reprojected = { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
      const dx = Math.abs(reprojected.x - e.screenRect.x);
      const dy = Math.abs(reprojected.y - e.screenRect.y);
      const dw = Math.abs(reprojected.w - e.screenRect.w);
      const dh = Math.abs(reprojected.h - e.screenRect.h);
      maxDelta = Math.max(maxDelta, dx, dy, dw, dh);
      expect(dx).toBeLessThanOrEqual(EPS);
      expect(dy).toBeLessThanOrEqual(EPS);
      expect(dw).toBeLessThanOrEqual(EPS);
      expect(dh).toBeLessThanOrEqual(EPS);
      compared++;
    }

    // Guards against a vacuous pass (e.g. an empty or all-behind-camera scene slipping through).
    expect(compared).toBeGreaterThanOrEqual(3);
    console.log(`gizmo camera cross-check: ${compared} entities compared, max delta ${maxDelta.toFixed(4)}px`);
  });
});
