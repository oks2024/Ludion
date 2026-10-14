// Pure geometry + decision logic for click-picking and the selection overlay (Plan/phase-6-editing.md
// decisions #4 and #5). None of this touches the DOM or the socket: the viewport reads a click and a
// `render.bounds` response, hands the plain numbers here, and gets back an engine-space point, a
// picked entity, or an overlay rectangle. Keeping it transport-free makes every edge case — clicks on
// a rect edge, overlapping rects, a missing camera, a zero-size canvas, off-viewport and behind-camera
// rects — unit-testable without a browser (see picking.test.ts).
//
// Two pixel spaces are in play. *Engine* pixels are what `render.bounds` speaks: origin top-left,
// +Y down, at the engine's render size (which equals the streamed frame's intrinsic dimensions, so it
// also equals `canvas.width`/`canvas.height`). *CSS* pixels are what the human clicks and where the
// overlay box is drawn: the canvas is letterboxed and CSS-scaled inside its stage, with independent
// x and y scales (never assume uniform). `clientToEnginePoint` maps a click the first way;
// `engineRectToCssRect` maps a rect the other way, offset into the stage's coordinate frame.

/** An entity handle as it appears on the wire (`index`/`generation`). */
export interface EntityId {
  readonly index: number;
  readonly generation: number;
}

/** A 2D point in some pixel space (engine or CSS — the caller keeps them straight). */
export interface Point {
  readonly x: number;
  readonly y: number;
}

/** A rectangle: top-left corner `(x, y)`, width `w`, height `h`, in some pixel space. */
export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** One entity's screen presence, normalized from a `render.bounds` `EntityBounds` entry. */
export interface EntityScreenBounds {
  readonly entity: EntityId;
  /** Whether `screenRect` is present and intersects the viewport (the honest on-image test). */
  readonly onScreen: boolean;
  /** The engine-pixel rect bounding the entity, or `null` when the whole AABB is behind the camera. */
  readonly screenRect: Rect | null;
  /** The world-space center of the entity's AABB — the depth key for the nearest-camera tie-break. */
  readonly worldCenter: readonly [number, number, number];
  /** The entity's world-space AABB corners, as parsed from `world_aabb` — the 3D pointer's raycast target. */
  readonly worldAabb: { readonly min: readonly [number, number, number]; readonly max: readonly [number, number, number] };
}

/** The canvas geometry a click needs to reach engine space: its on-screen rect and intrinsic size. */
export interface CanvasClientRect {
  /** `getBoundingClientRect()` of the canvas, in client (viewport) CSS pixels. */
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
  /** The canvas's intrinsic pixel size (`canvas.width`/`canvas.height`) — the engine render size. */
  readonly engineWidth: number;
  readonly engineHeight: number;
}

/** The layout an engine-space rect needs to become a CSS box positioned inside the stage. */
export interface OverlayLayout {
  /** The canvas's top-left offset relative to its stage's top-left, in CSS pixels. */
  readonly offsetX: number;
  readonly offsetY: number;
  /** The canvas's on-screen CSS size. */
  readonly cssWidth: number;
  readonly cssHeight: number;
  /** The canvas's intrinsic (engine) pixel size. */
  readonly engineWidth: number;
  readonly engineHeight: number;
}

/**
 * Maps a client-space click (from `MouseEvent.clientX`/`clientY`) into engine pixels, using
 * independent x/y scales derived from the canvas's on-screen rect versus its intrinsic size.
 * Returns `null` when the mapping is undefined — a zero-size on-screen rect (never displayed) or a
 * zero intrinsic size (no frame has arrived yet) — so the caller can decline to hit-test.
 */
export function clientToEnginePoint(client: Point, canvas: CanvasClientRect): Point | null {
  if (canvas.width <= 0 || canvas.height <= 0 || canvas.engineWidth <= 0 || canvas.engineHeight <= 0) {
    return null;
  }
  const scaleX = canvas.engineWidth / canvas.width;
  const scaleY = canvas.engineHeight / canvas.height;
  return {
    x: (client.x - canvas.left) * scaleX,
    y: (client.y - canvas.top) * scaleY,
  };
}

