// node --test scripts/deps-prune.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, existsSync, utimesSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { pruneDeps, MIN_AGE_MS } from './deps-prune.mjs';

// Setting a file's times also moves its ctime to the real now, so tests inject a `now` two floors ahead:
// every file is then old by ctime, and a file is "fresh" only through an mtime still ahead of that now.
const NOW = Date.now() + 2 * MIN_AGE_MS;

function fixture(files) {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'deps-prune-'));
  for (const [name, fresh] of files) {
    const file = path.join(dir, name);
    writeFileSync(file, 'x');
    if (fresh) {
      const t = new Date(NOW);
      utimesSync(file, t, t);
    }
  }
  return dir;
}

test('under the threshold nothing is deleted', () => {
  const dir = fixture([['a.o', false]]);
  assert.deepEqual(pruneDeps(dir, { threshold: 5, now: NOW }), { before: 1, removed: 0, after: 1 });
  assert.ok(existsSync(path.join(dir, 'a.o')));
  rmSync(dir, { recursive: true });
});

test('over the threshold only old .o files go', () => {
  const dir = fixture([['old.o', false], ['fresh.o', true], ['old.rlib', false]]);
  const r = pruneDeps(dir, { threshold: 2, now: NOW });
  assert.deepEqual(r, { before: 3, removed: 1, after: 2 });
  assert.ok(!existsSync(path.join(dir, 'old.o')));
  assert.ok(existsSync(path.join(dir, 'fresh.o')));
  assert.ok(existsSync(path.join(dir, 'old.rlib')));
  rmSync(dir, { recursive: true });
});

test('a missing directory is not fatal', () => {
  assert.equal(pruneDeps(path.join(os.tmpdir(), 'deps-prune-absent-xyz'), { threshold: 1 }), null);
});
