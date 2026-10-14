#!/usr/bin/env node
// Tests for scripts/public-tree.mjs's export against a scratch git repository.
//
// Usage:  node --test scripts/public-tree.test.mjs
//
// Zero npm dependencies — Node built-ins only. Runs unchanged on Windows and macOS.

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { exportTree, initGit } from './public-tree.mjs';

test('the export is the git-visible tree, incremental, and leaves foreign files alone', (t) => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-public-tree-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => {
    const r = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
    assert.equal(r.status, 0, r.stderr);
  };
  const write = (rel, text) => {
    mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    writeFileSync(path.join(root, rel), text);
  };
  git('init', '-q');
  write('src/a.txt', 'a\n');
  write('secret/b.txt', 'b\n');
  git('add', '-A');
  git('-c', 'user.name=t', '-c', 'user.email=t@example.com', 'commit', '-q', '-m', 'seed');
  write('.gitignore', '/target/\n/secret/\n'); // secret/ stays tracked, now private
  write('src/new.txt', 'untracked\n');

  const dest = path.join(root, 'target', 'public-tree', 'tree');
  const manifest = path.join(root, 'target', 'public-tree', 'files.txt');
  const first = exportTree(root, dest, manifest);
  assert.ok(existsSync(path.join(dest, 'src', 'a.txt')));
  assert.ok(existsSync(path.join(dest, 'src', 'new.txt')), 'untracked-not-ignored files are exported');
  assert.ok(!existsSync(path.join(dest, 'secret')), 'a tracked private path is not exported');
  assert.equal(first.removed, 0);

  // not in the manifest: the export's own dependency tree
  mkdirSync(path.join(dest, 'node_modules'), { recursive: true });
  writeFileSync(path.join(dest, 'node_modules', 'keep.txt'), 'k\n');
  const mtime = statSync(path.join(dest, 'src', 'a.txt')).mtimeMs;
  const second = exportTree(root, dest, manifest);
  assert.equal(second.copied, 0);
  assert.equal(statSync(path.join(dest, 'src', 'a.txt')).mtimeMs, mtime);

  rmSync(path.join(root, 'src', 'new.txt'));
  const third = exportTree(root, dest, manifest);
  assert.equal(third.removed, 1);
  assert.ok(!existsSync(path.join(dest, 'src', 'new.txt')));
  assert.equal(readFileSync(path.join(dest, 'node_modules', 'keep.txt'), 'utf8'), 'k\n');

  assert.throws(() => exportTree(root, path.join(root, 'elsewhere'), manifest), /outside/);
});

test('initGit makes the export its own repository, staged, and re-stages changes', (t) => {
  const root = mkdtempSync(path.join(os.tmpdir(), 'ludion-public-tree-git-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const tracked = () => spawnSync('git', ['ls-files'], { cwd: root, encoding: 'utf8' }).stdout.split('\n').filter(Boolean);
  writeFileSync(path.join(root, 'a.txt'), 'a\n');
  initGit(root);
  assert.ok(existsSync(path.join(root, '.git')));
  assert.deepEqual(tracked(), ['a.txt']);
  rmSync(path.join(root, 'a.txt'));
  writeFileSync(path.join(root, 'b.txt'), 'b\n');
  initGit(root);
  assert.deepEqual(tracked(), ['b.txt']);
});
