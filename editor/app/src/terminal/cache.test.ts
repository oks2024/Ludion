import { describe, expect, it } from 'vitest';
import {
  CACHE_TTL_MS,
  INITIAL_CACHE_STATE,
  cacheCountdownMs,
  contextFraction,
  formatContextTokens,
  formatCountdown,
  parseStatusPayload,
  parseStatusText,
  updateCacheState,
} from './cache';

const payload = (over: Partial<Record<string, unknown>> = {}) => ({
  context_window: {
    total_input_tokens: 68000,
    context_window_size: 200000,
    used_percentage: 34,
    current_usage: { cache_creation_input_tokens: 1200, cache_read_input_tokens: 5000 },
    ...over,
  },
});

describe('parseStatusPayload', () => {
  it('extracts the context_window fields', () => {
    const p = parseStatusPayload(payload());
    expect(p).toEqual({
      usedTokens: 68000,
      contextSize: 200000,
      usedPercent: 34,
      cacheCreation: 1200,
      cacheRead: 5000,
    });
  });

  it('degrades unknown shapes to all-null (never throws)', () => {
    expect(parseStatusPayload({})).toEqual({
      usedTokens: null,
      contextSize: null,
      usedPercent: null,
      cacheCreation: null,
      cacheRead: null,
    });
    expect(parseStatusPayload('nonsense').usedTokens).toBeNull();
  });
});

describe('parseStatusText', () => {
  it('returns null on non-JSON', () => {
    expect(parseStatusText('')).toBeNull();
    expect(parseStatusText('{ half')).toBeNull();
  });
  it('parses valid JSON text', () => {
    expect(parseStatusText(JSON.stringify(payload()))?.usedTokens).toBe(68000);
  });
});

describe('contextFraction', () => {
  it('prefers the explicit percentage', () => {
    expect(contextFraction(parseStatusPayload(payload({ used_percentage: 50 })))).toBeCloseTo(0.5);
  });
  it('derives from used/size when no percentage', () => {
    const p = parseStatusPayload(payload({ used_percentage: undefined, total_input_tokens: 50000, context_window_size: 200000 }));
    expect(contextFraction(p)).toBeCloseTo(0.25);
  });
  it('is null with nothing to go on', () => {
    expect(contextFraction(null)).toBeNull();
    expect(contextFraction(parseStatusPayload({}))).toBeNull();
  });
});

describe('formatContextTokens', () => {
  it('renders used/total compactly', () => {
    expect(formatContextTokens(parseStatusPayload(payload()))).toBe('68k/200k');
  });
  it('is an em dash before any payload', () => {
    expect(formatContextTokens(null)).toBe('—');
  });
});

describe('cache countdown', () => {
  it('stamps a write time on the first cache-creating turn and counts down from the TTL', () => {
    const s1 = updateCacheState(INITIAL_CACHE_STATE, parseStatusPayload(payload()), 1000);
    expect(s1.lastWriteAt).toBe(1000);
    expect(cacheCountdownMs(s1, 1000)).toBe(CACHE_TTL_MS);
    expect(cacheCountdownMs(s1, 1000 + 60_000)).toBe(CACHE_TTL_MS - 60_000);
  });

  it('does not re-stamp while the same turn keeps reporting the cache value', () => {
    const p = parseStatusPayload(payload());
    const s1 = updateCacheState(INITIAL_CACHE_STATE, p, 1000);
    const s2 = updateCacheState(s1, p, 2000); // same signature, later read
    expect(s2.lastWriteAt).toBe(1000);
  });

  it('re-stamps on a new cache-creating turn (changed signature)', () => {
    const s1 = updateCacheState(INITIAL_CACHE_STATE, parseStatusPayload(payload()), 1000);
    const s2 = updateCacheState(
      s1,
      parseStatusPayload(payload({ total_input_tokens: 71000, current_usage: { cache_creation_input_tokens: 800, cache_read_input_tokens: 9000 } })),
      50_000,
    );
    expect(s2.lastWriteAt).toBe(50_000);
  });

  it('ignores turns with no cache write', () => {
    const p = parseStatusPayload(payload({ current_usage: { cache_creation_input_tokens: 0, cache_read_input_tokens: 9000 } }));
    expect(updateCacheState(INITIAL_CACHE_STATE, p, 1000)).toEqual(INITIAL_CACHE_STATE);
  });

  it('clamps the countdown at zero and is null before any write', () => {
    const s1 = updateCacheState(INITIAL_CACHE_STATE, parseStatusPayload(payload()), 1000);
    expect(cacheCountdownMs(s1, 1000 + CACHE_TTL_MS + 5000)).toBe(0);
    expect(cacheCountdownMs(INITIAL_CACHE_STATE, 1000)).toBeNull();
  });
});

describe('formatCountdown', () => {
  it('renders m:ss with zero-padded seconds', () => {
    expect(formatCountdown(4 * 60_000 + 12_000)).toBe('4:12');
    expect(formatCountdown(41_000)).toBe('0:41');
    expect(formatCountdown(0)).toBe('0:00');
  });
  it('is an em dash when unknown', () => {
    expect(formatCountdown(null)).toBe('—');
  });
});
