import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import type { DirEntry } from '../../shell/host';
import {
  acceptModel,
  candidateLabel,
  checkModel,
  defaultDestination,
  envelope,
  runFolders,
  runModel,
  statusModel,
  timeoutArgs,
  verbMessages,
} from './model';
import { controlFor, quantize } from './controls';
import { recipeName, recipeProjectPath, runProjectPath } from './paths';

function dir(relPath: string): DirEntry {
  return { relPath, name: relPath.slice(relPath.lastIndexOf('/') + 1), isDir: true };
}

function file(relPath: string): DirEntry {
  return { relPath, name: relPath.slice(relPath.lastIndexOf('/') + 1), isDir: false };
}

// The samples are `ludion gen … --json` stdout, committed beside this view and checked against the
// CLI by its tests. Expected values below are read off their text, not off the readers.
const SAMPLE_DIR = fileURLToPath(new URL('./__fixtures__/', import.meta.url));

function sample(name: string): unknown {
  return JSON.parse(readFileSync(`${SAMPLE_DIR}${name}.json`, 'utf8'));
}

const SAMPLES = [
  'gen-accept',
  'gen-accept-failed',
  'gen-check',
  'gen-check-failed',
  'gen-nodes',
  'gen-run',
  'gen-run-partial',
  'gen-run-refused',
  'gen-run-timed-out',
  'gen-sheet',
  'gen-status',
];

const FAILING = [
  'gen-accept-failed',
  'gen-check-failed',
  'gen-run-partial',
  'gen-run-refused',
  'gen-run-timed-out',
];

function result(json: unknown, ok: boolean, stderr = '', exitCode = ok ? 0 : 1) {
  return { ok, json, stderr, exitCode };
}

describe('the gen --json samples', () => {
  it('are exactly the ones this file has a reader test for', () => {
    const found = readdirSync(SAMPLE_DIR)
      .filter((file) => /^gen-.*\.json$/.test(file))
      .map((file) => file.replace(/\.json$/, ''))
      .sort();
    expect(found).toEqual(SAMPLES);
  });

  it('each carry an envelope whose ok says whether the verb succeeded', () => {
    for (const name of SAMPLES) {
      const env = envelope(sample(name));
      expect(env, name).not.toBeNull();
      expect(env?.ok, name).toBe(!FAILING.includes(name));
    }
  });

  it('is no envelope without a boolean ok', () => {
    for (const value of [null, 7, 'ok', [], {}, { ok: 'yes' }, { diagnostics: [] }]) {
      expect(envelope(value)).toBeNull();
    }
  });
});

describe('gen run --json, off the samples', () => {
  it('reads a clean run: two candidates, nothing failed, no messages', () => {
    const json = sample('gen-run');
    const model = runModel(json);
    expect(model?.status).toBe('ok');
    expect(model?.candidates).toEqual([
      { index: 0, seed: 7, file: '0000.png' },
      { index: 1, seed: 8, file: '0001.png' },
    ]);
    expect(model?.failed).toEqual([]);
    expect(verbMessages(result(json, true))).toBeNull();
  });

  it('reads a partial run: the finished candidate, the failed job and the verb’s L1169', () => {
    const json = sample('gen-run-partial');
    const model = runModel(json);
    expect(model?.status).toBe('partial');
    expect(model?.dir).toBe('generation/runs/idol/20261008T120000Z-1a2b3c4d');
    expect(model?.candidates).toHaveLength(1);
    expect(model?.failed).toEqual([
      {
        job: 1,
        promptId: 'job-1',
        reason: 'error',
        node: '7',
        class: 'SaveImage',
        message: 'CUDA out of memory',
      },
    ]);
    const messages = verbMessages(result(json, false));
    expect(messages?.stderr).toBeNull();
    expect(messages?.diagnostics.map((d) => d.code)).toEqual(['L1169']);
    expect(messages?.diagnostics[0].message).toContain('CUDA out of memory');
  });

  it('keeps a timed-out run selectable: submitted, its folder, and L1170', () => {
    const json = sample('gen-run-timed-out');
    const model = runModel(json);
    expect(model?.status).toBe('submitted');
    expect(model?.dir).toBe('generation/runs/idol/20261008T120000Z-1a2b3c4d');
    expect(model?.candidates).toEqual([]);
    expect(model?.count).toBe(2);
    const messages = verbMessages(result(json, false));
    expect(messages?.diagnostics.map((d) => d.code)).toEqual(['L1170']);
  });

  it('reads a refusal as no run, with its L1167 as the message', () => {
    const json = sample('gen-run-refused');
    expect(runModel(json)).toBeNull();
    const messages = verbMessages(result(json, false));
    expect(messages?.stderr).toBeNull();
    expect(messages?.diagnostics.map((d) => d.code)).toEqual(['L1167']);
    expect(messages?.diagnostics[0].message).toBe(
      'cannot reach a ComfyUI server at `http://127.0.0.1:9`',
    );
  });
});