/** Reads a live canvas's {@link CanvasClientRect} — the input every click mapping starts from, so
 *  every frame surface reaches engine space the same way and a cursor lands on the same pixel a pick
 *  would. */
export function canvasClientRect(canvas: HTMLCanvasElement): CanvasClientRect {
  const r = canvas.getBoundingClientRect();
  return {
    left: r.left,
    top: r.top,
    width: r.width,
    height: r.height,
    engineWidth: canvas.width,
    engineHeight: canvas.height,
  };
}

/** Whether `p` lies within `r`, inclusive of every edge (a click exactly on the border hits). */
export function pointInRect(p: Point, r: Rect): boolean {
  return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
}

/** The area of a rect; the fallback ranking key when no camera depth is available. */
export function rectArea(r: Rect): number {
  return r.w * r.h;
}

/** The squared distance between two 3D points (squared to skip a needless `sqrt` in comparisons). */
function distanceSq(a: readonly [number, number, number], b: readonly [number, number, number]): number {
  const dx = a[0] - b[0];
  const dy = a[1] - b[1];
  const dz = a[2] - b[2];
  return dx * dx + dy * dy + dz * dz;
}

/**
 * Chooses which entity a click at `point` (engine pixels) selects, per plan decision #4.
 *
 * Candidates are the entities that are `onScreen` with a non-null `screenRect` that contains the
 * point. With a `camera` translation, the tie-break is nearest AABB center to the camera (the front-
 * most under the current view); without one (query failed, no `Camera`, or an unreadable transform),
 * it falls back to the smallest screen-rect area (the tightest cover). Ties resolve to the first
 * candidate in the list (extract order), keeping the pick deterministic. Returns `null` on no hit.
 */
export function chooseHit(
  point: Point,
  candidates: readonly EntityScreenBounds[],
  camera: readonly [number, number, number] | null,
): EntityId | null {
  const hits = candidates.filter(
    (c): c is EntityScreenBounds & { screenRect: Rect } =>
      c.onScreen && c.screenRect !== null && pointInRect(point, c.screenRect),
  );
  if (hits.length === 0) {
    return null;
  }
  const rank =
    camera !== null
      ? (c: EntityScreenBounds & { screenRect: Rect }) => distanceSq(c.worldCenter, camera)
      : (c: EntityScreenBounds & { screenRect: Rect }) => rectArea(c.screenRect);
  // Reduce keeping the first on ties (strict `<` only replaces on a smaller key).
  const best = hits.reduce((acc, c) => (rank(c) < rank(acc) ? c : acc));
  return best.entity;
}

/** Finds the (non-null) engine-pixel rect of `selection` in the latest bounds, or `null` if the
 *  entity is absent or currently behind the camera — either way the overlay box should hide. */
export function selectedScreenRect(candidates: readonly EntityScreenBounds[], selection: EntityId): Rect | null {
  const match = candidates.find(
    (c) => c.entity.index === selection.index && c.entity.generation === selection.generation,
  );
  return match?.screenRect ?? null;
}

/**
 * Projects an engine-pixel rect into a CSS box positioned inside the stage, applying independent x/y
 * scales and the canvas's offset within the stage. Returns `null` on a degenerate (zero) intrinsic
 * size, where no scale is defined. The result may extend outside the canvas (rects can run
 * off-viewport); the overlay element is `pointer-events: none` and clipped by nothing, which matches
 * the engine's own "rects may extend off-viewport" contract.
 */
export function engineRectToCssRect(rect: Rect, layout: OverlayLayout): Rect | null {
  if (layout.engineWidth <= 0 || layout.engineHeight <= 0) {
    return null;
  }
  const scaleX = layout.cssWidth / layout.engineWidth;
  const scaleY = layout.cssHeight / layout.engineHeight;
  return {
    x: layout.offsetX + rect.x * scaleX,
    y: layout.offsetY + rect.y * scaleY,
    w: rect.w * scaleX,
    h: rect.h * scaleY,
  };
}

