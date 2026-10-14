import { describe, expect, it } from 'vitest';
import { RECENT_PROJECTS_CAP, RECENT_PROJECTS_KEY, parseRecents, pushRecent } from './recents';

describe('parseRecents', () => {
  it('parses a JSON array of non-empty strings', () => {
    expect(parseRecents('["games/vigil", "games/other"]')).toEqual(['games/vigil', 'games/other']);
  });

  it('drops non-string and empty entries', () => {
    expect(parseRecents('["games/vigil", "", 3, null, "games/other"]')).toEqual(['games/vigil', 'games/other']);
  });

  it('reads garbage, a non-array, or nothing stored as no recents', () => {
    expect(parseRecents(null)).toEqual([]);
    expect(parseRecents('not json')).toEqual([]);
    expect(parseRecents('{"a": 1}')).toEqual([]);
    expect(parseRecents('"games/vigil"')).toEqual([]);
  });
});

describe('pushRecent', () => {
  it('puts the new entry first', () => {
    expect(pushRecent(['games/other'], 'games/vigil')).toEqual(['games/vigil', 'games/other']);
  });

  it('dedupes an earlier entry naming the same project, drive-letter case included', () => {
    expect(pushRecent(['games/other', 'C:/Work/Game'], 'c:\\work\\game\\')).toEqual([
      'c:\\work\\game\\',
      'games/other',
    ]);
  });

  it('caps at 8, dropping the oldest', () => {
    const full = Array.from({ length: RECENT_PROJECTS_CAP }, (_, i) => `games/p${i}`);
    const result = pushRecent(full, 'games/new');
    expect(result).toHaveLength(RECENT_PROJECTS_CAP);
    expect(result[0]).toBe('games/new');
    expect(result).not.toContain(`games/p${RECENT_PROJECTS_CAP - 1}`);
  });
});

describe('RECENT_PROJECTS_KEY', () => {
  it('is the persisted key', () => {
    expect(RECENT_PROJECTS_KEY).toBe('ludion.editor.recentProjects');
  });
});
