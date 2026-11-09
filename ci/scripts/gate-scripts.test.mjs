// Tests for the gate scripts in ci/scripts/ and the check.mjs run lock. Run from the repository root:
//   node --test ci/scripts/gate-scripts.test.mjs

import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { PROBES, missingNeedles } from './denylist-canary.mjs';
import { acquireLock, pruneScopeTemps } from '../../scripts/check-lock.mjs';
import { killTree, quoteForCmd, spawnTool } from '../../scripts/kill-tree.mjs';
import { headGeneratedFrom } from '../../scripts/drift-lib.mjs';
import { runGate as atlasGate } from '../../scripts/atlas-drift.mjs';
import { runGate as meshGate } from '../../scripts/mesh-edit-drift.mjs';
import { runGate as mapGate } from '../../scripts/map-drift.mjs';
import { runGate as voxelGate } from '../../scripts/voxel-drift.mjs';
import { findScriptWrites, findWrites, runGate as atomicGate } from './atomic-writes.mjs';
import { writeFileAtomic } from '../../scripts/atomic-write.mjs';
import { findPrefixedPrints, findUncodedFailurePrints, scanFiles } from './diagnostic-prints.mjs';
import { findScheduleText, scanFiles as scheduleFiles, unallowedHits, ALLOWLIST as SCHEDULE_ALLOWLIST } from './schedule-comments.mjs';
import { BEGIN as CG_BEGIN, END as CG_END, checkCrateGraph, renderEdges, spliceEdges } from '../../scripts/crate-graph-edges.mjs';
import { checkDocCitations } from '../../scripts/doc-citations.mjs';
import { render as renderDenylist } from '../../scripts/clippy-denylist.mjs';
import { bareVersions, checkManifests, nonFacadeDeps } from '../../scripts/manifest-rules.mjs';
import { countCodeLines } from '../../scripts/ecs-budget.mjs';
import { measure as measureTargets, plan as planTargetClean } from './target-guard.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const fingerprint = (rel) => {
  const abs = path.join(ROOT, rel);
  return { bytes: readFileSync(abs), mtimeMs: statSync(abs).mtimeMs };
};
const gate = (script, args = []) =>
  spawnSync('node', [path.join('ci', 'scripts', script), ...args], { cwd: ROOT, encoding: 'utf8' });

test('every probe names its banned path and its reason, and a missing one is reported', () => {
  for (const probe of PROBES) {
    const full = probe.needles.join('\n');
    assert.deepEqual(missingNeedles(probe, full), []);
    for (const needle of probe.needles) {
      assert.deepEqual(missingNeedles(probe, full.replace(needle, '')), [needle]);
    }
  }
});

test('the sim tier proves the thread and glam bans and the strict tier the type and libm bans', () => {
  const needles = PROBES.flatMap((p) => p.needles).join('\n');
  for (const want of ['std::collections::HashMap', 'f32::sin', 'std::thread::spawn', 'sim parallelism only via ludion_core::par', 'glam::Quat::slerp', 'glam::Vec4::dot']) {
    assert.ok(needles.includes(want), want);
  }
  assert.deepEqual(PROBES.map((p) => p.crate), ['crates/ludion-core', 'crates/ludion-ecs', 'crates/ludion-ecs']);
});

test('the denylist canary writes no tracked file', () => {
  const files = [
    'crates/ludion-core/src/lib.rs', 'crates/ludion-ecs/src/lib.rs',
    'crates/ludion-core/clippy.toml', 'crates/ludion-ecs/clippy.toml',
  ];
  const before = files.map(fingerprint);
  const r = gate('denylist-canary.mjs');
  assert.equal(r.status, 0, r.stdout + r.stderr);
  assert.match(r.stdout, /denylist ok/);
  files.forEach((f, i) => {
    const after = fingerprint(f);
    assert.ok(before[i].bytes.equals(after.bytes), `${f} bytes changed`);
    assert.equal(before[i].mtimeMs, after.mtimeMs, `${f} mtime changed`);
  });
});

