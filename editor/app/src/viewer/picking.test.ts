import { describe, expect, it } from 'vitest';
import {
  cameraTranslation,
  chooseHit,
  clientToEnginePoint,
  engineRectToCssRect,
  parseEntityBounds,
  pointInRect,
  rectArea,
  selectedScreenRect,
  type EntityScreenBounds,
} from './picking';

/** A bounds entry with sensible defaults; override only the fields a case cares about. */
function bounds(over: Partial<EntityScreenBounds> & { entity: EntityScreenBounds['entity'] }): EntityScreenBounds {
  return {
    onScreen: true,
    screenRect: { x: 0, y: 0, w: 10, h: 10 },
    worldCenter: [0, 0, 0],
    worldAabb: { min: [-1, -1, -1], max: [1, 1, 1] },
    ...over,
  };
}

describe('clientToEnginePoint', () => {
  it('maps a click through independent x and y scales', () => {
    // Canvas shown at 400x150 CSS px but is 800x300 engine px: 2x on x, 2x on y here but computed
    // separately, and offset by the canvas origin at (40, 20).
    const p = clientToEnginePoint(
      { x: 240, y: 95 },
      { left: 40, top: 20, width: 400, height: 150, engineWidth: 800, engineHeight: 300 },
    );
    expect(p).toEqual({ x: 400, y: 150 });
  });

  it('uses different x and y scales when the aspect is stretched', () => {
    const p = clientToEnginePoint(
      { x: 100, y: 50 },
      { left: 0, top: 0, width: 200, height: 100, engineWidth: 800, engineHeight: 200 },
    );
    // x scale 4, y scale 2.
    expect(p).toEqual({ x: 400, y: 100 });
  });

  it('returns null for a zero-size on-screen rect', () => {
    expect(
      clientToEnginePoint({ x: 5, y: 5 }, { left: 0, top: 0, width: 0, height: 100, engineWidth: 800, engineHeight: 600 }),
    ).toBeNull();
  });

  it('returns null before a frame has arrived (zero intrinsic size)', () => {
    expect(
      clientToEnginePoint({ x: 5, y: 5 }, { left: 0, top: 0, width: 400, height: 300, engineWidth: 0, engineHeight: 0 }),
    ).toBeNull();
  });

  it('produces negative engine coordinates for a click above/left of the canvas', () => {
    const p = clientToEnginePoint(
      { x: 10, y: 10 },
      { left: 40, top: 20, width: 400, height: 300, engineWidth: 800, engineHeight: 600 },
    );
    expect(p).toEqual({ x: -60, y: -20 });
  });
});

describe('pointInRect', () => {
  const r = { x: 10, y: 20, w: 30, h: 40 }; // covers x in [10,40], y in [20,60]

  it('is true strictly inside', () => {
    expect(pointInRect({ x: 25, y: 40 }, r)).toBe(true);
  });

  it('is inclusive on every edge and corner', () => {
    expect(pointInRect({ x: 10, y: 20 }, r)).toBe(true); // top-left corner
    expect(pointInRect({ x: 40, y: 60 }, r)).toBe(true); // bottom-right corner
    expect(pointInRect({ x: 10, y: 45 }, r)).toBe(true); // left edge
    expect(pointInRect({ x: 40, y: 45 }, r)).toBe(true); // right edge
  });

  it('is false just outside any edge', () => {
    expect(pointInRect({ x: 9.99, y: 40 }, r)).toBe(false);
    expect(pointInRect({ x: 40.01, y: 40 }, r)).toBe(false);
    expect(pointInRect({ x: 25, y: 19.99 }, r)).toBe(false);
    expect(pointInRect({ x: 25, y: 60.01 }, r)).toBe(false);
  });
});

describe('rectArea', () => {
  it('multiplies width by height', () => {
    expect(rectArea({ x: 0, y: 0, w: 4, h: 5 })).toBe(20);
  });
});

