import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { CheckSceneResult } from '../../shell/host';
import { parseCheckScene } from './readout';

// The two samples are `ludion check-scene --json` run from `crates/ludion-cli` on its own fixtures
// (`tests/fixtures/good_voxel.voxel.yaml`, `tests/fixtures/assembly/good.assembly.yaml`);
// `tests/check_scene_json.rs` fails when the CLI's output stops matching them. Expected values below
// are read off those fixtures' sources, not off the parser.

function sample(name: string): string {
  return readFileSync(fileURLToPath(new URL(`./__fixtures__/${name}`, import.meta.url)), 'utf8');
}

const ok = (stdout: string, stderr = ''): CheckSceneResult => ({ stdout, stderr, exitCode: 0 });

const VOXEL = sample('check-scene-voxel.json');
const ASSEMBLY = sample('check-scene-assembly.json');

describe('parseCheckScene — a voxel model', () => {
  it('reads size, fill, components and scale from the CLI sample', () => {
    const readout = parseCheckScene(ok(VOXEL));
    expect(readout.ok).toBe(true);
    expect(readout.kind).toBe('voxel');
    expect(readout.path).toBe('tests/fixtures/good_voxel.voxel.yaml');
    // size [2, 2, 1]; slices `AB` / `.A` fill three cells in one piece; no `scale:` declared.
    expect(readout.size).toEqual([2, 2, 1]);
    expect(readout.filled).toBe(3);
    expect(readout.components).toBe(1);
    expect(readout.scale).toBeNull();
    expect(readout.parts).toBeNull();
    expect(readout.clips).toEqual([]);
    expect(readout.diagnostics).toEqual([]);
    expect(readout.summary).toBe('2x2x1 · 3 filled · 1 component');
  });

  it('echoes a declared scale', () => {
    const object = { ...JSON.parse(VOXEL), scale: 0.24, filled: 283, components: 2 };
    const readout = parseCheckScene(ok(JSON.stringify(object)));
    expect(readout.scale).toBe(0.24);
    expect(readout.summary).toBe('2x2x1 · 283 filled · 2 components · scale 0.24');
  });

  it('a warning on a passing model reaches the status line and keeps ok', () => {
    const object = {
      ...JSON.parse(VOXEL),
      diagnostics: [{ code: 'L1129', severity: 'warning', message: 'two pieces', path: '/slices' }],
    };
    const readout = parseCheckScene(ok(JSON.stringify(object)));
    expect(readout.ok).toBe(true);
    expect(readout.diagnostics).toEqual([
      { code: 'L1129', severity: 'warning', message: 'two pieces', path: '/slices', help: null },
    ]);
    expect(readout.summary).toBe('2x2x1 · 3 filled · 1 component · L1129 two pieces');
  });
});

describe('parseCheckScene — an assembly', () => {
  it('reads the name, extent, components, part count and clip bindings from the sample', () => {
    const readout = parseCheckScene(ok(ASSEMBLY));
    expect(readout.ok).toBe(true);
    expect(readout.kind).toBe('assembly');
    expect(readout.path).toBe('tests/fixtures/assembly/good.assembly.yaml');
    // size [4, 3, 2]; parts `body` and `arm`; one clip, `walk`; the arm floats apart from the body.
    expect(readout.size).toEqual([4, 3, 2]);
    expect(readout.parts).toBe(2);
    expect(readout.components).toBe(2);
    expect(readout.clips).toEqual([{ name: 'walk', file: 'anims/walk.anim.yaml' }]);
    expect(readout.filled).toBeNull();
    expect(readout.scale).toBeNull();
  });

  it('the stray-component warning is the status line tail', () => {
    const readout = parseCheckScene(ok(ASSEMBLY));
    expect(readout.diagnostics).toHaveLength(1);
    expect(readout.diagnostics[0].code).toBe('L1129');
    expect(readout.diagnostics[0].severity).toBe('warning');
    expect(readout.diagnostics[0].path).toBe('/slices');
    expect(readout.summary.startsWith('assembly fixture · 4x3x2 · 2 parts · 1 clip · L1129 ')).toBe(
      true,
    );
  });

  it('the summary is one line though the warning message runs to several', () => {
    const readout = parseCheckScene(ok(ASSEMBLY));
    expect(readout.diagnostics[0].message).toContain('\n');
    expect(readout.summary).not.toContain('\n');
  });

  it('a nameless assembly reads as bare `assembly`', () => {
    const object = { ...JSON.parse(ASSEMBLY), name: null, diagnostics: [] };
    expect(parseCheckScene(ok(JSON.stringify(object))).summary).toBe(
      'assembly · 4x3x2 · 2 parts · 1 clip',
    );
  });
});