// --- Response parsing (unknown -> normalized) -------------------------------------------------- //
//
// The generated response bodies carry `[k: string]: unknown` index signatures that defeat TS's
// discriminant narrowing (see client.ts and editor/undo.ts for the same wart). Rather than sprinkle
// casts through the viewport, these two parsers take the raw body as `unknown` and return plain,
// fully-typed data (or `null` for the wrong shape) — which also makes them trivially testable with
// object literals, no generated types required.

function asNumber(v: unknown): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
}

function parseEntityId(v: unknown): EntityId | null {
  if (typeof v !== 'object' || v === null) return null;
  const o = v as { index?: unknown; generation?: unknown };
  const index = asNumber(o.index);
  const generation = asNumber(o.generation);
  return index !== null && generation !== null ? { index, generation } : null;
}

function parseVec3(v: unknown): [number, number, number] | null {
  if (!Array.isArray(v) || v.length < 3) return null;
  const x = asNumber(v[0]);
  const y = asNumber(v[1]);
  const z = asNumber(v[2]);
  return x !== null && y !== null && z !== null ? [x, y, z] : null;
}

function parseRect(v: unknown): Rect | null {
  if (typeof v !== 'object' || v === null) return null;
  const o = v as { x?: unknown; y?: unknown; w?: unknown; h?: unknown };
  const x = asNumber(o.x);
  const y = asNumber(o.y);
  const w = asNumber(o.w);
  const h = asNumber(o.h);
  return x !== null && y !== null && w !== null && h !== null ? { x, y, w, h } : null;
}

/**
 * Normalizes a `render.bounds` response body into `EntityScreenBounds[]`, computing each entity's
 * world-AABB center for the depth tie-break. Returns `null` if the body is not a `render_bounds`
 * result; skips any malformed entry rather than failing the whole pick.
 */
export function parseEntityBounds(body: unknown): EntityScreenBounds[] | null {
  if (typeof body !== 'object' || body === null) return null;
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'render_bounds' || !Array.isArray(b.entities)) return null;

  const out: EntityScreenBounds[] = [];
  for (const raw of b.entities) {
    if (typeof raw !== 'object' || raw === null) continue;
    const e = raw as { entity?: unknown; on_screen?: unknown; screen_rect?: unknown; world_aabb?: unknown };
    const entity = parseEntityId(e.entity);
    if (entity === null) continue;
    const aabb = e.world_aabb as { min?: unknown; max?: unknown } | undefined;
    const min = parseVec3(aabb?.min);
    const max = parseVec3(aabb?.max);
    if (min === null || max === null) continue;
    // `screen_rect` is null (behind camera) or absent on some entries; a malformed non-null rect is
    // treated as absent rather than dropping the entity (its AABB center still matters for overlap).
    const screenRect = e.screen_rect === null || e.screen_rect === undefined ? null : parseRect(e.screen_rect);
    out.push({
      entity,
      onScreen: e.on_screen === true,
      screenRect,
      worldCenter: [(min[0] + max[0]) / 2, (min[1] + max[1]) / 2, (min[2] + max[2]) / 2],
      worldAabb: { min, max },
    });
  }
  return out;
}

/**
 * Reads the camera translation out of a `query.entities` body (`filter: ['Camera'], include:
 * ['Transform']`): the first match's `Transform.translation`, or `null` when the body is the wrong
 * shape, no camera matched, or the transform is unreadable — every one of which the caller treats as
 * "no depth key, fall back to smallest area".
 */
export function cameraTranslation(body: unknown): [number, number, number] | null {
  if (typeof body !== 'object' || body === null) return null;
  const b = body as { kind?: unknown; entities?: unknown };
  if (b.kind !== 'query_result' || !Array.isArray(b.entities)) return null;
  for (const raw of b.entities) {
    if (typeof raw !== 'object' || raw === null) continue;
    const components = (raw as { components?: unknown }).components;
    if (typeof components !== 'object' || components === null) continue;
    const transform = (components as { Transform?: unknown }).Transform;
    if (typeof transform !== 'object' || transform === null) continue;
    const translation = parseVec3((transform as { translation?: unknown }).translation);
    if (translation !== null) return translation;
  }
  return null;
}
