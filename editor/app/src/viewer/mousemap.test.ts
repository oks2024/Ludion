import { describe, expect, it } from 'vitest';
import { f32Bits, mouseButtonInject, mouseMoveInject, pointerButtonName, releaseInjects } from './mousemap';
import { clientToEnginePoint } from './picking';

/** Rust's `f32::to_bits` reference values, for the wire-shape assertions. */
function bitsOf(x: number): number {
  const f = new Float32Array([x]);
  return new Uint32Array(f.buffer)[0];
}

describe('f32Bits', () => {
  it('matches f32::to_bits for whole and fractional pixels', () => {
    expect(f32Bits(0)).toBe(bitsOf(0));
    expect(f32Bits(400)).toBe(bitsOf(400));
    expect(f32Bits(150.5)).toBe(bitsOf(150.5));
    expect(f32Bits(-2.25)).toBe(bitsOf(-2.25));
  });

  it('returns an unsigned 32-bit integer even for negative values', () => {
    // -2.25f32's sign bit is set, so the pattern exceeds 2^31 — must not come back as a negative int.
    expect(f32Bits(-2.25)).toBeGreaterThan(0);
    expect(f32Bits(-2.25)).toBe(0xc0100000);
  });
});

describe('pointerButtonName', () => {
  it('maps the three named buttons by DOM index', () => {
    expect(pointerButtonName(0)).toBe('left');
    expect(pointerButtonName(1)).toBe('middle');
    expect(pointerButtonName(2)).toBe('right');
  });

  it('returns null for buttons the engine has no name for', () => {
    expect(pointerButtonName(3)).toBeNull(); // back
    expect(pointerButtonName(4)).toBeNull(); // forward
    expect(pointerButtonName(-1)).toBeNull(); // a move's "no button"
  });
});

describe('mouseMoveInject', () => {
  it('builds the pinned position_bits shape, not JSON numbers', () => {
    expect(mouseMoveInject({ x: 1.5, y: -2.25 })).toEqual({
      event: { kind: 'mouse_move', position_bits: [bitsOf(1.5), bitsOf(-2.25)] },
    });
  });

  it('composes with the reused picking transform: a viewport-center move maps to the frame center', () => {
    // Canvas shown 400x150 CSS px at engine size 800x300, offset (40, 20): the reused coordinate
    // mapping is `clientToEnginePoint`, exactly what click-picking uses — no second transform here.
    const point = clientToEnginePoint(
      { x: 240, y: 95 },
      { left: 40, top: 20, width: 400, height: 150, engineWidth: 800, engineHeight: 300 },
    );
    expect(point).toEqual({ x: 400, y: 150 });
    expect(mouseMoveInject(point!)).toEqual({
      event: { kind: 'mouse_move', position_bits: [bitsOf(400), bitsOf(150)] },
    });
  });

  it('composes at a corner of a CSS-scaled, offset canvas', () => {
    const point = clientToEnginePoint(
      { x: 40, y: 20 },
      { left: 40, top: 20, width: 400, height: 150, engineWidth: 800, engineHeight: 300 },
    );
    expect(point).toEqual({ x: 0, y: 0 });
    expect(mouseMoveInject(point!)).toEqual({
      event: { kind: 'mouse_move', position_bits: [bitsOf(0), bitsOf(0)] },
    });
  });
});

describe('mouseButtonInject', () => {
  it('builds a mouse_down for a press and mouse_up for a release, preserving button identity', () => {
    expect(mouseButtonInject('left', 'down')).toEqual({ event: { kind: 'mouse_down', button: 'left' } });
    expect(mouseButtonInject('right', 'up')).toEqual({ event: { kind: 'mouse_up', button: 'right' } });
    expect(mouseButtonInject('middle', 'down')).toEqual({ event: { kind: 'mouse_down', button: 'middle' } });
  });
});

describe('releaseInjects', () => {
  it('emits one mouse_up per held button, in iteration order', () => {
    expect(releaseInjects(new Set<'left' | 'right'>(['left', 'right']))).toEqual([
      { event: { kind: 'mouse_up', button: 'left' } },
      { event: { kind: 'mouse_up', button: 'right' } },
    ]);
  });

  it('is empty when nothing is held', () => {
    expect(releaseInjects(new Set())).toEqual([]);
  });
});