describe('chooseHit', () => {
  it('returns null when no rect contains the point', () => {
    const cs = [bounds({ entity: { index: 1, generation: 0 }, screenRect: { x: 0, y: 0, w: 5, h: 5 } })];
    expect(chooseHit({ x: 50, y: 50 }, cs, null)).toBeNull();
  });

  it('ignores off-screen entities even if their rect contains the point', () => {
    const cs = [
      bounds({ entity: { index: 1, generation: 0 }, onScreen: false, screenRect: { x: 0, y: 0, w: 100, h: 100 } }),
    ];
    expect(chooseHit({ x: 10, y: 10 }, cs, null)).toBeNull();
  });

  it('ignores entities with a null rect (behind the camera)', () => {
    const cs = [bounds({ entity: { index: 1, generation: 0 }, screenRect: null })];
    expect(chooseHit({ x: 0, y: 0 }, cs, null)).toBeNull();
  });

  it('returns the sole hit', () => {
    const cs = [bounds({ entity: { index: 7, generation: 2 }, screenRect: { x: 0, y: 0, w: 20, h: 20 } })];
    expect(chooseHit({ x: 5, y: 5 }, cs, null)).toEqual({ index: 7, generation: 2 });
  });

  it('picks the entity nearest the camera among overlapping rects', () => {
    // Both rects contain the point; the near one (world center closer to the camera) wins even though
    // its screen rect is the larger of the two (so area fallback would have chosen the other).
    const near = bounds({
      entity: { index: 1, generation: 0 },
      screenRect: { x: 0, y: 0, w: 100, h: 100 },
      worldCenter: [0, 0, 1],
    });
    const far = bounds({
      entity: { index: 2, generation: 0 },
      screenRect: { x: 0, y: 0, w: 10, h: 10 },
      worldCenter: [0, 0, 50],
    });
    const camera: [number, number, number] = [0, 0, -5];
    expect(chooseHit({ x: 5, y: 5 }, [far, near], camera)).toEqual({ index: 1, generation: 0 });
  });

  it('falls back to the smallest-area rect when no camera is given', () => {
    const big = bounds({
      entity: { index: 1, generation: 0 },
      screenRect: { x: 0, y: 0, w: 100, h: 100 },
      worldCenter: [0, 0, 1],
    });
    const small = bounds({
      entity: { index: 2, generation: 0 },
      screenRect: { x: 0, y: 0, w: 10, h: 10 },
      worldCenter: [0, 0, 50],
    });
    expect(chooseHit({ x: 5, y: 5 }, [big, small], null)).toEqual({ index: 2, generation: 0 });
  });

  it('resolves ties to the first candidate in extract order', () => {
    const a = bounds({ entity: { index: 1, generation: 0 }, screenRect: { x: 0, y: 0, w: 10, h: 10 }, worldCenter: [0, 0, 0] });
    const b = bounds({ entity: { index: 2, generation: 0 }, screenRect: { x: 0, y: 0, w: 10, h: 10 }, worldCenter: [0, 0, 0] });
    const camera: [number, number, number] = [0, 0, -1];
    expect(chooseHit({ x: 5, y: 5 }, [a, b], camera)).toEqual({ index: 1, generation: 0 });
    expect(chooseHit({ x: 5, y: 5 }, [a, b], null)).toEqual({ index: 1, generation: 0 });
  });
});

describe('selectedScreenRect', () => {
  const cs = [
    bounds({ entity: { index: 1, generation: 0 }, screenRect: { x: 1, y: 2, w: 3, h: 4 } }),
    bounds({ entity: { index: 2, generation: 5 }, screenRect: null }),
  ];

  it('returns the matching entity rect', () => {
    expect(selectedScreenRect(cs, { index: 1, generation: 0 })).toEqual({ x: 1, y: 2, w: 3, h: 4 });
  });

  it('returns null when the selected entity is absent', () => {
    expect(selectedScreenRect(cs, { index: 9, generation: 9 })).toBeNull();
  });

  it('returns null when generation differs (a stale handle)', () => {
    expect(selectedScreenRect(cs, { index: 1, generation: 1 })).toBeNull();
  });

  it('returns null when the selected entity is behind the camera (null rect)', () => {
    expect(selectedScreenRect(cs, { index: 2, generation: 5 })).toBeNull();
  });
});

describe('engineRectToCssRect', () => {
  it('scales and offsets an engine rect into the stage frame', () => {
    // 800x600 engine drawn at 400x300 CSS (0.5x each), canvas offset (20, 10) inside the stage.
    const css = engineRectToCssRect(
      { x: 100, y: 200, w: 40, h: 60 },
      { offsetX: 20, offsetY: 10, cssWidth: 400, cssHeight: 300, engineWidth: 800, engineHeight: 600 },
    );
    expect(css).toEqual({ x: 20 + 50, y: 10 + 100, w: 20, h: 30 });
  });

  it('applies independent x and y scales', () => {
    const css = engineRectToCssRect(
      { x: 100, y: 100, w: 10, h: 10 },
      { offsetX: 0, offsetY: 0, cssWidth: 200, cssHeight: 300, engineWidth: 800, engineHeight: 600 },
    );
    // x scale 0.25, y scale 0.5.
    expect(css).toEqual({ x: 25, y: 50, w: 2.5, h: 5 });
  });

  it('preserves off-viewport (negative) engine coordinates', () => {
    const css = engineRectToCssRect(
      { x: -40, y: -20, w: 80, h: 40 },
      { offsetX: 10, offsetY: 10, cssWidth: 400, cssHeight: 300, engineWidth: 400, engineHeight: 300 },
    );
    expect(css).toEqual({ x: 10 - 40, y: 10 - 20, w: 80, h: 40 });
  });

  it('returns null on a degenerate (zero) intrinsic size', () => {
    expect(
      engineRectToCssRect(
        { x: 0, y: 0, w: 10, h: 10 },
        { offsetX: 0, offsetY: 0, cssWidth: 400, cssHeight: 300, engineWidth: 0, engineHeight: 0 },
      ),
    ).toBeNull();
  });
});

