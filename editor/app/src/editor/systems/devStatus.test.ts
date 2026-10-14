import { describe, expect, it } from 'vitest';
import {
  diagnosticsForFile,
  isNewer,
  normalizePath,
  parseDevStatus,
  pathsMatch,
  type DevDiagnostic,
} from './devStatus';

describe('parseDevStatus', () => {
  it('parses an ok build with duration and restored frame', () => {
    const s = parseDevStatus(
      JSON.stringify({ state: 'ok', seq: 7, timestamp: '2026-07-19T10:00:00Z', duration_ms: 1234, restored_frame: 42 }),
    );
    expect(s).toEqual({
      state: 'ok',
      seq: 7,
      timestamp: '2026-07-19T10:00:00Z',
      durationMs: 1234,
      restoredFrame: 42,
      message: null,
      snapshot: null,
      diagnostics: [],
    });
  });

  it('parses a failed restore with its message and kept snapshot path', () => {
    const s = parseDevStatus(
      JSON.stringify({
        state: 'restore_failed',
        seq: 9,
        timestamp: 't',
        duration_ms: 512,
        message: 'engine.restore failed',
        snapshot: 'game/.ludion/dev-snapshot.json',
      }),
    );
    expect(s?.state).toBe('restore_failed');
    expect(s?.message).toBe('engine.restore failed');
    expect(s?.snapshot).toBe('game/.ludion/dev-snapshot.json');
    expect(s?.durationMs).toBe(512);
  });

  it('parses a failed build with diagnostics, mapping snake_case fields', () => {
    const s = parseDevStatus(
      JSON.stringify({
        state: 'failed',
        seq: 8,
        timestamp: 't',
        diagnostics: [
          { level: 'error', code: 'E0308', message: 'mismatched types', file: 'src/movement.rs', line: 12, column: 5, rendered: 'error[E0308]...' },
          { message: 'no file diag' },
          'garbage',
        ],
      }),
    );
    expect(s?.state).toBe('failed');
    expect(s?.diagnostics).toHaveLength(2);
    expect(s?.diagnostics[0]).toEqual({
      level: 'error',
      code: 'E0308',
      message: 'mismatched types',
      file: 'src/movement.rs',
      line: 12,
      column: 5,
      rendered: 'error[E0308]...',
    });
    // A diag with only a message defaults level to error and null-fills the rest.
    expect(s?.diagnostics[1]).toEqual({ level: 'error', code: null, message: 'no file diag', file: null, line: null, column: null, rendered: null });
  });

  it('returns null for invalid JSON, wrong shape, or an unknown state', () => {
    expect(parseDevStatus('not json')).toBeNull();
    expect(parseDevStatus('[]')).toBeNull();
    expect(parseDevStatus(JSON.stringify({ state: 'ok' }))).toBeNull(); // no seq
    expect(parseDevStatus(JSON.stringify({ state: 'bogus', seq: 1 }))).toBeNull();
  });
});

describe('isNewer', () => {
  const status = (seq: number) => parseDevStatus(JSON.stringify({ state: 'ok', seq }))!;
  it('is newer when no previous seq or the seq advanced', () => {
    expect(isNewer(null, status(1))).toBe(true);
    expect(isNewer(3, status(4))).toBe(true);
  });
  it('is not newer for an equal or older seq', () => {
    expect(isNewer(4, status(4))).toBe(false);
    expect(isNewer(5, status(4))).toBe(false);
  });
});

describe('pathsMatch / normalizePath', () => {
  it('normalizes separators, runs, and case', () => {
    expect(normalizePath('C:\\Repos\\Ludion\\src\\a.rs')).toBe('c:/repos/ludion/src/a.rs');
    expect(normalizePath('a//b')).toBe('a/b');
  });

  it('matches a relative rustc file against an absolute open path', () => {
    expect(pathsMatch('src/movement.rs', 'D:/Repos/Ludion/games/vigil/src/movement.rs')).toBe(true);
    expect(pathsMatch('src/movement.rs', 'D:\\Repos\\Ludion\\games\\vigil\\src\\movement.rs')).toBe(true);
    expect(pathsMatch('movement.rs', 'D:/Repos/Ludion/games/vigil/src/movement.rs')).toBe(true);
  });

  it('respects segment boundaries and exact equality', () => {
    expect(pathsMatch('movement.rs', '/x/enemy_movement.rs')).toBe(false);
    expect(pathsMatch('/x/a.rs', '/x/a.rs')).toBe(true);
    expect(pathsMatch('', '/x/a.rs')).toBe(false);
  });
});

describe('diagnosticsForFile', () => {
  const diag = (file: string | null, message: string): DevDiagnostic => ({
    level: 'error',
    code: null,
    message,
    file,
    line: 1,
    column: 1,
    rendered: null,
  });
  it('keeps only diagnostics whose file resolves to the open path', () => {
    const all = [diag('src/a.rs', 'A'), diag('src/b.rs', 'B'), diag(null, 'no file')];
    const got = diagnosticsForFile(all, '/proj/src/a.rs');
    expect(got.map((d) => d.message)).toEqual(['A']);
  });
});
