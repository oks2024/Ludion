import { describe, expect, it } from 'vitest';
import { colorFor, createRegistry, fnv1a, OTHER_COLOR, PALETTE_SLOTS, slotColor, slotFor } from './colors';
import { OTHER_KEY } from './model';

describe('fnv1a', () => {
  it('matches the reference 32-bit FNV-1a vectors', () => {
    expect(fnv1a('')).toBe(0x811c9dc5);
    expect(fnv1a('a')).toBe(0xe40c292c);
    expect(fnv1a('foobar')).toBe(0xbf9cf968);
  });

  it('is unsigned and stable for names that differ only at the end', () => {
    expect(fnv1a('bloom_h')).toBeGreaterThanOrEqual(0);
    expect(fnv1a('bloom_h')).not.toBe(fnv1a('bloom_v'));
  });
});

describe('slot assignment', () => {
  it('hands the palette out in first-seen order', () => {
    const registry = createRegistry();
    expect(slotFor(registry, 'enemy_ai')).toBe(0);
    expect(slotFor(registry, 'move_projectiles')).toBe(1);
    expect(slotFor(registry, 'quest_clock')).toBe(2);
  });

  it('keeps a name on its slot however the ranking moves', () => {
    const registry = createRegistry();
    const first = slotFor(registry, 'enemy_ai');
    slotFor(registry, 'move_projectiles');
    slotFor(registry, 'spawn_enemies');
    expect(slotFor(registry, 'enemy_ai')).toBe(first);
  });

  it('hashes into the palette once the slots are spent, and remembers the answer', () => {
    const registry = createRegistry();
    for (let i = 0; i < PALETTE_SLOTS; i += 1) {
      slotFor(registry, `taken_${i}`);
    }
    const overflow = slotFor(registry, 'late_system');
    expect(overflow).toBe(fnv1a('late_system') % PALETTE_SLOTS);
    expect(overflow).toBeGreaterThanOrEqual(0);
    expect(overflow).toBeLessThan(PALETTE_SLOTS);
    expect(slotFor(registry, 'late_system')).toBe(overflow);
  });

  it('gives two registries the same answer for the same first-seen order', () => {
    const a = createRegistry();
    const b = createRegistry();
    for (const name of ['x', 'y', 'z']) {
      expect(slotFor(a, name)).toBe(slotFor(b, name));
    }
  });
});

describe('colours', () => {
  it('names a validated categorical token, wrapping rather than inventing a property', () => {
    expect(slotColor(0)).toBe('var(--pf-cat-0)');
    expect(slotColor(PALETTE_SLOTS - 1)).toBe(`var(--pf-cat-${PALETTE_SLOTS - 1})`);
    expect(slotColor(PALETTE_SLOTS)).toBe('var(--pf-cat-0)');
    expect(slotColor(-1)).toBe(`var(--pf-cat-${PALETTE_SLOTS - 1})`);
  });

  it('paints the folded remainder with the fixed neutral, never a slot', () => {
    const registry = createRegistry();
    expect(colorFor(registry, OTHER_KEY)).toBe(OTHER_COLOR);
    // Asking for it does not consume a slot: the next real series still opens the palette.
    expect(colorFor(registry, 'system:enemy_ai')).toBe('var(--pf-cat-0)');
  });
});