describe('parseCheckScene — failures', () => {
  const FAILED = JSON.stringify({
    path: 'broken.voxel.yaml',
    kind: 'voxel',
    ok: false,
    diagnostics: [
      { code: 'L1114', message: 'slices has 1 layers', path: '/slices', help: 'add one' },
      { code: 'L1114', message: 'glyph Z names no palette entry', path: '/slices/0/0' },
    ],
  });

  it('an error omits severity and still reads as an error', () => {
    const readout = parseCheckScene({ stdout: `${FAILED}\n`, stderr: 'noise', exitCode: 1 });
    expect(readout.ok).toBe(false);
    expect(readout.kind).toBeNull();
    expect(readout.size).toBeNull();
    expect(readout.path).toBe('broken.voxel.yaml');
    expect(readout.diagnostics.map((d) => d.severity)).toEqual(['error', 'error']);
    expect(readout.diagnostics[0].help).toBe('add one');
    expect(readout.diagnostics[1].help).toBeNull();
    expect(readout.summary).toBe('L1114 slices has 1 layers');
  });

  it('flattens a related block into the list', () => {
    const object = JSON.stringify({
      ok: false,
      diagnostics: [
        {
          code: 'L1114',
          message: 'outer',
          related: [{ code: 'L1115', severity: 'warning', message: 'inner' }],
        },
      ],
    });
    const readout = parseCheckScene({ stdout: object, stderr: '', exitCode: 1 });
    expect(readout.diagnostics.map((d) => d.code)).toEqual(['L1114', 'L1115']);
  });

  it('no JSON at all (a usage error) falls back to the stderr tail', () => {
    const readout = parseCheckScene({
      stdout: '',
      stderr: 'error: --json prints the summary object\n',
      exitCode: 1,
    });
    expect(readout.ok).toBe(false);
    expect(readout.diagnostics).toEqual([]);
    expect(readout.summary).toBe('error: --json prints the summary object');
  });

  it('prose on stdout is not an object', () => {
    const readout = parseCheckScene({ stdout: 'x: OK (1x1x1)\n', stderr: '', exitCode: 1 });
    expect(readout.ok).toBe(false);
    expect(readout.summary).toBe('check-scene exited 1');
  });

  it('a killed process says so', () => {
    const readout = parseCheckScene({ stdout: '', stderr: '', exitCode: null });
    expect(readout.ok).toBe(false);
    expect(readout.summary).toBe('check-scene was killed');
  });

  it('ok:true with a nonzero exit is a failure', () => {
    const readout = parseCheckScene({ stdout: VOXEL, stderr: '', exitCode: 1 });
    expect(readout.ok).toBe(false);
    expect(readout.kind).toBeNull();
    expect(readout.summary).toBe('check-scene exited 1');
  });
});

describe('parseCheckScene — other output', () => {
  it('reads CRLF output', () => {
    const readout = parseCheckScene(ok(`${VOXEL.trim()}\r\n`));
    expect(readout.ok).toBe(true);
    expect(readout.kind).toBe('voxel');
  });

  it('a kind that is neither voxel nor assembly validates without inventing measurements', () => {
    const object = {
      path: 'a.sprite.yaml',
      kind: 'sprite',
      ok: true,
      diagnostics: [{ code: 'L1115', severity: 'warning', message: 'big' }],
    };
    const readout = parseCheckScene(ok(JSON.stringify(object)));
    expect(readout.ok).toBe(true);
    expect(readout.kind).toBe('other');
    expect(readout.size).toBeNull();
    expect(readout.summary).toBe('sprite · L1115 big');
  });

  it('takes the last non-empty line as the object', () => {
    const readout = parseCheckScene(ok(`progress noise\n${VOXEL}\n\n`));
    expect(readout.kind).toBe('voxel');
  });
});
