// Series colour: which of the eight categorical slots a name wears, and for how long.
//
// THE RULE THE WHOLE MODULE EXISTS FOR: **colour follows the entity, never its rank.** The breakdown
// bars are sorted by cost and the ranking moves every window; if the colour came from the row, the
// panel would repaint itself once a second and a human would learn nothing from it. A name takes a
// slot the first time it is seen and keeps it until the view unmounts.
//
// EIGHT SLOTS, NOT NINE. `--pf-cat-0..7` are the validated categorical palette (see index.css); a
// ninth hue is one nobody checked for colour-vision separation, so the ninth *series* folds into
// `Other` (`model.ts`'s `foldSeries`) and wears the fixed neutral below rather than a new colour.
// Overflow past the slots is still possible — several groups draw at once and each folds
// separately — and that is what the hash fallback is for: a stable, arbitrary slot, deliberately
// preferred over a cycling counter so the same name lands on the same colour in every session.
//
// Pure: names in, CSS custom-property references out. No DOM, no React.

import { OTHER_KEY } from './model';

/** The categorical palette's size. */
export const PALETTE_SLOTS = 8;

/** The folded remainder's colour — a neutral, never a slot: `Other` is not an identity. */
export const OTHER_COLOR = 'var(--pf-other)';

/** Names to slots for one view session. Mutable by design (see the header) and never persisted: a
 *  fresh mount is a fresh assignment, which is what makes first-seen order meaningful. */
export interface SlotRegistry {
  readonly assigned: Map<string, number>;
  next: number;
}

export function createRegistry(): SlotRegistry {
  return { assigned: new Map(), next: 0 };
}

/**
 * FNV-1a over the name's UTF-16 code units, as an unsigned 32-bit value.
 *
 * Chosen for being short enough to read and stable across engines — it is not a security primitive
 * and never leaves the client; all that is asked of it is that two names of similar shape
 * (`bloom_h`, `bloom_v`) do not systematically collide the way a length or first-character rule
 * would.
 */
export function fnv1a(text: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i += 1) {
    hash ^= text.charCodeAt(i);
    // The FNV prime, 16777619, by shifts: a 32-bit multiply overflows a JS double's exact range.
    hash = (hash + (hash << 1) + (hash << 4) + (hash << 7) + (hash << 8) + (hash << 24)) >>> 0;
  }
  return hash >>> 0;
}

/**
 * The slot `name` wears. First-seen names take slots in order; once the palette is spent, a name
 * hashes into it. Either way the answer is remembered, so it never changes under the same registry.
 */
export function slotFor(registry: SlotRegistry, name: string): number {
  const known = registry.assigned.get(name);
  if (known !== undefined) {
    return known;
  }
  const slot = registry.next < PALETTE_SLOTS ? registry.next : fnv1a(name) % PALETTE_SLOTS;
  registry.next += 1;
  registry.assigned.set(name, slot);
  return slot;
}

/** The CSS reference for a slot. Out-of-range wraps rather than producing an undefined property. */
export function slotColor(slot: number): string {
  const index = ((slot % PALETTE_SLOTS) + PALETTE_SLOTS) % PALETTE_SLOTS;
  return `var(--pf-cat-${index})`;
}

/** The colour a series key draws in — the one call sites use. */
export function colorFor(registry: SlotRegistry, key: string): string {
  return key === OTHER_KEY ? OTHER_COLOR : slotColor(slotFor(registry, key));
}