describe('gen status --json, off the sample', () => {
  it('reads the run record, the verdict, a queued job and a failed one', () => {
    const model = statusModel(sample('gen-status'));
    expect(model?.dir).toBe('generation/runs/idol/20261008T120000Z-1a2b3c4d');
    expect(model?.ready).toBe(false);
    expect(model?.pending).toEqual(['job-1']);
    expect(model?.failed).toEqual([
      {
        job: 0,
        promptId: 'job-0',
        reason: 'error',
        node: '7',
        class: 'SaveImage',
        message: 'CUDA out of memory',
      },
    ]);
    expect(model?.run?.status).toBe('submitted');
    expect(model?.run?.runId).toBe('20261008T120000Z-1a2b3c4d');
  });

  it('is null without a boolean ready', () => {
    for (const value of [null, 3, {}, { ready: 'yes' }, { ok: true }]) {
      expect(statusModel(value)).toBeNull();
    }
  });

  it('reads pending ids and failed jobs, and a null report as no run', () => {
    const model = statusModel({
      ok: true,
      dir: 'generation/runs/idol/x',
      report: null,
      ready: false,
      pending: ['job-0', 7, 'job-2'],
      failed: [
        { job: 0, prompt_id: 'job-0', reason: 'lost', node: null, class: null, message: 'gone' },
      ],
    });
    expect(model?.run).toBeNull();
    expect(model?.pending).toEqual(['job-0', 'job-2']);
    expect(model?.failed[0].reason).toBe('lost');
  });
});

describe('gen check and gen accept --json, off the samples', () => {
  it('gen-check: ok, with the downgraded L1167 as a warning', () => {
    const json = sample('gen-check');
    const model = checkModel(json);
    expect(model?.offline).toBe('ok');
    expect(model?.live).toBe('skipped');
    expect(model?.diagnostics.map((d) => [d.code, d.severity])).toEqual([['L1167', 'warning']]);
    // A success that carries a warning still has something to show.
    expect(verbMessages(result(json, true))?.diagnostics.map((d) => d.code)).toEqual(['L1167']);
  });

  it('gen-check-failed: a model exists, and the verb’s own L1162 is the message', () => {
    const json = sample('gen-check-failed');
    const model = checkModel(json);
    expect(model).not.toBeNull();
    expect(model?.offline).toBe('error');
    expect(verbMessages(result(json, false))?.diagnostics.map((d) => d.code)).toEqual(['L1162']);
  });

  it('gen-accept: what was written', () => {
    const model = acceptModel(sample('gen-accept'));
    expect(model?.source).toBe('generation/runs/idol/20261008T120000Z-1a2b3c4d/0000.png');
    expect(model?.destination).toBe('art/idol.png');
    expect(model?.sidecar).toBe('art/idol.png.origin.yaml');
  });

  it('gen-accept-failed: no model, L1172', () => {
    const json = sample('gen-accept-failed');
    expect(acceptModel(json)).toBeNull();
    expect(verbMessages(result(json, false))?.diagnostics.map((d) => d.code)).toEqual(['L1172']);
  });
});

