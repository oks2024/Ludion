import { describe, expect, it } from 'vitest';
import {
  PROJECT_DIR_KEY,
  isAbsolutePath,
  normalizePath,
  pickProjectDir,
  resolveProjectDir,
  sameProjectPath,
  toStoredProjectDir,
} from './projectDir';

const WIN_ROOT = 'C:\\Work\\lore\\ludion';
const POSIX_ROOT = '/Users/dev/ludion';

describe('normalizePath', () => {
  it('folds separators and strips trailing ones and a leading ./', () => {
    expect(normalizePath('C:\\Work\\lore\\ludion\\')).toBe('C:/Work/lore/ludion');
    expect(normalizePath('./games/vigil/')).toBe('games/vigil');
    expect(normalizePath('games\\vigil')).toBe('games/vigil');
    expect(normalizePath('')).toBe('');
    expect(normalizePath('.')).toBe('');
  });

  it('keeps a bare root its one separator', () => {
    expect(normalizePath('/')).toBe('/');
    expect(normalizePath('C:\\')).toBe('C:/');
  });
});

describe('isAbsolutePath', () => {
  it('recognises POSIX roots and Windows drives, either separator', () => {
    expect(isAbsolutePath('/repo')).toBe(true);
    expect(isAbsolutePath('C:/Work')).toBe(true);
    expect(isAbsolutePath('d:\\Work')).toBe(true);
    expect(isAbsolutePath('games/vigil')).toBe(false);
    expect(isAbsolutePath('')).toBe(false);
  });
});

describe('toStoredProjectDir', () => {
  it('stores a project inside the checkout relative to it', () => {
    expect(toStoredProjectDir('C:\\Work\\lore\\ludion\\games\\vigil', WIN_ROOT)).toBe('games/vigil');
    expect(toStoredProjectDir('C:/Work/lore/ludion/games/vigil/', WIN_ROOT)).toBe('games/vigil');
    expect(toStoredProjectDir('/Users/dev/ludion/games/vigil', POSIX_ROOT)).toBe('games/vigil');
  });

  it('folds drive-letter case on Windows and not on POSIX', () => {
    // The Windows file system is case-insensitive: `c:/work` and `C:/Work` are one directory.
    expect(toStoredProjectDir('c:/work/LORE/ludion/games/vigil', WIN_ROOT)).toBe('games/vigil');
    // A POSIX path with different case is a different directory: it stays absolute.
    expect(toStoredProjectDir('/Users/Dev/ludion/games/vigil', POSIX_ROOT)).toBe('/Users/Dev/ludion/games/vigil');
  });

  it('keeps a project outside the checkout absolute', () => {
    expect(toStoredProjectDir('D:/other/games/foo', WIN_ROOT)).toBe('D:/other/games/foo');
    // A sibling whose name merely extends the root name is outside it.
    expect(toStoredProjectDir('C:/Work/lore/ludion-2/games/vigil', WIN_ROOT)).toBe('C:/Work/lore/ludion-2/games/vigil');
  });

  it('keeps a relative input relative, and an absolute input absolute with no root', () => {
    expect(toStoredProjectDir('games/vigil', WIN_ROOT)).toBe('games/vigil');
    expect(toStoredProjectDir('.\\games\\vigil', '')).toBe('games/vigil');
    expect(toStoredProjectDir('C:/Work/lore/ludion/games/vigil', '')).toBe('C:/Work/lore/ludion/games/vigil');
  });

  it('names the checkout root itself as the empty relative path', () => {
    expect(toStoredProjectDir(WIN_ROOT, WIN_ROOT)).toBe('');
    expect(resolveProjectDir('', WIN_ROOT)).toBe('');
  });
});

describe('resolveProjectDir', () => {
  it('rejoins a relative value onto the checkout root, either separator', () => {
    expect(resolveProjectDir('games/vigil', WIN_ROOT)).toBe('C:/Work/lore/ludion/games/vigil');
    expect(resolveProjectDir('games/vigil', POSIX_ROOT)).toBe('/Users/dev/ludion/games/vigil');
    expect(resolveProjectDir('games/vigil', `${POSIX_ROOT}/`)).toBe('/Users/dev/ludion/games/vigil');
  });

  it('round-trips through the stored form onto a checkout on another drive', () => {
    // The whole point: the value persisted on the D: checkout resolves onto the C: one.
    const stored = toStoredProjectDir('D:/Repos/Ludion/games/vigil', 'D:/Repos/Ludion');
    expect(stored).toBe('games/vigil');
    expect(resolveProjectDir(stored, WIN_ROOT)).toBe('C:/Work/lore/ludion/games/vigil');
  });

  it('passes an absolute value through, root or no root', () => {
    expect(resolveProjectDir('D:\\other\\games\\foo', WIN_ROOT)).toBe('D:/other/games/foo');
    expect(resolveProjectDir('/srv/foo', '')).toBe('/srv/foo');
  });

  it('is empty for nothing stored, and for a relative value with no root to resolve against', () => {
    expect(resolveProjectDir(null, WIN_ROOT)).toBe('');
    expect(resolveProjectDir('', WIN_ROOT)).toBe('');
    // The browser dev page has no host and therefore no root: a relative value is unresolvable there.
    expect(resolveProjectDir('games/vigil', '')).toBe('');
  });
});

describe('pickProjectDir', () => {
  it('lets a launch argument win over anything stored', () => {
    expect(pickProjectDir('games/other', 'C:\\Work\\lore\\ludion\\games\\vigil', WIN_ROOT)).toBe('games/vigil');
  });

  it('persists a launch argument inside the checkout relative, and outside it absolute', () => {
    expect(pickProjectDir(null, 'C:\\Work\\lore\\ludion\\games\\vigil', WIN_ROOT)).toBe('games/vigil');
    expect(pickProjectDir(null, 'D:\\other\\games\\foo', WIN_ROOT)).toBe('D:/other/games/foo');
  });

  it('uses the stored value when there is no launch argument', () => {
    expect(pickProjectDir('games/vigil', null, WIN_ROOT)).toBe('games/vigil');
    expect(pickProjectDir('games/vigil', '', WIN_ROOT)).toBe('games/vigil');
  });

  it('re-collapses an absolute stored value inside the checkout to relative, the pre-fix shape', () => {
    expect(pickProjectDir('C:\\Work\\lore\\ludion\\games\\vigil', null, WIN_ROOT)).toBe('games/vigil');
  });

  it('falls back to no project (the picker) when both are empty or null', () => {
    expect(pickProjectDir(null, null, WIN_ROOT)).toBe('');
    expect(pickProjectDir('', '', WIN_ROOT)).toBe('');
  });
});

describe('sameProjectPath', () => {
  it('folds drive-letter case and ignores a trailing slash', () => {
    expect(sameProjectPath('C:/Work/Game', 'c:\\work\\game\\')).toBe(true);
    expect(sameProjectPath(`${WIN_ROOT}/games/vigil`, `${WIN_ROOT}/games/vigil/`)).toBe(true);
  });

  it('does not fold case on a POSIX path', () => {
    expect(sameProjectPath('/Users/Dev/ludion', '/Users/dev/ludion')).toBe(false);
  });

  it('is false for genuinely different directories', () => {
    expect(sameProjectPath('games/vigil', 'games/other')).toBe(false);
  });
});

describe('PROJECT_DIR_KEY', () => {
  it('is persisted under the key the editor has always used', () => {
    expect(PROJECT_DIR_KEY).toBe('ludion.editor.projectDir');
  });
});