test('the run lock refuses a live holder, frees on release and takes over a dead one', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-check-lock-'));
  try {
    const lockPath = path.join(dir, 'check.lock');
    const first = acquireLock(lockPath, { mode: 'default' });
    assert.ok(first.ok);
    const second = acquireLock(lockPath, { mode: 'full' });
    assert.equal(second.ok, false);
    assert.equal(second.holder.pid, process.pid);
    first.release();
    assert.equal(existsSync(lockPath), false);
    const third = acquireLock(lockPath, { mode: 'default' });
    assert.ok(third.ok);
    third.release();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the run lock takes over a lock held by a dead pid', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-check-lock-'));
  try {
    const lockPath = path.join(dir, 'check.lock');
    const dead = spawnSync('node', ['-e', '']);
    writeFileSync(lockPath, JSON.stringify({ pid: dead.pid, mode: 'default', startedAt: 'long ago' }));
    const taken = acquireLock(lockPath, { mode: 'default' });
    assert.ok(taken.ok);
    assert.equal(JSON.parse(readFileSync(lockPath, 'utf8')).pid, process.pid);
    taken.release();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the run lock does not take over an empty lock a holder is still writing', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-check-lock-'));
  try {
    const lockPath = path.join(dir, 'check.lock');
    writeFileSync(lockPath, '');
    const second = acquireLock(lockPath, { mode: 'default' });
    assert.equal(second.ok, false);
    assert.equal(existsSync(lockPath), true);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the run lock takes over an unreadable lock once it is older than the grace period', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-check-lock-'));
  try {
    const lockPath = path.join(dir, 'check.lock');
    writeFileSync(lockPath, '');
    const old = new Date(Date.now() - 120_000);
    utimesSync(lockPath, old, old);
    const taken = acquireLock(lockPath, { mode: 'default' });
    assert.ok(taken.ok);
    taken.release();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('the run lock leaves no temp or stale files behind', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-check-lock-'));
  try {
    const lockPath = path.join(dir, 'check.lock');
    const lock = acquireLock(lockPath, { mode: 'default' });
    assert.ok(lock.ok);
    assert.deepEqual(readdirSync(dir), ['check.lock']);
    lock.release();
    assert.deepEqual(readdirSync(dir), []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('killTree reaps a grandchild that ignores the step pipes', async () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-kill-tree-'));
  try {
    const pidFile = path.join(dir, 'grandchild.pid');
    const grandchildJs = path.join(dir, 'grandchild.js');
    const parentJs = path.join(dir, 'parent.js');
    writeFileSync(grandchildJs, `require('fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); setInterval(() => {}, 1000);`);
    writeFileSync(parentJs, `require('child_process').spawn(process.execPath, [${JSON.stringify(grandchildJs)}], { stdio: 'ignore' }); setInterval(() => {}, 1000);`);
    const child = spawnTool('node', [parentJs], { stdio: 'ignore' });
    const closed = new Promise((resolve) => child.on('close', resolve));
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
    for (let i = 0; i < 100 && !existsSync(pidFile); i++) await sleep(100);
    assert.ok(existsSync(pidFile), 'the grandchild never started');
    const grandchildPid = Number(readFileSync(pidFile, 'utf8'));
    killTree(child);
    await closed;
    const alive = () => { try { process.kill(grandchildPid, 0); return true; } catch { return false; } };
    for (let i = 0; i < 50 && alive(); i++) await sleep(100);
    assert.equal(alive(), false, 'the grandchild survived killTree');
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// --- source-driven drift gates: scratch trees of header-only files and a stub CLI runner ---

const scratchTree = (files) => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-drift-gate-'));
  for (const [rel, text] of Object.entries(files)) {
    const abs = path.join(dir, rel);
    mkdirSync(path.dirname(abs), { recursive: true });
    writeFileSync(abs, text);
  }
  return dir;
};
const stubLudion = (calls, reply) => (args) => {
  calls.push(args);
  return reply(args);
};
const ok = (stdout) => ({ status: 0, stdout, stderr: '' });
const fail = (stderr) => ({ status: 1, stdout: '', stderr });

// Mimics `generate-atlas --check`: succeeds only when the default document exists.
const atlasStub = (calls) =>
  stubLudion(calls, (args) => {
    const src = args[2];
    const doc = src.replace('-source.yaml', '.yaml');
    if (!existsSync(doc)) return fail(`${doc}: missing`);
    return ok(`${src}: up to date -> ${doc}, ${doc.replace('.yaml', '.png')}`);
  });

test('atlas-drift checks a source that has no output and reports the failure', () => {
  const root = scratchTree({ 'games/g/inter.font-source.yaml': 'kind: font-source\n' });
  try {
    const calls = [];
    const { errors, checked } = atlasGate({ root, ludion: atlasStub(calls) });
    assert.equal(checked, 1);
    assert.equal(calls.length, 1);
    assert.ok(errors.some((e) => e.includes('games/g/inter.font-source.yaml') && e.includes('failed')), errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('atlas-drift rejects a baked copy at a non-default path and a doc that lost its provenance', () => {
  const root = scratchTree({
    'games/g/inter.font-source.yaml': 'kind: font-source\n',
    'games/g/inter.font.yaml': 'format: 1\ngenerated_from: inter.font-source.yaml\n',
    'games/g/old/inter.font.yaml': 'format: 1\ngenerated_from: ../inter.font-source.yaml\n',
  });
  try {
    const errors = atlasGate({ root, ludion: atlasStub([]) }).errors;
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/g\/old\/inter\.font\.yaml names games\/g\/inter\.font-source\.yaml.*moved or orphaned/);
    writeFileSync(path.join(root, 'games/g/inter.font.yaml'), 'format: 1\nname: inter\n');
    const lost = atlasGate({ root, ludion: atlasStub([]) }).errors;
    assert.ok(lost.some((e) => e.includes('games/g/inter.font.yaml is the output of') && e.includes('does not name it')), lost.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('atlas-drift passes a clean pair', () => {
  const root = scratchTree({
    'games/g/ui.atlas-source.yaml': 'kind: atlas-source\n',
    'games/g/ui.atlas.yaml': 'format: 1\ngenerated_from: ui.atlas-source.yaml\n',
  });
  try {
    assert.deepEqual(atlasGate({ root, ludion: atlasStub([]) }), { errors: [], checked: 1 });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('mesh-edit-drift rejects an orphan tree and reports an edit with no tree', () => {
  const root = scratchTree({
    'games/m/a.mesh-edit.yaml': 'kind: mesh-edit\n',
    'games/m/one/mesh.mesh.yaml': 'format: 1\ngenerated_from: ../a.mesh-edit.yaml\n',
    'games/m/two/mesh.mesh.yaml': 'format: 1\ngenerated_from: ../a.mesh-edit.yaml\n',
    'games/m/b.mesh-edit.yaml': 'kind: mesh-edit\n',
  });
  try {
    const calls = [];
    const ludion = stubLudion(calls, (args) =>
      args[2].endsWith('b.mesh-edit.yaml')
        ? { status: 1, stdout: JSON.stringify({ out_dir: 'x', drift: ['mesh.mesh.yaml: missing'] }), stderr: '' }
        : ok(JSON.stringify({ out_dir: path.join(root, 'games/m/two').replaceAll('\\', '/'), drift: [] })),
    );
    const { errors, checked } = meshGate({ root, ludion });
    assert.equal(checked, 2);
    assert.ok(calls.every((c) => c.includes('--check') && c.includes('--json')));
    assert.ok(errors.some((e) => e.includes('games/m/one/mesh.mesh.yaml names games/m/a.mesh-edit.yaml') && e.includes('orphaned')), errors.join('\n'));
    assert.ok(errors.some((e) => e.includes('b.mesh-edit.yaml') && e.includes('mesh.mesh.yaml: missing')), errors.join('\n'));
    assert.equal(errors.length, 2, errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift checks a map with no scene without writing, and rejects a scene from elsewhere', () => {
  const root = scratchTree({
    'games/v/plaza.map.yaml': 'kind: map\n',
    'games/v/plaza.yaml': 'format: 1\ngenerated_from: plaza.map.yaml\n',
    'games/v/renamed.yaml': 'format: 1\ngenerated_from: plaza.map.yaml\n',
    'games/v/bare.map.yaml': 'kind: map\n',
    'games/v/odd.yaml': 'format: 1\ngenerated_from: foo.png\n',
  });
  try {
    const calls = [];
    const ludion = stubLudion(calls, (args) => {
      if (args[0] === 'map-check') return ok('seal: sealed from first marker `m` — no way out\n');
      const map = args[1];
      const scene = map.replace('.map.yaml', '.yaml');
      return existsSync(scene) ? ok(`${map}: up to date -> ${scene}`) : fail(`${scene}: missing`);
    });
    const { errors, checked } = mapGate({ root, ludion });
    assert.equal(checked, 2);
    assert.ok(calls.filter((c) => c[0] === 'generate-map').every((c) => c.includes('--check') && !c.includes('--out') && c.length === 3), JSON.stringify(calls));
    assert.ok(errors.some((e) => e.includes('games/v/bare.map.yaml') && e.includes('failed')), errors.join('\n'));
    assert.ok(errors.some((e) => e.includes('games/v/renamed.yaml names games/v/plaza.map.yaml') && e.includes('orphaned')), errors.join('\n'));
    assert.ok(errors.some((e) => e.includes('games/v/odd.yaml names foo.png') && e.includes('not a map source')), errors.join('\n'));
    assert.equal(errors.length, 3, errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift rechecks an override scene with its flags and rejects an unknown flag', () => {
  const root = scratchTree({
    'games/v/plaza.map.yaml': 'kind: map\n',
    'games/v/plaza.yaml': 'format: 1\ngenerated_from: plaza.map.yaml\n',
    'games/v/rooms/r1.yaml': 'format: 1\ngenerated_from: ../plaza.map.yaml --kit kits/b.kit.yaml --seed 3 --mirror x\n',
    'games/v/rooms/r2.yaml': 'format: 1\ngenerated_from: ../plaza.map.yaml --bogus 1\n',
  });
  try {
    const calls = [];
    const ludion = stubLudion(calls, (args) => {
      if (args[0] === 'map-check') return ok('seal: sealed from first marker `m` — no way out\n');
      return args.includes('--out') ? ok('') : ok(`${args[1]}: up to date -> ${args[1].replace('.map.yaml', '.yaml')}`);
    });
    const { errors, overrideChecked } = mapGate({ root, ludion });
    assert.equal(overrideChecked, 2);
    const call = calls.find((c) => c.includes('--out'));
    assert.deepEqual(call, [
      'generate-map',
      path.join(root, 'games/v/plaza.map.yaml'),
      '--out',
      path.join(root, 'games/v/rooms/r1.yaml'),
      '--kit',
      path.join(root, 'games/v/kits/b.kit.yaml'),
      '--seed',
      '3',
      '--mirror',
      'x',
      '--check',
    ]);
    assert.equal(calls.filter((c) => c.includes('--out')).length, 1, JSON.stringify(calls));
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/v\/rooms\/r2\.yaml.*--bogus/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift rechecks a map-generated prefab with --prefab', () => {
  const root = scratchTree({
    'games/v/plaza.map.yaml': 'kind: map\n',
    'games/v/plaza.yaml': 'format: 1\ngenerated_from: plaza.map.yaml\n',
    'games/v/rooms/a.prefab.yaml': 'format: 1\nkind: prefab\ngenerated_from: ../plaza.map.yaml\n',
    'games/v/rooms/b.prefab.yaml': 'format: 1\nkind: prefab\ngenerated_from: ../plaza.map.yaml --mirror z\n',
    'games/v/rooms/goblin.prefab.yaml': 'format: 1\nkind: prefab\ngenerated_from: goblin.gltf\n',
  });
  try {
    const calls = [];
    const ludion = stubLudion(calls, (args) => {
      if (args[0] === 'map-check') return ok('seal: sealed from first marker `m` — no way out\n');
      return args.includes('--out') ? ok('') : ok(`${args[1]}: up to date -> ${args[1].replace('.map.yaml', '.yaml')}`);
    });
    const { errors, overrideChecked } = mapGate({ root, ludion });
    assert.equal(overrideChecked, 2);
    const map = path.join(root, 'games/v/plaza.map.yaml');
    const prefabCalls = calls.filter((c) => c.includes('--out'));
    assert.deepEqual(prefabCalls, [
      ['generate-map', map, '--out', path.join(root, 'games/v/rooms/a.prefab.yaml'), '--prefab', '--check'],
      ['generate-map', map, '--out', path.join(root, 'games/v/rooms/b.prefab.yaml'), '--mirror', 'z', '--prefab', '--check'],
    ]);
    assert.equal(errors.length, 0, errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift fails a map whose walker can leave it (L1040)', () => {
  const root = scratchTree({
    'games/v/open.map.yaml': 'kind: map\n',
    'games/v/open.yaml': 'format: 1\ngenerated_from: open.map.yaml\n',
  });
  try {
    const ludion = stubLudion([], (args) =>
      args[0] === 'map-check'
        ? { status: 0, stdout: 'seal: LEAKS — warning L1040\n', stderr: 'warning[L1040]: the walker can leave the map: a walk reaches the edge\n  help: close the opening' }
        : ok(`${args[1]}: up to date -> ${args[1].replace('.map.yaml', '.yaml')}`));
    const { errors } = mapGate({ root, ludion });
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/v\/open\.map\.yaml: the walker can leave the map \(L1040\)/);
    assert.match(errors[0], /warning\[L1040\]/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift fails a map whose map-check dies, cannot spawn, or leaves the seal unchecked', () => {
  const files = {};
  for (const name of ['dies', 'nospawn', 'unchecked', 'sealed']) {
    files[`games/v/${name}.map.yaml`] = 'kind: map\n';
    files[`games/v/${name}.yaml`] = `format: 1\ngenerated_from: ${name}.map.yaml\n`;
  }
  const root = scratchTree(files);
  try {
    const ludion = stubLudion([], (args) => {
      if (args[0] !== 'map-check') return ok(`${args[1]}: up to date -> ${args[1].replace('.map.yaml', '.yaml')}`);
      if (args[1].endsWith('dies.map.yaml')) return { status: 101, stdout: '', stderr: 'panicked' };
      if (args[1].endsWith('nospawn.map.yaml')) return { status: null, error: new Error('spawn ENOENT'), stdout: '', stderr: '' };
      if (args[1].endsWith('unchecked.map.yaml')) return ok('seal: not checked — the grid is empty\n');
      return ok('seal: sealed from first marker `m` — no way out\n');
    });
    const { errors } = mapGate({ root, ludion });
    assert.equal(errors.length, 3, errors.join('\n'));
    for (const name of ['dies', 'nospawn', 'unchecked']) {
      assert.ok(errors.some((e) => e.includes(`games/v/${name}.map.yaml`)), errors.join('\n'));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift flags an unverified script scene and a silent generator', () => {
  const root = scratchTree({});
  try {
    const a = path.join(root, 'games/s/a.yaml');
    const files = {
      'games/s/gen.mjs': `// ludion: drift-generator\nconsole.log('verified ' + ${JSON.stringify(a)});\n`,
      'games/s/a.yaml': 'format: 1\ngenerated_from: gen.mjs\n',
      'games/s/b.yaml': 'format: 1\ngenerated_from: gen.mjs\n',
    };
    for (const [rel, text] of Object.entries(files)) {
      mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
      writeFileSync(path.join(root, rel), text);
    }
    const errors = mapGate({ root, ludion: () => fail('unused') }).errors;
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/s\/b\.yaml names games\/s\/gen\.mjs under generated_from but is not the output/);

    writeFileSync(path.join(root, 'games/s/gen.mjs'), '// ludion: drift-generator\n// prints nothing\n');
    const silent = mapGate({ root, ludion: () => fail('unused') }).errors;
    assert.ok(silent.some((e) => e.includes('printed no verified path')), silent.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift ignores a path with generated_from: provenance only', () => {
  const root = scratchTree({
    'games/s/lift.path.yaml': 'format: 1\nkind: path\ngenerated_from: generation/build.py\n',
  });
  try {
    const errors = mapGate({ root, ludion: () => fail('unused') }).errors;
    assert.ok(!errors.some((e) => e.includes('lift.path.yaml')), errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('map-drift runs a marked generator unasked and fails a verified scene without generated_from', () => {
  const root = scratchTree({});
  try {
    const a = path.join(root, 'games/s/a.yaml');
    for (const [rel, text] of Object.entries({
      'games/s/gen.mjs': `// ludion: drift-generator\nconsole.log('verified ' + ${JSON.stringify(a)});\n`,
      'games/s/a.yaml': 'format: 1\nname: hand-edited\n',
    })) {
      mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
      writeFileSync(path.join(root, rel), text);
    }
    const { errors, scriptChecked } = mapGate({ root, ludion: () => fail('unused') });
    assert.equal(scriptChecked, 1);
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/s\/a\.yaml is the output of games\/s\/gen\.mjs but does not name it.*restore/);
    writeFileSync(path.join(root, 'games/s/a.yaml'), 'format: 1\ngenerated_from: gen.mjs\n');
    assert.deepEqual(mapGate({ root, ludion: () => fail('unused') }).errors, []);
    writeFileSync(path.join(root, 'games/s/gen.mjs'), `console.log('verified ' + ${JSON.stringify(a)});\n`);
    const unmarked = mapGate({ root, ludion: () => fail('unused') });
    assert.ok(unmarked.errors.some((e) => e.includes('games/s/a.yaml') && e.includes('drift-generator')), unmarked.errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// A tiny real generator: prints its `kind: voxel-gen` JSON, with or without provenance.
const voxelGenerator = (named) => {
  const json = { format: 1, kind: 'voxel-gen', ...(named ? { generated_from: named } : {}) };
  return `process.stdout.write(${JSON.stringify(JSON.stringify(json))});\n`;
};
// Fails when the model file (args[3]) lacks `generated_from`; otherwise accepts it.
const voxelStub = (calls) =>
  stubLudion(calls, (args) =>
    readFileSync(args[3], 'utf8').includes('generated_from:') ? ok('') : fail(`${args[3]}: lost generated_from`));

test('voxel-drift checks a generator with no model and rejects a model from elsewhere', () => {
  const root = scratchTree({
    'games/v/a.voxel.gen.mjs': voxelGenerator('a.voxel.gen.mjs'),
    'games/v/a.voxel.yaml': 'format: 1\nsize:\n  - 1\ngenerated_from: a.voxel.gen.mjs\nslices:\n',
    'games/v/old/a.voxel.yaml': 'format: 1\ngenerated_from: ../a.voxel.gen.mjs\nslices:\n',
    'games/v/b.voxel.gen.mjs': voxelGenerator('b.voxel.gen.mjs'),
    'games/v/c.voxel.yaml': 'format: 1\ngenerated_from: gone.voxel.gen.mjs\nslices:\n',
  });
  try {
    const calls = [];
    const { errors } = voxelGate({ root, ludion: voxelStub(calls) });
    assert.ok(errors.some((e) => e.includes('games/v/b.voxel.gen.mjs has no committed model games/v/b.voxel.yaml')), errors.join('\n'));
    assert.ok(errors.some((e) => e.includes('games/v/old/a.voxel.yaml') && e.includes('orphaned copy')), errors.join('\n'));
    assert.ok(errors.some((e) => e.includes('games/v/c.voxel.yaml') && e.includes('does not exist beside it')), errors.join('\n'));
    assert.equal(errors.length, 3, errors.join('\n'));
    assert.equal(calls.length, 1, JSON.stringify(calls));
    assert.equal(calls[0][3], path.join(root, 'games/v/a.voxel.yaml'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('voxel-drift fails when every generator is a scaffold', () => {
  const root = scratchTree({
    'games/v/s.voxel.gen.mjs': voxelGenerator(null),
  });
  try {
    const calls = [];
    const { errors, checked, scaffolds } = voxelGate({ root, ludion: voxelStub(calls) });
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /every generator .* scaffold/);
    assert.equal(checked, 0);
    assert.equal(scaffolds, 1);
    assert.equal(calls.length, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('voxel-drift fails an existing model that neither side ties to a generator', () => {
  const root = scratchTree({
    'games/v/s.voxel.gen.mjs': voxelGenerator(null),
    'games/v/s.voxel.yaml': 'format: 1\nslices:\n',
  });
  try {
    const { errors, scaffolds } = voxelGate({ root, ludion: voxelStub([]) });
    assert.equal(scaffolds, 0);
    assert.ok(errors.some((e) => e.includes('games/v/s.voxel.yaml exists') && e.includes('generated_from')), errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('voxel-drift skips a scaffold and fails a generated model that lost generated_from', () => {
  const root = scratchTree({
    'games/v/s.voxel.gen.mjs': voxelGenerator(null),
    'games/v/g.voxel.gen.mjs': voxelGenerator('g.voxel.gen.mjs'),
    'games/v/g.voxel.yaml': 'format: 1\ngenerated_from: g.voxel.gen.mjs\nslices:\n',
  });
  try {
    const calls = [];
    const scaffold = voxelGate({ root, ludion: voxelStub(calls) });
    assert.deepEqual(scaffold.errors, []);
    assert.equal(scaffold.scaffolds, 1);
    assert.equal(scaffold.checked, 1);
    assert.equal(calls.length, 1);

    writeFileSync(path.join(root, 'games/v/a.voxel.gen.mjs'), voxelGenerator('a.voxel.gen.mjs'));
    writeFileSync(path.join(root, 'games/v/a.voxel.yaml'), 'format: 1\nslices:\n');
    const { errors } = voxelGate({ root, ludion: voxelStub(calls) });
    assert.equal(errors.length, 1, errors.join('\n'));
    assert.match(errors[0], /games\/v\/a\.voxel\.yaml:\n.*lost generated_from/);
    assert.equal(calls.length, 3);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('the diagnostic-prints gate also scans the editor host', () => {
  const scanned = scanFiles(ROOT).map((f) => path.relative(ROOT, f).split(path.sep).join('/'));
  assert.ok(scanned.includes('editor/app/src-tauri/src/stage.rs'), 'stage.rs is not scanned');
  assert.ok(scanned.includes('crates/ludion-cli/src/sync.rs'), 'the crates are still scanned');
});

test('every drift gate fails when it enumerates no sources', () => {
  const root = scratchTree({ 'games/g/readme.txt': 'nothing here\n' });
  try {
    const never = () => assert.fail('no source, no call');
    for (const [name, errors] of [
      ['atlas-drift', atlasGate({ root, ludion: never }).errors],
      ['mesh-edit-drift', meshGate({ root, ludion: never }).errors],
      ['map-drift', mapGate({ root, ludion: never }).errors],
      ['voxel-drift', voxelGate({ root, ludion: never }).errors],
    ]) {
      assert.equal(errors.length, 1, errors.join('\n'));
      assert.match(errors[0], new RegExp(`^${name}: found no .* under games/ or examples/`));
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('headGeneratedFrom finds a leading key past 8 KB of indented content and ignores a late one', () => {
  const root = scratchTree({
    'lead.yaml': `format: 1\ngenerated_from: '../x.mesh-edit.yaml'\npositions:\n${'  - 0.0\n'.repeat(4000)}`,
    'late.yaml': 'format: 1\nlist:\n  - a\ngenerated_from: x\n',
  });
  try {
    assert.equal(headGeneratedFrom(path.join(root, 'lead.yaml')), '../x.mesh-edit.yaml');
    assert.equal(headGeneratedFrom(path.join(root, 'late.yaml')), null);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('diagnostic-prints flags a hand-written error or warning print and nothing else', () => {
  const hits = (text) => findPrefixedPrints(text).length;
  assert.equal(hits('eprintln!("error: x");'), 1);
  assert.equal(hits('eprintln!(\n    "warning: x"\n);'), 1);
  assert.equal(hits('eprintln!("warning[{code}]: x");'), 1);
  assert.equal(hits('writeln!(err, "error: x")?;'), 1);
  assert.equal(hits('writeln!(&mut err, "error: x")?;'), 1);
  assert.equal(hits('writeln!(std::io::stderr(), "error: x")?;'), 1);
  assert.equal(hits('eprintln!(r"error: x");'), 1);
  assert.equal(hits('eprintln!(r#"warning: x"#);'), 1);
  assert.equal(findPrefixedPrints('a\nb\neprintln!("error: x");')[0].line, 3);
  assert.equal(hits('eprintln!("{diagnostic}");'), 0);
  assert.equal(hits('assert!(s.starts_with("error[L0001]"));'), 0);
  assert.equal(hits('child_failed("error[L1013]: x");'), 0);
});

test('diagnostic-prints flags a codeless failure print and nothing else', () => {
  const hits = (text) => findUncodedFailurePrints(text).length;
  assert.equal(hits('eprintln!("could not write x: {error}");'), 1);
  assert.equal(hits('eprintln!(\n    "failed to read {x}"\n);'), 1);
  assert.equal(hits('eprint!("cannot open x");'), 1);
  assert.equal(hits('eprintln!("{diagnostic}");'), 0);
  assert.equal(hits('let m = format!("could not x");'), 0);
  assert.equal(hits('println!("failed {stem}");'), 0);
});

test('diagnostic-prints passes on the tree', () => {
  const run = gate('diagnostic-prints.mjs');
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stderr, '');
});

test('atomic-writes flags a direct write outside test code and nothing else', () => {
  const hits = (text) => findWrites(text).length;
  assert.equal(hits('fn f() { std::fs::write(p, b); }'), 1);
  assert.equal(hits('fn f() { let f = File::create(p); }'), 1);
  assert.equal(hits('fn f() { fs::copy(a, b); }'), 1);
  assert.equal(hits('fn f() { File::create_new(p); }'), 1);
  assert.equal(hits('// fs::write(p, b)\nfn f() {}'), 0);
  assert.equal(hits('/// std::fs::write(p, b)\nfn f() {}'), 0);
  assert.equal(hits('fn f() { let s = "fs::write("; }'), 0);
  assert.equal(hits('fn f() { let s = r#"fs::write("#; }'), 0);
  const split = '#[cfg(test)] mod t { fn a() { fs::write(p, "{"); let c = \'{\'; } }\nfn b() { fs::write(q, x); }';
  assert.deepEqual(findWrites(split).map((h) => h.line), [2]);
  assert.equal(hits('#[cfg(all(test, feature = "wire"))] mod t { fn a() { fs::write(p, x); } }'), 0);
  assert.equal(hits('#[cfg(any(test, feature = "x"))] fn f() { fs::write(a, b) }'), 1);
  assert.equal(hits("fn f<'a>(x: &'a str) { fs::write(x, \"\") }"), 1);
  assert.equal(hits('struct S { #[cfg(test)] probe: u8, }\nfn f() { fs::write(p, b); }'), 1);
  assert.equal(hits('enum E { #[cfg(test)] Probe }\nfn f() { fs::write(p, b); }'), 1);
  assert.equal(hits('#[cfg(test)] fn f<A, B>(a: A) { fs::write(p, b); }'), 0);
});

test('atomic-writes script lane flags a direct write and nothing else', () => {
  const hits = (text) => findScriptWrites(text).length;
  assert.equal(hits("writeFileSync(p, b);"), 1);
  assert.equal(hits('fs.copyFileSync(a, b);'), 1);
  assert.equal(hits('const s = createWriteStream(p);'), 1);
  assert.equal(hits('// writeFileSync(p)\n * copyFileSync(a, b)'), 0);
  assert.equal(hits("import { writeFileSync } from 'node:fs';"), 0);
  assert.deepEqual(findScriptWrites('a\nfs.appendFileSync(p, x);').map((h) => h.line), [2]);

  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-atomic-'));
  try {
    mkdirSync(path.join(root, 'scripts'));
    writeFileSync(path.join(root, 'scripts', 'x.mjs'), 'writeFileSync(p, b);\n');
    writeFileSync(path.join(root, 'scripts', 'x.test.mjs'), 'writeFileSync(p, b);\n');
    const found = atomicGate({ root, roots: [], scriptRoots: ['scripts'], allowlist: [] });
    assert.equal(found.errors.length, 1, found.errors.join('\n'));
    assert.match(found.errors[0], /^scripts\/x\.mjs:1:/);
    assert.equal(found.scriptsScanned, 1);
    mkdirSync(path.join(root, 'games', 'g', '.ludion'), { recursive: true });
    writeFileSync(path.join(root, 'games', 'g', 'x.sprite.gen.mjs'), 'writeFileSync(p, b);\n');
    writeFileSync(path.join(root, 'games', 'g', '.ludion', 'scratch.mjs'), 'writeFileSync(p, b);\n');
    const games = atomicGate({ root, roots: [], scriptRoots: ['games'], allowlist: [] });
    assert.equal(games.errors.length, 1, games.errors.join('\n'));
    assert.match(games.errors[0], /^games\/g\/x\.sprite\.gen\.mjs:1:/);
    const empty = atomicGate({ root, roots: [], scriptRoots: ['nowhere'], allowlist: [] });
    assert.ok(empty.errors.some((e) => e.includes('the walk saw no script files under nowhere')), empty.errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('writeFileAtomic creates parents, overwrites, and leaves no temp sibling', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-atomic-'));
  try {
    const target = path.join(root, 'a', 'b', 'out.txt');
    writeFileAtomic(target, 'one\n', 'utf8');
    assert.equal(readFileSync(target, 'utf8'), 'one\n');
    writeFileAtomic(target, Buffer.from('two\n'));
    assert.equal(readFileSync(target, 'utf8'), 'two\n');
    assert.deepEqual(readdirSync(path.dirname(target)), ['out.txt']);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('atomic-writes exempts an out-of-line test module and reports a stale allowlist entry', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-atomic-'));
  try {
    mkdirSync(path.join(root, 'src'));
    writeFileSync(path.join(root, 'src', 'lib.rs'), '#[cfg(test)]\n#[path = "x_tests.rs"]\nmod tests;\nmod y;\n');
    writeFileSync(path.join(root, 'src', 'x_tests.rs'), 'fn t() { std::fs::write(p, b); }\n');
    writeFileSync(path.join(root, 'src', 'y.rs'), 'fn y() { std::fs::write(p, b); }\n');
    const found = atomicGate({ root, roots: ['src'], scriptRoots: [], allowlist: [] });
    assert.equal(found.errors.length, 1, found.errors.join('\n'));
    assert.match(found.errors[0], /^src\/y\.rs:1:/);
    const stale = atomicGate({ root, roots: ['src'], scriptRoots: [], allowlist: [{ file: 'src/gone.rs', needle: 'x', reason: 'r' }] });
    assert.ok(stale.errors.some((e) => /stale allowlist entry: src\/gone\.rs/.test(e)), stale.errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('atomic-writes catches a writer reverted from write_atomic to fs::write', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-atomic-'));
  try {
    mkdirSync(path.join(root, 'src'));
    const real = readFileSync(path.join(ROOT, 'crates', 'ludion-render-graph', 'src', 'png_io.rs'), 'utf8');
    assert.ok(real.includes('write_atomic('), 'png_io.rs no longer routes through write_atomic');
    writeFileSync(path.join(root, 'src', 'png_io.rs'), real);
    assert.deepEqual(atomicGate({ root, roots: ['src'], scriptRoots: [], allowlist: [] }).errors, []);
    writeFileSync(path.join(root, 'src', 'png_io.rs'), real.replaceAll('write_atomic(', 'std::fs::write('));
    const reverted = atomicGate({ root, roots: ['src'], scriptRoots: [], allowlist: [] });
    assert.ok(reverted.errors.some((e) => e.startsWith('src/png_io.rs:')), reverted.errors.join('\n'));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('atomic-writes passes on the tree', () => {
  const run = gate('atomic-writes.mjs');
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stderr, '');
});

// The canary derives REPO_ROOT from its own location, so a copy under a scratch `scripts/` sees a
// scratch repository; every case below exits before any git call.
function canaryIn(root, args = []) {
  mkdirSync(path.join(root, 'scripts'), { recursive: true });
  const script = path.join(root, 'scripts', 'privacy-canary.mjs');
  writeFileSync(script, readFileSync(path.join(ROOT, 'scripts', 'privacy-canary.mjs')));
  return spawnSync('node', [script, ...args], { cwd: root, encoding: 'utf8' });
}

test('the privacy canary skips a public clone, fails a private checkout with no list, and a missing --tokens file is a usage error', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'ludion-canary-'));
  try {
    const publicClone = canaryIn(dir);
    assert.equal(publicClone.status, 0, publicClone.stderr);
    assert.match(publicClone.stdout, /skipped/);

    mkdirSync(path.join(dir, 'ci', 'teamcity'), { recursive: true });
    const privateCheckout = canaryIn(dir);
    assert.equal(privateCheckout.status, 3, privateCheckout.stdout);
    assert.match(privateCheckout.stderr, /ci\/teamcity\/privacy-tokens\.txt/);

    const explicit = canaryIn(dir, ['--tokens', 'nope.txt']);
    assert.equal(explicit.status, 2, explicit.stdout);
    assert.match(explicit.stderr, /nope\.txt/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('every node --test lane names existing test files, never a glob', () => {
  // A glob matching nothing makes `node --test` pass with zero tests.
  const laneFiles = [
    ['scripts/check.mjs', (text) => [...text.matchAll(/cmd: 'node', args: \['--test', ([^\]]*)\]/g)].flatMap((m) => [...m[1].matchAll(/'([^']+)'/g)].map((a) => a[1]))],
    ['.github/workflows/ci.yml', (text) => [...text.matchAll(/node --test (.+)$/gm)].flatMap((m) => m[1].trim().split(/\s+/)).map((a) => a.replace(/^["']|["']$/g, ''))],
    ['ci/teamcity/provision.mjs', (text) => [...text.matchAll(/node --test ([^'\n]+)/g)].flatMap((m) => m[1].trim().split(/\s+/)).map((a) => a.replace(/^["']|["']$/g, ''))],
  ];
  for (const [rel, extract] of laneFiles) {
    if (!existsSync(path.join(ROOT, rel))) {
      assert.notEqual(rel, 'scripts/check.mjs');
      continue; // the TeamCity directory is private, so a public clone lacks it
    }
    const found = extract(readFileSync(path.join(ROOT, rel), 'utf8'));
    assert.ok(found.length > 0, `${rel}: no node --test lane found`);
    for (const arg of found) {
      assert.ok(!/[*?[]/.test(arg), `${rel}: "${arg}" is a glob`);
      if (arg.startsWith('games/vigil/') && !existsSync(path.join(ROOT, 'games', 'vigil'))) continue; // the Lore-only game; check.mjs gates vigilNodeStep on its presence
      assert.ok(existsSync(path.join(ROOT, arg)), `${rel}: "${arg}" is not an existing file`);
    }
  }
});

test('the schedule gate flags each rule and passes the near misses', () => {
  const rules = (text) => findScheduleText(text).map((h) => h.rule);
  assert.deepEqual(rules('// restated this wave'), ['current-wave']);
  assert.deepEqual(rules("/// **The wave's exit criterion.**"), ['current-wave']);
  assert.deepEqual(rules('// a later phase fills this in'), ['later-phase']);
  assert.deepEqual(rules('// this milestone only'), ['later-phase']);
  assert.deepEqual(rules('// a future editor panel'), ['later-phase']);
  assert.deepEqual(rules('// shown (M1)'), ['milestone-tag']);
  assert.deepEqual(rules('// until M2'), ['milestone-tag']);
  assert.deepEqual(rules('// Wave 4 will emit it'), ['forward-wave']);
  assert.deepEqual(rules('// lands in Wave 3'), ['forward-wave']);
  assert.deepEqual(rules('// until W2'), ['forward-wave']);
  assert.equal(findScheduleText('a\nb\n// this wave').at(0).line, 3);

  for (const ok of [
    'Phase::Sim runs first',
    "the next phase's timer",
    'a future frame',
    'a future version',
    '(Phase 8 Wave 3)',
    '`Plan/x.md` Wave 4',
    'toward the next spawn',
    'Milestone 1 CLOSED',
    'the M1 Pro chip',
    'gi-wave2 fixture',
  ]) {
    assert.deepEqual(findScheduleText(ok), [], ok);
  }
});

// The schedule gate walks `git ls-files`; the Lore-only CI checkouts have no git work tree.
const IN_GIT_TREE = spawnSync('git', ['rev-parse', '--is-inside-work-tree'], { cwd: ROOT, encoding: 'utf8' }).stdout?.trim() === 'true';

test('the schedule gate skips mirrors and generated files', { skip: !IN_GIT_TREE && 'no git work tree' }, () => {
  const files = scheduleFiles(ROOT);
  assert.ok(files.length > 100);
  assert.ok(!files.some((f) => f.includes('embedded-docs/') || f.endsWith('protocol.d.ts') || f.startsWith('Plan/')));
});

test('a schedule allowlist entry suppresses only its needle', () => {
  const file = 'crates/ludion-protocol/src/response.rs';
  assert.ok(SCHEDULE_ALLOWLIST.some((e) => e.file === file));
  assert.deepEqual(unallowedHits(file, '/// always `"png"` in M1.'), []);
  assert.equal(unallowedHits(file, '/// always `"png"` in M1.\n// this wave').length, 1);
  assert.deepEqual(unallowedHits('ci/scripts/schedule-comments.mjs', '// this wave'), []);
  assert.equal(unallowedHits('crates/x.rs', '// this wave').length, 1);
});

test('the ECS budget counts code lines before the test module only', () => {
  const text = ['pub fn a() {', '', '    // note', '    1', '}', '', '//! doc', '#[cfg(test)]', 'mod tests {', '    fn t() {}', '}', ''].join('\n');
  assert.equal(countCodeLines(text), 3);
  assert.equal(countCodeLines('x\n  #[cfg(test)]\ny\n'), 3);
  assert.equal(countCodeLines(''), 0);
});

test('the ECS budget keeps counting past a cfg(test) that is not a test module', () => {
  const text = ['#[cfg(test)]', 'use x::y;', 'fn a() {}', '#[cfg(test)]', 'mod tests {', '    fn t() {}', '}', ''].join('\n');
  assert.equal(countCodeLines(text), 3);
});

test('quoteForCmd quotes whitespace and cmd metacharacters and doubles embedded quotes', () => {
  assert.equal(quoteForCmd('cargo'), 'cargo');
  assert.equal(quoteForCmd('a b'), '"a b"');
  for (const t of ['a&b', 'a|b', 'a<b', 'a>b', 'a^b', 'a(b', 'a)b', '%PATH%', 'hi!']) assert.equal(quoteForCmd(t), `"${t}"`);
  assert.equal(quoteForCmd('say "hi"'), '"say ""hi"""');
  assert.equal(quoteForCmd(''), '""');
});

test('pruneScopeTemps removes dead scoped temp roots and keeps live, own and unrelated ones', () => {
  const tmp = mkdtempSync(path.join(os.tmpdir(), 'prune-'));
  try {
    for (const n of ['ludion-check-ab-scope-1', 'ludion-check-ab-scope-2', 'ludion-check-ab-scope-3', 'ludion-check-ab', 'other-scope-1']) mkdirSync(path.join(tmp, n));
    pruneScopeTemps(tmp, 'ludion-check-ab-scope-', (pid) => pid === 2, 3);
    assert.deepEqual(readdirSync(tmp).sort(), ['ludion-check-ab', 'ludion-check-ab-scope-2', 'ludion-check-ab-scope-3', 'other-scope-1']);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
});

test('the target guard deletes the largest trees until under both limits, and nothing when under', () => {
  const trees = [
    { path: 'target/debug', bytes: 60 },
    { path: 'target/package-verify', bytes: 30 },
    { path: 'fuzz/target', bytes: 20 },
  ];
  assert.deepEqual(planTargetClean(trees, 100, 120, 40), []);
  assert.deepEqual(planTargetClean(trees, 100, 80, 40).map((t) => t.path), ['target/debug']);
  // Under the size limit but short of free space: the free-space limit alone triggers a clean.
  assert.deepEqual(planTargetClean(trees, 10, 200, 60).map((t) => t.path), ['target/debug']);
  assert.deepEqual(planTargetClean(trees, 0, 1, 1000).length, 3);
});

test('the target guard measures target/ per entry and the other trees whole', () => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'target-guard-'));
  try {
    mkdirSync(path.join(root, 'target', 'debug', 'deps'), { recursive: true });
    writeFileSync(path.join(root, 'target', 'debug', 'deps', 'a.rlib'), Buffer.alloc(300));
    writeFileSync(path.join(root, 'target', 'CACHEDIR.TAG'), Buffer.alloc(5));
    mkdirSync(path.join(root, 'fuzz', 'target', 'x'), { recursive: true });
    writeFileSync(path.join(root, 'fuzz', 'target', 'x', 'b'), Buffer.alloc(70));
    const byPath = Object.fromEntries(measureTargets(root).map((t) => [t.path, t.bytes]));
    assert.deepEqual(byPath, { 'target/debug': 300, 'target/CACHEDIR.TAG': 5, 'fuzz/target': 70 });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

// ── crate-graph edge listing (scripts/crate-graph-edges.mjs) ─────────────────

const cgDep = (name, extra = {}) => ({ name, kind: null, optional: false, features: [], target: null, ...extra });
const cgPkg = (name, dependencies, features = {}) => ({ name, dependencies, features, manifest_path: `/r/crates/${name}/Cargo.toml` });
const cgMeta = () => ({
  packages: [
    cgPkg('ludion-app', [cgDep('ludion-mid', { optional: true }), cgDep('ludion-assets', { features: ['watch'] }), cgDep('serde', { features: ['derive'] }), cgDep('ludion-kit', { kind: 'dev' })], { wire: ['dep:ludion-mid'] }),
    cgPkg('ludion-mid', [cgDep('ludion-gpu')]),
    cgPkg('ludion-gpu', [cgDep('wgpu')]),
    cgPkg('ludion-assets', [cgDep('notify', { optional: true })], { watch: ['dep:notify'] }),
    cgPkg('ludion-kit', [cgDep('ludion-gpu', { kind: 'dev' })]),
  ],
});

test('crate-graph: renderEdges shows optional, feature, dev rows and drops third-party features', () => {
  const out = renderEdges(cgMeta());
  assert.match(out, /^ludion-app +→ assets\{watch\}, mid\(wire\) +\[serde\]$/m);
  assert.match(out, /^ {4}dev: kit$/m);
  assert.doesNotMatch(out, /derive/);
  assert.match(out, /^assets\/watch +← app$/m);
  assert.match(out, /^notify +← assets\(watch\)$/m);
});

test('crate-graph: renderEdges does not depend on package order', () => {
  const m = cgMeta();
  const reversed = { packages: [...m.packages].reverse() };
  assert.equal(renderEdges(reversed), renderEdges(m));
});

test('crate-graph: no-wgpu row follows normal edges only', () => {
  const line = renderEdges(cgMeta()).split('\n').find((l) => l.startsWith('no wgpu in the tree: '));
  assert.equal(line, 'no wgpu in the tree: assets, kit');
});

test('crate-graph: spliceEdges round-trips, is idempotent, and throws on a missing marker', () => {
  const doc = `head\n${CG_BEGIN}\nstale\n${CG_END}\ntail\n`;
  const once = spliceEdges(doc, '```\nnew\n```');
  assert.equal(once, `head\n${CG_BEGIN}\n\`\`\`\nnew\n\`\`\`\n${CG_END}\ntail\n`);
  assert.equal(spliceEdges(once, '```\nnew\n```'), once);
  assert.throws(() => spliceEdges('no markers', 'x'), /crate-graph/);
  assert.throws(() => spliceEdges(`${CG_BEGIN}\nx\n`, 'x'), /end generated/);
});

test('crate-graph: the real tree is in step', () => {
  assert.deepEqual(checkCrateGraph(ROOT).errors, []);
});

// ── doc citations (scripts/doc-citations.mjs) ────────────────────────────────

function citationRoot(files) {
  const root = mkdtempSync(path.join(os.tmpdir(), 'doc-cites-'));
  for (const [rel, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    writeFileSync(path.join(root, rel), text);
  }
  return root;
}

test('doc-citations: a good citation passes, a wrong-root one fails with the hint, a wrapped one resolves', () => {
  const root = citationRoot({
    'docs/guide.md': 'See `ludion docs guide` and `ludion docs <topic>`.\n',
    'dev-docs/internals.md': 'See `ludion devdocs internals`, or `ludion docs internals` (wrong).\n',
    'crates/a/src/lib.rs': '//! Read `ludion\n//! devdocs internals` and `ludion docs\n//! guide`.\n',
  });
  try {
    const { errors, count } = checkDocCitations(root);
    assert.equal(count, 5);
    assert.equal(errors.length, 1);
    assert.match(errors[0], /dev-docs\/internals\.md:1 cites `ludion docs internals`/);
    assert.match(errors[0], /use `ludion devdocs internals`/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('doc-citations: the real tree is in step', () => {
  assert.deepEqual(checkDocCitations(ROOT).errors, []);
});

// ── manifest rules (scripts/manifest-rules.mjs) ──────────────────────────────

test('manifests: bareVersions flags a bare string, inline table, target entry and table form', () => {
  const text = [
    '[dependencies]',
    'a = "1"',
    'b = { version = "2", features = ["x"] }',
    "[target.'cfg(windows)'.dependencies]",
    'c = "3"',
    '[dev-dependencies.d]',
    'version = "4"',
    'features = ["y"]',
  ].join('\n');
  assert.deepEqual(bareVersions(text), ['a', 'b', 'c', 'd']);
});

test('manifests: bareVersions passes workspace, path and multi-line path entries', () => {
  const text = [
    '[package]',
    'version = "0.1.0"',
    '[dependencies]',
    'a.workspace = true',
    'b = { workspace = true, optional = true }',
    'c = { path = "../c", version = "0.1.0" }',
    'd = { path = "../d", default-features = false, features = [',
    '    "x",',
    '    "y",',
    '] }',
    '[[bin]]',
    'name = "z"',
    'path = "src/main.rs"',
  ].join('\n');
  assert.deepEqual(bareVersions(text), []);
});

test('manifests: nonFacadeDeps flags an engine crate in [dependencies] only', () => {
  const text = [
    '[dependencies]',
    'ludion = { path = "../../crates/ludion" }',
    'ludion-core = { path = "../../crates/ludion-core" }',
    '[dev-dependencies]',
    'ludion-testkit = { path = "../../crates/ludion-testkit" }',
  ].join('\n');
  assert.deepEqual(nonFacadeDeps(text), ['ludion-core']);
  assert.deepEqual(nonFacadeDeps('[dependencies]\nludion = {{ludion_dep}}\n'), []);
});

test('manifests: the real tree passes', () => {
  assert.deepEqual(checkManifests(ROOT).errors, []);
});

test('clippy: the game tier bans what the sim tier bans, spelled through the facade', () => {
  const game = renderDenylist('game');
  assert.doesNotMatch(game, /ludion_core/);
  const paths = (text) => [...text.matchAll(/path = "([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(paths(game), paths(renderDenylist('sim')));
});