describe('what the samples do not cover', () => {
  it('keeps an unknown seed null and labels it ?, not 0', () => {
    const model = runModel({
      run_id: 'r',
      outputs: [
        { index: 3, seed: null, file: '0003.png' },
        { index: 4, seed: 304, file: '0004.png' },
      ],
    });
    expect(model?.candidates[0].seed).toBeNull();
    expect(model?.candidates.map(candidateLabel)).toEqual(['#3 seed ?', '#4 seed 304']);
  });

  it('reads an older run.json with no failed key as no failed jobs', () => {
    expect(runModel({ run_id: 'r', outputs: [] })?.failed).toEqual([]);
  });

  it('skips a malformed failed entry and reads any other reason as an error', () => {
    const model = runModel({
      run_id: 'r',
      failed: [
        { job: 2, prompt_id: 'p', reason: 'weird', message: 'm' },
        { prompt_id: 'no job' },
        'nope',
      ],
    });
    expect(model?.failed).toEqual([
      { job: 2, promptId: 'p', reason: 'error', node: null, class: null, message: 'm' },
    ]);
  });

  it('falls back to stderr when nothing parseable was printed', () => {
    expect(verbMessages(result(null, false, 'error: unexpected argument', 2))).toEqual({
      diagnostics: [],
      stderr: 'error: unexpected argument',
    });
    expect(verbMessages(result(null, false, '  \n', 2))).toEqual({
      diagnostics: [],
      stderr: 'the verb exited 2 and printed no object',
    });
  });

  it('shows stderr when a failing envelope carries no diagnostics, and nothing on success', () => {
    expect(verbMessages(result({ ok: false, diagnostics: [] }, false, 'boom'))).toEqual({
      diagnostics: [],
      stderr: 'boom',
    });
    expect(verbMessages(result({ ok: false, diagnostics: [] }, false, '', 3))).toEqual({
      diagnostics: [],
      stderr: 'the verb exited 3 and said nothing',
    });
    expect(verbMessages(result({ ok: true, diagnostics: [] }, true))).toBeNull();
  });
});

describe('the timeout field', () => {
  it('is no flag when empty, --timeout for a positive integer, and invalid otherwise', () => {
    expect(timeoutArgs('')).toEqual([]);
    expect(timeoutArgs('   ')).toEqual([]);
    expect(timeoutArgs(' 900 ')).toEqual(['--timeout', '900']);
    expect(timeoutArgs('604800')).toEqual(['--timeout', '604800']);
    for (const tooLong of ['604801', '99999999999999999999']) {
      expect(timeoutArgs(tooLong), tooLong).toBeNull();
    }
    for (const bad of ['0', '-5', '1.5', 'abc', '1e3']) {
      expect(timeoutArgs(bad), bad).toBeNull();
    }
  });
});

describe('gen check --json', () => {
  it('reads the verb’s own two verdicts and flattens its diagnostics', () => {
    const model = checkModel({
      recipe: 'generation/recipes/idol-portrait.gen.yaml',
      workflow: 'generation/workflows/txt2img.json',
      kind: 'image',
      overrides: {},
      offline: 'ok',
      live: 'skipped',
      diagnostics: [
        {
          code: 'L1167',
          severity: 'warning',
          message: 'nothing answered at http://127.0.0.1:8188',
          help: 'set LUDION_COMFY_URL',
          related: [{ code: 'L1164', message: 'no such class', path: '/inputs/Sampler' }],
        },
      ],
    });
    expect(model).not.toBeNull();
    expect(model?.kind).toBe('image');
    expect(model?.offline).toBe('ok');
    expect(model?.live).toBe('skipped');
    expect(model?.diagnostics).toEqual([
      {
        code: 'L1167',
        severity: 'warning',
        message: 'nothing answered at http://127.0.0.1:8188',
        path: null,
        help: 'set LUDION_COMFY_URL',
      },
      // A `related` block is context for the one above it, so it stays in the list.
      {
        code: 'L1164',
        severity: 'error',
        message: 'no such class',
        path: '/inputs/Sampler',
        help: null,
      },
    ]);
  });

  it('defaults an omitted severity to error, which is how the verb spells one', () => {
    const model = checkModel({
      recipe: 'r.gen.yaml',
      workflow: null,
      kind: null,
      offline: 'error',
      live: 'skipped',
      diagnostics: [{ code: 'L1161', message: 'unknown key `prompt`', path: '/prompt' }],
    });
    expect(model?.diagnostics[0].severity).toBe('error');
    expect(model?.kind).toBeNull();
    expect(model?.workflow).toBeNull();
  });

  it('is null for anything that is not a check object', () => {
    for (const value of [null, undefined, 42, 'ok', [], {}, { offline: 'ok' }]) {
      expect(checkModel(value)).toBeNull();
    }
  });
});

