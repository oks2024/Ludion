import { describe, expect, it } from 'vitest';
import {
  docKey,
  docLabel,
  docPath,
  isPatternDoc,
  newPatternKey,
  newPatternText,
  slugify,
} from './patternFiles';

describe('isPatternDoc', () => {
  it('accepts a top-level kind line, quoted or bare, with a trailing comment', () => {
    expect(isPatternDoc('format: 1\nkind: pattern\nname: boss\n')).toBe(true);
    expect(isPatternDoc('kind: "pattern"\n')).toBe(true);
    expect(isPatternDoc("kind: 'pattern'\n")).toBe(true);
    expect(isPatternDoc('kind: pattern  # the boss phase\n')).toBe(true);
    expect(isPatternDoc('format: 1\r\nkind: pattern\r\n')).toBe(true);
  });

  it('rejects every other document, and an INDENTED kind that belongs to something else', () => {
    expect(isPatternDoc('format: 1\nkind: atlas-source\n')).toBe(false);
    expect(isPatternDoc('format: 1\nname: plaza\nentities:\n  - name: a\n')).toBe(false);
    expect(isPatternDoc('entities:\n  - components:\n      kind: pattern\n')).toBe(false);
    expect(isPatternDoc('kind: patterns\n')).toBe(false);
    expect(isPatternDoc('')).toBe(false);
  });
});

describe('docKey', () => {
  it('forward-slashes a listing path and leaves it content-root-relative', () => {
    expect(docKey('/repo/games/vigil/scenes', 'patterns/boss.yaml')).toBe('patterns/boss.yaml');
    expect(docKey('/repo/games/vigil/scenes', 'patterns\\boss.yaml')).toBe('patterns/boss.yaml');
  });

  it('strips the root when an absolute path reaches it, whatever the separator or case', () => {
    expect(docKey('D:\\Repos\\Ludion\\games\\vigil\\scenes', 'D:/Repos/Ludion/games/vigil/scenes/patterns/boss.yaml')).toBe(
      'patterns/boss.yaml',
    );
    expect(docKey('/repo/scenes/', '/repo/scenes/boss.yaml')).toBe('boss.yaml');
  });
});

describe('docPath', () => {
  it('rejoins a key onto the content root', () => {
    expect(docPath('/repo/games/vigil/scenes', 'patterns/boss.yaml')).toBe(
      '/repo/games/vigil/scenes/patterns/boss.yaml',
    );
    expect(docPath('D:\\Repos\\Ludion\\games\\vigil\\scenes\\', '/patterns/boss.yaml')).toBe(
      'D:/Repos/Ludion/games/vigil/scenes/patterns/boss.yaml',
    );
  });
});

describe('docLabel', () => {
  it('is the file stem a picker row shows', () => {
    expect(docLabel('patterns/boss.yaml')).toBe('boss');
    expect(docLabel('boss.yaml')).toBe('boss');
    expect(docLabel('patterns/boss')).toBe('boss');
  });
});

describe('slugify / newPatternKey', () => {
  it('turns a typed name into a filename stem', () => {
    expect(slugify('Boss Phase 1')).toBe('boss-phase-1');
    expect(slugify('  sweep — left  ')).toBe('sweep-left');
    expect(slugify('!!!')).toBe('');
  });

  it('places a new document under the patterns folder, and refuses an empty name', () => {
    expect(newPatternKey('Boss Phase 1')).toBe('patterns/boss-phase-1.yaml');
    expect(newPatternKey('   ')).toBeNull();
  });
});

describe('newPatternText', () => {
  it('is itself a pattern document the picker would list', () => {
    expect(isPatternDoc(newPatternText('Boss Phase 1'))).toBe(true);
  });

  it('carries the slugged name, one bullet kind and one timeline entry, in block style', () => {
    const text = newPatternText('Boss Phase 1');
    expect(text).toContain('name: boss-phase-1');
    expect(text).toContain('  needle:');
    expect(text).toContain('    bullet: needle');
    expect(text).not.toContain('[');
    expect(text).not.toContain('{');
  });
});
