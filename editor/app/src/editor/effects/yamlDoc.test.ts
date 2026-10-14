import { describe, expect, it } from 'vitest';
import {
  docValue,
  float,
  parseDoc,
  removeAtPath,
  renderDoc,
  setAtPath,
  shortestFloat,
  valueAt,
} from './yamlDoc';

/** A hand-authored file: a header comment, a comment inside a collection, and both float and
 *  integer scalars — everything a round trip must not damage. */
const SOURCE = `format: 1
kind: pattern
# the first boss phase — the sweep comes second
name: boss-phase-1
bullets:
  needle:
    size:
      - 0.3
      - 0.3
    hit_radius: 0.12
    life: 240
timeline:
  # the opening spiral
  - at: 0
    bullet: needle
    speed: 4.0
loop: 240
`;

describe('round trip', () => {
  it('returns an untouched file byte for byte, comments included', () => {
    expect(renderDoc(parseDoc(SOURCE))).toBe(SOURCE);
  });

  it('keeps every comment and key order through an edit somewhere else', () => {
    const doc = parseDoc(SOURCE);
    setAtPath(doc, ['bullets', 'needle', 'life'], 300);
    const text = renderDoc(doc);
    expect(text).toContain('# the first boss phase');
    expect(text).toContain('  # the opening spiral');
    expect(text).toContain('life: 300');
    expect(text.indexOf('name:')).toBeLessThan(text.indexOf('bullets:'));
  });

  it('reports a broken file rather than opening an empty form over it', () => {
    expect(() => parseDoc('name: [unclosed\n')).toThrow();
  });

  it('reads the document as plain JS, envelope included', () => {
    expect(docValue(parseDoc(SOURCE)).kind).toBe('pattern');
    expect(valueAt(parseDoc(SOURCE), ['timeline', 0, 'bullet'])).toBe('needle');
  });
});

describe('setAtPath', () => {
  it('writes a nested path, creating the collections it needs, in block style', () => {
    const doc = parseDoc(SOURCE);
    setAtPath(doc, ['bullets', 'needle', 'motion', 0, 'ramp'], {
      frames: 30,
      to: float(2),
    });
    const text = renderDoc(doc);
    expect(text).toContain('    motion:\n      - ramp:\n          frames: 30\n          to: 2.0\n');
    expect(text).not.toContain('{');
  });

  it('appends to a list by writing its next index', () => {
    const doc = parseDoc(SOURCE);
    setAtPath(doc, ['timeline', 1], { at: 60, bullet: 'needle', speed: float(6) });
    expect(valueAt(doc, ['timeline']) as unknown[]).toHaveLength(2);
    expect(renderDoc(doc)).toContain('  - at: 60\n    bullet: needle\n    speed: 6.0\n');
  });

  it('gives a float a fraction digit and an integer none', () => {
    const doc = parseDoc(SOURCE);
    setAtPath(doc, ['bullets', 'needle', 'hit_radius'], float(1));
    setAtPath(doc, ['bullets', 'needle', 'life'], 120);
    const text = renderDoc(doc);
    expect(text).toContain('hit_radius: 1.0');
    expect(text).toContain('life: 120');
  });

  it('refuses to replace the whole document — the envelope is not the form to edit', () => {
    const doc = parseDoc(SOURCE);
    setAtPath(doc, [], { name: 'nope' });
    expect(renderDoc(doc)).toBe(SOURCE);
  });
});

describe('removeAtPath', () => {
  it('removes one key', () => {
    const doc = parseDoc(SOURCE);
    removeAtPath(doc, ['bullets', 'needle', 'hit_radius']);
    expect(renderDoc(doc)).not.toContain('hit_radius');
  });

  it('prunes the collection a removal emptied, rather than leaving flow-style `[]`', () => {
    const doc = parseDoc(SOURCE);
    removeAtPath(doc, ['timeline', 0]);
    const text = renderDoc(doc);
    expect(text).not.toContain('timeline:');
    expect(text).not.toContain('[]');
    expect(text).toContain('loop: 240');
  });

  it('stops pruning at the document root', () => {
    const doc = parseDoc('format: 1\nkind: pattern\ntimeline:\n  - at: 0\n');
    removeAtPath(doc, ['timeline', 0]);
    expect(renderDoc(doc)).toBe('format: 1\nkind: pattern\n');
  });
});

describe('shortestFloat', () => {
  it('brings an f32 widened to a double back to the decimal a human typed', () => {
    expect(shortestFloat(0.10000000149011612)).toBe(0.1);
    expect(shortestFloat(0.25)).toBe(0.25);
    expect(shortestFloat(2)).toBe(2);
    expect(shortestFloat(7.5)).toBe(7.5);
  });

  it('agrees with the Rust writer on the f32 cases its helper pins', () => {
    expect(shortestFloat(0.30000001192092896)).toBe(0.3);
    expect(shortestFloat(0.8999999761581421)).toBe(0.9);
    expect(shortestFloat(1.0000000116860974e-7)).toBe(1e-7);
  });
});