describe('a run record', () => {
  const RUN = {
    run_id: '20260918T113000Z-1a2b3c4d',
    recipe: 'generation/recipes/idol-portrait.gen.yaml',
    recipe_hash: 'aa',
    workflow: 'generation/workflows/txt2img.json',
    workflow_hash: 'bb',
    graph_hash: 'cc',
    seed_base: 300,
    count: 3,
    kind: 'image',
    comfy_version: '0.3.x',
    checkpoints: ['flux-2-klein-4b.safetensors'],
    prompt_ids: ['a', 'b', 'c'],
    seeds: [300, 301, 302],
    outputs: [
      { index: 2, seed: 302, file: '0002.png', node: 'Save', batch_index: 0, prompt_id: 'c' },
      { index: 0, seed: 300, file: '0000.png', node: 'Save', batch_index: 0, prompt_id: 'a' },
      { index: 1, seed: 301, file: '0001.png', node: 'Save', batch_index: 0, prompt_id: 'b' },
    ],
    failed: [],
    started: '2026-09-18T11:30:00Z',
    finished: '2026-09-18T11:34:00Z',
    duration_ms: 240000,
    status: 'ok',
    dir: 'generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d',
  };

  it('reads gen run --json, with the candidates in index order', () => {
    const model = runModel(RUN);
    expect(model?.runId).toBe('20260918T113000Z-1a2b3c4d');
    expect(model?.dir).toBe('generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d');
    expect(model?.kind).toBe('image');
    expect(model?.seedBase).toBe(300);
    expect(model?.status).toBe('ok');
    expect(model?.durationMs).toBe(240000);
    // The label IS the index, so the cells must be laid out in index order whatever order the
    // downloads finished in.
    expect(model?.candidates).toEqual([
      { index: 0, seed: 300, file: '0000.png' },
      { index: 1, seed: 301, file: '0001.png' },
      { index: 2, seed: 302, file: '0002.png' },
    ]);
  });

  it('reads a run.json off disk, which carries no dir of its own', () => {
    const { dir: _dropped, ...onDisk } = RUN;
    const model = runModel(onDisk);
    expect(model?.dir).toBeNull();
    expect(model?.candidates).toHaveLength(3);
  });

  it('reads a submitted run that has downloaded nothing yet', () => {
    const model = runModel({ ...RUN, status: 'submitted', outputs: [], duration_ms: 0 });
    expect(model?.status).toBe('submitted');
    expect(model?.candidates).toEqual([]);
    // `count` still says how many jobs went up, so the grid can say "0 of 3".
    expect(model?.count).toBe(3);
  });

  it('is null without a run id', () => {
    for (const value of [null, 7, [], { outputs: [] }]) {
      expect(runModel(value)).toBeNull();
    }
  });
});

describe('gen accept --json', () => {
  it('reads the four keys an image accept prints', () => {
    const model = acceptModel({
      source: 'generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d/0003.png',
      destination: 'games/vigil/scenes/assets/idol-reference.png',
      sidecar: 'games/vigil/scenes/assets/idol-reference.png.origin.yaml',
      origin: { tool: 'comfyui', seed: 303 },
    });
    expect(model?.destination).toBe('games/vigil/scenes/assets/idol-reference.png');
    expect(model?.sidecar).toBe('games/vigil/scenes/assets/idol-reference.png.origin.yaml');
    // An image accept has no meshes and no textures; those read empty, never absent.
    expect(model?.meshes).toEqual([]);
    expect(model?.textures).toEqual([]);
  });

  it('reads the two extra keys a mesh accept prints, and its null sidecar', () => {
    const model = acceptModel({
      source: 'generation/runs/idol-mesh/20260918T120000Z-99887766/0000.glb',
      destination: 'scenes/meshes/idol',
      sidecar: null,
      origin: { tool: 'comfyui' },
      meshes: [{ path: 'scenes/meshes/idol/meshes/idol.mesh.yaml', vertices: 4190, triangles: 4986 }],
      textures: [
        'scenes/meshes/idol/textures/abc.png',
        'scenes/meshes/idol/textures/def.png',
        'scenes/meshes/idol/textures/012.png',
      ],
    });
    // A mesh carries its provenance inside each document, so there is no sidecar to name.
    expect(model?.sidecar).toBeNull();
    expect(model?.meshes).toEqual([
      { path: 'scenes/meshes/idol/meshes/idol.mesh.yaml', vertices: 4190, triangles: 4986 },
    ]);
    expect(model?.textures).toHaveLength(3);
  });

  it('is null without a destination — nothing was written, so there is nothing to report', () => {
    for (const value of [null, 'wrote it', { source: 'a' }]) {
      expect(acceptModel(value)).toBeNull();
    }
  });
});