describe('parseEntityBounds', () => {
  it('normalizes a render_bounds body and computes AABB centers', () => {
    const body = {
      kind: 'render_bounds',
      render_size: [800, 600],
      entities: [
        {
          entity: { index: 3, generation: 1 },
          on_screen: true,
          screen_rect: { x: 130, y: 15, w: 40, h: 20 },
          world_aabb: { min: [3, 1.5, 0], max: [7, 3.5, 2] },
        },
      ],
    };
    expect(parseEntityBounds(body)).toEqual([
      {
        entity: { index: 3, generation: 1 },
        onScreen: true,
        screenRect: { x: 130, y: 15, w: 40, h: 20 },
        worldCenter: [5, 2.5, 1],
        worldAabb: { min: [3, 1.5, 0], max: [7, 3.5, 2] },
      },
    ]);
  });

  it('carries a null screen_rect through as null with onScreen false', () => {
    const body = {
      kind: 'render_bounds',
      entities: [
        {
          entity: { index: 1, generation: 0 },
          on_screen: false,
          screen_rect: null,
          world_aabb: { min: [0, 0, 0], max: [2, 2, 2] },
        },
      ],
    };
    const parsed = parseEntityBounds(body);
    expect(parsed).toEqual([
      {
        entity: { index: 1, generation: 0 },
        onScreen: false,
        screenRect: null,
        worldCenter: [1, 1, 1],
        worldAabb: { min: [0, 0, 0], max: [2, 2, 2] },
      },
    ]);
  });

  it('returns null for a body of the wrong kind', () => {
    expect(parseEntityBounds({ kind: 'query_result', entities: [] })).toBeNull();
  });

  it('returns null for non-object input', () => {
    expect(parseEntityBounds(null)).toBeNull();
    expect(parseEntityBounds('nope')).toBeNull();
  });

  it('skips malformed entries rather than failing the whole parse', () => {
    const body = {
      kind: 'render_bounds',
      entities: [
        { entity: { index: 1, generation: 0 } }, // no world_aabb -> skipped
        {
          entity: { index: 2, generation: 0 },
          on_screen: true,
          screen_rect: { x: 0, y: 0, w: 5, h: 5 },
          world_aabb: { min: [0, 0, 0], max: [2, 2, 2] },
        },
      ],
    };
    const parsed = parseEntityBounds(body);
    expect(parsed).toHaveLength(1);
    expect(parsed?.[0]?.entity).toEqual({ index: 2, generation: 0 });
  });
});

describe('cameraTranslation', () => {
  it('reads the first camera match Transform translation', () => {
    const body = {
      kind: 'query_result',
      entities: [{ entity: { index: 4, generation: 0 }, components: { Transform: { translation: [1, 2, 3] } } }],
    };
    expect(cameraTranslation(body)).toEqual([1, 2, 3]);
  });

  it('picks the first of multiple camera matches', () => {
    const body = {
      kind: 'query_result',
      entities: [
        { entity: { index: 4, generation: 0 }, components: { Transform: { translation: [1, 0, 0] } } },
        { entity: { index: 5, generation: 0 }, components: { Transform: { translation: [9, 0, 0] } } },
      ],
    };
    expect(cameraTranslation(body)).toEqual([1, 0, 0]);
  });

  it('skips a match whose Transform is unreadable and takes the next good one', () => {
    const body = {
      kind: 'query_result',
      entities: [
        { entity: { index: 4, generation: 0 }, components: {} },
        { entity: { index: 5, generation: 0 }, components: { Transform: { translation: [7, 8, 9] } } },
      ],
    };
    expect(cameraTranslation(body)).toEqual([7, 8, 9]);
  });

  it('returns null when no camera matched (empty result)', () => {
    expect(cameraTranslation({ kind: 'query_result', entities: [] })).toBeNull();
  });

  it('returns null for the wrong body kind', () => {
    expect(cameraTranslation({ kind: 'render_bounds', entities: [] })).toBeNull();
  });
});
