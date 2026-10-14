import { describe, expect, it } from 'vitest';
import {
  HOST_DEFAULT,
  MAX_STREAM_HEIGHT,
  MAX_STREAM_WIDTH,
  STREAM_SIZES,
  clampStreamSize,
  describeStreamSize,
  parseStreamSize,
  serializeStreamSize,
} from './streamSize';

describe('STREAM_SIZES', () => {
  it('leads with the host default, so the knob composes with a startup extent', () => {
    // The first choice is the one that sends nothing: a session started with `--render-size` /
    // `LUDION_RENDER_SIZE` must keep that extent until a human deliberately picks a number.
    expect(STREAM_SIZES[0]).toBeNull();
  });

  it('offers only extents the engine accepts, in descending pixel count', () => {
    const sizes = STREAM_SIZES.filter((size) => size !== null);
    expect(sizes.length).toBeGreaterThan(1);
    for (const [width, height] of sizes) {
      expect(width).toBeGreaterThanOrEqual(1);
      expect(width).toBeLessThanOrEqual(MAX_STREAM_WIDTH);
      expect(height).toBeGreaterThanOrEqual(1);
      expect(height).toBeLessThanOrEqual(MAX_STREAM_HEIGHT);
    }
    const pixels = sizes.map(([width, height]) => width * height);
    expect(pixels).toEqual([...pixels].sort((a, b) => b - a));
  });

  it('round-trips every offered choice through storage', () => {
    // The menu and the persisted string are the same vocabulary: a choice that could not be re-read
    // would silently revert to the host default on the next reload.
    for (const size of STREAM_SIZES) {
      expect(parseStreamSize(serializeStreamSize(size))).toEqual(size);
    }
  });
});

describe('serializeStreamSize', () => {
  it('writes an extent as WIDTHxHEIGHT and the host default as its own word', () => {
    expect(serializeStreamSize([960, 540])).toBe('960x540');
    expect(serializeStreamSize(null)).toBe(HOST_DEFAULT);
  });
});

describe('parseStreamSize', () => {
  it('reads a well-formed extent, whitespace and all', () => {
    expect(parseStreamSize('960x540')).toEqual([960, 540]);
    expect(parseStreamSize('  960 x 540  ')).toEqual([960, 540]);
  });

  it('treats anything unparseable as the host default rather than failing', () => {
    // A typo in a developer key must not stop the viewport from rendering.
    for (const raw of [null, '', HOST_DEFAULT, 'wide', '960', '960×540', '-960x540', '960x540x480']) {
      expect(parseStreamSize(raw)).toBeNull();
    }
  });

  it('treats a zero axis as the host default — an extent with no pixels is not a smaller stream', () => {
    expect(parseStreamSize('0x0')).toBeNull();
    expect(parseStreamSize('960x0')).toBeNull();
    expect(parseStreamSize('0x540')).toBeNull();
  });

  it('clamps an out-of-range extent instead of dropping the intent', () => {
    expect(parseStreamSize(`${MAX_STREAM_WIDTH + 1}x${MAX_STREAM_HEIGHT + 1}`)).toEqual([
      MAX_STREAM_WIDTH,
      MAX_STREAM_HEIGHT,
    ]);
  });
});

describe('clampStreamSize', () => {
  it('leaves a legal extent alone, both ends of both ranges included', () => {
    expect(clampStreamSize([960, 540])).toEqual([960, 540]);
    expect(clampStreamSize([1, 1])).toEqual([1, 1]);
    expect(clampStreamSize([MAX_STREAM_WIDTH, MAX_STREAM_HEIGHT])).toEqual([
      MAX_STREAM_WIDTH,
      MAX_STREAM_HEIGHT,
    ]);
  });

  it('bounds each axis by its own maximum, and rounds to whole pixels', () => {
    // The taller axis's lower ceiling must never bound the wider one.
    expect(clampStreamSize([MAX_STREAM_WIDTH, 99999])).toEqual([MAX_STREAM_WIDTH, MAX_STREAM_HEIGHT]);
    expect(clampStreamSize([0, -4])).toEqual([1, 1]);
    expect(clampStreamSize([960.4, 540.6])).toEqual([960, 541]);
  });
});

describe('describeStreamSize', () => {
  it('names the host default in words and an extent in pixels', () => {
    expect(describeStreamSize(null)).toBe('host default');
    expect(describeStreamSize([854, 480])).toBe('854×480');
  });
});
