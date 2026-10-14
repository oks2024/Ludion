// The schema-drift gate writes no tracked file. Run from the repository root:
//   node --test ci/scripts/schema-drift.test.mjs
//
// Runs where the gate's own precondition is checked: check.mjs's node lane (right after `schema-drift`),
// the ci.yml `schema-drift` job and TeamCity's `Ludion_SchemaDrift`, all after `npm ci` in
// editor/protocol. Missing dependencies fail here rather than skip, so this test cannot pass vacuously.

import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const fingerprint = (rel) => {
  const abs = path.join(ROOT, rel);
  return { bytes: readFileSync(abs), mtimeMs: statSync(abs).mtimeMs };
};
const gate = (script, args = []) =>
  spawnSync('node', [path.join('ci', 'scripts', script), ...args], { cwd: ROOT, encoding: 'utf8' });

test('schema-drift writes no tracked file', () => {
  if (!existsSync(path.join(ROOT, 'editor/protocol/node_modules/json-schema-to-typescript'))) {
    assert.fail('editor/protocol dependencies are not installed: run `npm ci` in editor/protocol');
  }
  const files = ['editor/protocol/src/protocol.d.ts', 'editor/app/src/editor/effects/patternSchema.ts'];
  const before = files.map(fingerprint);
  const r = gate('schema-drift.mjs', ['--fast']);
  assert.equal(r.status, 0, r.stdout + r.stderr);
  files.forEach((f, i) => {
    const after = fingerprint(f);
    assert.ok(before[i].bytes.equals(after.bytes), `${f} bytes changed`);
    assert.equal(before[i].mtimeMs, after.mtimeMs, `${f} mtime changed`);
  });
});
