#!/usr/bin/env node
// Tests for scripts/mirror-lib.mjs against fixture strings.
//
// Usage:  node --test scripts/mirror-lib.test.mjs
//
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { isPrivate, overlaps, parseLoreDumpFiles, parseLoreStatus, privatePaths } from './mirror-lib.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

describe('privatePaths', () => {
  test('anchored gitignore rules absent from loreignore, directories without the slash', () => {
    const git = '# c\n/.exarch/\n/infra/\n/CLAUDE.local.md\n/target\n.build/\n**/x\n*.pdb\n';
    const lore = '/target\n.build/\n*.pdb\n';
    assert.deepEqual(privatePaths(git, lore), ['.exarch', 'infra', 'CLAUDE.local.md']);
  });

  test('a missing loreignore makes every anchored rule private; globs never are', () => {
    assert.deepEqual(privatePaths('/a/\n/b/*.txt\n', undefined), ['a']);
  });

  test('the real ignore files yield the six known private paths', () => {
    const read = (f) => (existsSync(path.join(ROOT, f)) ? readFileSync(path.join(ROOT, f), 'utf8') : '');
    assert.deepEqual(privatePaths(read('.gitignore'), read('.loreignore')).sort(), [
      '.exarch', 'CLAUDE.local.md', 'Plan-Private', 'ci/teamcity', 'games/vigil', 'infra',
    ]);
  });
});

test('isPrivate matches the path and anything under it, not a sibling prefix', () => {
  const priv = ['infra', 'ci/teamcity'];
  assert.ok(isPrivate('infra/x/y', priv));
  assert.ok(isPrivate('infra', priv));
  assert.ok(isPrivate(String.raw`ci\teamcity\a`, priv));
  assert.ok(!isPrivate('infrastructure/x', priv));
  assert.ok(!isPrivate('ci/scripts/a', priv));
});

test('parseLoreStatus reads change lines and ignores headers', () => {
  const text = 'Repository abc\nOn branch main revision 5 -> ff\nChanges not staged for commit:\nM a/b.rs\nD gone/\nA new.txt\nUntracked files:\nTracked changes: 1 added\n';
  assert.deepEqual(parseLoreStatus(text), [
    { kind: 'M', path: 'a/b.rs' },
    { kind: 'D', path: 'gone/' },
    { kind: 'A', path: 'new.txt' },
  ]);
});

test('parseLoreDumpFiles keeps files (addr) and drops directories', () => {
  const text = [
    'Repository r',
    'scripts/ id 462 parent 0 sibling 437 mode 00 size 3 flags 0 child 2351',
    'scripts/a b.mjs id 2351 parent 462 sibling 0 mode 00 size 4 flags 1 addr 4ff9-01a1',
  ].join('\n');
  assert.deepEqual(parseLoreDumpFiles(text), ['scripts/a b.mjs']);
});

test('overlaps relates a path to a named file or directory in both directions', () => {
  assert.ok(overlaps('a/b.rs', ['a']));
  assert.ok(overlaps('a/', ['a/b.rs']));
  assert.ok(!overlaps('ab/c', ['a']));
});

test('overlaps treats `.` (the whole tree) as covering every path', () => {
  assert.ok(overlaps('a/b.rs', ['.']));
  assert.ok(overlaps('a/', ['./']));
  assert.ok(overlaps('README.md', ['']));
  assert.ok(overlaps('.', ['a/b.rs']));
});
