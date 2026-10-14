import { describe, expect, it } from 'vitest';
import { candidatePaths, findFnLine, isAbsolutePath, joinPath, resolveSystemSource } from './locate';

describe('candidatePaths', () => {
  it('produces the module-layout candidates for a nested fn, climbing the module chain', () => {
    expect(candidatePaths('vigil::a::b::sys')).toEqual([
      'src/a/b.rs',
      'src/a/b/mod.rs',
      'src/a.rs',
      'src/a/mod.rs',
      'src/lib.rs',
      'src/main.rs',
    ]);
  });

  it('handles a single-module fn', () => {
    expect(candidatePaths('vigil::movement::move_player')).toEqual([
      'src/movement.rs',
      'src/movement/mod.rs',
      'src/lib.rs',
      'src/main.rs',
    ]);
  });

  it('resolves a crate-root fn to lib.rs / main.rs only', () => {
    expect(candidatePaths('vigil::tick_clock')).toEqual(['src/lib.rs', 'src/main.rs']);
  });

  it('strips generic params and closure segments', () => {
    expect(candidatePaths('vigil::a::sys<T>')).toEqual(['src/a.rs', 'src/a/mod.rs', 'src/lib.rs', 'src/main.rs']);
    expect(candidatePaths('vigil::a::sys::{{closure}}')).toEqual([
      'src/a.rs',
      'src/a/mod.rs',
      'src/lib.rs',
      'src/main.rs',
    ]);
  });
});

describe('findFnLine', () => {
  const src = [
    '// header',
    'use foo::bar;',
    '',
    'pub fn spawn_wave(world: &mut World) {',
    '    // ...',
    '}',
    '',
    '#[allow(dead_code)]',
    'pub(crate) async fn move_player(dt: f32) {}',
    '',
    'fn helper() {}',
  ].join('\n');

  it('finds a plain fn (1-based)', () => {
    expect(findFnLine(src, 'helper')).toBe(11);
  });

  it('finds a pub fn', () => {
    expect(findFnLine(src, 'spawn_wave')).toBe(4);
  });

  it('finds a fn behind visibility + qualifiers, past an attribute line', () => {
    expect(findFnLine(src, 'move_player')).toBe(9);
  });

  it('returns null for a fn that is not defined here', () => {
    expect(findFnLine(src, 'nope')).toBe(null);
  });

  it('does not match a substring name', () => {
    expect(findFnLine('fn move_player_thing() {}', 'move_player')).toBe(null);
  });
});

describe('path helpers', () => {
  it('detects absolute paths across platforms', () => {
    expect(isAbsolutePath('/home/x/src/lib.rs')).toBe(true);
    expect(isAbsolutePath('C:/Work/vigil/src/lib.rs')).toBe(true);
    expect(isAbsolutePath('C:\\Work\\vigil\\src\\lib.rs')).toBe(true);
    expect(isAbsolutePath('src/lib.rs')).toBe(false);
  });

  it('joins with a single slash, trimming a trailing separator', () => {
    expect(joinPath('/proj', 'src/lib.rs')).toBe('/proj/src/lib.rs');
    expect(joinPath('/proj/', 'src/lib.rs')).toBe('/proj/src/lib.rs');
    expect(joinPath('C:\\proj\\', 'src/lib.rs')).toBe('C:\\proj/src/lib.rs');
  });
});

describe('resolveSystemSource', () => {
  const reader = (files: Record<string, string>) => async (path: string) => {
    const text = files[path];
    if (text === undefined) {
      throw new Error(`ENOENT ${path}`);
    }
    return text;
  };

  it('returns the first candidate containing the fn, with its line', async () => {
    const files = {
      '/proj/src/movement.rs': 'use x;\n\npub fn move_player() {}\n',
    };
    const result = await resolveSystemSource({
      fnPath: 'vigil::movement::move_player',
      registeredAtFile: 'games/vigil/src/lib.rs',
      projectDir: '/proj',
      read: reader(files),
    });
    expect(result).toEqual({ path: '/proj/src/movement.rs', text: files['/proj/src/movement.rs'], line: 3 });
  });

  it('skips a readable file that lacks the fn and keeps probing', async () => {
    const files = {
      '/proj/src/movement.rs': '// a re-export shim, no fn here\npub use crate::real::move_player;\n',
      '/proj/src/movement/mod.rs': 'fn move_player() {}\n',
    };
    const result = await resolveSystemSource({
      fnPath: 'vigil::movement::move_player',
      registeredAtFile: null,
      projectDir: '/proj',
      read: reader(files),
    });
    expect(result?.path).toBe('/proj/src/movement/mod.rs');
    expect(result?.line).toBe(1);
  });

  it('falls back to registered_at.file when the fn is nowhere in the layout candidates', async () => {
    const files = {
      '/proj/src/lib.rs': '// registration chain only\n',
      '/repo/games/vigil/src/systems.rs': 'pub fn orphan() {}\n',
    };
    const result = await resolveSystemSource({
      fnPath: 'vigil::orphan',
      registeredAtFile: '/repo/games/vigil/src/systems.rs',
      projectDir: '/proj',
      read: reader(files),
    });
    expect(result).toEqual({ path: '/repo/games/vigil/src/systems.rs', text: files['/repo/games/vigil/src/systems.rs'], line: 1 });
  });

  it('returns the first readable candidate (line 0) when the fn is unlocatable and no fallback reads', async () => {
    const files = { '/proj/src/lib.rs': '// nothing matches\n' };
    const result = await resolveSystemSource({
      fnPath: 'vigil::ghost',
      registeredAtFile: null,
      projectDir: '/proj',
      read: reader(files),
    });
    expect(result).toEqual({ path: '/proj/src/lib.rs', text: files['/proj/src/lib.rs'], line: 0 });
  });

  it('returns null when nothing can be read', async () => {
    const result = await resolveSystemSource({
      fnPath: 'vigil::a::sys',
      registeredAtFile: 'nope.rs',
      projectDir: '/proj',
      read: reader({}),
    });
    expect(result).toBe(null);
  });
});