describe('the run list', () => {
  it('is the top-level folders, newest first', () => {
    // A run id is a compact UTC timestamp + 8 hex of the graph hash, so descending lexicographic
    // order is newest-first: no dates to parse and no mtimes to trust.
    const listed = [
      dir('20260917T090000Z-aaaaaaaa'),
      dir('20260918T113000Z-1a2b3c4d'),
      dir('20260918T113000Z-1a2b3c4d/inputs'),
      file('20260918T113000Z-1a2b3c4d/run.json'),
      dir('20260918T090000Z-bbbbbbbb'),
    ];
    expect(runFolders(listed)).toEqual([
      '20260918T113000Z-1a2b3c4d',
      '20260918T090000Z-bbbbbbbb',
      '20260917T090000Z-aaaaaaaa',
    ]);
    expect(runFolders([])).toEqual([]);
    // `inputs/` inside a run is not a run, and neither is a file.
    expect(runFolders([dir('a/inputs'), file('sheet.png')])).toEqual([]);
  });

  it('orders a collision suffix numerically, so -10 is newer than -9', () => {
    const base = '20260918T113000Z-1a2b3c4d';
    const listed = [
      dir(`${base}-9`),
      dir(base),
      dir(`${base}-10`),
      dir(`${base}-2`),
      dir('20260918T120000Z-0000aaaa'),
    ];
    expect(runFolders(listed)).toEqual([
      '20260918T120000Z-0000aaaa',
      `${base}-10`,
      `${base}-9`,
      `${base}-2`,
      base,
    ]);
  });
});

describe('the accept destination the dialog proposes', () => {
  it('is a file for an image and a directory for a mesh', () => {
    expect(defaultDestination('idol-portrait', 3, 'image')).toBe(
      'scenes/assets/idol-portrait-3.png',
    );
    // A mesh accept's `--out` is a DIRECTORY, which must be absent or empty (`L1172`).
    expect(defaultDestination('idol-mesh', 0, 'mesh')).toBe('scenes/meshes/idol-mesh/');
    // An unknown kind is treated as an image: that is what every other verb defaults to, and the
    // verb itself refuses a mismatch rather than guessing.
    expect(defaultDestination('idol-portrait', 0, null)).toBe('scenes/assets/idol-portrait-0.png');
  });
});

describe('paths', () => {
  it('spell a recipe project-relative for the CLI and strip the double extension for a name', () => {
    expect(recipeProjectPath('idol-portrait.gen.yaml')).toBe(
      'generation/recipes/idol-portrait.gen.yaml',
    );
    expect(recipeProjectPath('idols/portrait.gen.yaml')).toBe(
      'generation/recipes/idols/portrait.gen.yaml',
    );
    expect(recipeName('idols/portrait.gen.yaml')).toBe('portrait');
    expect(recipeName('idol-portrait.gen.yaml')).toBe('idol-portrait');
    expect(runProjectPath('idol-portrait', '20260918T113000Z-1a2b3c4d')).toBe(
      'generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d',
    );
  });
});

describe('the controls a recipe number gets', () => {
  const field = (key: string, value: number, integer = true) => ({
    pointer: `/inputs/N/${key}`,
    key,
    value,
    offset: 0,
    length: 1,
    integer,
  });

  it('gives a seed a box, not a slider — a seed is a pick, not a tunable', () => {
    for (const key of ['seed', 'noise_seed']) {
      expect(controlFor(field(key, 300))).toEqual({ kind: 'number', step: 1, min: 0 });
    }
  });

  it('gives a known knob a slider and an unknown one a box', () => {
    expect(controlFor(field('width', 1024))).toEqual({
      kind: 'slider',
      range: { min: 256, max: 2048, step: 64 },
    });
    expect(controlFor(field('count', 8))).toEqual({
      kind: 'slider',
      range: { min: 1, max: 16, step: 1 },
    });
    // An input name comes from whatever node class the workflow uses, so the set is open by
    // construction: a box is honest about there being no sensible travel to offer.
    expect(controlFor(field('mystery_knob', 3))).toEqual({ kind: 'number', step: 1, min: null });
    expect(controlFor(field('mystery_ratio', 0.5, false))).toEqual({
      kind: 'number',
      step: 0.01,
      min: null,
    });
  });

  it('quantizes to the step grid and clamps into the range', () => {
    const cfg = { min: 0, max: 20, step: 0.1 };
    // The value written must be as short as the step is: no 7.000000000000001 in the file.
    expect(quantize(7.03, cfg)).toBe(7);
    expect(quantize(7.06, cfg)).toBe(7.1);
    expect(quantize(-5, cfg)).toBe(0);
    expect(quantize(99, cfg)).toBe(20);
    expect(quantize(Number.NaN, cfg)).toBe(0);
    const pixels = { min: 256, max: 2048, step: 64 };
    expect(quantize(1000, pixels)).toBe(1024);
    expect(quantize(1024, pixels)).toBe(1024);
  });
});
